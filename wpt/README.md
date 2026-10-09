# WPT runner

Runs Web Platform Tests directories against a product and reports subtest
pass percentages per directory (`docs/spec/15-testing-and-benchmarks.md` §6).
Phase gates are WPT thresholds per directory (`docs/spec/16-roadmap.md`);
`config.toml` lists the directories per phase.

| Path | Purpose |
|------|---------|
| `run.py` | runner: `checkout`, `list`, `serve`, `run`, `candidate`, `promote`, `compare` (stdlib only) |
| `config.toml` | pinned WPT commit, timeouts, directories per phase |
| `serve-config.json` | `./wpt serve` configuration (fixed ports, bound to 127.0.0.1) |
| `tools/driver-chromium.mjs` | `--product chromium` driver on Playwright's Chromium (`npm install` in `tools/`) |
| `tests/test_run.py` | non-vacuity tests for the judge and the round-trip (`python3 -m unittest discover -s wpt/tests`) |
| `expectations/<product>/*.ini` | known results, one file per test (`expectations/README.md`) |
| `expectations/<product>/BUCKETS.toml` | the bucket ledger — the unit a human reviews |
| `expectations/<product>/BASELINE.toml` | the stamp each directory was blessed at (commit, driver sha, plan digest) |
| `results/<product>/<dir>.json` | last run per directory (gitignored; gate numbers are recorded in this README) |
| `results/<product>/candidate/` | a generated, **unreviewed** candidate tree and its review sheet (gitignored) |
| `checkout/` | shallow WPT checkout at the pinned commit (gitignored) |

Quick start:

```bash
cd wpt/tools && npm install && npx playwright install chromium && cd ../..
python3 wpt/run.py run --product chromium --dir dom/nodes
```

See `docs/dev/wpt.md` for the full recipe, host-name setup, adding
directories and updating expectations.

## Exit codes

`run` and `compare` share one judge, so they share its contract. A pipeline
must branch on it, not on "did the command finish":

| code | meaning |
|------|---------|
| 0 | comparable run, every result matches a **reviewed** expectation |
| 1 | comparable run, results differ — a regression, an improvement, or an entry nobody has reviewed |
| 2 | tooling or usage error: no driver binary built, a bad ledger `status`, a port already in use. Never a statement about the engine |
| 3 | **not comparable**: a truncated run, a baseline-stamp mismatch, or a configured directory that yielded zero tests |
| 4 | `promote` refused; nothing was written |

Exit 3 is the one worth reading twice. It is not a quieter kind of red — it
says the number is not a measurement, and it names the stamp field that moved
(`wpt_commit`, `driver_sha256`, `plan_digest`). `--max-minutes N` and a
directory with no tests both land there.

## The expectations round-trip

`run` measures; it never writes a tracked file. Expectations change only
through a human-reviewed candidate:

```bash
python3 wpt/run.py run --product tentabrowser --dir dom/nodes --jobs 4   # 0/1/3
python3 wpt/run.py candidate --product tentabrowser --dir dom/nodes     # candidate only
less wpt/results/tentabrowser/candidate/dom/nodes.review.txt            # bucket table + delta
python3 wpt/run.py promote --product tentabrowser --dir dom/nodes --by "your name"
python3 wpt/run.py compare --product tentabrowser --dir dom/nodes       # re-judge, no server
```

A generated entry whose status is new or changed carries
`# reason: unreviewed`, and the judge treats that exactly like a mismatch, so
generating a tree never turns a run green on its own. `promote` refuses a
non-comparable run, a never-blessable shape (a `CRASH`, a deadline `NOTRUN`, a
driver `TIMEOUT` the Chromium control contradicts, `N duplicate test names`
without a reason and a `must-fix` ledger row), a bucket that is `forbidden`,
unledgered or changed in count, and any surviving `unreviewed`. It exits 4 and
writes nothing. `expectations/README.md` has the format, the bucket rules and
the never-blessable list.

## Products

- `chromium`: Playwright's pinned Chromium (P0). Serves as the control: a
  directory that Chromium fails is suspect before TentaBrowser is blamed.
