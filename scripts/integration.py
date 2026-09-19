#!/usr/bin/env python3
"""Offline process-level regressions; run against the newly built binary."""
import json
import http.server
import threading
import pathlib
import platform
import subprocess
import sys
import tempfile

porta = str(pathlib.Path(sys.argv[1]).resolve())

def run(*args, **kwargs):
    return subprocess.run([porta, *args], text=True, capture_output=True, timeout=30, **kwargs)

with tempfile.TemporaryDirectory(prefix='porta-integration-') as directory:
    root = pathlib.Path(directory)
    wasm = root / 'agent.wasm'
    # (module (func (export "_start"))) — no imports, network, or compiler needed.
    wasm.write_bytes(bytes.fromhex('0061736d0100000001040160000003020100070a01065f737461727400000a040102000b'))
    (root / 'agent.manifest.json').write_text(json.dumps({
        'schema_version': '1.0', 'name': 'test', 'version': '1.0.0',
        'capabilities': [], 'entry': '_start', 'tools': [], 'resources': [], 'prompts': [],
    }))
    # An untrusted sidecar must never be read as native executable code.
    sidecar = root / 'agent.wasm.porta-cache'
    sidecar.write_bytes(b'untrusted native cache')
    result = run('run', str(wasm))
    assert result.returncode == 0, result.stderr
    assert sidecar.read_bytes() == b'untrusted native cache'
    messages = [{'jsonrpc': '2.0', 'id': 0, 'method': 'initialize', 'params': {}}]
    messages += [{'jsonrpc': '2.0', 'method': 'notifications/initialized'},
                 {'jsonrpc': '2.0', 'method': 'notifications/cancelled', 'params': {'requestId': -1}}]
    messages += [{'jsonrpc': '2.0', 'id': f'日本語-{i}', 'method': 'ping'} for i in range(2000)]
    result = run('serve', str(wasm), input=''.join(json.dumps(m, ensure_ascii=False) + '\n' for m in messages))
    assert result.returncode == 0, result.stderr
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(replies) == 2001, len(replies)
    assert replies[-1]['id'] == '日本語-1999'
    print('PASS: WASM, untrusted cache, MCP stdio, Unicode, notifications, 2000 requests')

    # Imported host functions must be rejected before instantiation, even with full profile.
    for name in ('exec_command', 'http_request'):
        payload = b'\x01\x05porta' + bytes([len(name)]) + name.encode() + b'\x00\x00'
        forbidden = root / (name + '.wasm')
        forbidden.write_bytes(bytes.fromhex('0061736d01000000010401600000') + bytes([2, len(payload)]) + payload)
        result = run('run', str(forbidden), '--profile', 'full')
        assert result.returncode != 0 and 'unsupported host import' in result.stderr, result
    print('PASS: unchecked WASM host imports rejected')

    paths = []
    class RedirectServer(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            paths.append(self.path)
            self.send_response(302)
            self.send_header('Location', '/must-not-follow')
            self.end_headers()
            self.wfile.write(b'redirect not followed')
        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), RedirectServer)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        port = server.server_port
        request = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {
            'name': 'porta.http', 'arguments': {'url': f'http://127.0.0.1:{port}/redirect'}}}
        result = run('serve', str(wasm), '--profile', 'full', '--allow-net', f'127.0.0.1:{port}',
                     input=json.dumps(request) + '\n')
        assert result.returncode == 0, result.stderr
        reply = json.loads(result.stdout)
        assert reply['result']['content'][0]['text'] == 'redirect not followed', reply
        assert paths == ['/redirect'], paths
        if platform.system() == 'Darwin':
            probe = r'''import errno, os, socket, sys, urllib.parse
proxy = urllib.parse.urlsplit(os.environ['HTTPS_PROXY'])
with socket.create_connection((proxy.hostname, proxy.port), 2) as stream:
    stream.sendall(b'CONNECT blocked.example:443 HTTP/1.1\r\n\r\n')
    assert b'403 Forbidden' in stream.recv(4096)
try:
    socket.create_connection(('127.0.0.1', int(sys.argv[1])), 2)
except OSError as error:
    assert error.errno in (errno.EPERM, errno.EACCES), error
else:
    raise AssertionError('direct socket bypassed the proxy')
'''
            result = run('run', sys.executable, '--proxy-allow', 'api.example.com',
                         '--', '-c', probe, str(port))
            assert result.returncode == 0, result.stderr
            print('PASS: CONNECT policy denial and direct-socket bypass blocked')
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    print('PASS: checked HTTP tool does not follow redirects')

    if platform.system() == 'Darwin':
        result = run('run', '/bin/sh', '--', '-c', 'exit 7')
        assert result.returncode == 7, result
        # TOML entry uses the same proxy path, with no external requests.
        (root / 'porta.toml').write_text('[runtime]\ntype = "native"\ncommand = "/bin/sh"\n'
            '[sandbox]\nmounts = []\n[proxy]\nallow = ["api.example.com"]\n')
        result = run('up', '--', '-c', 'test -n "$HTTPS_PROXY"', cwd=root)
        assert result.returncode == 0, result.stderr
        assert 'proxy listening on 127.0.0.1:' in result.stderr
        # Verify actual OS enforcement by attempting a write outside all grants.
        result = run('run', '/bin/sh', '--', '-c', 'touch "$1"', 'sh', str(pathlib.Path(__file__).resolve().parent.parent / ('.' + root.name)))
        assert result.returncode != 0, result
        print('PASS: native exit code, porta.toml proxy enforcement, default write denial')
    else:
        result = run('run', '/bin/echo', '--', 'must-not-execute')
        assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
        print('PASS: unsupported native sandbox fails closed')
