#!/usr/bin/env python3
"""Compare real-engine structuredClone calls using preserved release examples.

Build and preserve both subjects before measurement. Use --comparison-kind
backend for V8 versus QuickJS, or revision for two builds of one backend.
No pre-feature structuredClone baseline exists; backend results are absolute
engine comparisons, not before/after speed claims.

Each subject checks every operation before and after its timed batch, warms
16 calls and times 256 calls. The driver takes 25 alternating AB/BA process
pairs by default (minimum 20), plus two unmeasured process pairs.
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

CALLS = 256
WARMUP_CALLS = 16
BUFFER_BYTES = 64 * 1024
GRAPH_SPARSE_LENGTH = 4097
CHECKSUM_PER_CALL = {
    "graph_clone": 171_828,
    "buffer_copy": 65_982,
    "buffer_transfer": 65_614,
}
OPERATIONS = tuple(CHECKSUM_PER_CALL)
METRICS = tuple(f"{operation}_ns_per_call" for operation in OPERATIONS) + (
    "wall_ms", "cpu_ms", "peak_rss_bytes"
)


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    if payload.get("scenario") != "S-structured-clone-timed":
        raise ValueError(f"unexpected scenario: {payload.get('scenario')!r}")
    if payload.get("backend") not in ("v8", "quickjs"):
        raise ValueError(f"unexpected backend: {payload.get('backend')!r}")
    for key, expected in (
        ("buffer_bytes", BUFFER_BYTES),
        ("graph_sparse_length", GRAPH_SPARSE_LENGTH),
        ("calls_per_operation", CALLS),
        ("warmup_calls_per_operation", WARMUP_CALLS),
    ):
        if type(payload.get(key)) is not int or payload[key] != expected:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    for operation, checksum_per_call in CHECKSUM_PER_CALL.items():
        checksum = payload.get(f"{operation}_checksum")
        elapsed = payload.get(f"{operation}_elapsed_ns")
        mean = payload.get(f"{operation}_ns_per_call")
        if type(checksum) is not int or checksum != checksum_per_call * CALLS:
            raise ValueError(f"invalid {operation}_checksum: {checksum!r}")
        if type(elapsed) is not int or elapsed <= 0:
            raise ValueError(f"invalid {operation}_elapsed_ns: {elapsed!r}")
        if type(mean) not in (int, float) or not math.isfinite(mean) or mean <= 0:
            raise ValueError(f"invalid {operation}_ns_per_call: {mean!r}")
        if not math.isclose(mean, elapsed / CALLS, rel_tol=1e-9, abs_tol=1e-9):
            raise ValueError(f"inconsistent {operation} elapsed time and mean")
    return payload


def current_source_metadata(backend: str) -> dict:
    crate = "tenta-js-v8" if backend == "v8" else "tenta-js-qjs"
    files = (
        f"crates/{crate}/examples/structured_clone_timed.rs",
        "bench/workloads/structured_clone_timed.rs",
        "bench/workloads/structured_clone_timed.js",
        "bench/structured_clone_timed.py",
        "bench/process_metrics.py",
        "bench/run.py",
    )
    return {
        "sha256": {path: file_sha256(ROOT / path) for path in files},
        "scope": "Current checkout benchmark sources only; these hashes do not prove the source of a preserved binary",
    }


def digest_argument(parser: argparse.ArgumentParser, value: str | None) -> None:
    if value is not None and (
        len(value) != 64 or any(ch not in "0123456789abcdef" for ch in value)
    ):
        parser.error("source manifest hash must be a lowercase SHA-256 digest")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--comparison-kind", required=True, choices=("backend", "revision"),
                        help="different engines or preserved builds of one engine")
    parser.add_argument("--baseline", required=True, type=Path, help="preserved release example")
    parser.add_argument("--candidate", required=True, type=Path, help="comparison release example")
    parser.add_argument("--runs", type=int, default=25, help="measured process pairs (minimum: 20)")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured process pairs")
    parser.add_argument("--timeout", type=float, default=120, help="seconds per subject process")
    parser.add_argument("--output", type=Path, help="JSON report path")
    parser.add_argument("--baseline-source-manifest-sha256",
                        help="optional user-recorded source manifest hash for the baseline build")
    parser.add_argument("--candidate-source-manifest-sha256",
                        help="optional user-recorded source manifest hash for the candidate build")
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    digest_argument(parser, args.baseline_source_manifest_sha256)
    digest_argument(parser, args.candidate_source_manifest_sha256)

    subjects = {name: path.expanduser().resolve() for name, path in
                (("baseline", args.baseline), ("candidate", args.candidate))}
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
    identities = {name: binary_info(path) for name, path in subjects.items()}
    if identities["baseline"]["sha256"] == identities["candidate"]["sha256"]:
        parser.error("baseline and candidate must be distinct preserved binaries")

    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {name: [] for name in subjects}
    seen_backends: dict[str, str] = {}
    for index in range(args.warmup + args.runs):
        order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
        for name in order:
            sample = run_json_process(subjects[name], args.timeout, validate_payload)
            previous = seen_backends.setdefault(name, sample["backend"])
            if sample["backend"] != previous:
                raise RuntimeError(f"{name} changed backend during measurement")
            if index >= args.warmup:
                samples[name].append(sample)
        if index == 0:
            same_backend = seen_backends["baseline"] == seen_backends["candidate"]
            if args.comparison_kind == "revision" and not same_backend:
                raise RuntimeError("revision comparison requires the same backend")
            if args.comparison_kind == "backend" and same_backend:
                raise RuntimeError("backend comparison requires different backends")

    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    backend_names = {name: seen_backends[name] for name in subjects}
    summary = {
        name: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
        for name, values in samples.items()
    }
    delta_percent = {
        metric: 100 * (summary["candidate"][metric]["p50"] /
                       summary["baseline"][metric]["p50"] - 1)
        if summary["candidate"][metric] and summary["baseline"][metric]
        and summary["baseline"][metric]["p50"] > 0 else None
        for metric in METRICS
    }
    report = {
        "scenario": "S-structured-clone-timed",
        "comparison_kind": args.comparison_kind,
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "caller-provided preserved binaries; release build required by protocol but not mechanically verified",
            "provided_source_manifest_sha256": {
                "baseline": args.baseline_source_manifest_sha256,
                "candidate": args.candidate_source_manifest_sha256,
            },
            "current_checkout_benchmark_source": {
                name: current_source_metadata(backend)
                for name, backend in backend_names.items()
            },
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
        "buffer_bytes": BUFFER_BYTES,
        "graph_sparse_length": GRAPH_SPARSE_LENGTH,
        "calls_per_operation": CALLS,
        "javascript_warmup_calls_per_operation": WARMUP_CALLS,
        "measurement": {
            "operation_ns_per_call": "Rust Instant around one Engine::evaluate of a warmed JavaScript batch of 256 calls; includes JavaScript loop and light result checksum, invocation parsing, native-failure check and return conversion; elapsed / 256",
            "graph_clone": "same prebuilt cyclic/shared graph with a length-4097 sparse array, ordered properties, a lone surrogate and a valid astral pair cloned on every iteration",
            "buffer_copy": "same prefilled 65536-byte ArrayBuffer cloned on every iteration; byte copy included",
            "buffer_transfer": "fresh 65536-byte ArrayBuffer allocation, four sentinel writes, transfer-list construction, clone and detachment on every iteration; allocation and zero-fill are included",
            "verification": "exact batch checksums plus full source/result identity, property-order, UTF-16 and byte checks before and after timing; verification scans excluded from operation timing",
            "excluded_overhead": "engine setup, shared JavaScript compilation, one preflight clone, 16 JavaScript warm-up calls and full checks before/after the timed batch",
            "wall_ms": "perf_counter_ns around the complete subject process, including setup, warm-up and verification",
            "cpu_ms": "wait4 user plus system CPU for the complete subject process; null where unavailable",
            "peak_rss_bytes": "wait4 ru_maxrss for the complete subject process; null where unavailable, not live clone heap size",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); backend comparisons are absolute engine differences, never a pre-feature speed claim",
            "noise_floor": "Differences within +/-10% do not support a performance claim",
            "source_provenance": "Subject binary SHA-256 hashes are checked before and after measurement; current-checkout source hashes and optional user-provided manifest hashes do not by themselves prove binary provenance",
        },
        "samples": samples,
        "summary": summary,
        "delta_percent": delta_percent,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-structured-clone-timed.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(f"comparison: {args.comparison_kind}; samples per subject: {args.runs}")
    print("subject/backend        operation          ns/call p50/p95")
    for name, metrics in summary.items():
        for operation in OPERATIONS:
            timing = metrics[f"{operation}_ns_per_call"]
            print(f"{name}/{backend_names[name]:9s} {operation:17s} "
                  f"{timing['p50']:10.2f}/{timing['p95']:.2f}")
        wall = metrics["wall_ms"]
        cpu = metrics["cpu_ms"]
        rss = metrics["peak_rss_bytes"]
        cpu_text = f"{cpu['p50']:.2f}/{cpu['p95']:.2f}" if cpu else "n/a"
        rss_text = f"{rss['p50']/1048576:.1f}/{rss['p95']/1048576:.1f}" if rss else "n/a"
        print(f"  process wall ms {wall['p50']:.2f}/{wall['p95']:.2f}; "
              f"CPU ms {cpu_text}; peak RSS MiB {rss_text}")
    print("candidate/baseline p50 difference: " + ", ".join(
        f"{operation} {delta_percent[f'{operation}_ns_per_call']:+.1f}%"
        for operation in OPERATIONS
    ))
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-structured-clone-timed failed: {exc}", file=sys.stderr)
        sys.exit(1)
