#!/usr/bin/env python3
"""Map the preserved GN host archive graph to collected upstream notices.

This is an attribution inventory, not a claim to legal certification. It keeps
optional/build-only notices as a disclosed superset rather than deleting them.
"""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--gn-project', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parent
manifest = json.loads((root / 'manifest.json').read_text())
notices = {}
for entry in manifest['files']:
    file = root / entry['path']
    assert file.stat().st_size == entry['bytes'] and hashlib.sha256(file.read_bytes()).hexdigest() == entry['sha256'], file
    notices.setdefault(entry['component'], []).append(entry['path'])
project = json.loads(args.gn_project.read_text())
targets = project['targets']
stack, seen, rows = ['//:rusty_v8'], set(), []
while stack:
    label = stack.pop()
    if label in seen:
        continue
    seen.add(label)
    target = targets[label]
    if '(' in label or target.get('type') in ('action', 'action_foreach', 'copy', 'executable', 'rust_proc_macro'):
        continue
    stack.extend(target.get('deps', []))
    rows.append((label, target))
components = {}

def component(path):
    if 'third_party/rust/chromium_crates_io/vendor/' in path:
        return 'rust-vendor/' + path.split('third_party/rust/chromium_crates_io/vendor/', 1)[1].split('/')[0]
    if 'third_party/rust-toolchain/' in path:
        return 'rust-stdlib'
    prefixes = {
        'third_party/abseil-cpp': 'abseil', 'third_party/dragonbox':'dragonbox',
        'third_party/fast_float':'fast_float', 'third_party/fp16':'fp16',
        'third_party/highway':'highway', 'third_party/icu':'icu',
        'third_party/libc++abi':'libcxxabi', 'third_party/libc++':'libcxx',
        'third_party/llvm-libc':'llvm-libc', 'third_party/simdutf':'simdutf',
        'v8/third_party/inspector_protocol':'inspector_protocol',
        'v8/third_party/utf8-decoder':'utf8-decoder', 'v8/third_party/rapidhash-v8':'rapidhash-v8',
        'v8/third_party/siphash':'siphash',
    }
    for prefix, name in prefixes.items():
        if prefix in path:
            return name
    return None

for label, target in rows:
    for path in [label] + target.get('sources', []) + target.get('include_dirs', []):
        name = component(path)
        if name:
            entry = components.setdefault(name, {'targets': set(), 'example_inputs': set()})
            entry['targets'].add(label)
            entry['example_inputs'].add(path)
for name, item in components.items():
    if name not in notices:
        raise SystemExit('Missing notice for host component ' + name)
    item['targets'] = sorted(item['targets'])
    item['example_inputs'] = sorted(item['example_inputs'])[:8]
    item['notices'] = notices[name]
record = {
    'scope': 'Default aarch64-apple-darwin native archive target; transitive non-tool targets, listed sources and header include paths. Not a certification of every binary obligation or another target.',
    'gn_project_sha256': hashlib.sha256(args.gn_project.read_bytes()).hexdigest(),
    'native_archive_sha256': '21f4d98158b2ac8695ad791bf8275e91dd1841b0423f946155c6ab19845c6f70',
    'components': dict(sorted(components.items())),
    'extra_notice_components': sorted(set(notices) - set(components)),
    'partition_alloc': 'No target in this preserved GN graph; pointer compression/shared cage is disabled in the accepted native configuration. Other configurations require their own notice audit.',
    'system_libraries': 'SDK 26.5 platform libraries are linked through the host SDK; no SDK/toolchain binary is included in the three-file artifact bundle.',
}
(root / 'host-component-map.json').write_text(json.dumps(record, indent=2) + '\n')
manifest['status'] = 'Observed default macOS arm64 host components mapped to pinned notices; includes a disclosed optional/build-tool superset; other target and redistribution review remain separate.'
manifest['remaining'] = ['Reaudit the native graph and notices for each additional target/configuration.', 'Include this notice directory and the binding MIT/local Apache licenses with any redistributed source/native bundle; review distribution-specific obligations before publishing.']
(root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
lines = ['# Native third-party notices', '',
'These exact upstream notice files accompany the default macOS arm64 native artifact. `manifest.json` records their source revisions, Git blob identities and SHA-256 values. `host-component-map.json` maps the preserved GN build inputs to notice components; optional and build-tool notices are retained as a disclosed superset.', '',
'The surrounding binding package also retains `LICENSE-MIT` for the Deno authors’ Rust bindings and `LICENSE-APACHE` for TentaBrowser changes. Original source-file copyright headers remain intact. This inventory is not a claim that every component is linked or that other targets are covered.', '',
'| Component | Notice files |', '| --- | --- |']
for name, paths in sorted(notices.items()):
    lines.append('| ' + name + ' | ' + ', '.join('[' + Path(p).name + '](' + p + ')' for p in paths) + ' |')
(root / 'NOTICES.md').write_text('\n'.join(lines) + '\n')
print(json.dumps({'observed_components':len(components), 'notice_components':len(notices), 'notice_files':len(manifest['files']), 'extra_components':len(record['extra_notice_components'])}))
