#!/usr/bin/env python3
"""Real WASM agents use explicitly granted MCP HTTP tools and durable records."""
import http.server
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import threading
import time
from wasm_fixtures import fixed_stdout_guest

porta, agent = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:3]]
history_check = str(pathlib.Path(sys.argv[3]).resolve()) if len(sys.argv) > 3 else None
secret = 'mcp-only-secret'
mode = 'json'
arguments = {'text': 'こんにちは'}
selected = 'remote_echo'
traffic = []
effects = []
leaks = []
entered = threading.Event()
release = threading.Event()

class Server(http.server.BaseHTTPRequestHandler):
    def answer(self, data, status=200, mime='application/json', headers=None):
        payload = data if isinstance(data, bytes) else json.dumps(data, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(payload)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        try:
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_DELETE(self):
        traffic.append(('delete', self.path))
        assert self.headers.get('MCP-Session-Id') == 'fixture-session'
        self.answer(b'', 204)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/model':
            traffic.append(('model', body))
            assert secret not in json.dumps(body)
            results = [m for m in body['messages'] if m['role'] == 'tool']
            if results:
                message = {'role': 'assistant', 'content': results[-1]['content']}
            else:
                message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                    'id': 'call-1', 'type': 'function',
                    'function': {'name': selected, 'arguments': json.dumps(arguments)}}]}
            self.answer({'choices': [{'message': message}]})
            return
        traffic.append((body.get('method'), body))
        assert self.path == '/mcp'
        assert self.headers.get('Authorization') == 'Bearer ' + secret
        assert self.headers.get('Accept') == 'application/json, text/event-stream'
        if mode == 'redirect':
            self.answer(b'', 307, headers={'Location': '/stolen'})
            return
        method = body.get('method')
        if method == 'initialize':
            assert body['params']['capabilities'] == {}
            result = {'protocolVersion': 'unknown' if mode == 'version' else '2025-11-25',
                      'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1'}}
            self.answer({'jsonrpc': '2.0', 'id': body['id'], 'result': result},
                        headers={'MCP-Session-Id': 'fixture-session'})
            return
        assert self.headers.get('MCP-Session-Id') == 'fixture-session'
        assert self.headers.get('MCP-Protocol-Version') == '2025-11-25'
        if method == 'notifications/initialized':
            self.answer(b'', 202)
            return
        assert method == 'tools/call' and body['params']['name'] == 'echo'
        effects.append(body['params'])
        if mode == 'block':
            entered.set()
            release.wait(10)
        if mode == 'slow':
            time.sleep(2)
        result = {'content': [{'type': 'text', 'text': body['params']['arguments']['text']}],
                  'structuredContent': {'echo': body['params']['arguments']['text']}, 'isError': mode == 'tool-error'}
        if mode.startswith('render_'):
            text = 'line one\n日本語 "quoted" \\path'
            result = {'content': [{'type': 'text', 'text': text}], 'isError': False}
            if mode == 'render_wrapper':
                result['structuredContent'] = {'result': text}
            elif mode == 'render_json':
                result['structuredContent'] = {'answer': 42}
                result['content'][0]['text'] = '{ "answer": 42 }'
            elif mode == 'render_conflict':
                result['structuredContent'] = {'different': True}
            elif mode == 'render_error':
                result['isError'] = True
            elif mode == 'render_multi':
                result['content'].append({'type': 'text', 'text': 'second'})
            elif mode == 'render_image':
                result['content'] = [{'type': 'image', 'mimeType': 'image/png', 'data': 'AA=='}]
            elif mode == 'render_missing_text':
                result['content'] = [{'type': 'text'}]
            elif mode == 'render_annotations':
                result['content'][0]['annotations'] = {'audience': ['assistant']}
        if mode.startswith('verified_'):
            verdict = {'passed': mode != 'verified_fail'}
            result = {'content': [{'type': 'text', 'text': json.dumps(verdict)}],
                      'structuredContent': verdict.copy(), 'isError': mode == 'verified_error'}
            if mode in ('verified_wrapped', 'verified_wrapped_conflict'):
                result['structuredContent'] = {'result': json.dumps({'passed': mode == 'verified_wrapped'})}
            if mode == 'verified_conflict':
                result['structuredContent'] = {'passed': False}
            if mode == 'verified_malformed':
                result['content'][0]['text'] = 'not JSON'
        response = {'jsonrpc': '2.0', 'id': body['id'], 'result': result}
        if mode == 'id':
            response['id'] = 999
        if mode == 'result':
            response['result'] = {'content': 'invalid'}
        if mode == 'rpc-error':
            response = {'jsonrpc': '2.0', 'id': body['id'], 'error': {'code': -32603, 'message': secret}}
        if mode == 'large':
            response['result']['content'][0]['text'] = 'x' * (1024 * 1024)
        if mode in ('sse', 'sse-cr', 'server-request', 'disconnect', 'events'):
            notification = {'jsonrpc': '2.0', 'method': 'notifications/progress', 'params': {'progress': 1}}
            if mode == 'server-request':
                notification = {'jsonrpc': '2.0', 'id': 'server', 'method': 'sampling/createMessage', 'params': {}}
            frames = ': heartbeat\r\nid: ignored\r\ndata:\r\n\r\n'
            frames += 'data: ' + json.dumps(notification) + '\r\n\r\n'
            if mode == 'events':
                frames += ': heartbeat\n\n' * 130
            if mode != 'disconnect':
                # Legal multiline JSON SSE data, UTF-8 and CRLF framing.
                frames += 'data: ' + json.dumps(response, ensure_ascii=False, indent=2).replace('\n', '\ndata: ') + '\n\n'
            if mode == 'sse-cr':
                frames = '\ufeff' + frames.replace('\r\n', '\n').replace('\n', '\r')
            self.answer(frames.encode(), mime='text/event-stream; charset=utf-8')
        else:
            self.answer(response)

    def log_message(self, *args):
        pass

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Server)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-mcp-agent-') as directory:
        root = pathlib.Path(directory)
        config = root / 'agent.toml'
        base = f'''version = 1
[agent]
wasm = {json.dumps(agent)}
[model]
endpoint = "http://127.0.0.1:{server.server_port}/model"
name = "fixture"
[[mcp_tools]]
name = "remote_echo"
remote_name = "echo"
endpoint = "http://127.0.0.1:{server.server_port}/mcp"
token_env = "PORTA_MCP_TOKEN"
input_schema = {{type="object",properties={{text={{type="string"}}}},required=["text"],additionalProperties=false}}
'''
        config.write_text(base)

        def environment(token=True):
            env = dict(os.environ)
            env.pop('PORTA_MCP_TOKEN', None)
            if token:
                env['PORTA_MCP_TOKEN'] = secret
            # These proxies must never receive either request.
            env.update(HTTPS_PROXY='http://127.0.0.1:1', HTTP_PROXY='http://127.0.0.1:1', ALL_PROXY='http://127.0.0.1:1')
            return env

        def run(*args, token=True):
            result = subprocess.run([porta, *map(str, args)], env=environment(token), capture_output=True,
                                    text=True, stdin=subprocess.DEVNULL, timeout=12)
            assert secret not in result.stdout + result.stderr, result
            return result

        def success(result):
            assert result.returncode == 0, result.stderr
            return result

        def failure(result, text):
            assert result.returncode != 0 and text in result.stderr, result

        for mode in ('json', 'sse', 'sse-cr', 'tool-error'):
            before = len(effects)
            result = success(run('agent', config, '--', 'echo'))
            output = json.loads(result.stdout)
            assert output['content'][0]['text'] == 'こんにちは' and output['isError'] == (mode == 'tool-error')
            assert len(effects) == before + 1
            assert traffic[-2][0] == 'delete'
        print('PASS: JSON/SSE MCP calls, lifecycle/session headers, Unicode and tool errors; no inherited proxies')

        for mode in ('render_plain', 'render_wrapper', 'render_json', 'render_conflict', 'render_error', 'render_multi', 'render_image', 'render_missing_text', 'render_annotations'):
            rendered = success(run('agent', config, '--', 'render'))
            if mode in ('render_plain', 'render_wrapper'):
                assert rendered.stdout.rstrip('\n') == 'line one\n日本語 "quoted" \\path', rendered
            elif mode == 'render_json':
                assert rendered.stdout.strip() == '{ "answer": 42 }', rendered
            else:
                result = json.loads(rendered.stdout)
                assert isinstance(result['content'], list), result
                if mode == 'render_conflict':
                    assert result['structuredContent'] == {'different': True}
                elif mode == 'render_error':
                    assert result['isError'] is True
        print('PASS: model sees unwrapped unambiguous MCP text; conflicts, errors, multipart and unsupported content retain envelopes')

        denied = root / 'deny-before.wasm'
        denied.write_bytes(fixed_stdout_guest({'passed': False, 'feedback': 'Obtain independent approval before calling remote tools.'}))
        config.write_text(base + f'\n[[before_tool_checks]]\nname="approval"\nwasm={json.dumps(str(denied))}\n[limits]\nmax_model_calls=2\n')
        mode = 'json'
        before = len(effects); start = len(traffic)
        blocked = success(run('agent', config, '--', 'blocked remote call'))
        assert json.loads(blocked.stdout)['error']['code'] == 'tool_policy_rejected'
        assert len(effects) == before and all(item[0] == 'model' for item in traffic[start:])
        config.write_text(base)
        print('PASS: pre-tool policy rejects remote operations before any MCP connection or effect')

        mode = 'json'
        arguments = {'text': 42}
        before = len(effects)
        start = len(traffic)
        result = success(run('agent', config, '--', 'echo'))
        assert json.loads(result.stdout)['error']['code'] == 'invalid_tool_arguments'
        assert len(effects) == before and all(t[0] == 'model' for t in traffic[start:])
        arguments = {'text': 'こんにちは'}
        selected = 'not_granted'
        failure(run('agent', config, '--', 'echo'), 'tool not granted')
        assert len(effects) == before
        selected = 'remote_echo'
        print('PASS: schemas and explicit tool grants reject calls before MCP connections')

        journal = root / 'run.jsonl'
        success(run('agent', config, '--record', journal, '--pause-after', '2', '--', 'echo'))
        before = len(effects)
        success(run('agent-resume', config, journal, token=False))
        assert len(effects) == before
        start = len(traffic)
        success(run('agent-replay', config, journal, token=False))
        assert len(traffic) == start and secret not in journal.read_text()
        print('PASS: completed MCP effects survive resume; offline replay needs no MCP token or traffic')

        no_token = root / 'no-token.jsonl'
        failure(run('agent', config, '--record', no_token, '--', 'echo', token=False), 'MCP credential is missing')
        assert json.loads(no_token.read_text().splitlines()[-1])['payload']['kind'] == 'result'
        success(run('agent-resume', config, no_token))
        print('PASS: missing credentials fail before intent and can be supplied on resume')

        if history_check:
            config.write_text(base + f'\n[[completion_checks]]\nname="tool_verdict"\nwasm={json.dumps(history_check)}\nparameters={{tool="remote_echo"}}\n[limits]\nmax_model_calls=3\n')
            mode = 'verified_pass'
            verified = root / 'verified.jsonl'
            success(run('agent', config, '--record', verified, '--', 'Verify the task'))
            before = len(traffic)
            success(run('agent-replay', config, verified, token=False))
            assert len(traffic) == before
            mode = 'verified_wrapped'
            success(run('agent', config, '--', 'Verify an SDK string result'))
            for mode in ('verified_fail', 'verified_error', 'verified_conflict', 'verified_malformed', 'verified_wrapped_conflict'):
                result = run('agent', config, '--', 'Do not accept unverified completion')
                failure(result, 'model call budget exceeded')
            config.write_text(base)
            print('PASS: actual MCP verification gate accepts a recorded pass, rejects failed/error/conflicting results, and replays offline')

        failures = {'redirect': 'HTTP 307', 'version': 'unsupported MCP protocol', 'id': 'id mismatch',
                    'result': 'invalid MCP tool result', 'rpc-error': 'JSON-RPC error', 'large': 'exceeds 1 MiB',
                    'server-request': 'ungranted client capability', 'disconnect': 'without a result',
                    'events': 'exceeds 128 events'}
        for mode, error in failures.items():
            record = root / (mode + '.jsonl')
            before = len(effects)
            failure(run('agent', config, '--record', record, '--', 'echo'), error)
            assert len(effects) - before <= 1
            after = len(traffic)
            failure(run('agent-resume', config, record), 'outcome is uncertain')
            assert len(traffic) == after
        print('PASS: redirects, protocol errors, oversized streams and server requests fail without retries')

        mode = 'slow'
        config.write_text(base + '\n[limits]\ntimeout_seconds = 1\n')
        started = time.monotonic()
        result = run('agent', config, '--', 'echo')
        assert result.returncode != 0 and ('timeout' in result.stderr or 'deadline' in result.stderr), result
        assert time.monotonic() - started < 2.5
        config.write_text(base)
        print('PASS: MCP transport obeys the remaining root deadline')

        mode = 'block'
        interrupted = root / 'interrupted.jsonl'
        process = subprocess.Popen([porta, 'agent', str(config), '--record', str(interrupted), '--', 'echo'],
                                   env=environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            assert entered.wait(10)
            process.kill()
            process.communicate(timeout=5)
            before = len(effects)
            original = interrupted.read_bytes(); prior_traffic = len(traffic)
            summary = json.loads(success(run('agent-journal', interrupted, token=False)).stdout)
            assert summary['status'] == 'uncertain' and summary['replay_verified'] is False
            assert summary['pending_operation']['kind'] == 'mcp_tool'
            assert summary['pending_operation']['name'] == 'remote_echo'
            assert 'arguments' not in summary['pending_operation']
            assert interrupted.read_bytes() == original and len(traffic) == prior_traffic
            failure(run('agent-resume', config, interrupted), 'outcome is uncertain')
            assert len(effects) == before
        finally:
            release.set()
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)
        print('PASS: SIGKILL during remote effects refuses duplicate execution on recovery')
finally:
    release.set()
    server.shutdown()
    server.server_close()
    thread.join()
