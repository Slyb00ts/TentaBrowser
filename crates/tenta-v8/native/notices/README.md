# Native notice inventory

This directory contains 91 exact upstream notice files and a disclosed
optional/build-tool superset. The unchanged `host-component-map.json` records
46 components observed in the native19 macOS arm64 GN graph and explicitly names
that earlier archive. `incremental-native23.json` relates those notices to the
new archive: the measured rebuild changed only the binding C++ object and
archive, using an already attributed V8 header. The native dependencies,
configuration, generated snapshot and Temporal ABI versions did not change.
This is an incremental applicability record, not a new complete inventory.

`incremental-native30.json` relates the same observed component map to the
callback-only termination increment. It rebuilt only the existing binding
translation unit and archive, using already attributed V8 internal headers.
The GN graph, generated snapshot, Temporal ABI and third-party notice texts
remain unchanged. This is not a fresh full-component audit.

`NOTICES.md` indexes the 64 covered components (46 observed and 18 retained as a
superset). The three `remote-*-inventory.json` files record exact Git identities
for V8 15.2.124.1 and its dependency revisions. The collection scripts check the
notice bytes against those identities and record SHA-256 hashes. The original
`map_host_components.py` reproduces the base native19 mapping; it does not claim
to inspect the native23/native30 artifact or a different configuration.

The default graph contains no PartitionAlloc target; pointer compression and
shared cage are disabled. Future configurations must recheck this. LLVM libc,
the embedded UTF-8 decoder, rapidhash and SipHash have explicit notices.

Retain the texts, original copyright headers, and binding MIT/local Apache
licenses with redistributed artifacts. Neither notices nor the focused 30-test
native result establish other target support, complete WebIDL behavior, or every
distribution-specific obligation.
