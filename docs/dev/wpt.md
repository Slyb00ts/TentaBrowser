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
and every subtest. The exit status is 1 when there are unexpected results.

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
  status flags it as unexpected until an expectation says otherwise.
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
   pass count in `wpt/README.md`.
3. Run it on `tentabrowser`, then record expectations (below).

Directories are only meaningful as testharness sets; reftests and crash tests
are not enumerated yet (the manifest has them, `list_tests` reads
`items.testharness` only).

## Updating expectations

Expectations live in `wpt/expectations/<product>/<wpt path>.ini`, one file per
test, in the format described in `wpt/expectations/README.md`. Missing files
mean "everything passes".

```bash
python3 wpt/run.py run --product tentabrowser --dir dom/nodes        # exit 1, lists unexpected results
python3 wpt/run.py update-expectations --product tentabrowser --dir dom/nodes
git diff wpt/expectations/                                           # review before committing
```

`update-expectations` rewrites the `.ini` files from the last results file:
it records every non-`OK` harness status and non-`PASS` subtest and deletes
the file for tests that now pass fully. Review the diff: an expectation that
disappears is an improvement, a new one is a regression that needs a reason
(`# reason:` comment with an issue link). Unexpected passes fail the run on
purpose so expectations get tightened rather than left stale.

## Changing the pinned commit

Edit `commit` in `wpt/config.toml`; the next `run.py` invocation fetches it
and regenerates the manifest. Re-run the baseline directories on `chromium`
and update the counts in `wpt/README.md` in the same commit, since test
counts change with upstream.
