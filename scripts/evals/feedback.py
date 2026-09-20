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


def output_message(path, task, artifacts):
    """What to say about a required output file that did not pass."""
    if path not in artifacts:
        return f'Create the required output file {path} with write_file.'
    try:
        actual = json.loads(artifacts[path])
    except ValueError as error:
        return (f'{path} is not valid JSON: {error}. Write actual JSON, not Markdown or '
                'literal backslash-n characters between fields.')
    paths = different_paths(task['expected_json'][path], actual)[:12]
    return (f'{path} has incorrect or missing values at JSON paths {paths}. Re-read the task '
            'and input data, correct these fields, then inspect the file.')


def test_order_message(calls):
    """What to say when the before/after test order was not followed."""
    tests = [i for i, call in enumerate(calls) if call['name'] == 'run_tests']
    writes = [i for i, call in enumerate(calls)
              if call['name'] == 'write_file' and call['arguments'].get('path') == 'stats.py']
    if writes and (not tests or min(writes) < min(tests)):
        return ('The required pre-edit test was omitted. This historical requirement cannot be '
                'repaired within this run; do not claim complete success.')
    if not writes:
        return ('Run run_tests BEFORE editing stats.py. Then edit the function and run '
                'run_tests AFTER the final edit.')
    return 'Run run_tests AFTER the last edit to stats.py.'


def check_message(key, task, artifacts, calls):
    """The one thing to tell the agent about this failed check, or nothing."""
    kind, _, path = key.partition(':')
    if kind == 'preserved':
        return f'Restore the original input file {path}; this task requires it to remain unchanged.'
    if kind == 'read':
        return f'Call read_file with path {path} to inspect the required input.'
    if kind == 'inspected':
        return (f'Call read_file with path {path} AFTER the last write to inspect the output. '
                'Rewriting the file does not satisfy inspection.')
    if kind == 'output':
        return output_message(path, task, artifacts)
    if key == 'python_tests':
        return ('Run run_tests to diagnose stats.py, then fix mean(values) according to the task '
                'and run tests again.')
    if key == 'tests_before_and_after':
        return test_order_message(calls)
    if kind == 'used':
        return (f'The task requires at least {task["required_tools"][path]} calls to {path}; '
                'follow the required before/after order.')
    return None


def feedback(task, artifacts, calls, checks):
    """One message per check that did not pass, in the order they were checked."""
    messages = (check_message(key, task, artifacts, calls)
                for key, passed in checks.items() if not passed)
    return [message for message in messages if message is not None]
