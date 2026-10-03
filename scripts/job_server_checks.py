"""The policy and HTTP half of the job suite: policies refused at load, the
server's start-up refusals, the API, concurrency, capacity, cancel, deletion,
SIGTERM and restart after a kill. Run through `job_integration.py`, which
builds the environment."""
import json
import os
import signal
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request

from job_integration import REPORT_ORDERS, TOKEN, check, workers_running


class Server:
    def __init__(self, env, token=TOKEN, flags=None):
        """`flags` replaces the default `--listen 127.0.0.1:<free port>`."""
        self.env = env
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            self.port = probe.getsockname()[1]
        environment = {'PATH': os.environ.get('PATH', ''), 'HOME': os.environ.get('HOME', '/')}
        if token is not None:
            environment['PORTA_JOB_TOKEN'] = token
        flags = flags if flags is not None else ['--listen', f'127.0.0.1:{self.port}']
        self.process = subprocess.Popen([env.porta, 'job-serve', '--policy', str(env.policy), *flags],
                                        env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def ready(self):
        for _ in range(100):
            try:
                if self.anonymous('/healthz')[0] == 200:
                    return self
            except OSError:
                time.sleep(0.05)
        raise AssertionError('the server did not start: ' + (self.process.stderr.read() if self.process.poll() is not None else ''))

    def anonymous(self, path):
        """A GET without the token."""
        return self.send(urllib.request.Request(f'http://127.0.0.1:{self.port}{path}'))

    def request(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(f'http://127.0.0.1:{self.port}{path}', data=data, method=method)
        if data is not None:
            request.add_header('Content-Type', 'application/json')
        request.add_header('Authorization', f'Bearer {TOKEN}')
        return self.send(request)

    @staticmethod
    def send(request):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = response.read()
                status = response.status
        except urllib.error.HTTPError as error:
            payload, status = error.read(), error.code
        kind = 'json' if payload[:1] in (b'{', b'[') else 'raw'
        return status, (json.loads(payload) if kind == 'json' else payload)

    def stop(self, sig=signal.SIGTERM):
        if self.process.poll() is None:
            self.process.send_signal(sig)
            self.process.wait(timeout=20)
        return self.process.returncode


def policy_refusals(env):
    original = env.policy.read_text()
    cases = [
        (original.replace('host = "scratch"', 'host = "modules"'), 'is writable and holds module'),
        (original.replace('host = "catalog"', 'host = "state"'), 'overlaps the service state directory'),
        (original.replace('timeout_ms = 3000', 'timeout_ms = 30000'), 'exceeds 15000'),
        (original.replace('sha256 = "', 'sha256 = "00', 1), 'the policy pins'),
        (original.replace('[service]', '[service]\nos_sandbox = "sometimes"'), 'must be "required" or "off"'),
    ]
    try:
        for text, needle in cases:
            env.policy.write_text(text)
            done = subprocess.run([env.porta, 'job-check', '--policy', str(env.policy)], capture_output=True, text=True, timeout=30)
            check(done.returncode != 0 and needle in done.stdout + done.stderr, f'a policy is refused at load: {needle}', done.stdout + done.stderr)
        env.policy.write_text(original)
        env.records.chmod(0o500)
        done = subprocess.run([env.porta, 'job-check', '--policy', str(env.policy)], capture_output=True, text=True, timeout=30)
        env.records.chmod(0o700)
        check(done.returncode != 0 and 'is not writable' in done.stdout + done.stderr,
              'a state directory the service cannot write is refused at load', done.stdout + done.stderr)
        env.policy.write_text(original.replace('[service]', '[service]\nos_sandbox = "off"'))
        code, record = env.run(env.job(args=['report'], files={'orders.csv': REPORT_ORDERS}, directories=[{'name': 'catalog', 'access': 'read'}]))
        check(code == 0 and record['isolation']['os_sandbox'] is False, 'with os_sandbox off a job runs on the WASM boundary alone', record['isolation'])
    finally:
        env.policy.write_text(original)
    code, record = env.run(env.job(args=['report'], files={'orders.csv': REPORT_ORDERS}, directories=[{'name': 'catalog', 'access': 'read'}]))
    check(record['isolation'] == {'wasm': 'wasmtime', 'worker_process': True, 'os_sandbox': True}, 'by default every worker also runs under the OS sandbox', record['isolation'])


def startup_refusals(env):
    for token, flags, needle in [
        (None, None, 'set PORTA_JOB_TOKEN'),
        ('short', None, 'at least 16 characters'),
        (None, ['--listen', '0.0.0.0:0', '--no-auth'], 'only on a loopback address'),
    ]:
        server = Server(env, token=token, flags=flags)
        out, err = server.process.communicate(timeout=20)
        check(server.process.returncode != 0 and needle in out + err, f'the server refuses to start: {needle}', out + err)


def service(env):
    server = Server(env).ready()
    try:
        status, _ = server.anonymous('/v1/policy')
        check(status == 401, 'the API requires the token')
        status, policy = server.request('GET', '/v1/policy')
        check(status == 200 and policy['label'] == 'integration' and str(env.root) not in json.dumps(policy),
              'the policy is visible without host paths', policy)

        report = env.job(args=['report'], files={'orders.csv': REPORT_ORDERS}, directories=[{'name': 'catalog', 'access': 'read'}])
        status, record = server.request('POST', '/v1/runs?wait=30', report)
        check(status == 200 and record['outcome'] == 'succeeded', 'a job submitted over HTTP succeeds', record)
        status, body = server.request('GET', f"/v1/runs/{record['run_id']}/output/report.json")
        check(status == 200 and body == {'orders': 3, 'total_cents': 2500}, 'its output file is served', body)
        status, _ = server.request('GET', f"/v1/runs/{record['run_id']}/output/../../policy.toml")
        check(status == 404, 'an output path outside the record is not served')
        status, record = server.request('POST', '/v1/runs', env.job(directories=[{'name': 'catalog', 'access': 'read-write'}]))
        check(status == 403 and record['stop_reason'] == 'access_exceeds_grant', 'an over-broad grant is refused with 403', record)

        # Two jobs at once, each with its own input; neither sees the other's.
        jobs = [env.job(args=['report'], files={'orders.csv': f'item,quantity\nwidget,{n}\n'},
                        directories=[{'name': 'catalog', 'access': 'read'}]) for n in (1, 7)]
        results = [None, None]

        def submit(index):
            results[index] = server.request('POST', '/v1/runs?wait=30', jobs[index])[1]
        threads = [threading.Thread(target=submit, args=(i,)) for i in (0, 1)]
        [t.start() for t in threads]
        [t.join() for t in threads]
        totals = [r['stdout']['text'] for r in results]
        check(totals == ['report: 1 orders, total 250 cents\n', 'report: 1 orders, total 1750 cents\n'],
              'concurrent jobs each see only their own input', totals)
        check(results[0]['run_id'] != results[1]['run_id'], 'each run has its own id')

        # Capacity: two long runs fill the policy's two slots; a third is refused.
        spin = env.job(args=['spin'], limits={'timeout_ms': 10000, 'fuel': 100000000000})
        first = server.request('POST', '/v1/runs', spin)[1]
        second = server.request('POST', '/v1/runs', spin)[1]
        status, third = server.request('POST', '/v1/runs', spin)
        check(status == 429 and third['stop_reason'] == 'capacity', 'a job beyond max_concurrent is refused', third)
        started = time.time()
        status, _ = server.request('POST', f"/v1/runs/{first['run_id']}/cancel")
        check(status == 202, 'a running job accepts cancel')
        status, cancelled = server.request('GET', f"/v1/runs/{first['run_id']}?wait=10")
        check(cancelled['outcome'] == 'stopped' and cancelled['stop_reason'] == 'cancelled' and time.time() - started < 5,
              'cancel stops the job promptly', cancelled)
        check(not (env.workspaces / first['run_id']).exists(), 'a cancelled job leaves no workspace')
        server.request('POST', f"/v1/runs/{second['run_id']}/cancel")
        server.request('GET', f"/v1/runs/{second['run_id']}?wait=10")
        status, _ = server.request('POST', f"/v1/runs/{first['run_id']}/cancel")
        check(status == 409, 'cancelling a finished run is a conflict')
        status, _ = server.request('DELETE', f"/v1/runs/{first['run_id']}")
        check(status == 204 and not (env.records / f"{first['run_id']}.json").exists(), 'a finished run can be deleted')
        check(any(e['event'] == 'deleted' and e['run_id'] == first['run_id'] for e in env.events()), 'the deletion is in the event log')
        status, listing = server.request('GET', '/v1/runs?limit=5')
        check(status == 200 and len(listing['runs']) == 5, 'runs are listed newest first')

        # Headers only: the refusal must come before the body is read.
        with socket.create_connection(('127.0.0.1', server.port), timeout=10) as raw:
            raw.sendall(f'POST /v1/runs HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 70000\r\n\r\n'.encode())
            reply = raw.recv(4096).decode()
        check(reply.startswith('HTTP/1.1 413'), 'an oversized request is refused before it is read', reply)

        # The module changes on disk after the policy pinned it.
        module = env.root / 'modules' / 'sample-job.wasm'
        original = module.read_bytes()
        module.write_bytes(original + b'\x00')
        status, record = server.request('POST', '/v1/runs?wait=30', env.job(args=['report']))
        module.write_bytes(original)
        check(record['outcome'] == 'refused' and record['stop_reason'] == 'module_changed', 'a module changed after pinning never runs', record)

        # SIGTERM: the running job ends as service_shutdown and the server exits.
        running = server.request('POST', '/v1/runs', spin)[1]
        time.sleep(0.3)
        code = server.stop()
        record = json.loads((env.records / f"{running['run_id']}.json").read_text())
        check(code == 0 and record['stop_reason'] == 'service_shutdown', 'SIGTERM stops running jobs and records why', record)
    finally:
        server.stop(signal.SIGKILL)

    # A server killed outright: its worker dies with it, and the next start
    # marks the run interrupted without running it again.
    server = Server(env).ready()
    running = server.request('POST', '/v1/runs', spin)[1]
    time.sleep(0.3)
    server.stop(signal.SIGKILL)
    check(not workers_running(settle=5), 'a worker does not outlive a killed server', workers_running(0))
    server = Server(env).ready()
    try:
        status, record = server.request('GET', f"/v1/runs/{running['run_id']}")
        check(record['status'] == 'finished' and record['stop_reason'] == 'interrupted', 'after a restart the run is marked interrupted', record)
        check(not (env.workspaces / running['run_id']).exists(), 'and its workspace is removed')
    finally:
        server.stop()
