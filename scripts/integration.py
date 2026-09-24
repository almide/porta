#!/usr/bin/env python3
"""Offline process-level regressions; run against the newly built binary."""
import json
import http.server
import threading
import pathlib
import platform
import resource
import shutil
import subprocess
import sys
import tempfile
import time

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

    # One version, said the same way everywhere. It used to be written in five
    # places, so `porta --version` and the manifest porta generates could
    # disagree about what produced a file. almide.toml still carries it because
    # the build needs it before any module exists, so the two are checked here.
    declared = next(line.split('"')[1] for line in
                    pathlib.Path('almide.toml').read_text().splitlines()
                    if line.startswith('version'))
    reported = run('--version').stdout.split()[-1]
    assert reported == declared, f'porta reports {reported}, almide.toml says {declared}'
    built = root / 'versioned.wasm'
    built.write_bytes(wasm.read_bytes())
    assert run('build', str(built)).returncode == 0
    generated = json.loads((root / 'versioned.manifest.json').read_text())['version']
    assert generated == declared, f'generated manifest says {generated}, not {declared}'
    print(f'PASS: one version ({declared}) in the binary, the manifest and almide.toml')

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
            probe = r'''import base64, errno, os, socket, sys, urllib.parse
proxy = urllib.parse.urlsplit(os.environ['HTTPS_PROXY'])
credential = base64.b64encode(f'{proxy.username}:{proxy.password}'.encode()).decode()
def connect(target, authorised=True):
    with socket.create_connection((proxy.hostname, proxy.port), 2) as stream:
        auth = f'Proxy-Authorization: Basic {credential}\r\n' if authorised else ''
        stream.sendall(f'CONNECT {target} HTTP/1.1\r\n{auth}\r\n'.encode())
        return stream.recv(4096)
# The proxy serves this run's client alone: without the run's credential it
# answers 407, and no policy question is even asked.
assert b'407 Proxy Authentication Required' in connect('blocked.example:443', authorised=False)
assert b'403 Forbidden' in connect('blocked.example:443')
# An allowed name that resolves to this machine is refused: a hostname on the
# allow-list is a promise about a public service, not a route to loopback.
assert b'403 Forbidden' in connect('localhost:443')
def denied(attempt):
    """Whether an egress attempt was refused by the sandbox rather than served."""
    try:
        attempt()
    except OSError as error:
        assert error.errno in (errno.EPERM, errno.EACCES), error
        return True
    raise AssertionError('egress bypassed the proxy')

# The proxy is the only way out. macOS refuses at connect and send rather than
# at socket(), so creating one of these proves nothing — the send has to fail.
assert denied(lambda: socket.create_connection(('127.0.0.1', int(sys.argv[1])), 2))
assert denied(lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(b'x', ('8.8.8.8', 53)))
assert denied(lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM).connect('/var/run/syslog'))
'''
            result = run('run', sys.executable, '--proxy-allow', 'api.example.com,localhost',
                         '--', '-c', probe, str(port))
            assert result.returncode == 0, result.stderr
            assert 'no proxy credential' in result.stderr and 'blocked addresses' in result.stderr, result.stderr
            print('PASS: CONNECT needs the run credential; policy and loopback denials; direct TCP, UDP and Unix-socket egress blocked')
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    print('PASS: checked HTTP tool does not follow redirects')

    # A directory the run is never granted. It has to be somewhere an ordinary
    # user can create — porta refuses to run as root, so /opt is out — and
    # outside both the always-writable roots and the strict read set, or the
    # tests below would assert nothing. A home directory is exactly that: not
    # /tmp, not /dev, not a system directory, and closed unless mounted.
    ungranted = pathlib.Path(tempfile.mkdtemp(prefix='porta-ungranted-', dir=pathlib.Path.home()))

    # What a first-time user types, before they have read about `--`. Each of
    # these used to exit 0 with nothing on the terminal — `porta run echo hello`
    # ran `echo` alone — and nothing distinguishes that from a command that
    # worked. A mistake porta can see has to be named, and has to fail.
    result = run('run', '/bin/echo', 'hello')
    assert result.returncode != 0 and 'hello' not in result.stdout, result
    assert 'after --' in result.stderr, result.stderr
    result = run('run', '/bin/echo', '--bogus', 'x')
    assert result.returncode != 0 and 'unknown option --bogus' in result.stderr, result
    result = run('run', '/bin/echo', '--', 'hello', '--bogus')
    assert result.returncode == 0 and result.stdout == 'hello --bogus\n', result
    result = run('run', 'no-such-command-porta-test', '--', 'x')
    assert result.returncode != 0 and 'command not found' in result.stderr, result
    result = run('run', '/bin/echo', '-v', str(root / 'no-such-mount'), '--', 'must-not-execute')
    assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
    assert 'mount' in result.stderr and 'no-such-mount' in result.stderr, result.stderr
    result = run('run', '/bin/echo', '-v', str(root / 'agent.wasm'), '--', 'must-not-execute')
    assert result.returncode != 0 and 'not a directory' in result.stderr, result
    result = run('no-such-subcommand')
    assert result.returncode != 0 and 'unknown command' in result.stderr, result
    empty = root / 'empty'
    empty.mkdir()
    result = run('up', cwd=empty)
    assert result.returncode != 0 and 'porta.toml' in result.stderr, result
    print('PASS: a mistyped command line is refused and named, never silently narrowed')

    # The caller's shell is full of credentials — API keys, agent sockets — and
    # none of it is the command's business. The child starts from an empty
    # environment plus what a command needs to run; a host variable crosses
    # only when -e or --env-pass names it.
    import os
    leaky = {**os.environ, 'PORTA_TEST_SECRET': 'host-only', 'LANG': os.environ.get('LANG', 'C.UTF-8')}
    result = run('run', '/bin/sh', '--', '-c', 'echo "secret=${PORTA_TEST_SECRET:-unset} path=${PATH:+set} home=${HOME:+set} lang=${LANG:+set}"', env=leaky)
    assert result.returncode == 0 and result.stdout.strip() == 'secret=unset path=set home=set lang=set', result
    result = run('run', '/bin/sh', '--env-pass', 'PORTA_TEST_SECRET', '--', '-c', 'echo "$PORTA_TEST_SECRET"', env=leaky)
    assert result.returncode == 0 and result.stdout.strip() == 'host-only', result
    # A listen grant without a network rule would grant nothing; it is refused
    # so a caller learns the two go together rather than believing a port was
    # closed.
    result = run('run', '/bin/echo', '--allow-bind', '8080', '--', 'must-not-execute')
    assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
    assert '--allow-net' in result.stderr, result.stderr
    print('PASS: the host environment stays outside unless named; --allow-bind needs --allow-net')

    # What a shell sees. The command's own exit code passes through in every
    # mode; a run porta refused exits with porta's own code, so a script can
    # tell "the command failed" from "the command never ran".
    result = run('run', '/bin/sh', '--proxy-allow', 'api.example.com', '--', '-c', 'exit 9')
    assert result.returncode == 9, result
    result = run('run', 'no-such-command-porta-test', '--', 'x')
    assert result.returncode == 127, result
    result = run('run', '/bin/echo', '-v', str(root / 'no-such-mount'), '--', 'x')
    assert result.returncode == 125, result
    # `check` says what this host enforces and exits 0 wherever porta runs;
    # `explain` shows the policy and runs nothing.
    result = run('check')
    assert result.returncode == 0 and result.stdout.startswith('porta on '), result
    result = run('explain', '/bin/echo', '-v', str(root), '--allow-net', '*:443', '--', 'must-not-execute')
    assert result.returncode == 0 and 'must-not-execute' in result.stdout.splitlines()[0], result
    assert 'mounts' in result.stdout and '*:443' in result.stdout and result.stdout.count('must-not-execute') == 1, result
    result = run('explain', '/bin/echo', '-v', str(root / 'no-such-mount'), '--', 'x')
    assert result.returncode == 0 and 'would refuse' in result.stdout, result
    print('PASS: exit codes pass through; refusals exit 125/126/127; check and explain run nothing')

    # --timeout bounds a native run's wall-clock: a hung command is killed and
    # reports 124, the code timeout(1) uses. A run that finishes first keeps
    # its own exit code, and 0 (the default) never kills.
    started = time.monotonic()
    result = run('run', '/bin/sh', '--timeout', '1', '--', '-c', 'sleep 30')
    elapsed = time.monotonic() - started
    assert result.returncode == 124, result
    assert elapsed < 10, f'timeout did not stop the run promptly: {elapsed:.1f}s'
    # The command leads its own process group, so a backgrounded child is
    # killed with it rather than orphaned to run out its sleep. The child is
    # found afterwards by its command line, not by the pid the run prints: in
    # the command's own PID namespace that pid is not the host's.
    result = run('run', '/bin/sh', '--timeout', '1', '--', '-c', 'sleep 314159 & wait')
    assert result.returncode == 124, result
    time.sleep(1)
    survivors = subprocess.run(['pgrep', '-f', 'sleep 314159'], capture_output=True, text=True).stdout.split()
    assert not survivors, f'a backgrounded child ({survivors}) outlived the timeout'
    result = run('run', '/bin/sh', '--timeout', '5', '--', '-c', 'exit 7')
    assert result.returncode == 7, result
    result = run('explain', '/bin/sh', '--timeout', '3', '--', '-c', 'x')
    assert result.returncode == 0 and 'time limit' in result.stdout and '3s' in result.stdout, result
    print('PASS: --timeout kills a hung run (124) with its whole group; a run that finishes keeps its code')

    # The resource ceilings are rlimits set between fork and exec, so the
    # kernel enforces them and every descendant inherits them. CPU past the
    # budget ends with SIGXCPU (152), a write past the file ceiling with
    # SIGXFSZ (153) and the file stops at the ceiling, and a process ceiling
    # of one leaves a shell unable to fork at all. A ceiling above this user's
    # hard limit is refused before the run rather than failing the spawn.
    started = time.monotonic()
    result = run('run', '/bin/sh', '--max-cpu', '1', '--', '-c', 'while :; do :; done')
    elapsed = time.monotonic() - started
    assert result.returncode == 152, result
    assert elapsed < 10, f'--max-cpu did not stop a CPU burn promptly: {elapsed:.1f}s'
    # Ignoring SIGXCPU buys nothing: Linux kills at the hard limit a second
    # later (137), and on macOS, where the kernel only ever signals, porta's
    # supervisor measures the group's CPU and kills it at the ceiling (152).
    started = time.monotonic()
    result = run('run', '/bin/sh', '--max-cpu', '1', '--', '-c', 'trap "" XCPU; while :; do :; done')
    elapsed = time.monotonic() - started
    assert result.returncode in (152, 137), result
    assert elapsed < 10, f'a process ignoring SIGXCPU outlived --max-cpu: {elapsed:.1f}s'
    big = root / 'big'
    result = run('run', '/bin/sh', '-v', str(root), '--max-file-size', '1', '--', '-c',
                 f'head -c 3000000 /dev/zero > {big}')
    assert result.returncode == 153, result
    assert big.stat().st_size <= 1024 * 1024, f'--max-file-size let a file grow to {big.stat().st_size}'
    result = run('run', '/bin/sh', '--max-procs', '1', '--', '-c', '/bin/echo one; /bin/echo two')
    assert result.returncode != 0 and 'two' not in result.stdout, result
    result = run('run', '/bin/sh', '--max-cpu', '5', '--max-procs', '500', '--max-file-size', '5', '--', '-c', 'echo fine')
    assert result.returncode == 0 and result.stdout.strip() == 'fine', result
    if resource.getrlimit(resource.RLIMIT_NPROC)[1] != resource.RLIM_INFINITY:
        result = run('run', '/bin/sh', '--max-procs', '99999999', '--', '-c', 'true')
        assert result.returncode == 125 and 'hard limit' in result.stderr, result
    result = run('explain', '/bin/sh', '--max-cpu', '2', '--max-procs', '300', '--max-file-size', '1', '--', '-c', 'x')
    assert result.returncode == 0 and '2s CPU' in result.stdout and '300 processes' in result.stdout \
        and '1 MiB' in result.stdout, result
    result = run('explain', '/bin/sh', '--max-cpu', '2', '--json', '--', '-c', 'x')
    assert json.loads(result.stdout)['limits'] == {'cpu_seconds': 2, 'processes': 0, 'file_size_mib': 0, 'memory_mib': 0}, result
    print('PASS: --max-cpu, --max-file-size and --max-procs are enforced by the kernel and inherited; one above the hard limit is refused')

    # --max-memory-mb: on Linux a cgroup v2 ceiling placed through the systemd
    # user manager, so only where a user manager runs for this user; on macOS
    # the supervisor polls the group's footprint and ends it at the ceiling.
    # Either way an allocation past the ceiling is killed (137) and a run
    # within it keeps its code. A Linux host without a user manager refuses
    # the flag before the run, naming the reason — never run without.
    user_manager = platform.system() == 'Linux' and os.path.exists(f'/run/user/{os.getuid()}/bus') \
        and (os.path.exists('/usr/bin/busctl') or os.path.exists('/bin/busctl'))
    # The hog keeps its allocation alive long enough for a polling supervisor.
    hog = "import time\nb = bytearray(200 * 1024 * 1024)\ntime.sleep(1)\nprint('ALLOCATED')"
    if user_manager or platform.system() == 'Darwin':
        result = run('run', sys.executable, '--max-memory-mb', '64', '--', '-c', hog)
        assert result.returncode == 137 and 'ALLOCATED' not in result.stdout, result
        result = run('run', sys.executable, '--max-memory-mb', '512', '--', '-c', hog)
        assert result.returncode == 0 and 'ALLOCATED' in result.stdout, result
        result = run('explain', '/bin/sh', '--max-memory-mb', '64', '--', '-c', 'x')
        assert result.returncode == 0 and '64 MiB resident' in result.stdout, result
        print('PASS: --max-memory-mb kills a run past the ceiling and leaves one within it alone')
    else:
        result = run('run', sys.executable, '--max-memory-mb', '64', '--', '-c', hog)
        assert result.returncode == 125 and 'ALLOCATED' not in result.stdout, result
        assert ('cgroup' in result.stderr or 'user manager' in result.stderr), result.stderr
        print('PASS: --max-memory-mb is refused where no cgroup ceiling can be placed (this host), never run without')

    # A WASI 0.2 component runs under the same capability check, budgets and
    # preopens as a core module. Its imports are interfaces, so the check maps
    # them: this one needs io, process, clock and random — `worker` has them,
    # `ai-agent` lacks clock and refuses it by name. Fuel still bounds it, and
    # inspect says what it is. The component is built here from its source
    # when the almide toolchain is present, as it is in CI.
    almide = pathlib.Path(porta).parent.parent / '.tools' / 'almide' / 'almide'
    if almide.is_file():
        component = root / 'component-hello.wasm'
        built = subprocess.run([str(almide), 'build', 'scripts/fixtures/component_hello.almd', '--target', 'wasm',
                                '--component', '-o', str(component)], text=True, capture_output=True, timeout=600)
        assert built.returncode == 0 and component.is_file(), built
        assert component.read_bytes()[4:8] == b'\x0d\x00\x01\x00', 'the fixture is not a component'
        result = run('run', str(component), '--profile', 'worker')
        assert result.returncode == 0 and result.stdout.strip() == 'hello from a component', result
        result = run('run', str(component), '--profile', 'ai-agent')
        assert result.returncode != 0 and 'wasi:clocks/wall-clock' in result.stderr and "'clock'" in result.stderr, result
        result = run('run', str(component), '--profile', 'worker', '--step-limit', '10')
        assert result.returncode != 0 and 'hello' not in result.stdout, result
        result = run('inspect', str(component))
        assert result.returncode == 0 and 'Component (WASI 0.2)' in result.stdout and 'wasi:cli/run' in result.stdout, result
        print('PASS: a WASI 0.2 component runs, is capability-checked by interface, is bounded by fuel, and inspects as one')
        # The same program as a WASI 0.3 component: async-lifted run, stdio
        # over component-model streams. Almide emits it with
        # ALMIDE_COMPONENT_P3=1; its world imports the filesystem interfaces
        # even when unused, so `worker` refuses it by name and `full` runs it.
        p3 = root / 'component-hello-p3.wasm'
        built = subprocess.run([str(almide), 'build', 'scripts/fixtures/component_hello.almd', '--target', 'wasm',
                                '--component', '-o', str(p3)], text=True, capture_output=True, timeout=600,
                               env={**os.environ, 'ALMIDE_COMPONENT_P3': '1'})
        assert built.returncode == 0 and p3.is_file(), built
        result = run('run', str(p3), '--profile', 'full')
        assert result.returncode == 0 and result.stdout.strip() == 'hello from a component', result
        result = run('run', str(p3), '--profile', 'worker')
        assert result.returncode != 0 and 'wasi:filesystem' in result.stderr, result
        result = run('run', str(p3), '--profile', 'full', '--step-limit', '10')
        assert result.returncode != 0 and 'hello' not in result.stdout, result
        result = run('inspect', str(p3))
        assert result.returncode == 0 and 'wasi:cli/run@0.3' in result.stdout, result
        print('PASS: a WASI 0.3 component runs through the async linker under the same check and budgets')
    else:
        print('SKIP: no almide toolchain at .tools/almide; the WASI 0.2 component test did not run')

    # Under strict reads, a command named bare is found the way the shell finds
    # it, and one outside the readable roots is refused up front with the
    # directory to grant — rather than started and left to die on EACCES.
    tooldir = ungranted / 'tools'
    tooldir.mkdir()
    (tooldir / 'porta-tool').write_text('#!/bin/sh\necho ran-anyway\n')
    (tooldir / 'porta-tool').chmod(0o755)
    on_path = {**os.environ, 'PATH': f"{tooldir}:{os.environ.get('PATH', '')}"}
    result = run('run', 'porta-tool', '--read-policy', 'strict', '-v', str(root), env=on_path)
    assert result.returncode == 126 and 'ran-anyway' not in result.stdout, result
    assert 'unreadable' in result.stderr and str(tooldir) in result.stderr, result.stderr
    result = run('run', 'porta-tool', '-v', str(root), env=on_path)
    assert result.returncode == 0 and result.stdout.strip() == 'ran-anyway', result
    print('PASS: a bare command outside the strict read roots is refused by name with the -v to grant')

    # --json gives explain and check a machine-readable form. explain reports
    # the effective policy; a run it would refuse reports the refusal instead.
    result = run('explain', '/bin/echo', '-v', str(root), '--allow-net', 'api.example.com:443',
                 '--timeout', '30', '--json', '--', 'hi')
    assert result.returncode == 0, result
    policy = json.loads(result.stdout)
    assert policy['command'] == '/bin/echo' and policy['args'] == ['hi'], policy
    assert policy['network'] == {'mode': 'allowlist', 'allow': ['api.example.com:443']}, policy
    assert policy['timeout_seconds'] == 30 and policy['reads'] == 'open', policy
    assert str(root) in policy['mounts'][0], policy
    result = run('explain', '/bin/echo', '-v', str(root / 'no-such-mount'), '--json', '--', 'x')
    assert result.returncode == 0 and 'refused' in json.loads(result.stdout), result
    result = run('check', '--json')
    assert result.returncode == 0, result
    host = json.loads(result.stdout)
    # A host may honestly lack two primitives: the memory ceiling (no systemd
    # user manager) and the PID namespace (unprivileged user namespaces
    # refused, as on a stock Ubuntu desktop or in a container); everything
    # else must be present.
    absent = [p['name'] for p in host['primitives'] if not p['present']]
    assert all('memory ceiling' in name or 'PID, mount and network namespace' in name for name in absent), host
    assert host['all_enforced'] is (host['missing'] == 0) and host['missing'] == len(absent), host
    assert host['primitives'] and all('present' in p for p in host['primitives']), host
    print('PASS: --json gives explain the effective policy (or the refusal) and check the host report, both parseable')

    # --no-net: nothing leaves, not even to a service on the host's loopback,
    # and it cannot be combined with a grant that would open some of it.
    result = run('explain', '/bin/echo', '-v', str(root), '--no-net', '--json')
    assert json.loads(result.stdout)['network'] == {'mode': 'none', 'allow': []}, result
    result = run('run', '/bin/echo', '-v', str(root), '--no-net', '--allow-net', '*:443', '--', 'must-not-run')
    assert result.returncode != 0 and 'must-not-run' not in result.stdout and '--no-net' in result.stderr, result
    listener = __import__('socket').socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    try:
        dial = ('import socket,sys\n'
                'try: socket.create_connection(("127.0.0.1", int(sys.argv[1])), 3); print("REACHED")\n'
                'except OSError: print("closed")')
        port = str(listener.getsockname()[1])
        result = run('run', sys.executable, '-v', str(root), '--', '-c', dial, port)
        assert 'REACHED' in result.stdout, result
        result = run('run', sys.executable, '-v', str(root), '--no-net', '--', '-c', dial, port)
        assert 'closed' in result.stdout, result
    finally:
        listener.close()
    print('PASS: --no-net reaches nothing, the host loopback included, and refuses a grant beside it')

    # Found by scripts/fuzz.py. A mount whose name holds a carriage return
    # before a newline lost its grant on macOS (the profile's per-run tagging
    # split lines the way str::lines does, and dropped the \r), and a ":ro"
    # inside a name was stripped from the working directory as if it ended it.
    for name in ('repo\r\nx', 'a:rob', 'q"(deny x)\n(allow default)'):
        odd = root / name
        odd.mkdir()
        result = run('run', '/bin/sh', '-v', str(odd), '--', '-c', 'printf x > "$1" && echo LANDED', 'sh', str(odd / 'in'))
        assert 'LANDED' in result.stdout and (odd / 'in').exists(), (name, result)
    print('PASS: mounts named with a carriage return, a newline, quotes or ":ro" inside keep their grant')

    # What a run closes beyond its grants is a preset, not code: the default
    # names credential stores, --preset none drops it, --deny-read adds to it,
    # and explain shows the result. A GitHub CLI token was not on the first,
    # hard-coded list; on Linux the default read mode closed none of them.
    # Under the real home, not /tmp: /tmp is granted to every run, and a
    # closed path inside a grant is one Landlock alone cannot close (the run
    # is refused then, which would test the refusal instead).
    fake_home = pathlib.Path(tempfile.mkdtemp(prefix='porta-preset-', dir=pathlib.Path.home()))
    token = fake_home / '.config' / 'gh' / 'hosts.yml'
    token.parent.mkdir(parents=True)
    token.write_text('oauth_token: MUST-NOT-LEAK\n')
    own = fake_home / 'notes' / 'private.txt'
    own.parent.mkdir()
    own.write_text('MUST-NOT-LEAK-EITHER\n')
    elsewhere = root / 'elsewhere'
    elsewhere.mkdir()
    as_home = {**os.environ, 'HOME': str(fake_home), 'PORTA_DENIALS': 'never'}
    result = run('run', '/bin/cat', '-v', str(elsewhere), '--', str(token), env=as_home)
    assert 'MUST-NOT-LEAK' not in result.stdout, result
    result = run('run', '/bin/cat', '-v', str(elsewhere), '--preset', 'none', '--', str(token), env=as_home)
    assert 'MUST-NOT-LEAK' in result.stdout, result
    result = run('run', '/bin/cat', '-v', str(elsewhere), '--', str(own), env=as_home)
    assert 'MUST-NOT-LEAK-EITHER' in result.stdout, result
    result = run('run', '/bin/cat', '-v', str(elsewhere), '--deny-read', '~/notes', '--', str(own), env=as_home)
    assert 'MUST-NOT-LEAK-EITHER' not in result.stdout, result
    closed = json.loads(run('explain', '/bin/echo', '-v', str(elsewhere), '--deny-read', '~/notes', '--json', env=as_home).stdout)
    assert closed['preset'] == 'default' and str(token.parent.resolve()) in closed['closed']['read'], closed
    assert str(own.parent.resolve()) in closed['closed']['read'] and '.envrc' in closed['closed']['protect'], closed
    for bad in (['--protect', '../outside'], ['--deny-read', 'relative/path'], ['--preset', str(root / 'no-such-preset.toml')]):
        result = run('run', '/bin/echo', '-v', str(elsewhere), *bad, '--', 'must-not-run', env=as_home)
        assert result.returncode == 125 and 'must-not-run' not in result.stdout, (bad, result)
    shutil.rmtree(fake_home, ignore_errors=True)
    print('PASS: the default preset closes credential stores; --preset none, --deny-read and explain work on it')

    # --why says what the sandbox refused even when the run went on: macOS
    # from the unified log, Linux by tracing the run with strace from outside.
    if platform.system() == 'Darwin' or shutil.which('strace'):
        outside = pathlib.Path(tempfile.mkdtemp(prefix='porta-why-', dir=pathlib.Path.home()))
        result = run('run', '/bin/sh', '-v', str(elsewhere), '--why', '--',
                     '-c', f'echo x > "{outside}/refused"; exit 3')
        shutil.rmtree(outside, ignore_errors=True)
        assert result.returncode == 3, result
        assert 'the sandbox refused this run' in result.stderr and f'-v {outside.resolve()}' in result.stderr, result.stderr
        result = run('run', '/bin/echo', '-v', str(elsewhere), '--why', '--', 'fine')
        assert result.returncode == 0 and 'refused nothing' in result.stderr, result.stderr
        print('PASS: --why names each refusal and the flag it needed, and says when there was none')
    else:
        print('PASS: --why not exercised (no strace on this Linux host)')

    # explain --save writes a porta.toml of the flags in use, so an invocation
    # a user converged on can be committed and re-run with `porta up`. Secrets
    # and -e values are left out on purpose: a committed file is the wrong place.
    with tempfile.TemporaryDirectory() as save_dir:
        cfg = pathlib.Path(save_dir) / 'porta.toml'
        result = run('explain', '/bin/echo', '-v', save_dir, '--allow-net', 'api.example.com:443',
                     '--read-policy', 'strict', '--timeout', '20', '--max-cpu', '30', '--env-pass', 'FOO',
                     '-e', 'TOKEN=must-not-be-saved', '--save', str(cfg), '--', 'saved')
        assert result.returncode == 0 and cfg.is_file(), result
        text = cfg.read_text()
        assert 'command = "/bin/echo"' in text and 'read-policy = "strict"' in text, text
        assert 'timeout = 20' in text and 'max-cpu = 30' in text and 'network = ["api.example.com:443"]' in text, text
        assert 'must-not-be-saved' not in text, 'a -e value leaked into the saved policy'
        result = run('up', '--', 'saved', cwd=save_dir)
        assert result.returncode == 0 and result.stdout.strip() == 'saved', result
        # A mount path holding a double quote must stay valid TOML, not close
        # the string early: the saver escapes it, and porta up reads it back.
        quoted_dir = pathlib.Path(save_dir) / 'od"d'
        quoted_dir.mkdir()
        result = run('explain', '/bin/echo', '-v', str(quoted_dir), '--save', str(quoted_dir / 'porta.toml'), '--', 'q')
        assert result.returncode == 0 and '\\"' in (quoted_dir / 'porta.toml').read_text(), result
        result = run('up', '--', 'q', cwd=str(quoted_dir))
        assert result.returncode == 0 and result.stdout.strip() == 'q', result
    print('PASS: explain --save writes a committable porta.toml (no secrets, TOML-escaped) that porta up runs')

    # A listener the tests below try to reach or to bind, and a helper that
    # says whether a bind attempt was refused by policy.
    bind_probe = '''import socket, sys
s = socket.socket()
try:
    s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1); print("bound")
except OSError as e:
    print("bind denied", e.errno)
'''

    # Whether this host can resolve a public name at all. The suite is offline
    # by design; the DNS checks below run only where the answer is yes, and
    # then assert that the sandbox neither breaks nor widens what the host has.
    import socket
    try:
        socket.getaddrinfo('example.com', 443)
        host_resolves = True
    except OSError:
        host_resolves = False
    resolve = 'import socket; print(socket.getaddrinfo("example.com", 443)[0][4][0])'

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

        # `--allow-net` closes every outbound channel but the granted ports, and
        # on macOS name resolution is one of those channels: getaddrinfo talks
        # to mDNSResponder over a socket. With that socket closed, the README's
        # own `curl --allow-net '*:443' https://example.com` failed to resolve.
        # The grant has to leave names resolvable; proxy mode has to not, since
        # the proxy resolves the CONNECT target and is the only egress there.
        if host_resolves:
            result = run('run', sys.executable, '--allow-net', '*:443', '--', '-c', resolve)
            assert result.returncode == 0, result.stderr
            result = run('run', sys.executable, '--proxy-allow', 'api.example.com', '--', '-c', resolve)
            assert result.returncode != 0, result
            print('PASS: --allow-net leaves names resolvable; proxy mode does not')
        else:
            print('SKIP: this host cannot resolve example.com; DNS checks not run')

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

        # What the field closes and porta did not, measured before this block
        # existed: another process's arguments, the login Keychain, the
        # credential directories under the home, the operator's repository
        # hooks and config, the files at a mount root a host tool trusts, and
        # the mount root itself.
        procargs = '''import ctypes, sys
libc = ctypes.CDLL(None, use_errno=True)
mib = (ctypes.c_int * 3)(1, 49, int(sys.argv[1])); size = ctypes.c_size_t(1 << 20)
buf = ctypes.create_string_buffer(size.value)
rc = libc.sysctl(mib, 3, buf, ctypes.byref(size), None, 0)
print("denied" if rc else ("LEAK" if b"MUST-NOT-LEAK" in buf.raw[:size.value] else "empty"))
'''
        decoy = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)', '--token=sk-live-MUST-NOT-LEAK'])
        try:
            result = run('run', sys.executable, '-v', str(workspace), '--', '-c', procargs, str(decoy.pid))
            assert result.returncode == 0 and result.stdout.strip() == 'denied', result
        finally:
            decoy.terminate()
            decoy.wait()
        result = run('run', '/usr/bin/security', '--', 'list-keychains')
        assert result.returncode != 0 and 'login.keychain' not in result.stdout, result
        fake_home = ungranted / 'home'
        (fake_home / '.aws').mkdir(parents=True)
        (fake_home / '.aws' / 'credentials').write_text('aws_secret_access_key = EXAMPLE\n')
        result = run('run', '/bin/cat', '-v', str(fake_home), '--', str(fake_home / '.aws' / 'credentials'),
                     env={**os.environ, 'HOME': str(fake_home)})
        assert result.returncode != 0 and 'EXAMPLE' not in result.stdout, result
        repo = ungranted / 'repo'
        repo.mkdir()
        subprocess.run(['git', 'init', '-q', str(repo)], check=True)
        subprocess.run(['git', '-C', str(repo), '-c', 'user.email=a@b', '-c', 'user.name=n', 'commit', '-q', '--allow-empty', '-m', 'init'], check=True)
        denied_writes = ['echo x > .git/hooks/pre-commit', 'git config user.name evil', 'mv .git g',
                         'echo x > .mcp.json', 'mkdir -p .claude/commands && echo x > .claude/commands/x.md',
                         'echo x > .zshrc']
        for attempt in denied_writes:
            result = run('run', '/bin/sh', '-v', str(repo), '--', '-c', attempt)
            assert result.returncode != 0, (attempt, result)
        assert not (repo / '.git' / 'hooks' / 'pre-commit').exists()
        assert not (repo / '.mcp.json').exists() and (repo / '.git').is_dir()
        result = run('run', '/bin/sh', '-v', str(repo), '--', '-c',
                     'echo hi > f && git add f && git -c user.email=a@b -c user.name=n commit -q -m x && '
                     'mkdir -p .claude && echo x > .claude/settings.local.json && echo worked')
        assert result.returncode == 0 and 'worked' in result.stdout, result
        result = run('run', '/bin/sh', '-v', str(repo), '-v', str(ungranted), '--', '-c', f'mv {repo} {repo}.moved')
        assert result.returncode != 0 and repo.is_dir(), result
        print('PASS: other processes\' arguments, the Keychain, credential directories, '
              'repository hooks and mount roots are closed')

        # Once --allow-net is in force a granted port is a port to reach, not
        # one to serve on. The SSH agent is closed by default in every mode.
        result = run('run', sys.executable, '--allow-net', '*:443', '--', '-c', bind_probe, '0')
        assert result.returncode == 0 and 'bind denied' in result.stdout, result
        result = run('run', sys.executable, '--allow-net', '*:443', '--allow-bind', '47123', '--', '-c', bind_probe, '47123')
        assert result.returncode == 0 and result.stdout.strip() == 'bound', result
        agent = os.environ.get('SSH_AUTH_SOCK')
        if agent and pathlib.Path(agent).exists():
            # ssh-add exits 2 whether the socket is refused by the sandbox or
            # simply absent; the socket is present here, so 2 means refused.
            result = run('run', '/usr/bin/ssh-add', '--env-pass', 'SSH_AUTH_SOCK', '--', '-l')
            assert result.returncode == 2, f'the SSH agent at {agent} was reachable: {result}'
            result = run('run', '/usr/bin/ssh-add', '--env-pass', 'SSH_AUTH_SOCK', '--allow-unix', agent, '--', '-l')
            assert result.returncode in (0, 1), result
            print('PASS: listening needs --allow-bind; the SSH agent needs --allow-unix')
        else:
            print('PASS: listening needs --allow-bind (no SSH agent on this host; --allow-unix not exercised)')

        # A refusal looks exactly like a broken tool until someone says which
        # flag it would have needed. After a failed run porta reads the
        # kernel's denial records for this run and says so.
        # The unified log is written asynchronously with no completion signal;
        # porta already asks it three times over a few seconds, and on a busy
        # CI runner even that has come back empty. Three runs, not one: a
        # footer that never appears is a failure, one that lags is the log's.
        for attempt in range(3):
            result = run('run', '/bin/sh', '--allow-net', '*:80', '--', '-c',
                         'echo x > "$1"; exit 3', 'sh', str(ungranted / 'denied.txt'), env={**os.environ, 'PORTA_DENIALS': 'always'})
            assert result.returncode == 3, result
            if '[porta] the sandbox refused this run' in result.stderr:
                break
            time.sleep(2)
        if '[porta] the sandbox refused this run' not in result.stderr:
            # Show what the log holds, so a missing footer can be told apart
            # from a missing record: lag, throttling, or a predicate miss.
            raw = subprocess.run(['/usr/bin/log', 'show', '--last', '2m', '--style', 'compact',
                                  '--predicate', 'senderImagePath CONTAINS "Sandbox"'],
                                 text=True, capture_output=True, timeout=120)
            lines = raw.stdout.splitlines()
            hits = [l for l in lines if 'denied.txt' in l or 'porta' in l]
            print(f'--- unified log: {len(lines)} Sandbox lines in the last 2m, {len(hits)} mentioning this run ---', file=sys.stderr)
            print('\n'.join(hits[-20:] or lines[-20:]), file=sys.stderr)
            print(f'--- log show rc={raw.returncode} stderr={raw.stderr.strip()[:300]} ---', file=sys.stderr)
        assert '[porta] the sandbox refused this run' in result.stderr, result.stderr
        assert f'-v {ungranted}' in result.stderr, result.stderr
        result = run('run', '/bin/sh', '--', '-c', 'exit 3', env={**os.environ, 'PORTA_DENIALS': 'never'})
        assert result.returncode == 3 and '[porta]' not in result.stderr, result
        print('PASS: a failed run says which flag each refusal would have needed')
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
        # Outside every grant, but somewhere an ordinary user can create:
        # the suite must not need root, because porta refuses to run as it.
        outside = ungranted / 'porta-denied'
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
                     '-v', str(workspace), '-v', str(pathlib.Path(sys.base_prefix).resolve()) + ':ro',
                     '--', '-c', families)
        assert result.returncode == 0, result.stderr
        opened = dict(line.split() for line in result.stdout.split('\n') if line)
        assert opened == {'tcp4': 'True', 'udp4': 'False', 'unix': 'False', 'tcp6': 'False',
                          'netlink': 'False', 'proxy_env': 'True'}, opened
        # Without proxy mode nothing claims to be the only egress, so nothing is
        # filtered: the restriction follows the claim, it is not always on.
        result = run('run', sys.executable, '-v', str(workspace),
                     '-v', str(pathlib.Path(sys.base_prefix).resolve()) + ':ro', '--', '-c', families)
        assert result.returncode == 0, result.stderr
        assert 'udp4 True' in result.stdout and 'unix True' in result.stdout, result.stdout
        print('PASS: proxy mode is the only egress — UDP, Unix, IPv6 and netlink all denied')

        # --allow-net names TCP ports, and Landlock's rule speaks for TCP alone:
        # an internet socket must then be TCP, and names resolve over TCP 53.
        result = run('run', sys.executable, '--allow-net', '*:443', '-v', str(workspace),
                     '-v', str(pathlib.Path(sys.base_prefix).resolve()) + ':ro',
                     '--', '-c', families + "print('res', __import__('os').environ.get('RES_OPTIONS'))")
        assert result.returncode == 0, result.stderr
        opened = dict(line.split() for line in result.stdout.split('\n') if line)
        assert opened['tcp4'] == 'True' and opened['tcp6'] == 'True' and opened['unix'] == 'True', opened
        assert opened['udp4'] == 'False' and opened['res'] == 'use-vc', opened
        print('PASS: under --allow-net an internet socket is TCP only, and names resolve over TCP')

        # A credential socket the preset names is covered in the command's
        # mount namespace, unless --allow-unix opens it.
        sockets = pathlib.Path(tempfile.mkdtemp(prefix='ssh-porta', dir=pathlib.Path.home()))
        agent = socket.socket(socket.AF_UNIX)
        agent.bind(str(sockets / 'agent.4242'))
        agent.listen(2)
        reach = 'import socket,sys\ntry: socket.socket(socket.AF_UNIX).connect(sys.argv[1]); print("reached")\nexcept OSError: print("refused")'
        try:
            closed = run('run', sys.executable, '-v', str(workspace), '--', '-c', reach, str(sockets / 'agent.4242'))
            opened = run('run', sys.executable, '-v', str(workspace), '--allow-unix', str(sockets / 'agent.4242'),
                         '--', '-c', reach, str(sockets / 'agent.4242'))
        finally:
            agent.close()
            shutil.rmtree(sockets)
        assert opened.stdout.strip() == 'reached', opened
        host = json.loads(run('check', '--json').stdout)
        if any('PID, mount and network namespace' in p['name'] and p['present'] for p in host['primitives']):
            assert closed.stdout.strip() == 'refused', closed
            print('PASS: a credential socket the preset names is refused unless --allow-unix opens it')
        else:
            assert 'these sockets stay reachable' in closed.stderr, closed
            print('PASS: without a mount namespace a credential socket stays reachable, and the run says so')

        # --read-policy strict confines reads to the granted mounts and the
        # platform's own directories. A credential outside both is unreadable.
        secrets = ungranted / 'porta-secrets'
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
                     '-v', str(workspace), '-v', str(pathlib.Path(sys.base_prefix).resolve()) + ':ro',
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

        # /proc is what the same user's other processes are visible through:
        # their command lines carry credentials. It is not in the system set,
        # and no interpreter here needs it, so a strict run must not reach it.
        decoy = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)',
                                  '--token=sk-live-MUST-NOT-LEAK'])
        try:
            peek = ('import glob,sys\n'
                    'seen=[]\n'
                    'for d in glob.glob("/proc/[0-9]*"):\n'
                    '    try: seen.append(open(d+"/cmdline").read())\n'
                    '    except OSError: pass\n'
                    'print("READ", len(seen), "MUST-NOT-LEAK" in "".join(seen))')
            result = run('run', sys.executable, '--read-policy', 'strict', '-v', str(workspace),
                         '-v', str(pathlib.Path(sys.base_prefix).resolve()) + ':ro', '--', '-c', peek)
            assert result.returncode == 0, result.stderr
            assert 'READ 0 False' in result.stdout, result.stdout
        finally:
            decoy.terminate()
            decoy.wait()
        print('PASS: a strict run cannot enumerate other processes through /proc')

        # In the default read mode /proc stays open, and a PID namespace of
        # the command's own is what hides other processes — where the host
        # gives one. The command must still end the way it would have: its
        # exit code, its signal, and --max-procs counting its own processes.
        host = json.loads(run('check', '--json').stdout)
        if any('PID, mount and network namespace' in p['name'] and p['present'] for p in host['primitives']):
            decoy = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)',
                                      '--token=sk-live-MUST-NOT-LEAK'])
            try:
                result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c',
                             f'cat /proc/{decoy.pid}/cmdline 2>/dev/null || echo hidden; '
                             'readlink /proc/self >/dev/null && echo SELF')
                assert 'MUST-NOT-LEAK' not in result.stdout and 'hidden' in result.stdout, result
                assert 'SELF' in result.stdout, result
            finally:
                decoy.terminate()
                decoy.wait()
            assert run('run', '/bin/sh', '-v', str(workspace), '--', '-c', 'exit 7').returncode == 7
            assert run('run', '/bin/sh', '-v', str(workspace), '--', '-c', 'kill -TERM $$').returncode == 143
            result = run('run', '/bin/sh', '-v', str(workspace), '--max-procs', '2', '--', '-c',
                         '/bin/true && echo forked')
            assert 'forked' in result.stdout, result
            print('PASS: the default read mode hides other processes in a PID namespace, '
                  'keeping exit codes, signals and --max-procs as they were')

        # Its own entry stays closed too. A rule on /proc/self would cover the
        # shell and none of the tools it starts, each with a pid of its own —
        # a grant that misleads — so nothing under /proc is granted.
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace), '--', '-c',
                     'cat /proc/self/status 2>/dev/null && echo SELF; ls /proc/1 >/dev/null 2>&1 && echo OTHER || echo closed')
        assert 'SELF' not in result.stdout and 'OTHER' not in result.stdout and 'closed' in result.stdout, result
        # /etc is granted file by file: what a command needs to start, never
        # what only root should read, and not the listing.
        result = run('run', '/bin/sh', '--read-policy', 'strict', '-v', str(workspace), '--', '-c',
                     'cat /etc/hosts >/dev/null && echo hosts; cat /etc/shadow 2>/dev/null && echo SHADOW; '
                     'ls /etc >/dev/null 2>&1 && echo LISTED || echo unlisted')
        assert 'hosts' in result.stdout and 'SHADOW' not in result.stdout and 'unlisted' in result.stdout, result
        # Cross-directory rename and link need the REFER right; without it
        # handled, `mv` inside a mount silently degrades to copy-and-delete.
        (workspace / 'a').mkdir(exist_ok=True)
        (workspace / 'b').mkdir(exist_ok=True)
        (workspace / 'a' / 'h').write_text('h')
        result = run('run', '/bin/ln', '-v', str(workspace), '--', str(workspace / 'a' / 'h'), str(workspace / 'b' / 'h'))
        assert result.returncode == 0 and (workspace / 'b' / 'h').exists(), result
        result = run('run', '/bin/ln', '-v', str(workspace), '--', str(workspace / 'a' / 'h'), str(ungranted / 'h'))
        assert result.returncode != 0 and not (ungranted / 'h').exists(), result
        print('PASS: strict keeps /proc closed and grants the /etc files a command needs; REFER honoured inside a mount')

        # The baseline seccomp filter, in every mode: the syscalls that reach
        # around a file policy or a debugger's boundary, the socket families a
        # TCP rule cannot see, and multipath TCP, which is routed past it.
        baseline = '''import ctypes, socket, threading
libc = ctypes.CDLL(None, use_errno=True)
def call(nr): r = libc.syscall(nr, 0, 0, 0, 0, 0); return ctypes.get_errno() if r < 0 else 0
print("unshare", call(%d)); print("ptrace", call(%d)); print("io_uring_setup", call(425)); print("clone3", call(435))
t = threading.Thread(target=lambda: None); t.start(); t.join(); print("threads ok")
def sock(*a):
    try: socket.socket(*a).close(); return "open"
    except OSError as e: return e.errno
print("mptcp", sock(socket.AF_INET, socket.SOCK_STREAM, 262))
print("packet", sock(17, socket.SOCK_RAW))
print("raw", sock(socket.AF_INET, socket.SOCK_RAW, 1))
print("udp", sock(socket.AF_INET, socket.SOCK_DGRAM))
print("unix", sock(socket.AF_UNIX, socket.SOCK_STREAM))
''' % ((272, 101) if platform.machine() == 'x86_64' else (97, 117))
        result = run('run', sys.executable, '-v', str(workspace), '--', '-c', baseline)
        assert result.returncode == 0, result
        got = dict(line.split(' ', 1) for line in result.stdout.strip().splitlines() if ' ' in line)
        assert got['unshare'] == '1' and got['ptrace'] == '1', got            # EPERM
        assert got['io_uring_setup'] == '38' and got['clone3'] == '38', got   # ENOSYS
        assert got['mptcp'] == '93' and got['packet'] == '97' and got['raw'] == '97', got
        assert got['udp'] == 'open' and got['unix'] == 'open', got
        assert 'threads ok' in result.stdout
        # A granted port is a port to reach; listening needs its own grant.
        result = run('run', sys.executable, '--allow-net', '*:443', '-v', str(workspace), '--', '-c', bind_probe, '0')
        assert result.returncode == 0 and 'bind denied' in result.stdout, result
        result = run('run', sys.executable, '--allow-net', '*:443', '--allow-bind', '47123', '-v', str(workspace), '--', '-c', bind_probe, '47123')
        assert result.returncode == 0 and result.stdout.strip() == 'bound', result
        # From ABI 6 the domain is scoped: no signal reaches a process outside it.
        import ctypes
        abi = ctypes.CDLL(None).syscall(444, None, 0, 1)
        if abi >= 6:
            result = run('run', '/bin/sh', '-v', str(workspace), '--', '-c', 'kill -0 1 2>/dev/null && echo SIGNALLED || echo scoped')
            assert 'scoped' in result.stdout, result
            print('PASS: baseline seccomp closes the bypass syscalls and families; bind needs a grant; signals are scoped')
        else:
            print(f'PASS: baseline seccomp closes the bypass syscalls and families; bind needs a grant '
                  f'(Landlock ABI {abi}: signal scoping not available here, not tested)')
    else:
        result = run('run', '/bin/echo', '--', 'must-not-execute')
        assert result.returncode != 0 and 'must-not-execute' not in result.stdout, result
        print('PASS: unsupported native sandbox fails closed')

    shutil.rmtree(ungranted, ignore_errors=True)
