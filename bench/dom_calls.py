#!/usr/bin/env python3
"""Compare two release builds of the real-engine S-dom-calls example.

    python3 bench/dom_calls.py --baseline /tmp/dom-calls-before \
        --candidate target/release/examples/dom_calls

Each subject process warms both JavaScript functions for 10,000 calls, then
times one million calls to each DOM method. This driver also runs two full
process warm-ups per subject before 25 interleaved A/B sample pairs.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import os
import sys
from pathlib import Path

from run import ROOT, machine_info, summarize
from process_metrics import binary_info, command_output, file_sha256, run_json_process

CALLS = 1_000_000
WARMUP_CALLS = 10_000
METRICS = ("has_ns_per_call", "get_ns_per_call", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    if payload.get("scenario") != "S-dom-calls":
        raise ValueError(f"unexpected scenario: {payload.get('scenario')!r}")
    if payload.get("calls_per_operation") != CALLS:
        raise ValueError("example did not run one million measured calls per operation")
    if payload.get("warmup_calls_per_operation") != WARMUP_CALLS:
        raise ValueError("example did not run 10,000 warm-up calls per operation")
    if "backend" in payload and payload["backend"] not in ("v8", "quickjs"):
        raise ValueError(f"unexpected backend: {payload['backend']!r}")
    for operation in ("has", "get"):
        elapsed = payload.get(f"{operation}_elapsed_ns")
        mean = payload.get(f"{operation}_ns_per_call")
        if type(elapsed) is not int or elapsed <= 0:
            raise ValueError(f"invalid {operation}_elapsed_ns: {elapsed!r}")
        if type(mean) not in (int, float) or not math.isfinite(mean) or mean <= 0:
            raise ValueError(f"invalid {operation}_ns_per_call: {mean!r}")
        if not math.isclose(mean, elapsed / CALLS, rel_tol=1e-9, abs_tol=1e-9):
            raise ValueError(f"inconsistent {operation} elapsed time and mean")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path, help="pre-change release example")
    parser.add_argument("--candidate", required=True, type=Path, help="post-change release example")
    parser.add_argument("--runs", type=int, default=25, help="measured A/B pairs (default: 25)")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured process A/B pairs")
    parser.add_argument("--timeout", type=float, default=30, help="seconds per process")
    parser.add_argument("--output", type=Path, help="JSON report path")
    parser.add_argument("--baseline-source-sha256", help="source hash recorded before the baseline build")
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    if args.baseline_source_sha256 and (
        len(args.baseline_source_sha256) != 64
        or any(ch not in "0123456789abcdef" for ch in args.baseline_source_sha256)
    ):
        parser.error("baseline source hash must be a lowercase SHA-256 digest")

    subjects = {name: path.expanduser().resolve() for name, path in
                (("baseline", args.baseline), ("candidate", args.candidate))}
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")

    identities = {name: binary_info(path) for name, path in subjects.items()}

    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {name: [] for name in subjects}
    for index in range(args.warmup + args.runs):
        for name, binary in subjects.items():
            result = run_json_process(binary, args.timeout, validate_payload)
            if index >= args.warmup:
                samples[name].append(result)

    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    backends = {name: values[0].get("backend") for name, values in samples.items()}
    candidate_crate = "tenta-js-qjs" if backends["candidate"] == "quickjs" else "tenta-js-v8"
    workload = ROOT / "bench" / "workloads" / "dom_calls.js"

    summaries = {
        name: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
        for name, values in samples.items()
    }
    baseline = summaries["baseline"]
    candidate = summaries["candidate"]
    delta_percent = {
        metric: 100 * (candidate[metric]["p50"] / baseline[metric]["p50"] - 1)
        for metric in METRICS if baseline[metric] and candidate[metric]
    }
    report = {
        "scenario": "S-dom-calls",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "candidate_example_source_sha256": file_sha256(
                ROOT / "crates" / candidate_crate / "examples" / "dom_calls.rs"
            ),
            "candidate_workload_file_sha256": file_sha256(workload) if workload.is_file() else None,
            "candidate_assembled_workload_sha256": (
                hashlib.sha256(workload.read_bytes() + b"        ").hexdigest()
                if workload.is_file() else None
            ),
            "workload_assembly": "source-side hash of shared JavaScript file plus eight spaces preserving the original V8 raw literal; not extracted from the binary" if workload.is_file() else None,
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "backends": backends,
        "runs_per_subject": args.runs,
        "process_warmups_per_subject": args.warmup,
        "calls_per_operation": CALLS,
        "javascript_warmup_calls_per_operation": WARMUP_CALLS,
        "interleaved_order": "baseline,candidate repeated",
        "measurement": {
            "has_ns_per_call": "Rust Instant around Engine::evaluate of a warmed JavaScript function containing one million hasAttribute('id') calls; elapsed / one million",
            "get_ns_per_call": "same for getAttribute('id')",
            "included_overhead": "Engine::evaluate compiles the small invocation expression and converts the returned number to a Rust string inside the timer",
            "excluded_overhead": "engine runtime/context setup, JavaScript function-body compilation, and 10,000-call warm-up",
            "native_path": "Real generated native DOM binding; backend is reported by each subject when available (older preserved V8 baselines omit it). This harness does not independently verify V8 fast-API dispatch",
            "wall_ms": "time.perf_counter_ns around each subject process, including startup and both measured loops",
            "cpu_ms": "wait4 user plus system CPU time for each subject process (null where wait4 is unavailable)",
            "peak_rss_bytes": "ru_maxrss from wait4 for each subject process (null where wait4 is unavailable)",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); differences within +/-10% are below the documented noise floor",
        },
        "samples": samples,
        "summary": summaries,
        "delta_percent": delta_percent,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-dom-calls.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as result_file:
        json.dump(report, result_file, indent=2)
        result_file.write("\n")

    print("subject    has ns/call p50/p95  get ns/call p50/p95  wall ms p50/p95  RSS MiB p50/p95")
    for name, summary in summaries.items():
        has = summary["has_ns_per_call"]
        get = summary["get_ns_per_call"]
        wall = summary["wall_ms"]
        rss = summary["peak_rss_bytes"]
        rss_display = (f"{rss['p50'] / 1048576:.1f}/{rss['p95'] / 1048576:.1f}"
                       if rss else "n/a")
        print(f"{name:9s}  {has['p50']:7.2f}/{has['p95']:7.2f}"
              f"       {get['p50']:7.2f}/{get['p95']:7.2f}"
              f"       {wall['p50']:7.2f}/{wall['p95']:7.2f}"
              f"       {rss_display}")
    print("candidate p50 change: " + ", ".join(
        f"{metric} {delta_percent[metric]:+.1f}%" for metric in
        ("has_ns_per_call", "get_ns_per_call")
    ))
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-dom-calls failed: {exc}", file=sys.stderr)
        sys.exit(1)
