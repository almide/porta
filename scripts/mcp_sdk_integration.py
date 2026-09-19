#!/usr/bin/env python3
"""Interop with the official mcp==1.30.0 Python SDK, including actual file effects."""
import http.server
import importlib.metadata
import json
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time

from mcp.server.fastmcp import FastMCP
import uvicorn

porta, agent = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:3]]
model_calls = []

class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        model_calls.append(body)
        results = [m for m in body['messages'] if m['role'] == 'tool']
        if results:
            message = {'role': 'assistant', 'content': results[-1]['content']}
        else:
            message = {'role': 'assistant', 'tool_calls': [{
                'id': 'write-1', 'type': 'function',
                'function': {'name': 'remote_write', 'arguments': json.dumps({'text': '公式 SDK 日本語'})}}]}
        payload = json.dumps({'choices': [{'message': message}]}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass

model = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
model_thread = threading.Thread(target=model.serve_forever, daemon=True)
model_thread.start()
try:
    for json_response, stateless in [(True, False), (False, False), (True, True), (False, True)]:
        with tempfile.TemporaryDirectory(prefix='porta-sdk-') as directory:
            root = pathlib.Path(directory)
            output = root / 'effect.txt'
            calls = []
            mcp = FastMCP('Porta interoperability', json_response=json_response, stateless_http=stateless)

            @mcp.tool()
            def write(text: str) -> dict:
                """Write to the fixture's fixed output file."""
                calls.append(text)
                output.write_text(text)
                return {'written': text}

            sock = socket.socket()
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
            server = uvicorn.Server(uvicorn.Config(mcp.streamable_http_app(), log_level='error'))
            thread = threading.Thread(target=server.run, kwargs={'sockets': [sock]}, daemon=True)
            thread.start()
            try:
                deadline = time.monotonic() + 10
                while not server.started and thread.is_alive() and time.monotonic() < deadline:
                    time.sleep(0.01)
                assert server.started, 'official SDK failed to start'
                config = root / 'agent.toml'
                config.write_text(f'''version = 1
[agent]
wasm = {json.dumps(agent)}
[model]
endpoint = "http://127.0.0.1:{model.server_port}/model"
name = "fixture"
[[mcp_tools]]
name = "remote_write"
remote_name = "write"
endpoint = "http://127.0.0.1:{port}/mcp"
input_schema = {{type="object",properties={{text={{type="string"}}}},required=["text"],additionalProperties=false}}
''')
                journal = root / 'run.jsonl'

                def run(*args):
                    result = subprocess.run([porta, *map(str, args)], capture_output=True, text=True,
                                            stdin=subprocess.DEVNULL, timeout=15)
                    assert result.returncode == 0, result.stderr
                    return result

                run('agent', config, '--record', journal, '--pause-after', '2', '--', 'write')
                assert output.read_text() == '公式 SDK 日本語' and len(calls) == 1
                output.write_text('human edit')
                resumed = run('agent-resume', config, journal)
                response = json.loads(resumed.stdout)
                assert response == {'written': '公式 SDK 日本語'}, response
                assert len(calls) == 1 and output.read_text() == 'human edit'
                # Remove the real endpoint before replay to prove offline behavior.
                server.should_exit = True
                thread.join(5)
                assert not thread.is_alive(), 'SDK server failed to stop'
                before = len(model_calls)
                replay = run('agent-replay', config, journal)
                assert replay.stdout == resumed.stdout and len(model_calls) == before
                assert output.read_text() == 'human edit'
                print(f'PASS: official SDK {importlib.metadata.version("mcp")} JSON={json_response} stateless={stateless}; write, resume and offline replay')
            finally:
                server.should_exit = True
                thread.join(5)
                sock.close()
finally:
    model.shutdown()
    model.server_close()
    model_thread.join()
