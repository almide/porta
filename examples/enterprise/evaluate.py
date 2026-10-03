"""Walk a running `porta job-serve` through the evaluation scenario.

Submits the sample jobs over the API and prints, for each, what was asked,
what came back, and whether that is what the evaluation expects: a granted
job succeeds, an over-broad request is refused, ungranted access is denied
inside the sandbox, limits stop runaway jobs, cancel stops a running one, and
every run has a record. Uses only the Python standard library.

    PORTA_JOB_TOKEN=... python3 examples/enterprise/evaluate.py --url http://127.0.0.1:8080
"""
import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

ORDERS = 'item,quantity\nwidget,4\ngadget,1\nbolt,20\n'
CATALOG = [{'name': 'catalog', 'access': 'read'}]


def job(args, **extra):
    body = {'version': 1, 'module': 'sample-job', 'args': args}
    body.update(extra)
    return body


SCENARIO = [
    ('granted work succeeds', job(['report'], files={'orders.csv': ORDERS}, directories=CATALOG),
     lambda r: r['outcome'] == 'succeeded' and 'total 2500 cents' in r.get('stdout', {}).get('text', '')),
    ('asking for write access the policy does not grant is refused', job(['report'], directories=[{'name': 'catalog', 'access': 'read-write'}]),
     lambda r: r['outcome'] == 'refused' and r['stop_reason'] == 'access_exceeds_grant'),
    ('asking for a directory the policy does not list is refused', job(['report'], directories=[{'name': 'home', 'access': 'read'}]),
     lambda r: r['outcome'] == 'refused' and r['stop_reason'] == 'directory_not_granted'),
    ('asking past a limit ceiling is refused', job(['spin'], limits={'timeout_ms': 3600000}),
     lambda r: r['outcome'] == 'refused' and r['stop_reason'] == 'limit_exceeds_ceiling'),
    ('ungranted reads and writes are denied inside the sandbox', job(['probe'], directories=CATALOG),
     lambda r: r['outcome'] == 'succeeded' and r.get('stdout', {}).get('text', '').count(': denied') == 6),
    ('an infinite loop stops at its deadline', job(['spin'], limits={'timeout_ms': 2000, 'fuel': 100000000000}),
     lambda r: r['stop_reason'] == 'timeout'),
    ('an infinite loop stops when its fuel runs out', job(['spin'], limits={'fuel': 50000000}),
     lambda r: r['stop_reason'] == 'fuel_exhausted'),
    ('unbounded allocation stops at the memory ceiling', job(['grow'], limits={'memory_mib': 16}),
     lambda r: r['stop_reason'] == 'memory_limit'),
    ('unbounded output stops at its cap', job(['flood'], limits={'max_output_bytes': 4096}),
     lambda r: r.get('stdout', {}).get('capped') and r['outcome'] == 'stopped'),
]


class Client:
    def __init__(self, url, token):
        self.url, self.token = url.rstrip('/'), token

    def call(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(self.url + path, data=data, method=method)
        request.add_header('Authorization', f'Bearer {self.token}')
        if data is not None:
            request.add_header('Content-Type', 'application/json')
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.status, json.loads(response.read() or b'null')
        except urllib.error.HTTPError as error:
            return error.code, json.loads(error.read() or b'null')


def line(passed, title, record):
    mark = 'PASS' if passed else 'FAIL'
    print(f"{mark}  {title}\n      run {record.get('run_id')}  outcome={record.get('outcome')}  "
          f"stop_reason={record.get('stop_reason')}  wall_ms={(record.get('usage') or {}).get('wall_ms')}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--url', default='http://127.0.0.1:8080')
    parser.add_argument('--token-env', default='PORTA_JOB_TOKEN')
    options = parser.parse_args()
    token = os.environ.get(options.token_env, '')
    if not token:
        sys.exit(f'set {options.token_env} to the token the server was started with')
    client = Client(options.url, token)
    status, policy = client.call('GET', '/v1/policy')
    if status != 200:
        sys.exit(f'GET /v1/policy answered {status}: {policy}')
    print(f"policy {policy['label']}  sha256={policy['sha256'][:16]}…  os_sandbox={policy.get('os_sandbox')}\n")
    failures = 0
    for title, body, expected in SCENARIO:
        _, record = client.call('POST', '/v1/runs?wait=60', body)
        passed = bool(expected(record))
        failures += not passed
        line(passed, title, record)

    _, running = client.call('POST', '/v1/runs', job(['spin'], limits={'timeout_ms': 10000, 'fuel': 100000000000}))
    time.sleep(0.5)
    asked = time.time()
    client.call('POST', f"/v1/runs/{running['run_id']}/cancel")
    _, record = client.call('GET', f"/v1/runs/{running['run_id']}?wait=30")
    passed = record['stop_reason'] == 'cancelled' and time.time() - asked < 5
    failures += not passed
    line(passed, 'cancel stops a running job', record)

    _, listing = client.call('GET', '/v1/runs?limit=20')
    recorded = all(r['status'] == 'finished' for r in listing['runs'])
    failures += not recorded
    print(f"\n{'PASS' if recorded else 'FAIL'}  every run above has a finished record ({len(listing['runs'])} listed)")
    print('      read one in full: GET /v1/runs/<run_id>; the event log is events.jsonl beside the records')
    print(f"\n{'all checks passed' if not failures else f'{failures} check(s) failed'}")
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
