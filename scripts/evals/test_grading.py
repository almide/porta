import json
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from evals.grading import grade

TASKS = {task['id']: task for task in json.loads((Path(__file__).parent / 'tasks.json').read_text())}

def call(name, **arguments):
    return {'name': name, 'arguments': arguments}

class GradingTests(unittest.TestCase):
    def test_missing_output_cannot_pass(self):
        task = TASKS['invoice_totals']
        checks = grade(task, dict(task['files']), [call('read_file', path='invoices.csv')])
        self.assertFalse(checks['output:totals.json'])
        self.assertFalse(all(checks.values()))

    def test_correct_result_requires_inspection_and_preserved_input(self):
        task = TASKS['invoice_totals']
        artifacts = dict(task['files'], **{'totals.json': '{"Aster":10.35,"Birch":11.0}'})
        calls = [call('read_file', path='invoices.csv'), call('write_file', path='totals.json')]
        self.assertFalse(all(grade(task, artifacts, calls).values()))
        calls += [call('read_file', path='totals.json')]
        self.assertTrue(all(grade(task, artifacts, calls).values()))
        artifacts['invoices.csv'] = 'modified input'
        self.assertFalse(all(grade(task, artifacts, calls).values()))

    def test_inspection_before_last_write_is_insufficient(self):
        task = TASKS['invoice_totals']
        artifacts = dict(task['files'], **{'totals.json': '{"Aster":10.35,"Birch":11.0}'})
        calls = [call('read_file', path='invoices.csv'), call('write_file', path='totals.json'),
                 call('read_file', path='totals.json'), call('write_file', path='totals.json')]
        self.assertFalse(grade(task, artifacts, calls)['inspected:totals.json'])

    def test_code_fix_requires_tests_before_and_after(self):
        task = TASKS['mean_empty_fix']
        artifacts = {'stats.py': 'def mean(values):\n return sum(values)/len(values) if values else 0.0'}
        calls = [call('read_file', path='stats.py'), call('run_tests'), call('write_file', path='stats.py'), call('run_tests')]
        self.assertTrue(all(grade(task, artifacts, calls).values()))
        self.assertFalse(grade(task, artifacts, calls[0:1] + calls[2:])['tests_before_and_after'])

if __name__ == '__main__':
    unittest.main()
