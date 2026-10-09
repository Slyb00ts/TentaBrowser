#!/usr/bin/env python3
"""Compare real-engine Web Storage calls using preserved release examples.

    python3 bench/storage_calls.py --comparison-kind backend \
        --baseline target/release/examples/storage_calls \
        --candidate target/release/examples/storage_calls_qjs

Use --comparison-kind revision for before/after builds of the same backend.
Each subject warms every operation for 1,000 calls and measures 100,000 calls.
The driver takes 25 alternating A/B, B/A process pairs by default.
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

KEYS = 128
CALLS = 100_000
WARMUP_CALLS = 1_000
OPERATIONS = ("get_item", "named_get", "set_item", "named_set", "own_keys", "symbol_create_delete")
METRICS = tuple(f"{operation}_ns_per_call" for operation in OPERATIONS) + (
    "wall_ms", "cpu_ms", "peak_rss_bytes"
)


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    if payload.get("scenario") != "S-storage-calls":
        raise ValueError(f"unexpected scenario: {payload.get('scenario')!r}")
    if payload.get("backend") not in ("v8", "quickjs"):
        raise ValueError(f"unexpected backend: {payload.get('backend')!r}")
    for key, expected in (("keys", KEYS), ("calls_per_operation", CALLS),
                          ("warmup_calls_per_operation", WARMUP_CALLS)):
        if type(payload.get(key)) is not int or payload[key] != expected:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    for operation in OPERATIONS:
        elapsed = payload.get(f"{operation}_elapsed_ns")
        mean = payload.get(f"{operation}_ns_per_call")
        if type(elapsed) is not int or elapsed <= 0:
            raise ValueError(f"invalid {operation}_elapsed_ns: {elapsed!r}")
        if type(mean) not in (int, float) or not math.isfinite(mean) or mean <= 0:
            raise ValueError(f"invalid {operation}_ns_per_call: {mean!r}")
        if not math.isclose(mean, elapsed / CALLS, rel_tol=1e-9, abs_tol=1e-9):
            raise ValueError(f"inconsistent {operation} elapsed time and mean")
    return payload


def source_metadata(backend: str) -> dict:
    crate, example = (("tenta-js-v8", "storage_calls.rs") if backend == "v8"
                      else ("tenta-js-qjs", "storage_calls_qjs.rs"))
    adapter = ROOT / "crates" / crate / "examples" / example
    shared_runner = ROOT / "bench" / "workloads" / "web_storage.rs"
    javascript = ROOT / "bench" / "workloads" / "web_storage.js"
    return {
        "adapter_path": str(adapter.relative_to(ROOT)),
        "adapter_sha256": file_sha256(adapter),
        "shared_runner_sha256": file_sha256(shared_runner),
        "javascript_sha256": file_sha256(javascript),
        "scope": "Current checkout source hashes; subject binary hashes identify measured builds",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--comparison-kind", required=True, choices=("backend", "revision"),
                        help="different engines or before/after builds of one engine")
    parser.add_argument("--baseline", required=True, type=Path, help="preserved release example")
    parser.add_argument("--candidate", required=True, type=Path, help="comparison release example")
    parser.add_argument("--runs", type=int, default=25, help="measured A/B pairs (minimum: 20)")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured A/B pairs")
    parser.add_argument("--timeout", type=float, default=120, help="seconds per subject process")
    parser.add_argument("--output", type=Path, help="JSON report path")
    parser.add_argument("--baseline-source-sha256", help="optional pre-build source snapshot hash")
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
    seen_backends: dict[str, str] = {}
    for index in range(args.warmup + args.runs):
        order = list(subjects)
        if index % 2:
            order.reverse()
        for name in order:
            sample = run_json_process(subjects[name], args.timeout, validate_payload)
            previous_backend = seen_backends.setdefault(name, sample["backend"])
            if sample["backend"] != previous_backend:
                raise RuntimeError(f"{name} changed engine backend during measurement")
            if index >= args.warmup:
                samples[name].append(sample)
        if index == 0:
            same_backend = seen_backends["baseline"] == seen_backends["candidate"]
            if args.comparison_kind == "revision" and not same_backend:
                raise RuntimeError("revision comparison requires the same engine backend")
            if args.comparison_kind == "backend" and same_backend:
                raise RuntimeError("backend comparison requires different engine backends")

    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    backends = {name: {sample["backend"] for sample in values} for name, values in samples.items()}
    if any(len(values) != 1 for values in backends.values()):
        raise RuntimeError(f"a subject changed backend during measurement: {backends}")
    backend_names = {name: next(iter(values)) for name, values in backends.items()}
    same_backend = backend_names["baseline"] == backend_names["candidate"]
    if args.comparison_kind == "revision" and not same_backend:
        raise RuntimeError("revision comparison requires the same engine backend")
    if args.comparison_kind == "backend" and same_backend:
        raise RuntimeError("backend comparison requires different engine backends")

    summary = {
        name: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
        for name, values in samples.items()
    }
    delta_percent = {
        metric: (100 * (summary["candidate"][metric]["p50"] /
                        summary["baseline"][metric]["p50"] - 1)
                 if summary["candidate"][metric] and summary["baseline"][metric]
                 and summary["baseline"][metric]["p50"] > 0 else None)
        for metric in METRICS
    }
    report = {
        "scenario": "S-storage-calls",
        "comparison_kind": args.comparison_kind,
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "current_source": {name: source_metadata(backend)
                               for name, backend in backend_names.items()},
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "backends": backend_names,
        "runs_per_subject": args.runs,
        "process_warmups_per_subject": args.warmup,
        "interleaved_order": "AB,BA alternating",
        "keys": KEYS,
        "calls_per_operation": CALLS,
        "javascript_warmup_calls_per_operation": WARMUP_CALLS,
        "measurement": {
            "operation_ns_per_call": "Rust Instant around Engine::evaluate of one warmed JavaScript function with 100,000 Storage accesses; includes invocation parsing, native-failure check and return conversion; elapsed / 100,000",
            "writes": "Set methods and named assignment alternate values on each complete 128-key sweep; final values are checked outside the timed region",
            "excluded_overhead": "runtime/context setup, shared JavaScript compilation, reset, 1,000-call warm-up and final-value/property-membership verification",
            "own_keys": "Reflect.ownKeys over 128 supported names and one ordinary Symbol; full key membership is checked outside timing, ordering is covered by separate correctness tests",
            "symbol_create_delete": "one ordinary Symbol property assignment and deletion per iteration; untimed controls verify its descriptor/value/deletion and absence after each measured batch",
            "wall_ms": "perf_counter_ns around the complete subject process, including startup, all resets, warm-ups and verifications",
            "cpu_ms": "wait4 user plus system CPU time for the complete subject process; null where unavailable",
            "peak_rss_bytes": "wait4 ru_maxrss for the complete subject process; null where unavailable, not live Storage heap size",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); backend comparisons are engine differences, not before/after regressions; differences within +/-10% are below the documented noise floor",
        },
        "samples": samples,
        "summary": summary,
        "delta_percent": delta_percent,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-storage-calls.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(f"comparison: {args.comparison_kind}; samples per subject: {args.runs}")
    print("subject/backend        operation      ns/call p50/p95")
    for name, metrics in summary.items():
        for operation in OPERATIONS:
            timing = metrics[f"{operation}_ns_per_call"]
            print(f"{name}/{backend_names[name]:9s} {operation:14s} "
                  f"{timing['p50']:10.2f}/{timing['p95']:.2f}")
        wall = metrics["wall_ms"]
        cpu = metrics["cpu_ms"]
        rss = metrics["peak_rss_bytes"]
        cpu_text = f"{cpu['p50']:.2f}/{cpu['p95']:.2f}" if cpu else "n/a"
        rss_text = f"{rss['p50']/1048576:.1f}/{rss['p95']/1048576:.1f}" if rss else "n/a"
        print(f"  process wall ms {wall['p50']:.2f}/{wall['p95']:.2f}; "
              f"CPU ms {cpu_text}; peak RSS MiB {rss_text}")
    print("candidate p50 change: " + ", ".join(
        f"{operation} {delta_percent[f'{operation}_ns_per_call']:+.1f}%"
        for operation in OPERATIONS
    ))
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-storage-calls failed: {exc}", file=sys.stderr)
        sys.exit(1)
