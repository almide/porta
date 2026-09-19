import json
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from evals.feedback import different_paths, feedback
from evals.grading import grade

TASKS = {task['id']: task for task in json.loads((Path(__file__).parent / 'tasks.json').read_text())}

class FeedbackTests(unittest.TestCase):
    def test_paths_identify_errors_without_supplying_answers(self):
        task = TASKS['invoice_totals']
        artifacts = dict(task['files'], **{'totals.json': '{"Aster":10.35,"Birch":100}'})
        calls = [{'name': 'read_file', 'arguments': {'path': 'invoices.csv'}}, {'name': 'write_file', 'arguments': {'path': 'totals.json'}}]
        text = ' '.join(feedback(task, artifacts, calls, grade(task, artifacts, calls)))
        self.assertIn('/Birch', text)
        self.assertIn('read_file', text)
        self.assertIn('AFTER', text)
        self.assertNotIn('11.0', text)
        self.assertNotIn('10.35', text)

    def test_invalid_json_is_distinguished_from_wrong_values(self):
        task = TASKS['config_migration']
        artifacts = dict(task['files'], **{'config.v2.json': '{\\n"version":2}'})
        text = ' '.join(feedback(task, artifacts, [], grade(task, artifacts, [])))
        self.assertIn('not valid JSON', text)

    def test_historical_requirement_is_not_relaxed(self):
        task = TASKS['mean_empty_fix']
        calls = [{'name': 'write_file', 'arguments': {'path': 'stats.py'}}]
        text = ' '.join(feedback(task, task['files'], calls, grade(task, task['files'], calls)))
        self.assertIn('cannot be repaired', text)

    def test_json_pointer_escaping(self):
        self.assertEqual(different_paths({'a/b~c': 2}, {'a/b~c': 1}), ['/a~1b~0c'])

if __name__ == '__main__':
    unittest.main()
