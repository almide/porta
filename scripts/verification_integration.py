#!/usr/bin/env python3
"""Real WASM completion gates, recovery, teams and hostile verdicts."""
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import threading
from wasm_fixtures import fixed_stdout_guest, no_environment_guest

porta, agent, tool, verifier, readonly = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:6]]
requests = []
mode = 'repair'
secret = 'verification-model-secret'

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        assert secret not in json.dumps(body)
        messages = body['messages']
        failed = messages[-1]['role'] == 'user' and 'Completion verification failed' in messages[-1]['content']
        if body['model'] == 'root' and not any(m['role'] == 'tool' for m in messages):
            name, arguments = 'delegate_worker', {'task': 'Create and verify the artifact'}
        elif failed and mode in ('repair', 'procedural'):
            name, arguments = 'write_file', {'path': 'result.json', 'content': '{"value":42}'}
            if mode == 'procedural' and 'call(s) to read_file' in messages[-1]['content']:
                name, arguments = 'read_file', {'path': 'result.json'}
        else:
            name = None
        message = {'role': 'assistant', 'content': 'All done.'}
        if name:
            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                'id': f'call-{len(requests)}', 'type': 'function',
                'function': {'name': name, 'arguments': json.dumps(arguments)}}]}
        payload = json.dumps({'choices': [{'message': message}]}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        """Silence the request log; these tests assert on their own output."""

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-verification-') as directory:
        root = pathlib.Path(directory).resolve()
        workspace = root / 'workspace'
        workspace.mkdir()
        output = workspace / 'result.json'
        local_verifier = root / 'check.wasm'
        shutil.copyfile(verifier, local_verifier)
        config = root / 'agent.toml'
        base = f'''version = 1
[agent]
wasm = {json.dumps(agent)}
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "worker"
token_env = "PORTA_TEST_TOKEN"
[[tools]]
name = "write_file"
wasm = {json.dumps(tool)}
mounts = [{{host="workspace",guest=".",read_only=false}}]
[[completion_checks]]
name = "artifact"
wasm = "check.wasm"
parameters = {{path="result.json",expected={{value=42}}}}
mounts = [{{host="workspace",guest="."}}]
'''
        config.write_text(base)

        def run(*args, credential=True):
            env = dict(os.environ)
            env.pop('PORTA_TEST_TOKEN', None)
            if credential:
                env['PORTA_TEST_TOKEN'] = secret
            result = subprocess.run([porta, *map(str, args)], env=env, capture_output=True,
                                    text=True, stdin=subprocess.DEVNULL, timeout=15)
            assert secret not in result.stdout + result.stderr
            return result

        def success(result):
            assert result.returncode == 0, result.stderr
            return result

        def failure(result, error):
            assert result.returncode != 0 and error in result.stderr, result

        def metrics(result):
            lines = [line for line in result.stderr.splitlines() if line.startswith('[porta agent] {')]
            return json.loads(lines[-1].removeprefix('[porta agent] '))

        journal = root / 'run.jsonl'
        first = success(run('agent', config, '--record', journal, '--', 'Produce the artifact'))
        assert output.read_text() == '{"value":42}' and first.stdout.strip() == 'All done.'
        counts = metrics(first)
        assert counts['verification_calls'] == 2 and counts['verification_failures'] == 1 and counts['tool_calls'] == 1, counts
        feedback = [m for req in requests for m in req['messages'] if 'verification failed' in (m.get('content') or '')]
        assert feedback and 'artifact' in feedback[0]['content']
        assert secret not in journal.read_text()
        print('PASS: premature done is rejected; the WASM agent repairs the artifact and only then completes')

        output.write_text('human changed this after completion')
        before = len(requests)
        replay = success(run('agent-replay', config, journal, credential=False))
        assert replay.stdout == first.stdout and len(requests) == before
        assert output.read_text() == 'human changed this after completion'
        print('PASS: completed-run replay uses recorded verdicts without reading or rewriting current artifacts')

        # Crash boundary: a recorded pass without durable completion cannot certify
        # files that changed while the process was stopped.
        prefix = root / 'prefix.jsonl'
        lines = journal.read_text().splitlines(keepends=True)
        assert json.loads(lines[-1])['payload']['kind'] == 'complete'
        prefix.write_text(''.join(lines[:-1]))
        prefix.chmod(0o600)
        result = run('agent-resume', config, prefix, credential=False)
        failure(result, 'required model credential is missing')
        assert len(requests) == before and output.read_text() == 'human changed this after completion'
        appended = [json.loads(line)['payload'] for line in prefix.read_text().splitlines()[len(lines)-1:]]
        assert any(e['kind'] == 'result' and e['response'].get('passed') is False for e in appended), appended
        success(run('agent-resume', config, prefix))
        assert output.read_text() == '{"value":42}'
        print('PASS: live resume rechecks artifacts after a cached pass and resumes correction without repeating old writes')

        # Required operations come from the host, not model claims or guest state.
        output.unlink()
        mode = 'procedural'
        procedure = base.replace('expected={value=42}', 'expected={value=42},required_tool_calls={write_file=1,read_file=1}')
        procedure += f'\n[[tools]]\nname="read_file"\nwasm={json.dumps(tool)}\nmounts=[{{host="workspace",guest="."}}]\n'
        config.write_text(procedure)
        result = success(run('agent', config, '--', 'Produce and inspect the artifact'))
        assert metrics(result)['verification_failures'] == 2 and metrics(result)['tool_calls'] == 2
        assert output.read_text() == '{"value":42}'
        print('PASS: operator checks enforce actual host-observed tool calls, including output inspection')

        # A model cannot bypass a failed check by repeatedly emitting done.
        output.unlink()
        mode = 'stubborn'
        config.write_text(base + '\n[limits]\nmax_steps = 8\n')
        failure(run('agent', config, '--', 'Produce artifact'), 'step budget exceeded')
        assert not output.exists()
        print('PASS: repeated completion claims consume the shared step budget without bypassing the gate')

        config.write_text(base)
        # Read-only enforcement is independent of verifier honesty.
        shutil.copyfile(readonly, local_verifier)
        success(run('agent', config, '--', 'Check read-only enforcement'))
        assert not (workspace / 'tamper.txt').exists()
        config.write_text(base.replace('guest="."}', 'guest=".",read_only=false}'))
        failure(run('agent', config, '--', 'bad grant'), 'completion check mounts must be read-only')
        config.write_text(base)
        print('PASS: verifier writes are denied and writable check grants fail at launch')
        local_verifier.write_bytes(no_environment_guest({'passed': True}))
        success(run('agent', config, '--', 'Check verifier environment'))
        print('PASS: verifier WASI receives no host environment or model credentials')

        # Every configured check must pass, not just the first one.
        shutil.copyfile(verifier, local_verifier)
        output.write_text('{"value":42}')
        deny = root / 'deny.wasm'
        deny.write_bytes(fixed_stdout_guest({'passed': False, 'feedback': 'second check rejects completion'}))
        config.write_text(base + f'\n[[completion_checks]]\nname="second"\nwasm={json.dumps(str(deny))}\n[limits]\nmax_steps=8\n')
        result = run('agent', config, '--', 'Do not bypass the second check')
        failure(result, 'step budget exceeded')
        assert metrics(result)['verification_calls'] == 4 and metrics(result)['verification_failures'] == 2
        print('PASS: every completion check is mandatory, including checks after an earlier pass')

        liar = root / 'liar.wasm'
        liar.write_bytes(fixed_stdout_guest({'action': 'done', 'output': 'passed', 'state': {
            'passed': True, 'tool_calls': [{'name': 'write_file', 'arguments': {'path': 'result.json'}}]}}))
        forged = base.replace(json.dumps(agent), json.dumps(str(liar)))
        forged = forged.replace('expected={value=42}', 'expected={value=42},required_tool_calls={write_file=1}')
        config.write_text(forged + '\n[limits]\nmax_steps=8\n')
        result = run('agent', config, '--', 'Do not trust guest-provided history')
        failure(result, 'step budget exceeded')
        assert metrics(result)['tool_calls'] == 0 and metrics(result)['verification_failures'] > 0
        config.write_text(base)
        output.unlink()
        print('PASS: fabricated guest state cannot satisfy host-observed operation requirements')

        for verdict, error in [({'passed': 'true'}, 'invalid completion verdict'),
                               ({'passed': False}, 'must explain'),
                               ({'passed': True, 'grant': 'shell'}, 'invalid completion verdict')]:
            local_verifier.write_bytes(fixed_stdout_guest(verdict))
            failure(run('agent', config, '--', 'bad verdict'), error)
        local_verifier.write_bytes(bytes.fromhex('0061736d0100000001040160000003020100070a01065f737461727400000a0901070003400c000b0b'))
        config.write_text(base + '\n[limits]\ntimeout_seconds = 1\nfuel_per_step = 9223372036854775807\n')
        failure(run('agent', config, '--', 'infinite verifier'), 'guest execution failed')
        config.write_text(base)
        print('PASS: malformed/ambiguous verdicts fail closed and CPU-bound verification is interrupted')

        shutil.copyfile(verifier, local_verifier)
        original = local_verifier.read_bytes()
        local_verifier.write_bytes(original + b'\x00\x02\x01x')
        failure(run('agent-replay', config, journal), 'fingerprint')
        local_verifier.write_bytes(original)
        failure(run('agent', config, '--record', workspace / 'forbidden.jsonl', '--', 'task'), 'outside every tool mount')
        print('PASS: check artifacts are fingerprinted and journals remain outside check/tool mounts')

        mode = 'repair'
        parent = root / 'team.toml'
        parent.write_text(f'''version = 1
[agent]
wasm = {json.dumps(agent)}
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "root"
[[agents]]
name = "worker"
config = "agent.toml"
''')
        team_journal = root / 'team.jsonl'
        result = success(run('agent', parent, '--record', team_journal, '--', 'Delegate artifact production'))
        assert output.exists() and metrics(result)['verification_failures'] == 1
        before = len(requests)
        success(run('agent-replay', parent, team_journal, credential=False))
        assert len(requests) == before
        output.unlink()
        parent.write_text(parent.read_text() + '\n[limits]\nmax_model_calls = 2\n')
        failure(run('agent', parent, '--', 'Delegate artifact production'), 'team model call budget exceeded')
        assert not output.exists()
        print('PASS: delegated agents must satisfy their checks and cannot reset root budgets; team replay stays offline')
finally:
    server.shutdown()
    server.server_close()
    thread.join()
