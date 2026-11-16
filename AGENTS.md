# AGENTS.md

Guide for AI coding agents and contributors working in TentaBrowser. Read
this before doing anything else; then read the specification documents your
task touches.

## What this is

TentaBrowser is a headless browser engine written from scratch in Rust for
the TentaFlow agent platform. It is a library first (`tentabrowser` crate),
with a CLI, an MCP server and a CDP server as thin hosts. Hard requirements
include audio, video and WebRTC, five platforms (macOS, Linux, Windows,
Android, iOS), a fast `search` mode, and being measurably faster and lighter
than the reference implementation.

The specification lives in `docs/spec/` and is the source of truth:

- `docs/spec/00-index.md` lists every document and what it answers.
- `docs/spec/16-roadmap.md` says which phase we are in, its work packages
  and its exit gates. Do not start work from a later phase without an
  explicit ask.
- `docs/spec/18-decisions.md` records decisions; do not re-open one
  silently. Propose a new decision entry if you need to change one.
- `idl/MANIFEST.toml` (from P0) maps every web interface to its phase,
  owning crate and WPT directory; it is generated from the golden-site API
  inventories in `golden/inventory/`.

## Reference implementation

`../obscura-reference` (sibling directory, Apache-2.0) is the prior art we
studied. It is **read-only material**: never a dependency, never copied
without attribution, never a source of design by default. See
`docs/spec/17-obscura-lessons.md` for what we keep and what we change.

## Working rules

- Build the smallest coherent change that moves a roadmap gate.
- Every performance-relevant change comes with a benchmark result from
  `bench/` (interleaved runs, p50/p95, RSS, scenario name). Claims below the
  ±10% noise floor are not claims. See `docs/spec/13-performance.md` §8.
- Budgets in the spec are CI gates once the feature lands; targets (ⓣ) are
  reported. If you cannot meet a budget, say so in the PR with numbers; do
  not relax the budget silently.
- Tests exercise behaviour: fixture pages, WPT subsets, client conformance.
  No fixed sleeps; use virtual time or settle.
- Run tests with `cargo nextest`, not `cargo test`.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo xtask lint-deps`
  pass on every commit.
- `unsafe`, SIMD and assembly follow `docs/spec/13-performance.md` §5:
  scalar reference kept, differential test, runtime detection, benchmark in
  the commit.
- Every native boundary (V8, QuickJS, BoringSSL, codecs, FFI) is wrapped in
  `catch_unwind`; `panic = "unwind"` stays pinned in release profiles.
- Every host filesystem access goes through `FsPolicy`; every network
  access through `tenta-net` (`docs/spec/19-security.md`).
- Never add hostname-specific behaviour to layout, style or networking.
  Search provider parse plans are the one place site-specific data lives,
  and they are data files, not code.
- Language: code, comments, commits, docs in English. Commit messages:
  `type: concise description` (`feat`, `fix`, `perf`, `test`, `docs`,
  `build`, `ci`, `refactor`).

## Layout of the repository (target state)

```
crates/            see docs/spec/02-architecture.md §2 for the crate map (foundation / feature / assembly)
crates/tenta-h2/   published fork of h2 with the fingerprint patch, upstream diff kept minimal
idl/               WebIDL sources and MANIFEST.toml
profiles/          browser fingerprint profiles (TOML)
providers/         search provider parse plans (TOML) and fixtures
bench/             benchmark harness, scenarios, baselines, reference SPA
tests/fixtures/    offline fixture pages served by the in-process server
tests/fingerprint/ JA3/JA4/Akamai conformance
render-fixtures/   render corpus with paired captures and structural checks
wpt/               WPT runner, config, expectations
golden/            real-site scenarios and API inventories (nightly)
docs/spec/         the specification
docs/dev/          build, profiling, debugging recipes
xtask/             repository tooling
```

## Build and test

On a new workstation run `./setup.sh` first (`--check` only reports): it
installs the system build dependencies, the pinned Rust toolchain and the cargo
tools, and builds the native V8 bundle when the cache has none.

V8-enabled commands require a pinned local native bundle. Follow
[`docs/dev/v8-artifacts.md`](docs/dev/v8-artifacts.md), install the verified bundle
in a durable cache, and run
`eval "$(python3 crates/tenta-v8/native/install_bundle.py env)"` in the shell
before the Cargo commands below when they build V8: it sets
`TENTA_V8_ARTIFACT_DIR` and, for a bundle built on this machine,
`TENTA_V8_LOCAL_PIN`. `crates/tenta-v8/artifact_pins.rs` holds reviewed bundles
only; do not add a workstation build to it. The local binding
refuses missing/mismatched artifacts and unsupported configurations; do not use
an upstream prebuilt archive as a substitute. Run the documented Temporal lock
validator after resolving dependencies.

```bash
cargo build --release -p tentabrowser-cli   # target/release/tentabrowser
cargo nextest run --workspace               # never `cargo test`
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
cargo xtask lint-deps && cargo xtask idl-manifest && cargo deny check
python3 bench/run.py --scenario S-static-text-cli --compare reference --runs 25
```

Status (2026-09-23): P0 foundation and the P1 network, HTML, DOM and search
implementation are present in `tenta-net`, `tenta-html`, `tenta-dom`,
`tenta-page`, `tenta-browser`, `tenta-search` and the `tentabrowser` CLI
(`fetch`, `search`, `scrape`). The roadmap recorded the P0 exit locally;
revalidation of every P1 exit gate is separate from this inventory.

P2 is active. The current tree has V8 and QuickJS page paths, the shared
event loop and timer/provenance machinery, watchdogs, generated WebIDL
bindings, and partial fetch, XHR, forms, Blob/File and XML support. Stylo
backs inline styles, CSS.supports and the native stylesheet model. QuickJS
parity, workers/SAB, storage, streams/WebSocket, Web Crypto, full CSSOM,
history/location, iframes/CSP/IntersectionObserver, Google Full search and the P2
WPT/conformance/budget gates remain outstanding. See
`docs/dev/implementation-status.md` for precise package evidence and gaps.
P3–P8 work (layout, paint, hosts, media, WebRTC, mobile and hardening) has
not started beyond crate skeletons. Do not describe focused Range or
React/Vue smoke probes as full P2 or hydration conformance.

The CLI has opt-in `fetch --eval` through its `v8` feature, implemented by
atomic `Page::goto_with_eval` before navigation releases the runtime. It does
not provide a persistent evaluation session or await returned Promises.
`bench/dom_calls.py` measures real-engine `S-dom-calls` with interleaved
baseline/candidate executables; the general runner still does not implement
every in-process scenario.
