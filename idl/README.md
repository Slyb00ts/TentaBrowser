# `idl/`: WebIDL sources and the interface manifest

This directory holds the WebIDL surface TentaBrowser implements and the
manifest that maps every interface to the phase that ships it, its owning
crate and its WPT directory (`docs/spec/05-javascript.md` §2 and Appendix A).

```
idl/
├── MANIFEST.toml     generated: one entry per interface seen on a golden site
├── phase-rules.toml  hand-maintained: interface name pattern -> phase, crate, spec, WPT dir
├── gen_manifest.py   generator (Python 3.11+, standard library only)
├── toy/              the toy interface used by the tenta-idl generator (P0)
├── <spec>/           WebIDL files copied from the specs, added per work package, then read
│                     by `crates/tenta-js/build.rs` (present today: `dom/`, `url/`, `fetch/`,
│                     `encoding/`, `xhr/`, `fileapi/`)
```

## Workflow: inventories → rules → manifest

1. **Inventories.** `golden/tracer` drives each golden site (DuckDuckGo,
   Google, YouTube, Teams) in headless Chromium with a `Proxy`-based tracer
   injected before any page script and records every interface member the
   site touches, plus CSS, network, worker, Intl and WebAssembly usage. The
   results are committed as `golden/inventory/<site>.json` and re-captured
   quarterly (`docs/dev/golden.md`).
2. **Rules.** `phase-rules.toml` is the human judgement: ordered `[[rule]]`
   entries whose anchored regular expression matches interface names and
   assigns `phase`, `crate`, `spec` (the WebIDL source spec, which becomes
   the directory `idl/<spec>/`) and `wpt` (the WPT directory the runner
   executes for that family). `phase = "unscheduled"` marks families the
   roadmap does not ship; `kind = "ecmascript"` marks intrinsics that have no
   WebIDL. `[[member_rule]]` entries name members implemented later or
   elsewhere than their interface (layout geometry on `Element`, canvas on
   `HTMLCanvasElement`, ...), so a phase can ship an interface with stubs.
3. **Manifest.** `python3 idl/gen_manifest.py` merges the inventories, applies
   the rules and writes `MANIFEST.toml`. Entries carry `idl_file`
   (`idl/<spec>/<Interface>.webidl`), `phase`, `crate`, `wpt_dir`, `seen_in`,
   `members_seen`, `accesses` (a priority signal), `realms` (window / worker /
   service_worker, informing `[Exposed]`), `top_members` and
   `deferred_members`. Interfaces that match no rule, or whose family is
   unscheduled, land in `[[unassigned]]` for a human to place by editing the
   rules. `python3 idl/gen_manifest.py --check` fails when the committed
   manifest is stale and is meant to run from `cargo xtask` in CI (the crate
   map lists IDL manifest checks among xtask's duties).

Agents plan IDL coverage from `MANIFEST.toml`, not from guesses
(`docs/spec/15-testing-and-benchmarks.md` §7). When an interface is missing
from the manifest because no golden site touched it but a work package needs
it (a WPT directory in a phase gate, a client conformance test), add a rule
and, if it is not in any inventory, add it to the inventory by extending a
golden scenario rather than by hand-editing the manifest.

## What is deliberately not here yet

The WebIDL files themselves are copied from the specs in P1/P2 by the work
packages that implement each family (`16-roadmap.md` P2 (c) "IDL coverage
per spec family from the manifest"), trimmed to the members we implement.
P0 only decides where each file will live and which phase owns it.
