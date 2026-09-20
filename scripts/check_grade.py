#!/usr/bin/env python3
"""Fail when the analyzed code quality grade falls below the recorded floor.

Reads a codopsy report and prints what the score is made of, so a regression
says which files lost points rather than only that the number moved.
"""
import argparse
import json
import math

parser = argparse.ArgumentParser()
parser.add_argument('report')
parser.add_argument('--min-score', type=float, default=90.0)
parser.add_argument('--worst', type=int, default=8, help='how many files to list')
args = parser.parse_args()
with open(args.report) as source:
    report = json.load(source)

root = report['targetDir'].rstrip('/') + '/'
analyzed = [f for f in report['files'] if 'score' in f]
assert analyzed, 'the report contains no analyzed file'

weighted, weights, issues = 0.0, 0.0, 0
for entry in analyzed:
    weight = math.sqrt(len(entry['complexity']['functions']) + 1)
    weighted += entry['score']['score'] * weight
    weights += weight
    issues += len(entry['issues'])
mean = weighted / weights
penalty = min(math.sqrt(issues) * 0.8, 15)

print(f'weighted mean {mean:.2f} - penalty {penalty:.2f} ({issues} issues) '
      f"= {report['score']['overall']} ({report['score']['grade']})")
for entry in sorted(analyzed, key=lambda f: f['score']['score'])[:args.worst]:
    print(f"  {entry['score']['score']:3}  {entry['file'].replace(root, '')}  "
          f"{entry['score']['breakdown']}")

overall = report['score']['overall']
assert overall >= args.min_score, (
    f'code quality grade fell to {overall} ({report["score"]["grade"]}); '
    f'the floor is {args.min_score:g}. Raise the files listed above, or change '
    f'the floor deliberately in .github/workflows/ci.yml.')
print(f'PASS: code quality grade {overall} ({report["score"]["grade"]}) '
      f'is at or above the floor of {args.min_score:g}')
