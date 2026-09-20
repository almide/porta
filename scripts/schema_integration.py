#!/usr/bin/env python3
"""Validate arguments before real WASM writes; exercise repair and replay."""
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading

porta, agent, tool = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:4]]
arguments = {}
repair = False
requests = []
external = []
valid = {'path': 'result.txt', 'content': 'accepted'}

class Model(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        external.append(self.path)
        self.send_error(404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        results = [json.loads(m['content']) for m in body['messages'] if m['role'] == 'tool']
        if not results or repair == 'repeat' or repair and len(results) == 1:
            args = valid if results and repair != 'repeat' else arguments
            message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                'id': f'call-{len(results)}', 'type': 'function',
                'function': {'name': 'write_file', 'arguments': json.dumps(args)}}]}
        else:
            message = {'role': 'assistant', 'content': json.dumps(results[-1])}
        payload = json.dumps({'choices': [{'message': message}]}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        """Silence the request log; these tests assert on their own output."""


def toml(value):
    if isinstance(value, dict):
        return '{' + ', '.join(json.dumps(k) + '=' + toml(v) for k, v in value.items()) + '}'
    if isinstance(value, list):
        return '[' + ','.join(map(toml, value)) + ']'
    return json.dumps(value)

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-schema-') as directory:
        root = pathlib.Path(directory)
        workspace = root / 'workspace'
        workspace.mkdir()
        output = workspace / 'result.txt'
        config = root / 'agent.toml'
        schema = {'type': 'object', 'properties': {
            'path': {'const': 'result.txt'}, 'content': {'type': 'string', 'minLength': 1},
            'options': {'type': 'object', 'properties': {'count': {'type': 'integer', 'minimum': 1, 'maximum': 3}},
                        'required': ['count'], 'additionalProperties': False},
            'tags': {'type': 'array', 'items': {'enum': ['a', 'b']}, 'uniqueItems': True, 'maxItems': 2}},
            'required': ['path', 'content'], 'additionalProperties': False}

        def configure(spec):
            config.write_text(f'''version = 1
[agent]
wasm = {json.dumps(agent)}
[model]
endpoint = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
name = "schema-test"
[limits]
max_steps = 12
[[tools]]
name = "write_file"
wasm = {json.dumps(tool)}
mounts = [{{host="workspace",guest=".",read_only=false}}]
input_schema = {toml(spec)}
''')

        def run(*args):
            return subprocess.run([porta, *map(str, args)], capture_output=True, text=True,
                                  stdin=subprocess.DEVNULL, timeout=15)

        def success(result):
            assert result.returncode == 0, result.stderr
            return result

        configure(schema)
        cases = [dict(valid, content=42), {'path': 'result.txt'}, dict(valid, extra=True),
                 dict(valid, path='other.txt'), dict(valid, options={'count': 0}),
                 dict(valid, options={'count': 1.5}), dict(valid, options={'count': 4}),
                 dict(valid, tags=['a', 'a']), dict(valid, tags=['c'])]
        for arguments in cases:
            result = success(run('agent', config, '--', 'write'))
            assert json.loads(result.stdout)['error']['code'] == 'invalid_tool_arguments', result
            assert list(workspace.iterdir()) == [], 'invalid arguments reached the writing tool'
        print('PASS: required, types, constants, nested bounds, arrays and extra fields prevent WASM writes')

        arguments = dict(valid, options={'count': 2}, tags=['a', 'b'])
        result = success(run('agent', config, '--', 'write'))
        assert json.loads(result.stdout).get('ok') and output.read_text() == 'accepted'
        output.unlink()
        print('PASS: valid nested arguments execute the granted WASM tool')

        # Validation errors are deterministic events, not uncertain tool effects.
        arguments = dict(valid, content=42)
        repair = True
        journal = root / 'repair.jsonl'
        success(run('agent', config, '--record', journal, '--pause-after', '2', '--', 'write'))
        assert not output.exists()
        entries = [json.loads(line)['payload'] for line in journal.read_text().splitlines()]
        assert not any(e.get('request', {}).get('kind') == 'tool' for e in entries)
        success(run('agent-resume', config, journal))
        assert output.read_text() == 'accepted'
        output.write_text('human change')
        before = len(requests)
        success(run('agent-replay', config, journal))
        assert len(requests) == before and output.read_text() == 'human change'
        output.unlink()
        repair = False
        print('PASS: model repairs invalid arguments; checkpoint and offline replay preserve validation')

        # Internal references and unions use full schema semantics.
        configure({'$defs': {'text': {'type': 'string', 'pattern': '^accepted$'}},
                   'type': 'object', 'properties': {'content': {'$ref': '#/$defs/text'}},
                   'oneOf': [{'required': ['path']}, {'required': ['alternative']}]})
        arguments = valid
        success(run('agent', config, '--', 'write'))
        assert output.read_text() == 'accepted'
        output.unlink()
        arguments = dict(valid, alternative=True)
        result = success(run('agent', config, '--', 'write'))
        assert json.loads(result.stdout)['error']['code'] == 'invalid_tool_arguments'
        assert not output.exists()
        print('PASS: internal references and oneOf are enforced')

        configure({'type': 'object', 'properties': {'content': {'type': 'string', 'format': 'email'}}})
        arguments = valid
        result = success(run('agent', config, '--', 'write'))
        assert json.loads(result.stdout)['error']['code'] == 'invalid_tool_arguments'
        assert not output.exists()
        print('PASS: known formats are asserted')

        configure(schema)
        arguments = dict(valid, content=42)
        repair = 'repeat'
        before = len(requests)
        result = run('agent', config, '--', 'write')
        assert result.returncode != 0 and 'step budget exceeded' in result.stderr, result
        assert len(requests) - before <= 6 and not output.exists()
        repair = False
        print('PASS: repeated invalid tool attempts exhaust the shared step budget')

        # A valid local schema would allow execution if file resolution were enabled.
        local_schema = root / "external.json"
        local_schema.write_text(json.dumps(schema))
        # No model traffic or external schema retrieval on invalid configuration.
        for spec in [ {'type': 'invalid'}, {'type': 'object', 'required': 'content'},
                      {'$ref': f'http://127.0.0.1:{server.server_port}/schema'},
                      {'$ref': local_schema.as_uri()}, {'$ref': '#/$defs/missing'},
                      {'type': 'string', 'format': 'not-a-format'} ]:
            configure(spec)
            before = len(requests)
            result = run('agent', config, '--', 'write')
            assert result.returncode != 0 and 'input_schema' in result.stderr, result
            assert len(requests) == before and not external
        print('PASS: malformed schemas and external references fail before model or tool execution')
finally:
    server.shutdown()
    server.server_close()
    thread.join()
