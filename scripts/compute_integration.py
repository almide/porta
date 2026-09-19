#!/usr/bin/env python3
"""Real WASM computation, exact JSON transport, confinement and durable replay."""
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading

porta, agent, compute, writer = [str(pathlib.Path(p).resolve()) for p in sys.argv[1:5]]
requests = []
script, input_json, write_result = 'input', 'null', False


class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(request)
        results = [m for m in request['messages'] if m['role'] == 'tool']
        if not results:
            name, args = 'compute', {'script': script, 'input_json': input_json}
        elif write_result and len(results) == 1:
            result = json.loads(results[0]['content'])
            assert result['ok'], result
            name, args = 'write_file', {'path': 'result.json', 'content': result['json']}
        else:
            name = None
        if name:
            message = {'role': 'assistant', 'tool_calls': [{'id': f'call-{len(results)}', 'type': 'function',
                       'function': {'name': name, 'arguments': json.dumps(args)}}]}
        else:
            message = {'role': 'assistant', 'content': 'complete'}
        body = json.dumps({'choices': [{'message': message}]}).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Model)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='porta-compute-') as directory:
        root = pathlib.Path(directory).resolve()
        work = root / 'work'
        work.mkdir()
        config = root / 'agent.toml'
        base = f'''version=1
[agent]
wasm={json.dumps(agent)}
[model]
endpoint="http://127.0.0.1:{server.server_port}/model"
name="fixture"
[limits]
max_steps=12
max_model_calls=4
fuel_per_step=50000000
[[tools]]
name="compute"
wasm={json.dumps(compute)}
[[tools]]
name="write_file"
wasm={json.dumps(writer)}
mounts=[{{host="work",guest=".",read_only=false}}]
'''
        config.write_text(base)

        def run(*args, error=None):
            result = subprocess.run([porta, *map(str, args)], text=True, capture_output=True, timeout=20)
            if error:
                assert result.returncode != 0 and error in result.stderr, result
            else:
                assert result.returncode == 0, result.stderr
            return result

        def result():
            return json.loads(next(m['content'] for m in requests[-1]['messages'] if m['role'] == 'tool'))

        cases = [
            ('input.a + input.b', '{"a":0.1,"b":0.2}', '0.3'),
            ('input', '9007199254740993', '9007199254740993'),
            ('input', '18446744073709551615', '18446744073709551615'),
            ('let total = 0.0; for row in input { if row.include { total += row.amount; } } total',
             '[{"include":true,"amount":4.10},{"include":false,"amount":99},{"include":true,"amount":-0.30}]', '3.80'),
            ('input.new_name = input.remove("old_name"); input',
             '{"old_name":7,"nested":{"old_name":8},"label":"日本語"}',
             '{"label":"日本語","nested":{"old_name":8},"new_name":7}'),
            ('print("noise"); debug("noise"); input', 'null', 'null'),
        ]
        for script, input_json, expected in cases:
            run('agent', config, '--', 'compute')
            assert result() == {'ok': True, 'json': expected}, result()
        print('PASS: real WASM decimal aggregation, large integers, nested JSON and Unicode survive the broker without rounding')

        for script, input_json in [
            ('read_file("/etc/passwd")', 'null'), ('env("HOME")', 'null'),
            ('timestamp()', 'null'), ('import "evil" as x;', 'null'),
            ('1 / 0', 'null'), ('9223372036854775807 + 1', 'null'),
            ('input', '0.12345678901234567890123456789'), ('input', '{'),
            ('fn f() { f() } f()', 'null'), ('[0] * 100000', 'null'), ('loop {}', 'null'),
        ]:
            run('agent', config, '--', 'reject invalid compute')
            assert result()['ok'] is False and result()['error']['code'] == 'compute_failed', result()
        assert not list(work.iterdir())
        print('PASS: no file/environment/clock/import APIs; invalid input, unsupported precision, overflow and resource errors return repairable results')

        script, input_json, write_result = 'input.a + input.b', '{"a":0.1,"b":0.2}', True
        journal = root / 'run.jsonl'
        start = len(requests)
        run('agent', config, '--record', journal, '--pause-after', '2', '--', 'compute then write')
        assert len(requests) == start + 1 and not (work / 'result.json').exists()
        run('agent-resume', config, journal)
        assert (work / 'result.json').read_text() == '0.3'
        (work / 'result.json').write_text('external edit')
        start = len(requests)
        run('agent-replay', config, journal)
        assert len(requests) == start and (work / 'result.json').read_text() == 'external edit'
        print('PASS: compute result resumes into a separately granted writer; replay preserves current artifacts without model calls')

        write_result, script, input_json = False, 'input + 1', '4'
        config.write_text(base.replace('fuel_per_step=50000000', 'fuel_per_step=4000000'))
        run('agent', config, '--', 'compute within smaller fuel limit')
        assert result() == {'ok': True, 'json': '5'}
        script, input_json = 'loop {}', 'null'
        bounded = root / 'bounded.jsonl'
        run('agent', config, '--record', bounded, '--', 'bounded compute', error='fuel')
        last = json.loads(bounded.read_text().splitlines()[-1])['payload']
        assert last['kind'] == 'intent' and last['request']['kind'] == 'tool'
        assert last['request']['request']['name'] == 'compute'
        print('PASS: host fuel interrupts runaway script execution inside WASM')
finally:
    server.shutdown()
    server.server_close()
    thread.join()
