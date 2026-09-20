"""Pure artifact and procedure grading for the published small task suite."""
import json
from collections import Counter
from .mean_fixture import check_python

def grade(task, artifacts, calls):
    checks = {}
    for name, content in task['files'].items():
        if name not in task.get('editable', []):
            checks['preserved:' + name] = artifacts.get(name, '') == content
    for name in task['files']:
        checks['read:' + name] = any(call['name'] == 'read_file' and call['arguments'].get('path') == name for call in calls)
    for name, expected in task.get('expected_json', {}).items():
        try:
            checks['output:' + name] = json.loads(artifacts.get(name, '')) == expected
        except (OSError, ValueError):
            checks['output:' + name] = False
        writes = [i for i, call in enumerate(calls) if call['name'] == 'write_file' and call['arguments'].get('path') == name]
        reads = [i for i, call in enumerate(calls) if call['name'] == 'read_file' and call['arguments'].get('path') == name]
        checks['inspected:' + name] = bool(writes and reads and max(reads) > max(writes))
    if task.get('python_test'):
        result = check_python(artifacts.get('stats.py', ''))
        checks['python_tests'] = result['passed'] == result['total']
        tests = [i for i, call in enumerate(calls) if call['name'] == 'run_tests']
        writes = [i for i, call in enumerate(calls) if call['name'] == 'write_file' and call['arguments'].get('path') == 'stats.py']
        checks['tests_before_and_after'] = bool(tests and writes and min(tests) < min(writes) and max(tests) > max(writes))
    counts = Counter(call['name'] for call in calls)
    for name, minimum in task.get('required_tools', {}).items():
        checks['used:' + name] = counts[name] >= minimum
    return checks

