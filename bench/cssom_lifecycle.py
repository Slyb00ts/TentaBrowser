#!/usr/bin/env python3
"""Measure flat CSSOM before/after and candidate-only detached-rule churn.

    python3 bench/cssom_lifecycle.py \
        --baseline path/to/pre-nested/cssom_flat \
        --candidate path/to/post-nested/cssom_flat \
        --detached-candidate path/to/post-nested/cssom_detached

Each flat size alternates baseline/candidate order for 25 measured process
pairs. The detached workload has no pre-nested equivalent and is reported
without a before/after delta. In-process timings exclude parsing and warm-up;
process CPU and peak RSS include them.
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

FLAT_SIZES = (64, 256, 1024)
DETACHED_SIZES = (16, 64, 256)
HANDLE_OPERATIONS = 1_048_576
MUTATION_CYCLES = 16_384
DROPPED_HANDLES = 4096
FLAT_METRICS = (
    "handle_ns_per_operation", "mutation_ns_per_pair", "wall_ms", "cpu_ms", "peak_rss_bytes"
)
DETACHED_METRICS = (
    "mutation_ns_per_child", "drop_ns_per_handle", "wall_ms", "cpu_ms", "peak_rss_bytes"
)


def _positive_integer(payload: dict, key: str) -> int:
    value = payload.get(key)
    if type(value) is not int or value <= 0:
        raise ValueError(f"{key} must be a positive integer, got {value!r}")
    return value


def validate_flat(payload: object, size: int) -> dict:
    if not isinstance(payload, dict) or payload.get("scenario") != "S-cssom-flat":
        raise ValueError("expected an S-cssom-flat JSON object")
    for key, expected in (
        ("rules", size),
        ("handle_operations", HANDLE_OPERATIONS),
        ("mutation_cycles", MUTATION_CYCLES),
    ):
        if type(payload.get(key)) is not int or payload[key] != expected:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    handle_ns = _positive_integer(payload, "handle_elapsed_ns")
    mutation_ns = _positive_integer(payload, "mutation_elapsed_ns")
    return {
        **payload,
        "handle_ns_per_operation": handle_ns / HANDLE_OPERATIONS,
        "mutation_ns_per_pair": mutation_ns / MUTATION_CYCLES,
    }


def validate_detached(payload: object, size: int) -> dict:
    if not isinstance(payload, dict) or payload.get("scenario") != "S-cssom-detached":
        raise ValueError("expected an S-cssom-detached JSON object")
    for key, expected in (
        ("rules", size),
        ("cycles", DROPPED_HANDLES // size),
        ("dropped_handles", DROPPED_HANDLES),
    ):
        if type(payload.get(key)) is not int or payload[key] != expected:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    mutation_ns = _positive_integer(payload, "mutation_elapsed_ns")
    drop_ns = _positive_integer(payload, "drop_elapsed_ns")
    return {
        **payload,
        "mutation_ns_per_child": mutation_ns / DROPPED_HANDLES,
        "drop_ns_per_handle": drop_ns / DROPPED_HANDLES,
    }


def _summaries(samples: list[dict], metrics: tuple[str, ...]) -> dict:
    return {metric: summarize([sample[metric] for sample in samples]) for metric in metrics}


def _p50_change(baseline: dict | None, candidate: dict | None) -> float | None:
    if not baseline or not candidate or baseline["p50"] <= 0:
        return None
    return 100 * (candidate["p50"] / baseline["p50"] - 1)


def _source_metadata() -> dict:
    sources = {
        "driver": ROOT / "bench/cssom_lifecycle.py",
        "process_metrics": ROOT / "bench/process_metrics.py",
        "flat_example": ROOT / "crates/tenta-css/examples/cssom_flat.rs",
        "flat_workload": ROOT / "bench/workloads/cssom_flat.rs",
        "detached_example": ROOT / "crates/tenta-css/examples/cssom_detached.rs",
        "detached_workload": ROOT / "bench/workloads/cssom_detached.rs",
    }
    missing = [str(path.relative_to(ROOT)) for path in sources.values() if not path.is_file()]
    if missing:
        raise RuntimeError(f"CSSOM benchmark source is missing: {', '.join(missing)}")
    return {str(path.relative_to(ROOT)): file_sha256(path) for path in sources.values()}


def _metric_text(summary: dict | None, scale: float = 1) -> str:
    return (f"{summary['p50']/scale:.2f}/{summary['p95']/scale:.2f}"
            if summary else "n/a")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path, help="preserved pre-nested flat example")
    parser.add_argument("--candidate", required=True, type=Path, help="post-nested flat example")
    parser.add_argument("--detached-candidate", required=True, type=Path,
                        help="post-nested detached example")
    parser.add_argument("--runs", type=int, default=25, help="A/B pairs per size (minimum: 20)")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured pairs per size")
    parser.add_argument("--timeout", type=float, default=120, help="seconds per process")
    parser.add_argument("--output", type=Path, help="JSON report path")
    parser.add_argument("--baseline-source-sha256", help="pre-build source snapshot hash")
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    if args.baseline_source_sha256 and (
        len(args.baseline_source_sha256) != 64
        or any(ch not in "0123456789abcdef" for ch in args.baseline_source_sha256)
    ):
        parser.error("baseline source hash must be a lowercase SHA-256 digest")

    subjects = {
        "baseline": args.baseline.expanduser().resolve(),
        "candidate": args.candidate.expanduser().resolve(),
        "detached_candidate": args.detached_candidate.expanduser().resolve(),
    }
    for name, binary in subjects.items():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f"{name} must be executable: {binary}")
    identity_before = {name: binary_info(binary) for name, binary in subjects.items()}
    if identity_before["baseline"]["sha256"] == identity_before["candidate"]["sha256"]:
        parser.error("baseline and candidate flat examples must be distinct builds")
    source_metadata = _source_metadata()

    started = dt.datetime.now(dt.timezone.utc)
    flat: dict[str, dict[str, list[dict]]] = {}
    detached: dict[str, list[dict]] = {}
    for flat_size, detached_size in zip(FLAT_SIZES, DETACHED_SIZES, strict=True):
        key = str(flat_size)
        detached_key = str(detached_size)
        flat[key] = {"baseline": [], "candidate": []}
        detached[detached_key] = []
        for index in range(args.warmup + args.runs):
            order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
            for name in order:
                sample = run_json_process(
                    subjects[name], args.timeout,
                    lambda payload, size=flat_size: validate_flat(payload, size),
                    ("--size", key),
                )
                if index >= args.warmup:
                    flat[key][name].append(sample)
            sample = run_json_process(
                subjects["detached_candidate"], args.timeout,
                lambda payload, size=detached_size: validate_detached(payload, size),
                ("--size", detached_key),
            )
            if index >= args.warmup:
                detached[detached_key].append(sample)

    identity_after = {name: binary_info(binary) for name, binary in subjects.items()}
    for name in subjects:
        if identity_after[name]["sha256"] != identity_before[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")

    flat_summary = {
        size: {name: _summaries(samples, FLAT_METRICS)
               for name, samples in subjects_by_name.items()}
        for size, subjects_by_name in flat.items()
    }
    detached_summary = {
        size: _summaries(samples, DETACHED_METRICS) for size, samples in detached.items()
    }
    flat_delta = {
        size: {metric: _p50_change(summary["baseline"][metric], summary["candidate"][metric])
               for metric in FLAT_METRICS}
        for size, summary in flat_summary.items()
    }
    report = {
        "scenario": "S-cssom-lifecycle",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "current_source_sha256": source_metadata,
            "source_hash_scope": "Current checkout; subject binary hashes identify measured builds",
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identity_before,
        "flat_sizes": FLAT_SIZES,
        "detached_sizes": DETACHED_SIZES,
        "runs_per_size_per_subject": args.runs,
        "process_warmups_per_size_per_subject": args.warmup,
        "flat_interleaved_order": "AB,BA alternating",
        "measurement": {
            "flat_handle": "rule_at read lock, Arc clone and handle drop; the handle crosses black_box; 1048576 operations; Stylo parse and per-rule warm-up excluded",
            "flat_mutation": "16384 insert/delete pairs after 32 warm-up pairs; entire pair included",
            "detached_mutation": "4096 child inserts and deletes, including selector parsing and marker maintenance, divided across cycles of 16/64/256 live detached children",
            "detached_drop": "4096 deleted child-handle drops; each size has 4096/size batches; marker reclamation included",
            "wall_ms": "whole-process wall time including setup, parsing, warm-up and final verification",
            "cpu_ms": "whole-process user plus system CPU; null on platforms without wait4",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null on platforms without wait4, not a live Stylo heap measurement",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1), flat workload only; +/-10% is below the repo noise floor",
            "detached_baseline": "none: the nested mutation API does not exist in the pre-nested binary",
        },
        "flat_samples": flat,
        "detached_samples": detached,
        "flat_summary": flat_summary,
        "detached_summary": detached_summary,
        "flat_delta_percent": flat_delta,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-cssom-lifecycle.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(f"S-cssom-lifecycle: {args.runs} measured process runs per subject and size")
    print("flat size subject   handle ns/op p50/p95  mutation ns/pair p50/p95   CPU ms p50/p95    peak RSS MiB p50/p95")
    for size, by_name in flat_summary.items():
        for name, metrics in by_name.items():
            print(f"{size:>9} {name:9s} "
                  f"{_metric_text(metrics['handle_ns_per_operation']):>22s} "
                  f"{_metric_text(metrics['mutation_ns_per_pair']):>26s} "
                  f"{_metric_text(metrics['cpu_ms']):>16s} "
                  f"{_metric_text(metrics['peak_rss_bytes'], 1048576):>22s}")
    print("detached size  mutation ns/child p50/p95  drop ns/handle p50/p95   CPU ms p50/p95   peak RSS MiB p50/p95")
    for size, metrics in detached_summary.items():
        print(f"{size:>13s} "
              f"{_metric_text(metrics['mutation_ns_per_child']):>26s} "
              f"{_metric_text(metrics['drop_ns_per_handle']):>23s} "
              f"{_metric_text(metrics['cpu_ms']):>16s} "
              f"{_metric_text(metrics['peak_rss_bytes'], 1048576):>22s}")
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-cssom-lifecycle failed: {exc}", file=sys.stderr)
        sys.exit(1)
