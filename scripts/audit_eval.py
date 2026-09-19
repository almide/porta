#!/usr/bin/env python3
"""Regrade saved evidence and verify matched inference inputs without inference."""
import argparse
import json
import gzip
import tomllib
from collections import defaultdict
from evals.grading import grade
from evals.feedback import feedback

parser = argparse.ArgumentParser()
parser.add_argument('report')
parser.add_argument('--expected-repeats', type=int, default=3)
args = parser.parse_args()
with (gzip.open(args.report, 'rt') if args.report.endswith('.gz') else open(args.report)) as source:
    report = json.load(source)
tasks = {task['id']: task for task in report['tasks']}
summary = defaultdict(lambda: {'full_pass': 0, 'artifact_pass': 0, 'successful_exit_failed_task': 0, 'process_errors': 0, 'total': 0})
compute_counts = defaultdict(lambda: {'calls': 0, 'script_errors': 0, 'transport_or_runtime_errors': 0})
if report.get('shared_compute'):
    assert report['shared_compute'] == {'transport':'shared MCP HTTP -> Porta MCP stdio -> WASM', 'fuel':50000000, 'memory_pages':1024, 'timeout_seconds':20, 'mounts':[], 'environment':[]}
    assert sum(t['name'] == 'compute' for t in report['tool_definitions']) == 1
    for name in ('compute_wasm', 'compute_manifest', 'compute_runner'):
        digest = report['sha256'][name]
        assert len(digest) == 64 and all(c in '0123456789abcdef' for c in digest)
    preflight = report['compute_preflight']
    assert len(preflight) == 1 and preflight[0]['name'] == 'compute'
    assert preflight[0]['arguments'] == {'script':'input + 0.2', 'input_json':'0.1'}
    assert preflight[0]['compute_result'] == {'ok':True, 'json':'0.3'}
    assert preflight[0]['compute_mcp_result']['isError'] is False
    assert json.loads(preflight[0]['compute_mcp_result']['content'][0]['text']) == preflight[0]['compute_result']
expected_tools = [{'type': 'function', 'function': {'name': t['name'], 'description': t.get('description', ''), 'parameters': t['inputSchema']}} for t in report['tool_definitions']]
assert report['runs'], 'no measured runs'
expected = {(runtime, task, repeat) for runtime in ('porta_wasm', 'docker_agent_native') for task in tasks for repeat in range(args.expected_repeats)}
actual = [(run['runtime'], run['task'], run['repeat']) for run in report['runs']]
assert len(actual) == len(expected) and set(actual) == expected, 'incomplete or duplicated evaluation matrix' 
for run in report['runs']:
    task = tasks[run['task']]
    if report.get('before_tool_policies'):
        expected_policy = {'tool': 'write_file', 'requires': 'run_tests', 'requires_arguments': {}} if task.get('python_test') else None
        assert report['before_tool_policies'].get(task['id']) == expected_policy, 'policy differs from declared task prerequisite'
        config = tomllib.loads(report['configurations']['porta_per_task'][task['id']])
        guards = config.get('before_tool_checks', [])
        if expected_policy:
            assert len(guards) == 1 and guards[0]['parameters'] == expected_policy, 'configured policy mismatch'
            if run['runtime'] == 'porta_wasm':
                prior_test = False
                for event in run['tool_calls']:
                    if event['name'] == 'run_tests':
                        prior_test = True
                    if event['name'] == 'write_file':
                        assert prior_test, 'Porta executed an edit before the required test'
        else:
            assert not guards, 'unexpected task policy'
    checks = grade(task, run['artifacts'], run['tool_calls'])
    assert checks == run['checks'], (run['runtime'], run['task'], 'stored grading mismatch')
    passed = run['exit_code'] == 0 and not run['timed_out'] and all(checks.values())
    assert passed == run['passed'], 'stored pass flag mismatch'
    assert run['model_calls'], 'run made no model requests'
    for call in run['model_calls']:
        request = call['request']
        assert request['model'] == report['model'], 'model mismatch'
        assert request['temperature'] == report['parameters']['temperature'], 'temperature mismatch'
        assert request['max_tokens'] == report['parameters']['max_tokens'], 'token limit mismatch'
        assert request['tools'] == expected_tools, 'model-visible tool declaration mismatch'
        assert request['messages'][0] == {'role': 'system', 'content': report['instruction']}, 'system instruction mismatch'
        assert request['messages'][1] == {'role': 'user', 'content': task['prompt']}, 'task prompt mismatch'
        assert 'response' in call and 'transport_error' not in call, 'inference transport failure'
    for index, event in enumerate(run['tool_calls']):
        if event['name'] == 'compute':
            assert report.get('shared_compute'), 'undeclared compute service'
            counts = compute_counts[run['runtime']]
            counts['calls'] += 1
            assert set(event['arguments']) == {'script', 'input_json'}
            if 'compute_failure' in event:
                assert isinstance(event['compute_failure'], str) and event['compute_failure']
                assert 'compute_result' not in event and 'compute_mcp_result' not in event
                counts['transport_or_runtime_errors'] += 1
            else:
                result, wire = event['compute_result'], event['compute_mcp_result']
                assert type(result['ok']) is bool
                assert len(wire['content']) == 1 and wire['content'][0]['type'] == 'text'
                assert json.loads(wire['content'][0]['text']) == result, 'compute payload lost or altered'
                assert wire['isError'] is (not result['ok']), 'compute status mismatch'
                if result['ok']:
                    assert isinstance(result['json'], str)
                    json.loads(result['json'])
                else:
                    assert result['error']['code'] == 'compute_failed'
                    counts['script_errors'] += 1
        if event['name'] == 'verify_task':
            snapshot_checks = grade(task, event['verification_snapshot'], run['tool_calls'][:index + 1])
            expected_verdict = {'passed': all(snapshot_checks.values()), 'checks': snapshot_checks}
            if report.get('verification_feedback') == 'actionable':
                expected_verdict['feedback'] = feedback(task, event['verification_snapshot'], run['tool_calls'][:index + 1], snapshot_checks)
            assert event['verification'] == expected_verdict, 'verification snapshot mismatch'
    if report.get('evaluation_mode') == 'shared_verification_tool_with_porta_completion_gate' and run['runtime'] == 'porta_wasm' and run['exit_code'] == 0:
        assert run['tool_calls'][-1]['name'] == 'verify_task' and run['tool_calls'][-1]['verification']['passed'], 'Porta completed without a final passing verification'
    item = summary[run['runtime']]
    item['total'] += 1
    item['full_pass'] += passed
    item['artifact_pass'] += all(v for k, v in checks.items() if k.startswith(('preserved:', 'output:', 'python_tests')))
    item['successful_exit_failed_task'] += run['exit_code'] == 0 and not passed
    item['process_errors'] += run['exit_code'] != 0
for runtime, counts in summary.items():
    assert report['summary'][runtime]['passed'] == counts['full_pass']
    assert report['summary'][runtime]['total'] == counts['total']
print(json.dumps(summary, indent=2))
if report.get('shared_compute'):
    print(json.dumps({'shared_compute': compute_counts}, indent=2))
print('PASS: regraded artifacts/procedures and matched model, temperature, token limits, prompts and tool declarations')
