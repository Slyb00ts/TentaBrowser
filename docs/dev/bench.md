# Benchmark harness

`bench/run.py` drives the TentaBrowser CLI, the reference implementation and
headless Chromium through the scenarios in `bench/scenarios.toml`
(`docs/spec/15-testing-and-benchmarks.md` §2). Runs are interleaved
A/B/A/B on the same machine; the report gives p50/p95/max wall time, CPU time,
peak RSS and time to first request, as JSON in `bench/results/` (gitignored)
and a Markdown table on stdout. Reference baselines recorded at P0 live in
`bench/baselines/`.

## Prerequisites

- Python 3.11+ (standard library only; `psutil`, if installed, adds the peak
  RSS of the whole process tree, which matters for Chromium's helper
  processes).
- `cargo` — the harness builds `target/release/fixture-server` from
  `crates/tenta-testutil` unless `--no-build` is given.
- The reference binary at `~/repos/rust/obscura-reference/target/release/obscura`
  (`cargo build --release --features render` in that checkout) and Google
  Chrome at its macOS path; both paths are set in `[subjects]` of
  `bench/scenarios.toml`.
- The TentaBrowser CLI at `target/release/tentabrowser`
  (`cargo build -p tentabrowser-cli --release`). Scenarios the CLI cannot run
  yet (JavaScript, render) simply have no `commands.tentabrowser` entry in
  `scenarios.toml` and report `not-implemented` instead of failing.

## Running

```bash
python3 bench/run.py --list                                   # scenarios, phases, modes
python3 bench/run.py --scenario S-static-text-cli --compare reference
python3 bench/run.py --scenario S-js-xhr --scenario S-dynamic-scripts --subjects reference,chromium --runs 20
python3 bench/run.py --scenario all --subjects reference --runs 5 --label smoke
python3 bench/run.py --scenario S-static-text-cli --subjects reference --extra-args reference=--wait,0
python3 bench/run.py --scenario S-js-xhr --subjects reference --rtt 40 --server-args "--delay-path /api/*=100"
```

