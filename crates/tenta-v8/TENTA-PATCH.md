# Local V8 bindings and native changes

This unpublished package derives its Rust bindings from rusty_v8 152.2.0
(MIT, Deno authors). `UPSTREAM.json` identifies the exact crate archive and VCS
metadata. Upstream marked the package tree dirty; the archive checksum is the
source identity, not an asserted clean Git equivalence.

Three additive Rust APIs match the native artifact:

- `PropertyHandlerFlags::WEB_IDL_LEGACY` selects legacy property creation results
  and key chronology. Generated WebIDL indexed, named-only numeric and named
  handlers must opt in. Unmarked objects retain upstream algorithms. A traced
  ephemeron table on each creation NativeContext records ordinary string order;
  capacity is reserved before mutation and successful creation commits without
  allocation. Native controls cover replacement, deletion, cloning, GC,
  cross-realm access and snapshots.
- `Value::is_structured_clone_ordinary_object()` reads native instance types
  without page property/prototype lookups. It accepts ordinary records and the
  pinned serializer's ordinary intrinsic prototypes, rejecting callable objects,
  proxies, API wrappers, arrays and specialized builtins. Those require separate
  clone behavior. The method alone does not implement structured cloning.
- `Isolate::terminate_execution_now_from_callback()` installs the uncatchable
  termination exception before an active slow API callback returns. A sticky
  hard host failure supersedes an ordinary pending page exception. This unsafe
  API may allocate and is restricted to the owning isolate thread's slow
  function/property callbacks; watchdog, Fast API, GC and heap-limit paths must
  keep their existing termination mechanisms.

The package contains no complete native engine checkout. Its build guard
requires a reviewed local archive, generated binding and provenance record.
Target, features and all three hashes must match; final linker bytes are checked
in a Cargo-configuration-private OUT_DIR. Each copy is staged, hashed and renamed
so repeated builds can replace old read-only private outputs. Failed copies stop
builds without changing the earlier accepted destination. Source, mirror and
upstream-download bypasses are rejected.

The helper-only macOS arm64 native30 build passed 30/30 tests with zero skips,
including ordinary-exception replacement and no subsequent JavaScript, catch or
finally execution. Its focused workspace structured-clone run passed 34/34;
the selected native/backend/browser integration run passed 203/203, with 1579
tests outside the filter. The native23 Storage Symbol latency
regression remains open and is not addressed by this callback helper.

The preceding native23 tests passed 23/23 with zero skips. The original
runner returned an error after native exit0 because its PASS parser did not
accept progress counters; the reviewed postprocessing receipt verifies the
unchanged log, source, artifacts and run timestamps without rerunning tests.
The separate binding-package route passed 51/51 (23 native and 28 build/guard
controls), including the read-only-copy regression, and all-target Clippy passed
with warnings denied. Browser tests, performance and other targets remain
separate gates. Native allocation-failure injection and full native panic
containment are not established by these focused tests.

`native/recipe` preserves the native23 base (25 changed files and 46 verified
missing source inputs) and layers the six-file native30 increment on it.
Source-only reconstruction matched all 35 recorded native30 inputs.
`native/BUILDING.md` describes the accepted
local artifact setup and the remaining source-build provisioning limits.
Temporal headers came from temporal_capi 0.2.3. Its Rust dependency is exact;
`native/validation/verify_temporal_lock.py` must also check all seven accepted
Temporal/Diplomat versions and checksums after workspace lock resolution.

Keep the MIT/Apache license files, original copyright headers and `native/notices`
with redistributed source/artifacts. The native23/native30 notice applicability
records reuse the unchanged host component inventory for each two-action increment;
other targets need their own review. No publication is part of this preparation.
