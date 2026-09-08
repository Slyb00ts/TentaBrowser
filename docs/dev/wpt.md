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
   `web-platform.test` does not resolve to `127.0.0.1` and passes
   `--host-resolver-rules` to the browser, mapping `web-platform.test`,
   `*.web-platform.test`, `not-web-platform.test` and
   `*.not-web-platform.test` to loopback. The server binds to `127.0.0.1`
   (`serve-config.json`, `server_host`), so no name resolution is needed on
   the server side.
2. Hosts file (CI, and required for the `tentabrowser` product until it
   accepts resolver rules): add the lines printed by
   `python3 wpt/run.py serve` (or `./wpt make-hosts-file` inside the
   checkout) to `/etc/hosts`.

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
`add_completion_callback` and forwards `{status, message, tests[]}` through
an exposed binding. The `tentabrowser` product will inject the same hook
through its `Tenta.wptResult` binding instead of a network route; the
plan/result JSON contract between `run.py` and a driver is documented on
`Driver` in `wpt/run.py`, and `tools/driver-chromium.mjs` is the reference
implementation.

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
