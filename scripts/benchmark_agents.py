#!/usr/bin/env python3
"""Process-start to final-answer latency against an identical local model fixture.

Docker Agent runs in native mode here. This is not a container/isolation benchmark
or a model-quality evaluation. Failed runs abort instead of improving the average.
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import platform
import re
import signal
import statistics
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument('--porta', default='target/porta')
parser.add_argument('--agent', default='examples/chat-agent/agent.wasm')
parser.add_argument('--runs', type=int, default=7)
parser.add_argument('--porta-record', action='store_true', help='Record a fresh durable Porta journal in each invocation')
parser.add_argument('--memory-runs', type=int, default=0, help='Separate macOS /usr/bin/time -l runs; never included in latency samples')
parser.add_argument('--output', default='target/agent-benchmark.json')
args = parser.parse_args()
if args.runs < 3:
    parser.error('at least 3 measured runs are required')
if args.memory_runs < 0 or args.memory_runs and platform.system() != 'Darwin':
    parser.error('--memory-runs requires macOS and a nonnegative count')
porta = str(pathlib.Path(args.porta).resolve())
agent = str(pathlib.Path(args.agent).resolve())
answer = 'benchmark-ok'
requests = []

class Fixture(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append({'path': self.path, 'model': body.get('model'), 'stream': body.get('stream', False), 'body': body})
        if body.get('stream'):
            chunk = {'id': 'fixture-1', 'object': 'chat.completion.chunk', 'created': 1, 'model': 'fixture',
                     'choices': [{'index': 0, 'delta': {'role': 'assistant', 'content': answer}, 'finish_reason': None}]}
            end = {**chunk, 'choices': [{'index': 0, 'delta': {}, 'finish_reason': 'stop'}]}
            usage = {**chunk, 'choices': [], 'usage': {'prompt_tokens': 10, 'completion_tokens': 2, 'total_tokens': 12}}
            payload = ('data: ' + json.dumps(chunk) + '\n\ndata: ' + json.dumps(end) + '\n\ndata: ' + json.dumps(usage) + '\n\ndata: [DONE]\n\n').encode()
            content_type = 'text/event-stream'
        else:
            payload = json.dumps({'id': 'fixture-1', 'object': 'chat.completion', 'created': 1, 'model': 'fixture',
                'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': answer}, 'finish_reason': 'stop'}],
                'usage': {'prompt_tokens': 10, 'completion_tokens': 2, 'total_tokens': 12}}).encode()
            content_type = 'application/json'
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
    def log_message(self, *args):
        pass

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-benchmark-') as directory:
        root = pathlib.Path(directory)
        endpoint = f'http://127.0.0.1:{server.server_port}/v1'
        (root / 'porta.toml').write_text(f'''version = 1
[agent]
wasm = {json.dumps(agent)}
instruction = "Return benchmark-ok."
[model]
endpoint = "{endpoint}/chat/completions"
name = "fixture"
token_env = "PORTA_BENCH_TOKEN"
''')
        (root / 'docker.yaml').write_text(f'''agents:
  root:
    model: fixture
    instruction: Return benchmark-ok.
models:
  fixture:
    provider: openai
    model: fixture
    base_url: {endpoint}
    token_key: PORTA_BENCH_TOKEN
''')
        env = dict(os.environ, PORTA_BENCH_TOKEN='local-fixture-token', OPENAI_API_KEY='local-fixture-token', TELEMETRY_ENABLED='false', DOCKER_AGENT_AUTO_INSTALL='false')
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
            env.pop(key, None)
        docker = ['docker', 'agent', '--config-dir', str(root / 'config'), '--data-dir', str(root / 'data'), '--cache-dir', str(root / 'cache')]
        versions = {'porta': subprocess.check_output([porta, '--version'], text=True).strip(),
                    'docker_agent': subprocess.check_output(docker + ['version'], env=env, text=True).strip()}
        timings = {'porta_wasm': [], 'docker_agent_native': []}
        evidence = []
        rss = {name: [] for name in timings}
        def measure(name, index, phase):
            journal_path = root / f'journal-{phase}-{index}.jsonl'
            command = ([porta, 'agent', str(root / 'porta.toml')] + (['--record', str(journal_path)] if args.porta_record else []) + ['--', 'Return benchmark-ok.'] if name == 'porta_wasm' else
                       docker + ['run', str(root / 'docker.yaml'), '--exec', '--json', '--session-db', str(root / f'session-{index}.db'), 'Return benchmark-ok.'])
            resource_path = root / f'rusage-{name}-{index}.txt'
            if phase == 'memory':
                command = ['/usr/bin/time', '-l', '-o', str(resource_path)] + command
            before_requests = len(requests)
            start = time.perf_counter()
            process = subprocess.Popen(command, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       text=True, stdin=subprocess.DEVNULL, start_new_session=True)
            timed_out = False
            try:
                stdout, stderr = process.communicate(timeout=30)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, stderr = process.communicate()
                timed_out = True
            result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
            elapsed = (time.perf_counter() - start) * 1000
            calls = requests[before_requests:]
            if name == 'porta_wasm':
                final_answer = result.stdout.strip()
            else:
                try:
                    events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                    final_answer = ''.join(event.get('content', '') for event in events if event.get('type') == 'agent_choice')
                except (ValueError, TypeError, AttributeError):
                    final_answer = None
            record = {'runtime': name, 'index': index, 'phase': phase, 'elapsed_ms': elapsed,
                      'exit_code': result.returncode, 'timed_out': timed_out, 'stdout': result.stdout, 'stderr': result.stderr,
                      'requests': calls, 'final_answer': final_answer}
            record['passed'] = not timed_out and result.returncode == 0 and final_answer == answer and len(calls) == 1
            evidence.append(record)
            if not record['passed']:
                # Preserve the failure; do not publish a median over a successful subset.
                failure_path = pathlib.Path(args.output).with_suffix('.failed.json')
                failure_path.parent.mkdir(parents=True, exist_ok=True)
                failure_path.write_text(json.dumps({'status': 'failed', 'runs': evidence}, indent=2) + '\n')
                raise RuntimeError(f'{name} did not complete exactly one local model call; evidence: {failure_path}')
            if name == 'porta_wasm' and args.porta_record:
                journal_bytes = journal_path.read_bytes()
                record['journal'] = {'bytes': len(journal_bytes), 'records': [json.loads(line) for line in journal_bytes.decode().splitlines()]}
            if phase == 'memory':
                raw = resource_path.read_text()
                matches = re.findall(r'^\s*(\d+)\s+maximum resident set size\s*$', raw, re.MULTILINE)
                if len(matches) != 1 or int(matches[0]) <= 0:
                    raise RuntimeError(f'missing or ambiguous macOS RSS measurement: {raw}')
                record.update(max_rss_bytes=int(matches[0]), resource_usage=raw)
                rss[name].append(int(matches[0]))
            return elapsed
        # One process warmup each. Measured invocations still use fresh processes,
        # fresh WASM instances, and fresh Docker Agent session databases.
        for name in timings:
            measure(name, 'warmup', 'warmup')
        for index in range(args.runs):
            order = list(timings) if index % 2 == 0 else list(reversed(timings))
            for name in order:
                timings[name].append(measure(name, index, 'latency'))
        for index in range(args.memory_runs):
            order = list(timings) if index % 2 == 0 else list(reversed(timings))
            for name in order:
                measure(name, f'memory-{index}', 'memory')
        report = {
            'scope': 'fresh process to final answer; one local fixture model call; no tools',
            'isolation': {'porta_wasm': 'WASM guest', 'docker_agent_native': 'native, no --sandbox'},
            'not_measured': ['real-model quality', 'Docker sandbox startup', 'tool throughput', 'production latency', 'simultaneous aggregate process-tree RSS'],
            'runs': evidence,
            'parameters': {'latency_runs': args.runs, 'memory_runs': args.memory_runs, 'warmups_per_runtime': 1, 'porta_record': args.porta_record},
            'memory': {'measured': args.memory_runs > 0, 'method': '/usr/bin/time -l, separate fresh-process trials on macOS' if args.memory_runs else None,
                       'unit': 'bytes', 'samples_max_rss_bytes': rss,
                       'median_max_rss_bytes': {name: statistics.median(values) if values else None for name, values in rss.items()}},
            'platform': platform.platform(), 'versions': versions, 'samples_ms': timings,
            'median_ms': {name: statistics.median(samples) for name, samples in timings.items()},
            'guest_bytes': pathlib.Path(agent).stat().st_size,
            'runtime_bytes': pathlib.Path(porta).stat().st_size,
            'sha256': {name: hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
                       for name, path in [('porta', porta), ('guest', agent), ('harness', __file__)]},
            'requests': requests,
        }
        pathlib.Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        pathlib.Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report['median_ms'], indent=2))
        print(f'Wrote {args.output}')
finally:
    server.shutdown()
    server.server_close()
    thread.join()
