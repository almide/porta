#!/usr/bin/env python3
"""Regrade the small compute capability smoke without claiming benchmark wins."""
import gzip
import hashlib
import json
import sys
from decimal import Decimal

path = sys.argv[1]
with (gzip.open(path, 'rt') if path.endswith('.gz') else open(path)) as source:
    report = json.load(source)
for name, source in report['sources'].items():
    assert hashlib.sha256(source.encode()).hexdigest() == report['sha256'][name], 'source snapshot mismatch'
expected = {
    'decimal': '3.80',
    'transform': '{"new_name":7,"nested":{"old_name":8},"label":"日本語"}',
    'integer': '{"counter":9007199254740994}',
}


def equivalent(actual, expected_text):
    try:
        return json.loads(actual, parse_float=Decimal) == json.loads(expected_text, parse_float=Decimal)
    except (TypeError, ValueError):
        return False


def check_intent(pending, run):
    """Check one recorded intent, and say whether it was a model call."""
    if pending['kind'] == 'model':
        assert pending['request']['messages'][1] == {'role': 'user', 'content': run['prompt']}
        return 1
    assert pending['request']['name'] == 'compute'
    return 0


def replay(run):
    """Walk one run's journal, checking its hash chain and pairing every intent
    with its result. Returns the model-call count and the tool responses."""
    previous, pending, index, models = '', None, 0, 0
    results = []
    entries = run['journal']
    assert entries[0]['payload']['kind'] == 'header'
    assert entries[0]['payload']['task'] == run['prompt']
    assert entries[-1]['payload']['kind'] == 'complete'
    for position, entry in enumerate(entries):
        payload = entry['payload']
        canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
        digest = hashlib.sha256((previous + '\n' + canonical).encode()).hexdigest()
        assert entry['previous'] == previous and entry['hash'] == digest, 'journal hash mismatch'
        previous = digest
        kind = payload['kind']
        if kind == 'header':
            assert position == 0
        elif kind == 'intent':
            assert pending is None and payload['index'] == index
            pending = payload['request']
            assert pending['kind'] in ('model', 'tool')
            models += check_intent(pending, run)
        elif kind == 'result':
            assert pending is not None and payload['index'] == index
            results += [payload['response']] if pending['kind'] == 'tool' else []
            pending, index = None, index + 1
        elif kind == 'complete':
            assert position == len(entries) - 1 and pending is None
            assert payload['output'] == run['stdout'].strip()
        else:
            raise AssertionError('unexpected journal record')
    return models, results


assert len(report['runs']) == len(expected)
assert {run['task'] for run in report['runs']} == set(expected)
summary = []
for run in report['runs']:
    assert run['expected_json'] == expected[run['task']]
    models, results = replay(run)
    computed = bool(results) and results[-1].get('ok') is True and equivalent(results[-1].get('json'), run['expected_json'])
    answered = equivalent(run['stdout'], run['expected_json'])
    summary.append({'task': run['task'], 'passed': run['exit_code'] == 0 and computed and answered,
                    'model_calls': models, 'tool_calls': len(results),
                    'tool_errors': sum(r.get('ok') is not True for r in results),
                    'computed_expected_result': computed, 'answered_expected_result': answered})
print(json.dumps(summary, ensure_ascii=False, indent=2))
print('PASS: complete smoke evidence and hash chains checked; grades recomputed including failures (not authenticated replay or general quality)')
