import unittest
from mean_fixture import check_python

class FixtureTests(unittest.TestCase):
    def test_original_bug(self):
        self.assertEqual(check_python('def mean(values):\n return sum(values) / len(values)'), {'passed': 4, 'total': 5})

    def test_valid_repairs(self):
        for source in ['def mean(values):\n return sum(values)/len(values) if values else 0.0',
                       'def mean(values):\n if not values:\n  return 0.0\n return sum(values)/len(values)',
                       'def mean(values):\n if len(values) == 0:\n  return 0.0\n else:\n  return sum(values)/len(values)']:
            with self.subTest(source=source):
                self.assertEqual(check_python(source), {'passed': 5, 'total': 5})

    def test_model_code_cannot_execute(self):
        for source in ['import os\ndef mean(values):\n return 0',
                       '@print("unexpected")\ndef mean(values):\n return 0',
                       'def mean(values=print("unexpected")):\n return 0',
                       'def mean(values: print("unexpected")):\n return 0',
                       'def mean(values):\n return __import__("os").system("false")',
                       'def mean(values):\n while True:\n  pass',
                       'def mean(values):\n return mean(values)',
                       'def mean(values):\n return "x" * 999999999999999999999999',
                       'def mean(values):\n return 2 ** 999999999999',
                       'def mean(values):\n return [0] * 999999999999',
                       'def mean(values):\n return values.__class__',
                       'def mean(values):\n return [v for v in values]']:
            with self.subTest(source=source):
                self.assertLess(check_python(source)['passed'], 5)

if __name__ == '__main__':
    unittest.main()
