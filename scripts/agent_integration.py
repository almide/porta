#!/usr/bin/env python3
"""Real WASM agent/model/tool loop, using a local deterministic model server."""
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
from types import SimpleNamespace
import time
from wasm_fixtures import no_environment_guest

porta, agent, tool, probe = map(lambda p: str(pathlib.Path(p).resolve()), sys.argv[1:5])
requests = []
mode = 'add'
refusal = False
# Replies the fixture model still owes; the handler counts them down.
state = SimpleNamespace(empty_remaining=0)
secret = 'porta-fixture-secret-never-in-wasm'

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append((body, self.headers.get('Authorization')))
        messages = body['messages']
        results = [m for m in messages if m['role'] == 'tool']
        if not results:
            name, arguments = {
                'add': ('add', {'a': 20, 'b': 22}),
                'unknown': ('shell', {'command': 'touch escaped'}),
                'write': ('write_file', {'path': 'result.txt', 'content': 'written inside WASM'}),
                'read_escape': ('read_file', {'path': '../secret.txt'}),
                'read_symlink': ('read_file', {'path': 'escape'}),
            }[mode]
            if body['model'] == 'fixture-root':
                name, arguments = 'delegate_worker', {'task': 'Calculate 20 + 22 with your tool'}
            message = {'role': 'assistant', 'content': None, 'tool_calls': [
                {'id': 'call-1', 'type': 'function', 'function': {'name': name, 'arguments': json.dumps(arguments)}}]}
        else:
            # Completion is derived from the WASM tool's actual result.
            message = {'role': 'assistant', 'content': results[-1]['content']}
        if state.empty_remaining > 0:
            state.empty_remaining -= 1
            message = {'role': 'assistant', 'content': '  \n', 'tool_calls': []}
        if refusal:
            if refusal == 'with_tools':
                message['refusal'] = 'Cannot complete this request.'
            else:
                message = {'role': 'assistant', 'content': None, 'refusal': 'Cannot complete this request.'}
        payload = json.dumps({'choices': [{'message': message, 'finish_reason': 'stop'}]}).encode()
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
    with tempfile.TemporaryDirectory(prefix='porta-agent-') as directory:
        root = pathlib.Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        (root / 'secret.txt').write_text('outside-secret')
        (workspace / 'escape').symlink_to(root / 'secret.txt')

        def config(name='add', read_only=True, wasm=agent, limits='', extra=''):
            path = root / 'agent.toml'
            mount = '' if name == 'add' else f'mounts = [{{host="workspace",guest=".",read_only={str(read_only).lower()}}}]\n'
            path.write_text(f'''version = 1
[agent]
wasm = {json.dumps(wasm)}
instruction = "Use tools to solve the task."
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "fixture-model"
token_env = "PORTA_TEST_TOKEN"
[limits]
{limits}
[[tools]]
name = "{name}"
wasm = {json.dumps(tool)}
description = "Fixture WASM tool"
{mount}
{extra}
''')
            return path

        def run(path, **kwargs):
            env = dict(os.environ, PORTA_TEST_TOKEN=secret)
            result = subprocess.run([porta, 'agent', str(path), '--', 'Calculate 日本語'],
                                    env=env, text=True, capture_output=True, timeout=15, **kwargs)
            assert secret not in result.stdout + result.stderr, result
            return result

        result = run(config())
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout) == {'ok': '42'}, result.stdout
        assert len(requests) == 2
        assert all(auth == 'Bearer ' + secret for _, auth in requests)
        assert all(secret not in json.dumps(body) for body, _ in requests)
        assert requests[0][0]['model'] == 'fixture-model'
        assert requests[0][0]['tools'][0]['function']['name'] == 'add'
        print('PASS: real WASM model -> tool -> model -> completion; host-only credential')

        for empties in (1, 2):
            state.empty_remaining = empties
            before = len(requests)
            expected = empties + 2
            result = run(config())
            assert result.returncode == 0 and json.loads(result.stdout) == {'ok': '42'}, result
            assert len(requests) - before == expected
            assert all(message.get('content', '').strip() for body, _ in requests[before:] for message in body['messages'] if message.get('role') == 'user')
        state.empty_remaining = 10
        before = len(requests)
        result = run(config())
        assert result.returncode != 0 and 'after two recovery attempts' in result.stderr, result
        assert len(requests) - before == 3
        state.empty_remaining = 1
        before = len(requests)
        result = run(config(limits='max_model_calls = 1'))
        assert result.returncode != 0 and 'model call budget exceeded' in result.stderr, result
        assert len(requests) - before == 1
        state.empty_remaining = 0
        refusal = True
        before = len(requests)
        result = run(config())
        assert result.returncode != 0 and 'model refused the request' in result.stderr, result
        assert len(requests) - before == 1
        refusal = 'with_tools'
        mode = 'write'
        result = run(config('write_file', read_only=False))
        assert result.returncode != 0 and 'model refused the request' in result.stderr, result
        assert not (workspace / 'result.txt').exists()
        mode = 'add'
        refusal = False
        print('PASS: empty responses recover with preserved history, bounded attempts and shared model budget')

        assert all('temperature' not in body for body, _ in requests)
        for temperature in ('0.0', '0.7', '2.0'):
            path = config()
            path.write_text(path.read_text().replace('[limits]', 'temperature = ' + temperature + '\n[limits]'))
            result = run(path)
            assert result.returncode == 0, result.stderr
            assert requests[-1][0]['temperature'] == float(temperature)
        for temperature in ('-0.1', '2.1', 'nan', 'inf'):
            path = config()
            path.write_text(path.read_text().replace('[limits]', 'temperature = ' + temperature + '\n[limits]'))
            before = len(requests)
            result = run(path)
            assert result.returncode != 0 and 'model.temperature' in result.stderr, result
            assert len(requests) == before
        print('PASS: optional model temperature is forwarded and invalid values fail before requests')

        result = run(config(wasm=probe))
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout) == {'host_read': 'denied'}, result.stdout
        print('PASS: agent WASM cannot read host filesystem')
        env_guest = root / 'no-env.wasm'
        env_guest.write_bytes(no_environment_guest())
        result = run(config(wasm=str(env_guest)))
        assert result.returncode == 0 and result.stdout.strip() == 'no-env', result
        print('PASS: WASI environment exposes no host variables or model credentials')


        mode = 'write'
        result = run(config('write_file'))
        assert result.returncode == 0, result.stderr
        assert not (workspace / 'result.txt').exists(), 'read-only mount allowed a write'
        assert 'err' in json.loads(result.stdout), result.stdout
        result = run(config('write_file', read_only=False))
        assert result.returncode == 0, result.stderr
        assert (workspace / 'result.txt').read_text() == 'written inside WASM'
        for mode in ('read_escape', 'read_symlink'):
            result = run(config('read_file'))
            assert result.returncode == 0, result.stderr
            assert 'err' in json.loads(result.stdout), result.stdout
            assert 'outside-secret' not in result.stdout
        print('PASS: per-tool read-only/read-write grants, traversal and symlink confinement')

        mode = 'unknown'
        result = run(config())
        assert result.returncode != 0 and 'tool not granted: shell' in result.stderr, result
        mode = 'add'
        for limits, error in [('max_steps = 1', 'step budget exceeded'),
                              ('max_model_calls = 1', 'model call budget exceeded'),
                              ('fuel_per_step = 1', 'guest execution failed'),
                              ('memory_pages = 1', 'instantiate guest')]:
            result = run(config(limits=limits))
            assert result.returncode != 0 and error in result.stderr, result
        print('PASS: unknown tools denied; step, model-call, fuel and memory budgets enforced')

        result = run(config(extra='typo_grant = true'))
        assert result.returncode != 0 and 'unknown field' in result.stderr, result
        # A CPU-only infinite loop must be cut off by the wall-clock deadline.
        infinite = root / 'loop.wasm'
        infinite.write_bytes(bytes.fromhex('0061736d0100000001040160000003020100070a01065f737461727400000a0901070003400c000b0b'))
        started = time.monotonic()
        result = run(config(wasm=str(infinite), limits='timeout_seconds = 1\nfuel_per_step = 9223372036854775807'))
        assert result.returncode != 0 and 'guest execution failed' in result.stderr, result
        assert time.monotonic() - started < 5
        print('PASS: strict configuration and real-time interruption of runaway WASM')

        # Parent and child use the same WASM decision loop but independent grants.
        child = root / 'worker.toml'
        config().replace(child)
        parent = root / 'team.toml'
        parent.write_text(f'''version = 1
[agent]
wasm = {json.dumps(agent)}
instruction = "Delegate arithmetic to the worker."
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "fixture-root"
token_env = "PORTA_TEST_TOKEN"
[[agents]]
name = "worker"
config = "worker.toml"
description = "Solve arithmetic using WASM tools."
''')
        requests.clear()
        result = run(parent)
        assert result.returncode == 0, result.stderr
        assert json.loads(json.loads(result.stdout)['ok']) == {'ok': '42'}, result.stdout
        assert [r[0]['model'] for r in requests] == ['fixture-root', 'fixture-model', 'fixture-model', 'fixture-root']
        assert requests[0][0]['tools'][0]['function']['name'] == 'delegate_worker'
        assert requests[1][0]['tools'][0]['function']['name'] == 'add'
        metrics = json.loads(result.stderr.splitlines()[-1].removeprefix('[porta agent] '))
        assert metrics['delegations'] == 1 and metrics['model_calls'] == 4, metrics
        print('PASS: isolated WASM team delegates work, uses a WASM tool and returns the result')

        normal = parent.read_text()
        for budget, expected in [('max_model_calls = 2', 'team model call budget exceeded'),
                                 ('max_steps = 3', 'team step budget exceeded')]:
            parent.write_text(normal + '\n[limits]\n' + budget + '\n')
            result = run(parent)
            assert result.returncode != 0 and expected in result.stderr, result
        parent.write_text(normal)
        child.write_text(child.read_text() + '\n[[agents]]\nname = "back"\nconfig = "team.toml"\n')
        requests.clear()
        result = run(parent)
        assert result.returncode != 0 and 'cyclic agent delegation' in result.stderr, result
        assert not requests
        print('PASS: delegation cannot bypass team budgets; cycles rejected before any model request')

finally:
    server.shutdown()
    server.server_close()
    thread.join()
