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

    # Resources and prompts: what the manifest declares is what the server
    # lists, reading one dispatches a reserved tool into the module, and a name
    # the manifest does not carry is refused rather than dispatched.
    described = root / 'described.wasm'
    described.write_bytes(wasm.read_bytes())
    (root / 'described.manifest.json').write_text(json.dumps({
        'schema_version': '1.0', 'name': 'described', 'version': '1.0.0', 'capabilities': [],
        'entry': '_start', 'tools': [],
        'resources': [{'uri': 'file:///notes.txt', 'name': 'notes',
                       'description': 'Some notes', 'mime_type': 'text/plain'}],
        'prompts': [{'name': 'greet', 'description': 'Greet someone',
                     'arguments': [{'name': 'who', 'description': 'who to greet', 'required': True}]}],
    }))
    asked = [{'jsonrpc': '2.0', 'id': 1, 'method': 'resources/list', 'params': {}},
             {'jsonrpc': '2.0', 'id': 2, 'method': 'resources/read', 'params': {'uri': 'file:///notes.txt'}},
             {'jsonrpc': '2.0', 'id': 3, 'method': 'resources/read', 'params': {'uri': 'file:///missing'}},
             {'jsonrpc': '2.0', 'id': 4, 'method': 'prompts/list', 'params': {}},
             {'jsonrpc': '2.0', 'id': 5, 'method': 'prompts/get',
              'params': {'name': 'greet', 'arguments': {'who': 'world'}}},
             {'jsonrpc': '2.0', 'id': 6, 'method': 'prompts/get', 'params': {'name': 'nope'}}]
    result = run('serve', str(described), input=''.join(json.dumps(m) + '\n' for m in asked))
    assert result.returncode == 0, result.stderr
    answers = {reply['id']: reply for reply in map(json.loads, result.stdout.splitlines())}
    assert answers[1]['result']['resources'] == [{
        'uri': 'file:///notes.txt', 'name': 'notes',
        'description': 'Some notes', 'mimeType': 'text/plain'}], answers[1]
    assert answers[2]['result']['contents'][0]['mimeType'] == 'text/plain', answers[2]
    assert 'Unknown resource' in answers[3]['error']['message'], answers[3]
    assert answers[4]['result']['prompts'][0]['arguments'][0] == {
        'name': 'who', 'description': 'who to greet', 'required': True}, answers[4]
    assert answers[5]['result']['messages'][0]['role'] == 'user', answers[5]
    assert 'Unknown prompt' in answers[6]['error']['message'], answers[6]
    print('PASS: resources and prompts listed from the manifest; unknown names refused')

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
            """Silence the request log; these tests assert on their own output."""

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
        result = run('run', '/bin/sh', '--read-policy', 'nonsense', '--', '-c', 'echo must-not-execute')
        assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
        print('PASS: native exit code, porta.toml proxy enforcement, default write denial, '
              'unknown read policy refused')

        # --read-policy strict confines reads to the granted mounts and the
        # platform's own directories. A credential outside both is unreadable.
        workspace = root / 'confined'
        workspace.mkdir()
        (workspace / 'input.txt').write_text('workspace input\n')
        secrets = root / 'secrets'
        secrets.mkdir()
        (secrets / 'credentials').write_text('aws_secret_access_key = EXAMPLE\n')
        read_secret = f'cat {secrets / "credentials"}'
        result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c', read_secret)
        assert result.returncode == 0 and 'EXAMPLE' in result.stdout, result
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace),
                     '--', '-c', read_secret)
        assert result.returncode != 0, result
        assert 'EXAMPLE' not in result.stdout, result
        # The granted mount and the system directories a command needs stay
        # readable, and the shell starts without a denial on its way in.
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace),
                     '--', '-c', f'cat {workspace / "input.txt"}; grep -c . /etc/hosts')
        assert result.returncode == 0 and 'workspace input' in result.stdout, result
        assert 'not permitted' not in result.stderr, result
        # A command the policy does not cover is refused with the grant that
        # would fix it, not with a bare exec error from the kernel.
        interpreter = pathlib.Path(sys.executable).resolve()
        if not str(interpreter).startswith('/usr'):
            result = run('run', str(interpreter), '--read-policy', 'strict',
                         '-v', str(workspace), '--', '-c', 'print("must-not-execute")')
            assert result.returncode != 0, result
            assert 'must-not-execute' not in result.stdout, result
            assert 'unreadable' in result.stdout + result.stderr, result
        print('PASS: strict read policy confines reads to grants and system paths')
    elif platform.system() == 'Linux':
        result = run('run', '/bin/sh', '--', '-c', 'exit 7')
        assert result.returncode == 7, result
        workspace = root / 'landlock'
        workspace.mkdir()
        # Inside the mount the command writes; outside every grant it cannot.
        result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c', 'echo x > "$1"', 'sh',
                     str(workspace / 'inside'))
        assert result.returncode == 0, result.stderr
        assert (workspace / 'inside').read_text() == 'x\n'
        outside = pathlib.Path('/opt') / ('porta-denied-' + root.name)
        result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c', 'echo x > "$1"', 'sh', str(outside))
        assert result.returncode != 0, result
        assert not outside.exists(), outside
        # A rule this kernel cannot express must refuse the run, not widen it.
        result = run('run', '/bin/sh', '--allow-net', 'example.com:*', '-v', str(workspace),
                     '--', '-c', 'echo must-not-execute')
        assert result.returncode != 0, result
        assert 'must-not-execute' not in result.stdout, result
        assert 'numeric TCP port' in result.stdout + result.stderr, result
        # The same connect succeeds when its port is granted and fails when another is.
        import socket
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0))
        listener.listen(2)
        open_port = listener.getsockname()[1]
        connect = 'import socket,sys;socket.create_connection(("127.0.0.1",int(sys.argv[1])),2)'
        try:
            result = run('run', sys.executable, '--allow-net', f'*:{open_port}', '-v', str(workspace),
                         '--', '-c', connect, str(open_port))
            assert result.returncode == 0, result.stderr
            result = run('run', sys.executable, '--allow-net', '*:443', '-v', str(workspace),
                         '--', '-c', connect, str(open_port))
            assert result.returncode != 0, result
        finally:
            listener.close()
        print('PASS: Landlock write denial, port denial and unexpressible rule refused')

        # Proxy mode claims the loopback proxy is the only egress. Landlock's
        # rules reach TCP only, so a seccomp filter has to close the rest; the
        # claim is false if any other socket family still opens.
        families = '''import socket, sys
def opens(*args):
    try:
        socket.socket(*args).close()
        return True
    except OSError:
        return False
print('tcp4', opens(socket.AF_INET, socket.SOCK_STREAM))
print('udp4', opens(socket.AF_INET, socket.SOCK_DGRAM))
print('unix', opens(socket.AF_UNIX, socket.SOCK_STREAM))
print('tcp6', opens(socket.AF_INET6, socket.SOCK_STREAM))
print('netlink', opens(socket.AF_NETLINK, socket.SOCK_RAW, 0))
print('proxy_env', bool(__import__('os').environ.get('HTTPS_PROXY')))
'''
        result = run('run', sys.executable, '--proxy-allow', 'api.example.com',
                     '-v', str(workspace), '-v', str(pathlib.Path(sys.base_prefix).resolve()),
                     '--', '-c', families)
        assert result.returncode == 0, result.stderr
        opened = dict(line.split() for line in result.stdout.split('\n') if line)
        assert opened == {'tcp4': 'True', 'udp4': 'False', 'unix': 'False', 'tcp6': 'False',
                          'netlink': 'False', 'proxy_env': 'True'}, opened
        # Without proxy mode nothing claims to be the only egress, so nothing is
        # filtered: the restriction follows the claim, it is not always on.
        result = run('run', sys.executable, '-v', str(workspace),
                     '-v', str(pathlib.Path(sys.base_prefix).resolve()), '--', '-c', families)
        assert result.returncode == 0, result.stderr
        assert 'udp4 True' in result.stdout and 'unix True' in result.stdout, result.stdout
        print('PASS: proxy mode is the only egress — UDP, Unix, IPv6 and netlink all denied')

        # --read-policy strict confines reads to the granted mounts and the
        # platform's own directories. A credential outside both is unreadable.
        secrets = pathlib.Path('/opt') / ('porta-secrets-' + root.name)
        secrets.mkdir()
        (secrets / 'credentials').write_text('aws_secret_access_key = EXAMPLE\n')
        (workspace / 'input.txt').write_text('workspace input\n')
        read_secret = f'cat {secrets / "credentials"}'
        result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c', read_secret)
        assert result.returncode == 0 and 'EXAMPLE' in result.stdout, result
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace),
                     '--', '-c', read_secret)
        assert result.returncode != 0, result
        assert 'EXAMPLE' not in result.stdout, result
        # The granted mount and the system directories a command needs stay readable.
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace),
                     '--', '-c', f'cat {workspace / "input.txt"}')
        assert result.returncode == 0 and 'workspace input' in result.stdout, result
        # An interpreter is runnable when the policy covers it, which for a
        # hosted toolchain under /opt means granting its own directory. Without
        # that grant the run is refused, and says which grant would fix it
        # rather than failing with a bare exec error.
        interpreter = pathlib.Path(sys.executable).resolve()
        # The grant is the whole installation: an interpreter outside the
        # system set cannot reach its own standard library either.
        result = run('run', str(interpreter), '--read-policy', 'strict',
                     '-v', str(workspace), '-v', str(pathlib.Path(sys.base_prefix).resolve()),
                     '--', '-c', 'print("interpreter ran")')
        assert result.returncode == 0 and 'interpreter ran' in result.stdout, result
        if not str(interpreter).startswith('/usr'):
            result = run('run', str(interpreter), '--read-policy', 'strict',
                         '-v', str(workspace), '--', '-c', 'print("must-not-execute")')
            assert result.returncode != 0, result
            assert 'must-not-execute' not in result.stdout, result
            assert 'unreadable' in result.stdout + result.stderr, result
        result = run('run', '/bin/sh', '--read-policy', 'nonsense', '-v', str(workspace),
                     '--', '-c', 'echo must-not-execute')
        assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
        print('PASS: strict read policy confines reads to grants and system paths, '
              'leaving an interpreter runnable')
    else:
        result = run('run', '/bin/echo', '--', 'must-not-execute')
        assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
        print('PASS: unsupported native sandbox fails closed')
