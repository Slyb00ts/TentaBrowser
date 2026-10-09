#!/usr/bin/env python3
"""Compare two core-only QuickJS DOMString conversion builds.

Build and preserve the baseline example before applying the candidate string
change, then rebuild the same example from identical workload/manifest bytes.
The driver runs 25 interleaved A/B process pairs per case by default. Timed
loops include only String::to_string_lossy, checksum accumulation and drop;
whole-process CPU/RSS also include QuickJS setup and output verification.
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

CASES = {
    "ascii_names": (2_000_000, 8_500_000),
    "latin1": (1_000_000, 5_000_000),
    "paired_unicode": (1_000_000, 6_000_000),
    "lone_surrogate": (250_000, 1_250_000),
    "large": (10_000, 163_880_000),
}
METRICS = ("ns_per_operation", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate(payload: object, case: str) -> dict:
    operations, checksum = CASES[case]
    warmup = max(operations // 100, 64)
    if not isinstance(payload, dict) or payload.get("scenario") != "S-qjs-domstring-lossy":
        raise ValueError("expected S-qjs-domstring-lossy payload")
    for key, expected in (
        ("case", case),
        ("operations", operations),
        ("warmup_operations", warmup),
        ("checksum", checksum),
    ):
        if type(payload.get(key)) is not type(expected) or payload[key] != expected:
            raise ValueError(f"unexpected {key}: {payload.get(key)!r}")
    elapsed = payload.get("elapsed_ns")
    if type(elapsed) is not int or elapsed <= 0:
        raise ValueError(f"invalid elapsed_ns: {elapsed!r}")
    return {**payload, "ns_per_operation": elapsed / operations}


def source_hashes() -> dict[str, str]:
    paths = (
        ROOT / "bench/qjs_domstring_lossy.py",
        ROOT / "bench/process_metrics.py",
        ROOT / "bench/run.py",
        ROOT / "bench/workloads/qjs_domstring_lossy.rs",
        ROOT / "crates/tenta-quickjs-core/examples/qjs_domstring_lossy.rs",
        ROOT / "crates/tenta-quickjs-core/Cargo.toml",
        ROOT / "crates/tenta-quickjs-core/src/value/string.rs",
        ROOT / "Cargo.lock",
    )
    missing = [str(path.relative_to(ROOT)) for path in paths if not path.is_file()]
    if missing:
        raise RuntimeError(f"benchmark source is missing: {', '.join(missing)}")
    return {str(path.relative_to(ROOT)): file_sha256(path) for path in paths}


def summaries(samples: dict[str, list[dict]]) -> dict:
    return {
        name: {
            metric: summarize([sample[metric] for sample in values])
            for metric in METRICS
        }
        for name, values in samples.items()
    }


def p50_change(before: dict | None, after: dict | None) -> float | None:
    if before is None or after is None or before["p50"] <= 0:
        return None
    return 100 * (after["p50"] / before["p50"] - 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=25, help="measured A/B pairs per case, at least 20")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured process pairs per case")
    parser.add_argument("--timeout", type=float, default=30, help="seconds per subject process")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--baseline-source-sha256", help="baseline build source-manifest digest")
    parser.add_argument("--candidate-source-sha256", help="candidate build source-manifest digest")
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    for label in ("baseline_source_sha256", "candidate_source_sha256"):
        value = getattr(args, label)
        if value and (len(value) != 64 or any(char not in "0123456789abcdef" for char in value)):
            parser.error(f"{label} must be a lowercase SHA-256 digest")

    subjects = {
        "baseline": args.baseline.expanduser().resolve(),
        "candidate": args.candidate.expanduser().resolve(),
    }
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be executable: {path}")
    identities = {name: binary_info(path) for name, path in subjects.items()}
    if identities["baseline"]["sha256"] == identities["candidate"]["sha256"]:
        parser.error("baseline and candidate binaries have the same hash")
    sources_before = source_hashes()
    started = dt.datetime.now(dt.timezone.utc)
    by_case = {}
    for case in CASES:
        samples: dict[str, list[dict]] = {name: [] for name in subjects}
        for index in range(args.warmup + args.runs):
            order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
            for name in order:
                sample = run_json_process(
                    subjects[name], args.timeout,
                    lambda payload, selected=case: validate(payload, selected),
                    ("--case", case),
                )
                if index >= args.warmup:
                    samples[name].append(sample)
        summary = summaries(samples)
        by_case[case] = {
            "samples": samples,
            "summary": summary,
            "p50_change_percent": {
                metric: p50_change(summary["baseline"][metric], summary["candidate"][metric])
                for metric in METRICS
            },
        }

    for name, path in subjects.items():
        if binary_info(path) != identities[name]:
            raise RuntimeError(f"{name} binary changed during measurement")
    if source_hashes() != sources_before:
        raise RuntimeError("benchmark source changed during measurement")
    report = {
        "scenario": "S-qjs-domstring-lossy",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "candidate_source_sha256": args.candidate_source_sha256,
            "current_benchmark_source_sha256": sources_before,
            "source_scope": "Current checkout hashes; supplied build manifest hashes require separate preservation receipts",
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "cases": list(CASES),
        "runs_per_case_per_subject": args.runs,
        "process_warmups_per_case_per_subject": args.warmup,
        "interleaved_order": "AB,BA alternating within each case",
        "measurement": {
            "ns_per_operation": "Instant around warmed to_string_lossy conversions, checksum and string drop; setup/output checks excluded",
            "wall_ms": "whole subject process including QuickJS runtime/context and verification",
            "cpu_ms": "wait4 user plus system CPU; null where wait4 is unavailable",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null where wait4 is unavailable",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); +/-10% is below the repository noise floor",
            "baseline_workload": "same source is required by build procedure; binary hashes alone do not prove it",
        },
        "results": by_case,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-qjs-domstring-lossy.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("case             baseline ns/op p50/p95   candidate ns/op p50/p95   change p50    RSS MiB p50 baseline/candidate")
    for case, result in by_case.items():
        before = result["summary"]["baseline"]
        after = result["summary"]["candidate"]
        before_ns, after_ns = before["ns_per_operation"], after["ns_per_operation"]
        before_rss, after_rss = before["peak_rss_bytes"], after["peak_rss_bytes"]
        rss = (f"{before_rss['p50']/1048576:.1f}/{after_rss['p50']/1048576:.1f}"
               if before_rss and after_rss else "n/a")
        print(f"{case:17s} {before_ns['p50']:8.2f}/{before_ns['p95']:8.2f}"
              f"          {after_ns['p50']:8.2f}/{after_ns['p95']:8.2f}"
              f"      {result['p50_change_percent']['ns_per_operation']:+7.1f}%"
              f"       {rss}")
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as error:
        print(f"S-qjs-domstring-lossy failed: {error}", file=sys.stderr)
        sys.exit(1)