- `tentabrowser`: the engine itself, driven by `tenta-wpt-driver` (a `[[bin]]`
  of the `tenta-browser` crate, built with `--features v8`). It replaces
  `resources/testharnessreport.js` on the wire with its own hook, and the hook
  posts the result back over `fetch()`. The driver contract both products
  implement is documented on the `Driver` class in `run.py`; this one is
  described in `docs/dev/wpt.md`.

  Measured on `dom/nodes` (2026-09-21). **Run this product at `--jobs 4`** (or
  higher) for the gate: 3.8x the throughput of one job for the same pass set.

  | Run | Subtests | Files OK | Wall time |
  |-----|----------|----------|-----------|
  | `--jobs 1` | 2032/5247 (38.73 %) | 251/334 | 736 s |
  | `--jobs 4` | 2032/5247 (38.73 %) | 251/334 | 194 s |
  | `--jobs 4`, after the two-clock fix | 2658/5981 (44.44 %) | 257/334 | 184 s |

  The first two rows predate the two-clock fix, so the driver's deadline was what
  reported 70 of their 73 timeout files and none of those files' subtests were
  ever counted. The third row is not the fix's doing: the same tree carried the
  DOM work that landed as `c65bbf8` (`document.implementation`, `defaultView`),
  and `DOMImplementation-createDocument` (+174 subtests) and `hasFeature` (+136)
  alone account for most of the gain. What the fix itself changed is the timeout
  class — files that lose everything to the driver's deadline went 70 → 18, files
  reporting no subtests at all 77 → 41, and 4 passes that used to be discarded
  with their file came back. No file lost a pass.

  Before P2's isolate entry guard landed, `--jobs 4` aborted the driver with
  `SIGABRT` after 163 s — an engine teardown panic on a shared page thread;
  `docs/dev/wpt.md` has the mechanism.

## QuickJS runs

Build the shared driver with `--features quickjs` or `--features v8,quickjs`,
then select `--product tentabrowser-qjs`:

```bash
cargo build --release -p tenta-browser --features v8,quickjs --bin tenta-wpt-driver
python3 wpt/run.py run --product tentabrowser-qjs --dir dom/nodes --jobs 4
```

`tentabrowser` explicitly selects V8; `tentabrowser-qjs` explicitly selects
QuickJS. A missing backend fails instead of falling back. Plans and result
JSON identify the backend, and the runner rejects a mismatched or older
unlabelled driver result. Stored reports are also checked before comparison,
candidate generation or promotion; historical unlabelled Tenta reports must
be rerun to provide backend evidence. Expectations, candidate ledgers and results live
under separate product directories, so V8 results cannot bless QuickJS.
The `wpt-driver` feature only gates the shared transport target; enabling it
without an engine does not provide a JavaScript runtime.

These switches enable measurement, not a conformance claim. Full P2 WPT
thresholds remain to be measured separately on both engines.

## Pinned WPT commit

`b89af32bc8f42d678f444eb0703bca015ddcf240` (`web-platform-tests/wpt` master,
2026-09-06). Change it in `config.toml`; `run.py` re-fetches and regenerates
the manifest on the next run.

## Baseline: `dom/nodes/` on Chromium

Recorded 2026-09-06 with the pinned commit, Playwright 1.63.0 Chromium
153.0.8010.12 (`chromium-1243`), macOS, `--jobs 4`:

| Directory | Files | Files OK | Subtests | Pass | Pass % | Wall time |
|-----------|-------|----------|----------|------|--------|-----------|
| `dom/nodes` | 334 | 332 | 15 110 | 15 044 | 99.56% | 23 s |

Two files time out (`moveBefore/fullscreen-preserve.html` needs a user
gesture; `insertion-removing-steps/change-event.window.html` calls
`test_driver.send_keys`, and the P0 driver provides no testdriver back end).
The 66 failing subtests are Chromium's own (mostly namespace-validation edge
cases in `createElementNS`/`createDocument` and one XML doctype comment case);
they are candidates for `expectations/chromium/` once the P2 gate is enforced.
