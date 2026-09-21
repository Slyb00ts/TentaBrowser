# Running Web Platform Tests

The runner lives in `wpt/` (`wpt/README.md` lists the files). It reports
subtest pass percentages per directory, which is what the roadmap gates
measure (`docs/spec/16-roadmap.md`, `docs/spec/15-testing-and-benchmarks.md`
§6).

## Prerequisites

- Python 3.11+ (`tomllib`), `git`, and `openssl` on `PATH` (used by
  `./wpt serve` for its certificates).
- Node 20+ for the Chromium driver: `cd wpt/tools && npm install &&
  npx playwright install chromium`. Dependencies stay under
  `wpt/tools/node_modules` (gitignored).
- Disk: the shallow checkout is about 1.1 GB.

Host names. WPT tests address the server as `web-platform.test` and its
subdomains. Two options:

1. No hosts file changes (default for local runs): `run.py` detects that
   `web-platform.test` does not resolve to `127.0.0.1` and reports it in the
   plan. Chromium is given `--host-resolver-rules`, mapping
   `web-platform.test`, `*.web-platform.test`, `not-web-platform.test` and
   `*.not-web-platform.test` to loopback; the `tentabrowser` driver puts the
   same mapping in its own DNS cache. The server binds to `127.0.0.1`
   (`serve-config.json`, `server_host`), so no name resolution is needed on
   the server side.
2. Hosts file (CI, and handy for debugging a page by hand in the browser):
   add the lines printed by `python3 wpt/run.py serve` (or
   `./wpt make-hosts-file` inside the checkout) to `/etc/hosts`. With it,
   `run.py` reports `resolver_rules: false` and no product needs the mapping.

## Running

```bash
python3 wpt/run.py checkout                      # shallow clone at the pinned commit (once)
python3 wpt/run.py list --dir dom/nodes          # URLs and timeouts the manifest yields
python3 wpt/run.py run --product chromium --dir dom/nodes
python3 wpt/run.py run --product chromium --dir dom --dir html/dom --jobs 8 --max-minutes 30
python3 wpt/run.py serve                         # keep a server up for manual debugging
```

`run` performs, in order: checkout at the pinned commit; manifest generation
(`./wpt manifest`, about 40 s the first time, cached in
`wpt/checkout/MANIFEST.json`); `./wpt serve` with `wpt/serve-config.json`
(log at `wpt/results/<product>-serve.log`); enumeration of the testharness
tests below each directory from the manifest, including `.any.js`/`.window.js`
wrappers, `?variant` URLs and `timeout: long` metadata; the product driver;
comparison against `wpt/expectations/<product>/`; and
`wpt/results/<product>/<dir>.json` with the summary, the unexpected results
and every subtest. The exit status is 0 when every result matches a reviewed
expectation and 1 when one does not; 2 means the tool or its inputs are wrong
and 3 means the run is not comparable (see "Exit codes" below). Results are
judged by the same function `compare` uses, so a stored results file can be
re-judged later without a server.

`--max-minutes N` stops starting new tests after N minutes; tests not started
are recorded as `NOTRUN` and excluded from the pass count. Use `--channel
chrome` or `--executable PATH` to run a different Chromium build.

Timeouts: 10 s per test, 60 s for tests marked `timeout: long`
(`config.toml [runner]`). A `TIMEOUT` harness status means the completion
callback never fired; a page crash reports `CRASH`.

### How results are collected

Every test loads in a fresh page. The driver replaces
`/resources/testharnessreport.js` on the wire with a hook that calls
`add_completion_callback` and forwards `{status, message, tests[]}` to the
driver. The `tentabrowser` product injects the same hook through its own
transport: the context's interceptor answers that path with the hook script and
the script posts the payload back with `fetch()` (see below). The plan/result
JSON contract between `run.py` and a driver is documented on `Driver` in
`wpt/run.py`; `tools/driver-chromium.mjs` is the reference implementation.

## The `tentabrowser` product

`--product tentabrowser` runs a Rust binary in the crate this repository builds:
`tenta-wpt-driver` is a `[[bin]]` of `tenta-browser`, gated on that package's
`v8` feature. `run.py` writes `wpt/results/.plan.json`, runs
`tenta-wpt-driver <plan.json> <results.json>` and reads the result file back —
the same shape as the Chromium driver, and no `cargo run` in the loop.

```bash
cargo build --release -p tenta-browser --features v8 --bin tenta-wpt-driver
python3 wpt/run.py run --product tentabrowser --dir dom/nodes

# an explicit binary, or $TENTA_WPT_DRIVER; otherwise target/release, then target/debug
python3 wpt/run.py run --product tentabrowser --dir dom/nodes --driver target/debug/tenta-wpt-driver
```

