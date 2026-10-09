#!/usr/bin/env python3
"""Refuse a Rust Temporal dependency set different from the native acceptance run."""
import argparse
import json
from pathlib import Path
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('--lockfile', type=Path, required=True)
parser.add_argument('--expected', type=Path, default=Path(__file__).resolve().parent.parent / 'temporal-abi-lock.json')
args = parser.parse_args()
actual = tomllib.loads(args.lockfile.read_text())['package']
expected = json.loads(args.expected.read_text())['packages']
for wanted in expected:
    matches = [package for package in actual if package['name'] == wanted['name']]
    if len(matches) != 1 or any(matches[0].get(key) != value for key, value in wanted.items()):
        raise SystemExit(f'Temporal native acceptance dependency mismatch: expected {wanted}, found {matches}')
print(json.dumps({'verified_packages': len(expected), 'lockfile': str(args.lockfile)}))
