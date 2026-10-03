"""Jobs: policy, worker, records and the HTTP API, against the real binary.

Every case runs the committed sample module or a WASM fixture assembled here,
through `porta job-run` or `porta job-serve`, and checks the record and the
host afterwards: what was granted worked, what was not was refused or denied,
limits stopped the run with their own reason, and nothing was left behind.

    python3 scripts/job_integration.py target/porta
"""
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from wasm_fixtures import no_environment_guest, section, string, uleb  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
SAMPLE = ROOT / 'examples' / 'enterprise' / 'sample-job' / 'sample-job.wasm'
TOKEN = 'evaluation-token-0123456789'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def host_import_module():
    """A module that imports a host function porta never provides."""
    types = b'\x01\x60\x00\x00'
    imports = b'\x01' + string('porta') + string('exec_command') + b'\x00\x00'
    exports = b'\x01' + string('_start') + b'\x00\x01'
    body = b'\x02\x00\x0b'
    return (b'\x00asm\x01\x00\x00\x00' + section(1, types) + section(2, imports)
            + section(3, b'\x01\x00') + section(7, exports) + section(10, b'\x01' + body))


def symlink_module(target):
    """Creates /output/leak -> target with WASI path_symlink (fd 4 is /output)."""
    types = b'\x02\x60\x05\x7f\x7f\x7f\x7f\x7f\x01\x7f\x60\x00\x00'
    imports = b'\x01' + string('wasi_snapshot_preview1') + string('path_symlink') + b'\x00\x00'
    exports = b'\x02' + string('memory') + b'\x02\x00' + string('_start') + b'\x00\x01'
    old, new = target.encode(), b'leak'
    # path_symlink(old_ptr=16, old_len, fd=4, new_ptr=256, new_len); drop errno
    code = (b'\x41\x10' + b'\x41' + uleb(len(old)) + b'\x41\x04' + b'\x41\x80\x02' + b'\x41' + uleb(len(new))
            + b'\x10\x00\x1a\x0b')
    body = b'\x00' + code
    data = (b'\x00\x41\x10\x0b' + uleb(len(old)) + old) + (b'\x00\x41\x80\x02\x0b' + uleb(len(new)) + new)
    return (b'\x00asm\x01\x00\x00\x00' + section(1, types) + section(2, imports)
            + section(3, b'\x01\x01') + section(5, b'\x01\x00\x01') + section(7, exports)
            + section(10, b'\x01' + uleb(len(body)) + body) + section(11, b'\x02' + data))


class Env:
    def __init__(self, porta):
        self.porta = str(Path(porta).resolve())
        self.root = Path(tempfile.mkdtemp(prefix='porta-jobs-'))
        (self.root / 'modules').mkdir()
        (self.root / 'catalog').mkdir()
        (self.root / 'scratch').mkdir()
        (self.root / 'catalog' / 'prices.csv').write_text('item,price_cents\nwidget,250\ngadget,1200\nbolt,15\n')
        modules = {
            'sample-job': SAMPLE.read_bytes(),
            'host-import': host_import_module(),
            'symlink-out': symlink_module('../../../../../../../../../../etc/hosts'),
            'no-env': no_environment_guest(),
        }
        entries = []
        for name, data in modules.items():
            (self.root / 'modules' / f'{name}.wasm').write_bytes(data)
            entries.append(f'[[modules]]\nname = "{name}"\nwasm = "modules/{name}.wasm"\nsha256 = "{sha(data)}"\n')
        self.policy = self.root / 'policy.toml'
        self.policy.write_text('\n'.join([
            'version = 1', 'label = "integration"', 'env_names = ["REPORT_CURRENCY"]', '',
            '[service]', 'records = "state/records"', 'workspaces = "state/workspaces"',
            'max_concurrent = 2', 'retain_output = true', 'max_request_bytes = 65536', '',
            '[limits]', 'timeout_ms = 15000', 'fuel = 100000000000', 'memory_mib = 64',
            'max_output_bytes = 1048576', 'max_input_bytes = 32768', '',
            '[defaults]', 'timeout_ms = 3000', 'fuel = 1000000000', 'memory_mib = 32',
            'max_output_bytes = 65536', 'max_input_bytes = 32768', '',
            *entries,
            '[[directories]]', 'name = "catalog"', 'host = "catalog"', 'access = "read"', '',
            '[[directories]]', 'name = "scratch"', 'host = "scratch"', 'access = "read-write"', '',
        ]))
        self.records = self.root / 'state' / 'records'
        self.workspaces = self.root / 'state' / 'workspaces'

    def job(self, **fields):
        job = {'version': 1, 'module': 'sample-job'}
        job.update(fields)
        return job

    def run(self, job):
        path = self.root / 'job.json'
        path.write_text(json.dumps(job) if not isinstance(job, str) else job)
        done = subprocess.run([self.porta, 'job-run', '--policy', str(self.policy), str(path)],
                              capture_output=True, text=True, timeout=60)
        assert done.stdout.strip(), done
        return done.returncode, json.loads(done.stdout)

    def events(self):
        return [json.loads(line) for line in (self.records / 'events.jsonl').read_text().splitlines()]


