#!/usr/bin/env python3
"""Complete the observed host build's native notice inputs from pinned trees."""
import concurrent.futures
import hashlib
import importlib.util
import json
from pathlib import Path
import urllib.request

root = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('collector', root / 'collect_notices.py')
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)
rows = [
    ('llvm-libc', 'https://chromium.googlesource.com/external/github.com/llvm/llvm-project/libc', '75ae50cd48c8a9d70f552ccff040029968778da9', ''),
    ('utf8-decoder', 'https://chromium.googlesource.com/v8/v8', 'refs/tags/15.2.124.1', 'third_party/utf8-decoder'),
    ('rapidhash-v8', 'https://chromium.googlesource.com/v8/v8', 'refs/tags/15.2.124.1', 'third_party/rapidhash-v8'),
    ('siphash', 'https://chromium.googlesource.com/v8/v8', 'refs/tags/15.2.124.1', 'third_party/siphash'),
]

def inventory(row):
    name, repo, revision, subdir = row
    source = f'{repo}/+/{revision}/{subdir}?format=JSON'
    with urllib.request.urlopen(source, timeout=30) as response:
        data = response.read(2 * 1024 * 1024)
    listing = json.loads(data.removeprefix(b")]}'\n"))
    notices = [entry for entry in listing['entries'] if entry['name'].upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))]
    return {'name':name, 'repository':repo, 'revision':revision, 'subdirectory':subdir, 'tree':listing['id'], 'notices':notices}

with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    inventory_rows = list(pool.map(inventory, rows))
(root / 'remote-completion-inventory.json').write_text(json.dumps(inventory_rows, indent=2) + '\n')
records = []
for row in inventory_rows:
    records.extend(collector.collect_remote(root, row))
manifest = json.loads((root / 'manifest.json').read_text())
old = {entry['path']: entry for entry in manifest['files']}
for record in records:
    old[record['path']] = record
manifest['files'] = sorted(old.values(), key=lambda item:item['path'])
(root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'additional_files':len(records), 'components':[(r['name'],[n['name'] for n in r['notices']]) for r in inventory_rows]}))
