#!/usr/bin/env python3
"""Compare two runs of the S-css-load example: load a page that links N stylesheets, then capture it.

    python3 bench/css_load.py --baseline /tmp/css-load-before \
        --candidate target/release/examples/css_load_bench

The subject is `crates/tenta-browser/examples/css_load_bench.rs`. It loads a page
with SHEETS (default 10) stylesheets of about 8 KiB each served after RTT_MS
(default 20) by the in-process fixture server, waits for `load`, takes one
viewport screenshot and prints the milliseconds of the load, the capture and
both. `--baseline-env NAME=VALUE` sets an environment variable for the baseline
subject only, which is how the pre-change behaviour (stylesheets fetched when
the capture lays the page out, not while the document loads) was reproduced from
one source tree. Runs are interleaved baseline, candidate, baseline, ...
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import sys
from pathlib import Path

from run import ROOT, machine_info, summarize
from process_metrics import binary_info, command_output, file_sha256, run_json_process

METRICS = ("load_ms", "screenshot_ms", "total_ms", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    for key in ("sheets", "rtt_ms", "load_ms", "screenshot_ms", "total_ms"):
        value = payload.get(key)
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            raise ValueError(f"invalid {key}: {value!r}")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--baseline-env", action="append", default=[], metavar="NAME=VALUE")
    parser.add_argument("--sheets", type=int, default=10)
    parser.add_argument("--rtt-ms", type=int, default=20)
    parser.add_argument("--runs", type=int, default=25, help="measured A/B pairs (default: 25)")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured process A/B pairs")
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 20:
        parser.error("runs must be at least 20")
    subjects = {"baseline": args.baseline.expanduser().resolve(),
                "candidate": args.candidate.expanduser().resolve()}
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
    baseline_env = dict(item.split("=", 1) for item in args.baseline_env)
    identities = {name: binary_info(path) for name, path in subjects.items()}
    os.environ["SHEETS"] = str(args.sheets)
    os.environ["RTT_MS"] = str(args.rtt_ms)

    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {name: [] for name in subjects}
    for index in range(args.warmup + args.runs):
        for name, binary in subjects.items():
            added = baseline_env if name == "baseline" else {}
            os.environ.update(added)
            try:
                result = run_json_process(binary, args.timeout, validate_payload)
            finally:
                for key in added:
                    os.environ.pop(key, None)
            if index >= args.warmup:
                samples[name].append(result)

    summaries = {name: {m: summarize([s[m] for s in values]) for m in METRICS}
                 for name, values in samples.items()}
    delta = {m: 100 * (summaries["candidate"][m]["p50"] / summaries["baseline"][m]["p50"] - 1)
             for m in METRICS if summaries["baseline"][m] and summaries["candidate"][m]}
    report = {
        "scenario": "S-css-load",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "sheets": args.sheets,
        "rtt_ms": args.rtt_ms,
        "baseline_env": baseline_env,
        "build": {
            "profile": "release",
            "example_source_sha256": file_sha256(
                ROOT / "crates" / "tenta-browser" / "examples" / "css_load_bench.rs"),
            "rustc": command_output("rustc", "--version"),
        },
        "subjects": identities,
        "runs_per_subject": args.runs,
        "interleaved_order": "baseline,candidate repeated",
        "summary": summaries,
        "delta_percent": delta,
        "samples": samples,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-css-load.json")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print("subject    load ms p50/p95    capture ms p50/p95   total ms p50/p95    RSS MiB p50")
    for name, s in summaries.items():
        print(f"{name:9s}  {s['load_ms']['p50']:7.1f}/{s['load_ms']['p95']:7.1f}"
              f"    {s['screenshot_ms']['p50']:7.1f}/{s['screenshot_ms']['p95']:7.1f}"
              f"     {s['total_ms']['p50']:7.1f}/{s['total_ms']['p95']:7.1f}"
              f"    {s['peak_rss_bytes']['p50'] / 1048576:.1f}")
    print("candidate p50 change: " + ", ".join(f"{m} {delta[m]:+.1f}%" for m in ("load_ms", "screenshot_ms", "total_ms", "peak_rss_bytes")))
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-css-load failed: {exc}", file=sys.stderr)
        sys.exit(1)