The driver takes `--features v8` on the `tenta-browser` line only; it is the
only place JavaScript is needed in this repository.

What it does, per test:

- one engine for the whole plan, with one io thread and one page thread per
  worker, and one context **per worker** — each with `allow_private_network` and
  `ignore_https_errors`, because the WPT server is on loopback with test
  certificates. Each context has its own interceptor, resolver, cache and cookie
  jar; the interceptor answers three kinds of request:
  - `/resources/testharnessreport.js` (any path ending with it) is fulfilled
    with the completion hook — `setup({ output: false, timeout_multiplier })`
    plus an `add_completion_callback` that posts the harness status, the
    message and every subtest as JSON;
  - `/_tenta/wpt-result/<n>` is where that POST arrives: the interceptor reads
    the request body, hands it to the test that is waiting, and answers 200
    without touching the network;
  - everything else continues, after the WPT hosts
    (`web-platform.test`, `not-web-platform.test` and their subdomains) have
    been mapped to loopback in the process's DNS cache when the plan says
    `resolver_rules`. That mapping is refreshed on every request: a remembered
    answer's life is capped by the resolver (60 s), so a single seed at start-up
    would expire mid-run.
- a fresh page, navigated with `Settle::VisuallyStable` and a quiet window
  longer than the test's budget. `Settle::Load` cannot be used — it tears the
  engine down at `load`, and the harness completes strictly after `load` — and
  `Settle::NetworkIdle` can end before the hook's `fetch()` lands, because that
  fetch is issued from a timer callback. The wait is raced against the hook and
  the per-test deadline from `config.toml [runner]`; the navigation future is
  dropped when either wins, which cancels it, and the page is closed. A test
  whose hook never arrives is `TIMEOUT` with no subtests, and `run.py`'s exit
  status flags it as unexpected until an expectation says otherwise. That shape
  is also on the never-blessable list when the Chromium control for the same
  file reports subtests: it is a driver defect, not a finding, and blessing it
  would hide the next one.
- **the driver's deadline is a backstop, not the reporting path.** `testharness.js`
  runs its own timer over the same nominal budget, but it starts that timer when
  the harness script executes — after page creation, the document fetch, the
  parse up to the harness tag and the harness script's own fetch — while the
  driver starts its deadline before the page exists. Left alone, the harness's
  deadline is therefore always the later one and the driver's coarse `TIMEOUT`
  wins, losing every subtest the harness had already recorded. The interceptor
  answers the report script with the plan's *remaining* budget as
  `setup({ timeout_multiplier })`, which scales `Tests.prototype.timeout_length`
  before the harness re-arms its timer, so the harness fires at the driver's
  deadline and reports its own status and its whole subtest list. The multiplier
  is floored at `0.25`, because the same property scales in-page budgets
  (`step_timeout`, `step_wait_func`, an explicit `async_test` timeout) and an
  invented subtest `FAIL` would be worse than a coarse timeout; the driver
  warns when the floor binds, and the value should be re-derived from those
  lines. The driver's own arm is `budget + 250 ms`, the room the in-process
  round trip (hook delivery, then the result POST) needs, and it is only ever
  paid by a test whose harness never reported at all.
- one test at a time per worker; `jobs` tests run at once. A worker owns a
  context, and the context's interceptor answers the report-script path with a
  hook naming the index *that* worker is running — so attribution is right by
  construction, and a shared hub would make both hooks name the same test. The
  pool runs on the driver's own current-thread runtime through a `LocalSet`, so
  the workers need no `Send` bound. Results are written by index, so they always
  come back in plan order whatever order the workers finish in. The run deadline
  (`--max-minutes`) is checked when a worker claims a test, before its page
  exists: a test claimed after it is `NOTRUN`, and a test already started drains
  to its own hook or per-test deadline rather than being aborted.

Nothing ties a worker to a page thread: the pool hands threads out round-robin,
so two workers' pages share one as soon as they drift apart. That is deliberate
and it is why the isolate-entry discipline lives in the engine rather than in
this driver — the same collision is reachable from the public API by calling
`context.new_page()` twice, and a worker-to-thread pin would fix only the
driver's case.