Options: `--runs N` (default 20 from `[defaults]`), `--warmup N` (discarded
runs, default 1), `--rtt MS` (overrides the scenario's simulated RTT),
`--timeout S`, `--extra-args SUBJECT=ARG,ARG` (append to one subject's
command), `--server-args` (passed to `fixture-server`), `--out-dir`,
`--label`, `--no-build`, `-v` (progress per round on stderr).

Every measured run is one cold process. Per scenario the harness starts one
`fixture-server` with the scenario's `--rtt` and `--log-requests`, then runs
`warmup + runs` rounds; each round runs every subject once, in order, so
drift affects all subjects equally. A run is a failure when the exit code is
non-zero, when stdout lacks `expect_stdout_contains`, or when the
`expect_output_file` is missing or empty; failed runs are counted and excluded
from the statistics, and the first failure's stderr tail is kept in the JSON.

## What is measured

| Field | Source |
|-------|--------|
| `wall_ms` | `time.perf_counter` around spawn → `wait4` |
| `cpu_ms` | `ru_utime + ru_stime` from `wait4` (the subject process plus children it waited for) |
| `peak_rss_bytes` | `ru_maxrss` from `wait4`: the subject process itself. This is the same kernel counter `/usr/bin/time -l` prints; for Chromium it covers only the browser process |
| `peak_rss_tree_bytes` | with `psutil`: RSS of the process tree sampled every 5 ms; `null` otherwise |
| `ttfr_ms` | time to first request: the fixture server logs `REQ <unix_ns> …` on arrival, the harness subtracts the subject's spawn timestamp on the same wall clock. This is the "process start to first byte on the wire" number of `S-cold-fetch` |
| `requests` | requests the fixture server saw during the run |

Percentiles are nearest-rank over the successful runs. Anything within the
±10% noise floor is not a difference (`docs/spec/13-performance.md` §8).
Check the load average before recording numbers you intend to publish; the
JSON records the machine but not competing work.

## Subject notes

- Reference: `fetch --dump text --allow-private-network --quiet` uses its
  default adaptive settle (up to 5 s, returns when quiescent). `S-cold-fetch`
  passes `--wait 0` because the tiny page has no pending work; use
  `--extra-args reference=--wait,0` to record the fixed-delay variant of other
  scenarios.
- Chromium: `--headless=new --user-data-dir=<tmp> --incognito … --dump-dom`.
  `--incognito` is required: with a private profile directory alone, Chrome
  144 prints the DOM and never exits. `--dump-dom` serialises at `load`, so
  JS scenarios add `--virtual-time-budget=1000` to let XHR/fetch and dynamic
  scripts settle first. Screenshots use `--window-size=W,H --screenshot=<file>`.

## Adding a scenario

1. Add the fixture under `tests/fixtures/<name>/` with a `README.md` and,
   where meaningful, `expected.txt`. Put a unique marker string in the settled
   DOM so the harness can validate output.
2. Append a `[[scenario]]` table to `bench/scenarios.toml` with `name`,
   `phase`, `fixture`, `path`, `mode`, `applicable_to`, `definition`,
   `rtt_ms`, `expect_stdout_contains` or `expect_output_file = true`, and a
   `[scenario.commands]` entry per subject that can run it (placeholders
   `{url}`, `{out}`, `{profile}`). Leave a subject out of `commands` to have
   it reported as `not-implemented`.
3. `mode = "cli"` scenarios run today. `inprocess` scenarios need the
   library harness; list them anyway so the table is complete.
   `fixture_root = "<dir>"` serves a different tree at `/` for that scenario;
   the search scenarios use it to serve `providers/fixtures/`.
4. Run it with `--runs 3` first, check the marker validation catches an
   empty or truncated output, then record with N ≥ 20.

## Search scenarios

`S-search-fast` and `S-search-cli-cold` replay a recorded SERP from the P0
provider survey. Two things make that work without a second harness:

- The scenario sets `fixture_root = "providers/fixtures"`, so its
  `fixture-server` serves the recorded SERPs, and `path` names one of them
  (`/duckduckgo_html/rust-browser-engine.html`).
- The CLI's hidden `--search-endpoint <plan_id>=<url>` points a parse plan at
  another endpoint. The plan still appends its own parameters
  (`?q=…&kp=-1`), which the file server ignores, so the request is exactly
  the one a real query would send and the response is exactly the recorded
  page.

```bash
python3 bench/run.py --scenario S-search-fast --subjects tentabrowser --runs 25
python3 bench/run.py --scenario S-search-cli-cold --subjects tentabrowser --runs 25
```

`S-search-fast` passes `--warm` (pre-connect before the query, which is what
"warm pool" means) and `--json`. The harness measures wall time; the number
R-06 budgets at 5 ms p50 is `engine_overhead_ms` in the trailing meta line of
the CLI's JSON output, which the harness does not parse. Read it directly:

```bash
./target/release/tentabrowser search "rust browser engine" --provider ddg --warm --json \
  --allow-private-network --search-endpoint duckduckgo_html=$URL | tail -1 |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["meta"]["timing"]["engine_overhead_ms"])'
```

## Profiling a cold run

`tentabrowser <command> --log debug` prints one `phases` line per run with the
wall time of each stage (`engine`, `navigate`, `extract`, `write`, `close`)
and the process total. It is the first thing to look at when `S-cold-fetch`
moves; `--log trace` adds the engine's own spans.

## Fixture server

`fixture-server [--root DIR] [--port N] [--rtt MS] [--delay-path GLOB=MS]... [--log-requests]`
serves `tests/fixtures/` on `127.0.0.1` and prints `LISTENING http://…` once
bound. The same server is available in-process from tests
(`tenta_testutil::FixtureServer`). Endpoints: `/api/echo`, `/api/slow?ms=N`,
`/api/never`, `/api/sse`, `/api/redirect`, `/api/cookie`,
`/api/status?code=N&retry_after=…`. RTT is applied as half before the
response head and half before the body.
