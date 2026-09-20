#!/usr/bin/env python3
"""Durable resume/replay with real WASM, real file effects and no external API."""
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

porta, agent, tool = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
secret = 'journal-fixture-model-secret'
requests = []
block = False
entered = threading.Event()
release = threading.Event()
reject_network = False
response_delay = 0
empty_after_tool = 0

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        global empty_after_tool
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        if reject_network:
            self.send_error(503)
            return
        if block:
            entered.set()
            release.wait(15)
        time.sleep(response_delay)
        messages = body['messages']
        results = [m for m in messages if m['role'] == 'tool']
        if results:
            message = {'role': 'assistant', 'content': results[-1]['content']}
        else:
            name, arguments = ('delegate_worker', {'task': 'Write the requested result'}) if body['model'] == 'root' else (
                'write_file', {'path': 'result.txt', 'content': 'original write'})
            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                'id': 'call-1', 'type': 'function', 'function': {'name': name, 'arguments': json.dumps(arguments)}}]}
        if results and empty_after_tool > 0:
            empty_after_tool -= 1
            message = {'role': 'assistant', 'content': None, 'tool_calls': []}
        payload = json.dumps({'choices': [{'message': message}]}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        try:
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            pass
    def log_message(self, *args):
        """Silence the request log; these tests assert on their own output."""

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-journal-') as directory:
        root = pathlib.Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        local_agent = root / 'agent.wasm'
        local_tool = root / 'tools.wasm'
        shutil.copyfile(agent, local_agent)
        shutil.copyfile(tool, local_tool)
        config = root / 'agent.toml'
        config.write_text(f'''version = 1
[agent]
wasm = "agent.wasm"
instruction = "Use the supplied tools."
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "worker"
token_env = "PORTA_TEST_TOKEN"
[[tools]]
name = "write_file"
wasm = "tools.wasm"
mounts = [{{host="workspace",guest=".",read_only=false}}]
''')
        result_file = workspace / 'result.txt'

        def environment(credential=True):
            env = dict(os.environ)
            env.pop('PORTA_TEST_TOKEN', None)
            if credential:
                env['PORTA_TEST_TOKEN'] = secret
            return env

        def run(*args, credential=True):
            result = subprocess.run([porta, *map(str, args)], env=environment(credential), capture_output=True,
                                    text=True, stdin=subprocess.DEVNULL, timeout=15)
            assert secret not in result.stdout + result.stderr
            return result

        def success(result):
            assert result.returncode == 0, result.stderr
            return result

        def failure(result, message):
            assert result.returncode != 0 and message in result.stderr, result

        def record(path, turns=None, cfg=config):
            args = ['agent', cfg, '--record', path]
            if turns is not None:
                args += ['--pause-after', str(turns)]
            return run(*args, '--', 'Write a result 日本語')

        def entries(path):
            return [json.loads(line)['payload'] for line in path.read_text().splitlines()]

        def clone(source, destination):
            shutil.copyfile(source, destination)
            destination.chmod(0o600)

        def inspect(path, status):
            original = path.read_bytes(); count = len(requests)
            result = success(run('agent-journal', path, credential=False))
            report = json.loads(result.stdout)
            assert report['status'] == status and report['hash_chain_verified'] is True and report['replay_verified'] is False, report
            assert path.read_bytes() == original and len(requests) == count
            assert 'original write' not in result.stdout and 'Write a result' not in result.stdout and 'Interrupted task' not in result.stdout
            assert not any(key in report for key in ('task', 'output', 'messages', 'arguments'))
            return report

        # Persist before and after the tool separately, then resume in a new process.
        journal = root / 'run.jsonl'
        before = len(requests)
        paused = success(record(journal, 2))
        assert not paused.stdout and 'checkpoint saved' in paused.stderr
        assert result_file.read_text() == 'original write'
        assert len(requests) == before + 1
        assert stat.S_IMODE(journal.stat().st_mode) == 0o600
        assert secret not in journal.read_text()
        assert entries(journal)[-1]['kind'] == 'checkpoint'
        assert inspect(journal, 'checkpoint')['completed_operations'] == 2
        result_file.write_text('user edit after checkpoint')
        resumed = success(run('agent-resume', config, journal))
        assert json.loads(resumed.stdout) == {'ok': 'wrote 14 bytes to result.txt'}
        assert result_file.read_text() == 'user edit after checkpoint', 'resume duplicated the write'
        assert len(requests) == before + 2, 'resume repeated the first model request'
        assert entries(journal)[-1]['kind'] == 'complete'
        summary = inspect(journal, 'complete')
        assert summary['operations_by_kind'] == {'model': 2, 'tool': 1}
        assert summary['operations_by_actor'] == {'root': 3} and summary['pending_operation'] is None
        journal.chmod(0o400); inspect(journal, 'complete'); journal.chmod(0o600)
        unavailable = root / 'agent-unavailable.wasm'
        local_agent.rename(unavailable)
        try: inspect(journal, 'complete')
        finally: unavailable.rename(local_agent)
        print('PASS: durable checkpoint resumes without repeating a completed model call or file write')

        # Replay has no credential and a rejecting endpoint; journals stay byte-identical.
        reject_network = True
        before = len(requests)
        original = journal.read_bytes()
        replayed = success(run('agent-replay', config, journal, credential=False))
        assert replayed.stdout == resumed.stdout
        assert len(requests) == before and journal.read_bytes() == original
        assert result_file.read_text() == 'user edit after checkpoint'
        reject_network = False
        print('PASS: replay verifies WASM decisions without network, credentials, writes or journal mutation')

        # Every completed broker result is a recoverable crash boundary.
        lines = journal.read_bytes().splitlines(keepends=True)
        for index, payload in enumerate(entries(journal)):
            if payload['kind'] != 'result':
                continue
            prefix = root / f'prefix-{index}.jsonl'
            prefix.write_bytes(b''.join(lines[:index + 1]))
            prefix.chmod(0o600)
            inspect(prefix, 'incomplete')
            result_file.write_text('checkpoint marker')
            before = len(requests)
            success(run('agent-resume', config, prefix))
            completed = entries(journal)[:index + 1]
            past_models = sum(e['kind'] == 'intent' and e['request']['kind'] == 'model' for e in completed)
            assert len(requests) == before + 2 - past_models
            if any(e['kind'] == 'intent' and e['request']['kind'] == 'tool' for e in completed):
                assert result_file.read_text() == 'checkpoint marker'
        print('PASS: every durable result boundary can be resumed with no repeated completed operations')

        # Empty provider replies after a completed write recover from history.
        # Resume must retain both the tool result and the consecutive-empty limit.
        recovery = root / 'empty-recovery.jsonl'
        empty_after_tool = 2
        before = len(requests)
        success(record(recovery, 3))
        assert len(requests) == before + 2 and empty_after_tool == 1
        result_file.write_text('external marker during recovery')
        recovered = success(run('agent-resume', config, recovery))
        assert len(requests) == before + 4
        assert result_file.read_text() == 'external marker during recovery'
        assert all(any(m['role'] == 'tool' for m in body['messages']) for body in requests[before + 2:])
        before_replay = len(requests)
        assert success(run('agent-replay', config, recovery, credential=False)).stdout == recovered.stdout
        assert len(requests) == before_replay
        exhausted = root / 'empty-exhausted.jsonl'
        empty_after_tool = 10
        before = len(requests)
        success(record(exhausted, 4))
        assert len(requests) == before + 3
        result_file.write_text('marker before final empty reply')
        failure(run('agent-resume', config, exhausted), 'after two recovery attempts')
        assert len(requests) == before + 4
        assert result_file.read_text() == 'marker before final empty reply'
        empty_after_tool = 0
        print('PASS: empty-response recovery survives resume without duplicated writes or reset recovery budgets; replay stays offline')

        # Missing credentials are a preflight failure, not an uncertain external operation.
        no_key = root / 'missing-key.jsonl'
        success(record(no_key, 1))
        failure(run('agent-resume', config, no_key, credential=False), 'credential is missing')
        assert entries(no_key)[-1]['kind'] == 'result'
        result_file.write_text('after preflight failure')
        success(run('agent-resume', config, no_key))
        assert result_file.read_text() == 'after preflight failure'

        before = len(requests)
        saved_config = config.read_text()
        config.write_text(saved_config + '\n# changed policy identity\n')
        failure(run('agent-replay', config, journal), 'fingerprint mismatch')
        config.write_text(saved_config)
        original_wasm = local_agent.read_bytes()
        local_agent.write_bytes(original_wasm + b'\x00\x05\x04test')
        failure(run('agent-replay', config, journal), 'fingerprint mismatch')
        local_agent.write_bytes(original_wasm)
        assert len(requests) == before
        bad = root / 'bad.jsonl'
        clone(journal, bad)
        data = bad.read_text().replace('original write', 'tampered write', 1)
        bad.write_text(data)
        failure(run('agent-replay', config, bad), 'hash chain mismatch')
        failure(run('agent-journal', bad), 'hash chain mismatch')
        clone(journal, bad)
        bad.write_bytes(bad.read_bytes()[:-1])
        failure(run('agent-resume', config, bad), 'truncated')
        failure(run('agent-journal', bad), 'truncated')
        clone(journal, bad)
        bad.chmod(0o644)
        failure(run('agent-resume', config, bad), 'permissions')
        failure(run('agent-journal', bad), 'permissions')
        failure(record(journal), 'open agent journal')
        failure(record(workspace / 'exposed.jsonl'), 'outside every tool mount')
        assert not (workspace / 'exposed.jsonl').exists()
        print('PASS: config/WASM binding, integrity chain, truncation, permissions and mount isolation')

        # A validly rehashed but semantically changed record must diverge against WASM.
        records = [json.loads(line)['payload'] for line in journal.read_text().splitlines()]
        records[1]['request']['decision']['messages'][0]['content'] = 'different decision'
        previous = ''
        encoded = []
        for payload in records:
            canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
            digest = hashlib.sha256((previous + '\n' + canonical).encode()).hexdigest()
            encoded.append(json.dumps({'previous': previous, 'hash': digest, 'payload': payload}, ensure_ascii=False))
            previous = digest
        bad.chmod(0o600)
        bad.write_text('\n'.join(encoded) + '\n')
        assert inspect(bad, 'complete')['replay_verified'] is False
        failure(run('agent-replay', config, bad), 'replay diverged')
        print('PASS: replay recomputes guest decisions and detects divergence beyond file checksums')

        # Root and child operations share a journal and budget across restarts.
        team = root / 'team.toml'
        team.write_text(f'''version = 1
[agent]
wasm = "agent.wasm"
instruction = "Delegate work to the worker."
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "root"
token_env = "PORTA_TEST_TOKEN"
[[agents]]
name = "worker"
config = "agent.toml"
''')
        team_journal = root / 'team.jsonl'
        before = len(requests)
        success(record(team_journal, 2, team))
        assert len(requests) == before + 3
        assert any(e.get('request', {}).get('actor') == 'root/delegate_worker' for e in entries(team_journal))
        result_file.write_text('team checkpoint marker')
        answer = success(run('agent-resume', team, team_journal))
        assert len(requests) == before + 4 and result_file.read_text() == 'team checkpoint marker'
        assert success(run('agent-replay', team, team_journal, credential=False)).stdout == answer.stdout
        assert len(requests) == before + 4
        assert set(inspect(team_journal, 'complete')['operations_by_actor']) == {'root', 'root/delegate_worker'}
        limited = root / 'limited.toml'
        limited.write_text(saved_config + '\n[limits]\nmax_model_calls = 1\n')
        limited_journal = root / 'limited.jsonl'
        success(record(limited_journal, 2, limited))
        result_file.write_text('budget checkpoint marker')
        before = len(requests)
        for _ in range(2):
            failure(run('agent-resume', limited, limited_journal), 'model call budget exceeded')
        assert len(requests) == before and result_file.read_text() == 'budget checkpoint marker'
        print('PASS: delegated-team continuation and root budgets survive resume and replay')

        # Paused wall time is free; prior active time is charged on resumption.
        timed = root / 'timed.toml'
        timed.write_text(saved_config + '\n[limits]\ntimeout_seconds = 2\n')
        idle_journal = root / 'idle.jsonl'
        success(record(idle_journal, 1, timed))
        time.sleep(2.1)
        success(run('agent-resume', timed, idle_journal))
        active_journal = root / 'active.jsonl'
        response_delay = 1.2
        success(record(active_journal, 1, timed))
        assert entries(active_journal)[-1]['elapsed_ms'] >= 1100
        result = run('agent-resume', timed, active_journal)
        assert result.returncode != 0 and ('timeout' in result.stderr or 'deadline' in result.stderr), result
        response_delay = 0
        failure(run('agent', config, '--pause-after', '99999999999999999999999', '--', 'task'), 'too large')
        failure(run('agent', config, '--pause-after', '1', '--', 'task'), 'requires --record')
        failure(run('agent-replay', config, limited_journal), 'requires a completed journal')
        print('PASS: resume preserves active-time budget, excludes pause time, and rejects invalid CLI input')


        # Kill a process while its external request is in flight. Never silently retry.
        block = True
        pending = root / 'uncertain.jsonl'
        process = subprocess.Popen([porta, 'agent', str(config), '--record', str(pending), '--', 'Interrupted task'],
                                   env=environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            assert entered.wait(10), 'model request did not start'
            failure(run('agent-resume', config, pending), 'already in use')
            failure(run('agent-journal', pending), 'already in use')
            os.kill(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
            before = len(requests)
            summary = inspect(pending, 'uncertain')
            assert summary['completed_operations'] == 0
            assert summary['pending_operation'] == {'index': 0, 'actor': 'root', 'kind': 'model', 'name': None}
            structured = root / 'structured-metadata.jsonl'
            modified = [json.loads(line)['payload'] for line in pending.read_text().splitlines()]
            modified[-1]['request']['actor'] = {'private': 'must-not-print-structured'}
            modified[-1]['request']['kind'] = ['must-not-print-structured']
            modified[-1]['request']['request']['name'] = {'private': 'must-not-print-structured'}
            prior = ''; encoded = []
            for payload in modified:
                canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
                checksum = hashlib.sha256((prior + '\n' + canonical).encode()).hexdigest()
                encoded.append(json.dumps({'previous': prior, 'hash': checksum, 'payload': payload}, ensure_ascii=False)); prior = checksum
            structured.write_text('\n'.join(encoded) + '\n'); structured.chmod(0o600)
            redacted = inspect(structured, 'uncertain')
            assert redacted['pending_operation'] == {'index': 0, 'actor': None, 'kind': None, 'name': None}
            assert 'must-not-print-structured' not in json.dumps(redacted)
            failure(run('agent-resume', config, pending), 'outcome is uncertain')
            assert len(requests) == before
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)
            release.set()
            block = False
        print('PASS: exclusive writer lock and SIGKILL recovery refuse uncertain operation replay')
        failure(run('agent-journal'), 'expected one journal path')
        failure(run('agent-journal', journal, 'extra'), 'expected one journal path')
        assert 'without executing' in success(run('agent-journal', '--help')).stdout
        print('PASS: read-only journal inspection reports stopped states and actors without code, credentials, effects or conversation disclosure')
finally:
    release.set()
    server.shutdown()
    server.server_close()
    thread.join()