The driver's diagnostics go to stderr at `WARN`; `TENTA_LOG=info` adds a line per
test with its status and subtest passes, which is the only progress signal a run
against a whole directory has. Results go to the file, never to stdout.
`TENTA_LOG=debug` also logs the harness timeout multiplier the interceptor
computed for each report script, and a `WARN` marks the runs where the `0.25`
floor bound. That warning rate is how the floor should be re-derived: a floor
that never binds can be raised toward 1, which makes the whole concern inert.

**Concurrency works.** Measured 2026-09-21 on `dom/nodes`, after the engine's
entry guard (`tenta-js-v8`'s `IsolateEntry`, above).

| `--jobs` | Subtests | Files OK | Wall time | Driver-deadline timeouts |
|----------|----------|----------|-----------|--------------------------|
| 1 | 2032/5247 (38.73 %) | 251/334 | 736 s | 70 |
| 4 | 2032/5247 (38.73 %) | 251/334 | 194 s | 70 |
| 4, after the two-clock fix | 2658/5981 (44.44 %) | 257/334 | 184 s | 18 |

The first two rows predate the two-clock fix, so the driver's deadline was what
reported those 70 files and the 38.73 % denominator is missing every subtest of
theirs. The third row's pass count is not the fix's doing either: the same tree
carried the DOM work that landed as `c65bbf8` (`document.implementation`,
`defaultView`), and `DOMImplementation-createDocument` (+174 subtests) and
`hasFeature` (+136) alone account for most of the gain. The fix's own measurable effect is the timeout
class — 70 → 18 files that lose everything to the driver's deadline, 77 → 41
files reporting no subtests at all, and 4 passes that used to be discarded with
their file. No file lost a pass. `--jobs 1` was not re-measured after the fix;
the recovered files mostly report `NOTRUN` and the harness's own "Test timed out"
subtest, because their tests really were still pending when the deadline passed —
what the fix buys is the record, not the pass.

At both job counts the same pass set came back and four jobs are 3.8x faster, so
the four-job run is the practical one. The `--jobs 1` figure recorded before the
guard landed was 738 s, so the guard's enter/exit pair per engine entry costs
nothing measurable (-0.3 %, inside the ±10 % noise floor of
`docs/spec/13-performance.md` §8).

`--jobs 4` used to abort the driver with `SIGABRT` after 163 s: a page thread
keeps one `PageRuntime` per page it hosts
(`crates/tenta-page/src/runtime.rs`), each owning a `v8::OwnedIsolate`, and page
threads are handed out by a **global** round-robin
(`crates/tenta-browser/src/threads.rs`), so two workers' pages land on one thread
as soon as the workers drift apart. An engine entry did not make its own isolate
the thread's current one, so the second engine's scopes were built on the first's
isolate, `ContextScope::new` panicked, and the `OwnedIsolate::drop` assertion
that followed during the unwind aborted the process. `tenta-js-v8`'s
`IsolateEntry` opens that window at every entry:

```rust
let _entry = self.entered();
```

and closes it on every exit, including the unwind path that `catch_unwind`
takes; `Engine::drop` enters its own isolate for teardown, because
`OwnedIsolate::drop` requires it to be current. `crates/tenta-js-v8/tests/isolate_entry.rs`
is the differential test for the guard, and
`crates/tenta-browser/tests/isolate_lifecycle.rs` for two live pages on one page
thread closed in both orders.

A directory that measures 0 % or comes back all-`TIMEOUT` is a driver problem
rather than a finding: check that the hook was requested at all in
`wpt/results/tentabrowser-serve.log`, and re-run a single small directory to see
whether the harness script itself fails to load.

## Adding a directory

1. Add the directory to the phase's `directories` list in `wpt/config.toml`
   (paths as in the WPT tree, for example `css/css-flexbox`).
2. Run it on `chromium` first to establish the control baseline and note the
   pass count in `wpt/README.md`. The control is not decoration: `promote`
   refuses a driver-attributed `TIMEOUT` with no subtests when the Chromium
   result for the same file reports subtests, so a directory with no control
   stored in `wpt/results/chromium/` can be neither blessed nor diagnosed.
3. Run it on `tentabrowser`, then generate a candidate and promote it
   ("Updating expectations" below). A directory whose run yields zero tests
   exits 3 rather than producing an empty candidate, which is how a typo in
   this list is caught.

Directories are only meaningful as testharness sets; reftests and crash tests
are not enumerated yet (the manifest has them, `list_tests` reads
`items.testharness` only).

## Updating expectations

Expectations live in `wpt/expectations/<product>/<wpt path>.ini`, one file per
test, in the format described in `wpt/expectations/README.md`. Missing files
mean "everything passes".

**Do not generate them straight into the live tree.** A mechanical
`expected: ERROR` on `dom/events/passive-by-default.html`, for instance, absorbs
20 subtests *and* a nameable interface-mapping defect ("20 duplicate test
names": our `constructor.name` returns `HTMLElement` for two different targets).
The live tree is a signed statement, so it is written only by an explicit
`promote`, from a candidate a human has read:

```bash
python3 wpt/run.py run --product tentabrowser --dir dom/nodes --jobs 4   # exit 0/1/3
python3 wpt/run.py candidate --product tentabrowser --dir dom/nodes     # writes a CANDIDATE only
less wpt/results/tentabrowser/candidate/dom/nodes.review.txt            # the review unit: the bucket table
python3 wpt/run.py promote --product tentabrowser --dir dom/nodes --by "your name"
python3 wpt/run.py compare --product tentabrowser --dir dom/nodes       # exit 0; no server needed
git diff wpt/expectations/                                              # the review of record
```

- `candidate` writes `wpt/results/<product>/candidate/<dir>/**/*.ini` plus a
  review sheet, a proposed bucket ledger and nothing else. `wpt/results/` is
  gitignored; `wpt/expectations/` is human-reviewed. That boundary is the whole
  point.
- **Regeneration is a merge, not an overwrite.** An entry whose status is
  unchanged re-emits verbatim with its bucket and reason; a new or changed entry
  emits `# reason: unreviewed`; an entry that no longer fails is dropped; a file
  with nothing left to expect gets no candidate at all, so `promote` deletes it.
  After the first blessing the diff is only what changed, which is what makes
  "review the diff" a real instruction instead of a 13 000-line chore.
- **`unreviewed` is judged exactly like a mismatch.** Generating an expectation
  tree never turns a run green on its own; the run stays red until a human
  writes a reason.
- **The review unit is the bucket, not the entry.** `wpt/expectations/<product>/BUCKETS.toml`
  holds one row per bucket (`key`, `label`, `issue`, `status`, `entries`) with
  `status` in `accepted | must-fix | forbidden`. `dom` alone is ~265 files and
  ~13 000 lines and the nine P2 directories project to 30 000-60 000 entries:
  that cannot be reviewed entry by entry. Read the ledger and the delta table,
  diff the tree. A bucket whose count **grew** or **shrank** is red, and
  re-blessing it means editing `entries` by hand — that edit is the review.
- **`promote` has four refusals and no override**: a non-comparable run, a
  never-blessable result shape (a `CRASH`, a harness `NOTRUN` from
  `run deadline reached`, a driver-attributed `TIMEOUT` with no subtests on a
  file the Chromium control reports subtests for, `N duplicate test names`
  without a reason and a `must-fix` row), a bucket that is `forbidden`, has no
  ledger row, or changed count, and any entry still carrying `unreviewed`. It
  exits 4 and writes nothing.
- Unexpected passes fail the run on purpose so expectations get tightened rather
  than left stale, and an `.ini` whose test no longer exists is a *stale
  expectation* — also unexpected. The full list is in
  `wpt/expectations/README.md`.

### Exit codes

`run` and `compare` share one judge, so they share its contract:

| code | meaning |
|------|---------|
| 0 | comparable run, every result matches a reviewed expectation |
| 1 | comparable run, results differ (regression, improvement, or an entry with no review) |
| 2 | tooling or usage error — no driver binary built, a bad ledger `status`, a port in use. Never a statement about the engine |
| 3 | **not comparable**: a truncated run, a baseline-stamp mismatch, or a configured directory that yielded zero tests |
| 4 | `promote` refused |

Exit 3 prints *which* stamp field moved next to the pass-percentage delta:
`wpt_commit`, `driver_sha256` or `plan_digest`, against
`wpt/expectations/<product>/BASELINE.toml`. `browser.version` is the crate
version, so it is constant across driver commits and cannot stamp a baseline;
`driver_sha256` is the sha256 of the `tenta-wpt-driver` binary, computed in
`run.py`. No driver JSON-contract change was needed for any of this.

A configured directory that yields no testharness tests is exit 3, not a
silent skip. That is deliberate: `wpt/config.toml`'s P2 list historically named
`cssom`, which is not a manifest root (the tests live at `css/cssom`), and a
skip let the P2 gate go green having measured 8 of its 9 directories.

## Changing the pinned commit

Edit `commit` in `wpt/config.toml`; the next `run.py` invocation fetches it
and regenerates the manifest. Re-run the baseline directories on `chromium`
and update the counts in `wpt/README.md` in the same commit, since test
counts change with upstream.