def workers_running(settle=2.0):
    """Worker processes still alive after up to `settle` seconds of teardown."""
    deadline = time.time() + settle
    while True:
        listing = subprocess.run(['ps', '-axo', 'pid=,command='], capture_output=True, text=True).stdout
        found = [line for line in listing.splitlines()
                 if len(line.split()) > 2 and line.split()[1].endswith('porta') and '__job-worker' in line.split()[2:]]
        if not found or time.time() > deadline:
            return found
        time.sleep(0.1)


def check(condition, message, detail=None):
    if not condition:
        raise AssertionError(f'{message}: {json.dumps(detail, indent=2) if detail is not None else ""}')
    print(f'ok  {message}')


REPORT_ORDERS = 'item,quantity\nwidget,4\ngadget,1\nbolt,20\n'


def one_shot(env):
    code, record = env.run(env.job(args=['report'], files={'orders.csv': REPORT_ORDERS},
                                   directories=[{'name': 'catalog', 'access': 'read'}]))
    check(code == 0 and record['outcome'] == 'succeeded' and record['stop_reason'] == 'exit', 'a granted job succeeds', record)
    check(record['stdout']['text'] == 'report: 3 orders, total 2500 cents\n', 'its result is the expected one', record['stdout'])
    report = env.records / record['run_id'] / 'output' / 'report.json'
    check(json.loads(report.read_text()) == {'orders': 3, 'total_cents': 2500}, 'its output file is retained with the record')
    check(record['output_files'][0]['sha256'] == sha(report.read_bytes()), 'the record carries the output digest')
    check(record['policy']['label'] == 'integration' and record['policy']['sha256'] == sha(env.policy.read_bytes()),
          'the record names the policy version and digest', record['policy'])
    check(record['module']['sha256'] == sha(SAMPLE.read_bytes()), 'the record names the module digest')
    for key in ('run_id', 'submitted_at', 'started_at', 'finished_at', 'job_sha256', 'limits', 'grants', 'usage'):
        check(key in record, f'the record has {key}')
    check(record['workspace_removed'] and not (env.workspaces / record['run_id']).exists(), 'the workspace is removed')
    events = [e['event'] for e in env.events() if e['run_id'] == record['run_id']]
    check(events == ['started', 'finished'], 'events.jsonl has the start and the end', events)
    check(REPORT_ORDERS not in (env.records / 'events.jsonl').read_text(), 'the event log holds no job input')

    code, record = env.run(env.job(args=['probe'], directories=[{'name': 'catalog', 'access': 'read'}]))
    lines = record['stdout']['text'].splitlines()
    check(code == 0 and len(lines) == 6 and all(line.endswith(': denied') for line in lines),
          'every ungranted read and write is denied inside the sandbox', lines)
    check(sorted(p.name for p in (env.root / 'catalog').iterdir()) == ['prices.csv'], 'the read-only directory is unchanged')

    code, record = env.run(env.job(args=['spin'], limits={'timeout_ms': 1500, 'fuel': 100000000000}))
    check(code == 1 and record['outcome'] == 'stopped' and record['stop_reason'] == 'timeout', 'an infinite loop stops at the deadline', record)
    check(1400 <= record['usage']['wall_ms'] < 6000, 'it stops near the deadline', record['usage'])
    check(not workers_running(), 'no worker process is left', workers_running())
    check(not any(env.workspaces.iterdir()), 'no workspace is left after a stopped run')

    code, record = env.run(env.job(args=['spin'], limits={'fuel': 20000000}))
    check(record['stop_reason'] == 'fuel_exhausted' and record['usage']['fuel_consumed'] == 20000000, 'fuel exhaustion stops the run', record)
    code, record = env.run(env.job(args=['grow'], limits={'memory_mib': 16}))
    check(record['stop_reason'] == 'memory_limit' and record['usage']['memory_grow_denied'], 'the memory ceiling stops the run', record)
    check(record['usage']['peak_memory_bytes'] <= 16 * 1024 * 1024, 'linear memory never passed the ceiling', record['usage'])
    code, record = env.run(env.job(args=['flood'], limits={'max_output_bytes': 4096}))
    check(record['stdout']['bytes'] == 4096 and record['stdout']['capped'] and record['outcome'] == 'stopped',
          'output stops at its cap', record['stdout'] | {'reason': record['stop_reason']})

    refusals = [
        (env.job(module='nope'), 'module_not_registered'),
        (env.job(directories=[{'name': 'catalog', 'access': 'read-write'}]), 'access_exceeds_grant'),
        (env.job(directories=[{'name': 'home', 'access': 'read'}]), 'directory_not_granted'),
        (env.job(limits={'timeout_ms': 999999}), 'limit_exceeds_ceiling'),
        (env.job(env={'HOME': '/root'}), 'env_not_allowed'),
        (env.job(files={'../escape': 'x'}), 'invalid_file_name'),
        (env.job(network=True), 'invalid_job'),
        (env.job(input='x' * 40000), 'input_too_large'),
        ('{"version": 1, "module": ', 'invalid_job'),
    ]
    for job, reason in refusals:
        code, record = env.run(job)
        check(code == 2 and record['outcome'] == 'refused' and record['stop_reason'] == reason,
              f'a job asking beyond the policy is refused: {reason}', record)
        check((env.records / f"{record['run_id']}.json").exists(), f'the refusal is recorded: {reason}')
    code, record = env.run(env.job(args=['report'], env={'REPORT_CURRENCY': 'JPY'}, files={'orders.csv': REPORT_ORDERS},
                                   directories=[{'name': 'catalog', 'access': 'read'}]))
    check(code == 0 and record['grants']['env_names'] == ['REPORT_CURRENCY'], 'a granted environment name is accepted')
    check('JPY' not in json.dumps(record), 'environment values stay out of the record')

    code, record = env.run(env.job(module='no-env'))
    check(code == 0 and record['outcome'] == 'succeeded', 'a module sees no environment variable of the host', record)
    code, record = env.run(env.job(module='no-env', env={'REPORT_CURRENCY': 'JPY'}))
    check(record['outcome'] == 'failed' and record['stop_reason'] == 'trap', 'and sees exactly the variables its job set', record)

    code, record = env.run(env.job(module='host-import'))
    check(record['outcome'] == 'refused' and record['stop_reason'] == 'link_refused', 'a module importing a host function porta does not grant never starts', record)
    code, record = env.run(env.job(module='symlink-out'))
    leak = [f for f in record['output_files'] if f['path'] == 'leak']
    check(leak and leak[0].get('ignored') == 'symlink' and 'sha256' not in leak[0], 'a symlink left in /output is never followed', record['output_files'])
    check(not (env.records / record['run_id'] / 'output' / 'leak').exists(), 'nothing behind the symlink is retained')


def main():
    from job_server_checks import policy_refusals, service, startup_refusals
    env = Env(sys.argv[1])
    try:
        one_shot(env)
        policy_refusals(env)
        startup_refusals(env)
        service(env)
    finally:
        shutil.rmtree(env.root, ignore_errors=True)
    print('job integration: all checks passed')


if __name__ == '__main__':
    main()
