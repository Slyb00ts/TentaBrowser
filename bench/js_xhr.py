#!/usr/bin/env python3
"""Measure real in-process S-js-xhr navigation samples.

Build: cargo build --release -p tenta-browser --features v8 --example js_xhr
Run: python3 bench/js_xhr.py --candidate target/release/examples/js_xhr
Add --baseline /path/to/preserved/js_xhr for interleaved before/after samples.
Process RSS includes the fixture server and browser; it is not a page RSS delta.
This driver does not measure the last-mutation-to-settle gap.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import sys
from pathlib import Path

from process_metrics import binary_info, command_output, file_sha256, run_json_process
from run import ROOT, machine_info, summarize

METRICS = ("navigation_ms", "engine_init_ms", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    expected = {"scenario": "S-js-xhr", "backend": "v8", "requests": 3,
                "verified": True, "settle": "networkidle", "rtt_ms": 0}
    for key, value in expected.items():
        if type(payload.get(key)) is not type(value) or payload[key] != value:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    for key in ("navigation_ns", "engine_init_ns"):
        if type(payload.get(key)) is not int or payload[key] <= 0:
            raise ValueError(f"invalid {key}: {payload.get(key)!r}")
    return {**payload, "navigation_ms": payload["navigation_ns"] / 1e6,
            "engine_init_ms": payload["engine_init_ns"] / 1e6}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--runs", type=int, default=25)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    subjects = {}
    if args.baseline:
        subjects["baseline"] = args.baseline.expanduser().resolve()
    subjects["candidate"] = args.candidate.expanduser().resolve()
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
    # Capture identity before execution and reject a concurrently replaced build.
    identities = {name: binary_info(path) for name, path in subjects.items()}
    started = dt.datetime.now(dt.timezone.utc)
    samples = {name: [] for name in subjects}
    for index in range(args.warmup + args.runs):
        for name, binary in subjects.items():
            sample = run_json_process(binary, args.timeout, validate_payload)
            if index >= args.warmup:
                samples[name].append(sample)
    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    summary = {name: {metric: summarize([sample[metric] for sample in values])
                      for metric in METRICS} for name, values in samples.items()}
    report = {
        "scenario": "S-js-xhr", "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(), "subjects": identities,
        "runs_per_subject": args.runs, "process_warmups_per_subject": args.warmup,
        "order": ",".join(subjects) + " repeated",
        "source": {
            "example_sha256": file_sha256(ROOT / "crates/tenta-browser/examples/js_xhr.rs"),
            "fixture_sha256": file_sha256(ROOT / "tests/fixtures/js-xhr/index.html"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
            "rustc": command_output("rustc", "--version"),
        },
        "measurement": {
            "navigation_ms": "Rust Instant around page.goto through return after settle/runtime teardown; includes runtime creation, excludes engine/context/page setup and correctness assertions",
            "engine_init_ms": "Rust Instant around Engine::build, reported separately",
            "wall_ms": "subject process duration including fixture server, setup, assertions and teardown",
            "cpu_ms": "wait4 user plus system CPU time for the entire subject process; null where unavailable",
            "peak_rss_bytes": "wait4 ru_maxrss of the entire subject process including fixture server; NOT page RSS delta",
            "limitations": "No last-mutation-to-settle gap or allocation counts; a single subject establishes a baseline, not a comparative performance claim; differences within +/-10% are below the noise floor",
        },
        "samples": samples, "summary": summary,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-js-xhr-inprocess.json")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    for name, metrics in summary.items():
        timing = metrics["navigation_ms"]
        rss = metrics["peak_rss_bytes"]
        memory = f"{rss['p50'] / 1048576:.1f}/{rss['p95'] / 1048576:.1f}" if rss else "n/a"
        print(f"{name}: navigation p50/p95 {timing['p50']:.2f}/{timing['p95']:.2f} ms; "
              f"process RSS p50/p95 {memory} MiB")
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-js-xhr failed: {exc}", file=sys.stderr)
        sys.exit(1)
