"""Actionable failure feedback; never supplies expected answer values."""
import json


def different_paths(expected, actual, path=''):
    if isinstance(expected, dict) and isinstance(actual, dict):
        paths = []
        for key in sorted(set(expected) | set(actual)):
            pointer = path + '/' + key.replace('~', '~0').replace('/', '~1')
            if key not in expected or key not in actual:
                paths.append(pointer)
            else:
                paths.extend(different_paths(expected[key], actual[key], pointer))
        return paths
    if expected != actual:
        return [path or '/']
    return []


def feedback(task, artifacts, calls, checks):
    messages = []
    for key, passed in checks.items():
        if passed:
            continue
        kind, _, path = key.partition(':')
        if kind == 'preserved':
            messages.append(f'Restore the original input file {path}; this task requires it to remain unchanged.')
        elif kind == 'read':
            messages.append(f'Call read_file with path {path} to inspect the required input.')
        elif kind == 'inspected':
            messages.append(f'Call read_file with path {path} AFTER the last write to inspect the output. Rewriting the file does not satisfy inspection.')
        elif kind == 'output':
            if path not in artifacts:
                messages.append(f'Create the required output file {path} with write_file.')
            else:
                try:
                    actual = json.loads(artifacts[path])
                    paths = different_paths(task['expected_json'][path], actual)[:12]
                    messages.append(f'{path} has incorrect or missing values at JSON paths {paths}. Re-read the task and input data, correct these fields, then inspect the file.')
                except ValueError as error:
                    messages.append(f'{path} is not valid JSON: {error}. Write actual JSON, not Markdown or literal backslash-n characters between fields.')
        elif key == 'python_tests':
            messages.append('Run run_tests to diagnose stats.py, then fix mean(values) according to the task and run tests again.')
        elif key == 'tests_before_and_after':
            tests = [i for i, call in enumerate(calls) if call['name'] == 'run_tests']
            writes = [i for i, call in enumerate(calls) if call['name'] == 'write_file' and call['arguments'].get('path') == 'stats.py']
            if writes and (not tests or min(writes) < min(tests)):
                messages.append('The required pre-edit test was omitted. This historical requirement cannot be repaired within this run; do not claim complete success.')
            elif not writes:
                messages.append('Run run_tests BEFORE editing stats.py. Then edit the function and run run_tests AFTER the final edit.')
            else:
                messages.append('Run run_tests AFTER the last edit to stats.py.')
        elif kind == 'used':
            messages.append(f'The task requires at least {task["required_tools"][path]} calls to {path}; follow the required before/after order.')
    return messages
