# Dependency review notes

## paste 1.0.15 through tenta-v8 152.2.0

Reviewed 2026-09-23, including an independent Sol review.

[`RUSTSEC-2024-0436`](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)
is an informational unmaintained-package advisory with no patched release.
The locked dependency is direct from local `tenta-v8 152.2.0`, which retains
the upstream macro implementation and dependency; the
[upstream manifest](https://github.com/denoland/rusty_v8/blob/v152.2.0/Cargo.toml)
requires `paste = "1.0"`. Upstream's current main manifest also retained this
dependency at review time.

The Rust binding uses the `paste!` procedural macro for identifier generation
in `src/typed_array.rs` and `src/cppgc.rs`. It runs during compilation, rather
than as a page runtime component. This limits its role, but does not remove
build-time supply-chain risk or constitute a full audit of the macro.

`deny.toml` contains an exception for this advisory ID only. It does not
suppress new vulnerability advisories, change the yanked-package policy,
or exempt V8 or other unmaintained packages generally. The maintenance risk
is accepted temporarily because upstream requires this package and no fixed
release exists. The local V8 fork was introduced separately for native WebIDL behavior and
structured-clone classification. It retains this macro unchanged; replacing it
requires its own compatibility review. No `[patch.crates-io]` override is used.

The advisory-ID exception applies to any occurrence of the affected package,
not only the V8 dependency route. Dependency-tree review must identify and
assess any new route; this review covers only the current V8 use. An unused
ignored advisory is denied, so removing the dependency requires removing the
stale exception as well.

Recheck upstream and the dependency tree on every V8 upgrade. Remove the
exception once the dependency disappears or rusty_v8 adopts and releases a
maintained replacement. A newly reported vulnerability requires a fresh assessment;
this note does not authorize ignoring it.

## Native QuickJS forks from rquickjs 0.12.2

Reviewed 2026-09-23, with an independent Sol source critique. The two renamed,
unpublished local packages preserve upstream licenses, exact archive checksums
and rquickjs revision in their `UPSTREAM.json` files. See the
[joint native patch record](../../crates/tenta-quickjs-core/TENTA-PATCH.md) for
the specific native hooks, ownership reproduction, sanitizer results and
remaining platform validation. The source patch keeps public QuickJS class
ABI structures unchanged and reuses rquickjs allocation/tracing machinery.

A native setter regression reproduced SIGSEGV with upstream's owning
assignment conversion and passed when the borrowed argument was duplicated;
C and Rust AddressSanitizer probes also passed on aarch64 macOS. This is a
focused ownership check, not a full audit of QuickJS or a browser conformance
claim. The original rquickjs dependency routes are banned, and dependency
layering resolves aliases to their real package names so the local route is
visible to checks. Recheck the native diff and package provenance on every
upgrade. Timezone hooks and other platform requirements remain outstanding.
The upstream GC trace/finalizer dispatch also needs a fail-closed panic
contract before the complete native-boundary containment gate can be claimed;
the current review covers operational hooks and the browser's own guarded
host destruction, not arbitrary upstream class tracing or destruction.

## Local V8 bindings and reviewed native artifacts

Integrated 2026-09-24 after independent Sol source and artifact-route review.
The renamed, unpublished `tenta-v8` package retains upstream Rust binding
licenses and exact source provenance. Native source reconstruction, dependency
notices and build records live in
[`crates/tenta-v8/native/`](../../crates/tenta-v8/native/BUILDING.md).
The build accepts only a matching, hash-verified macOS arm64 bundle; other
configurations fail closed. Registry `v8` is banned to prevent an accidental
return to an archive without the native hooks. The seven-package Temporal ABI
lock validator is a separate required check after dependency resolution.

The native 23-test suite and isolated 51-test artifact-route suite passed.
Those checks do not establish browser conformance, performance, or support on
other platforms. Browser integration and broader gates are recorded separately
in [implementation status](implementation-status.md). No archive is published
by this local integration.
