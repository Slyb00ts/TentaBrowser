#!/usr/bin/env python3
"""Compare preserved release CSSOM wrapper-graph examples in alternating AB/BA pairs."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path

from process_metrics import binary_info, command_output, file_sha256, run_json_process
from run import ROOT, machine_info, summarize

SCENARIO = "S-cssom-wrapper-graph"
COUNTS = {
    "rules": 64, "read_warmup_calls": 128, "read_calls": 8192,
    "mutation_pairs": 256, "trim_rounds": 3,
    "read_checksum": 24576, "mutation_checksum": 1024,
}
METRICS = ("read_ms", "mutation_ms", "trim_ms", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object, backend: str) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("CSSOM example output must be an object")
    if payload.get("scenario") != SCENARIO or payload.get("backend") != backend:
        raise ValueError("CSSOM example reported a different scenario or backend")
    for name, expected in COUNTS.items():
        value = payload.get(name)
        if type(value) is not int or value != expected:
            raise ValueError(f"unexpected {name}: {value!r}")
    for name in ("read_elapsed_ns", "mutation_elapsed_ns", "trim_elapsed_ns"):
        value = payload.get(name)
        if type(value) is not int or value <= 0:
            raise ValueError(f"invalid {name}: {value!r}")
    for name in ("gc_epoch_before", "gc_epoch_after"):
        value = payload.get(name)
        if type(value) is not int or value < 0:
            raise ValueError(f"invalid {name}: {value!r}")
    if payload["gc_epoch_after"] - payload["gc_epoch_before"] < COUNTS["trim_rounds"]:
        raise ValueError("explicit trim did not advance the expected GC epochs")
    return {
        **payload,
        **{f"{phase}_ms": payload[f"{phase}_elapsed_ns"] / 1_000_000
           for phase in ("read", "mutation", "trim")},
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", required=True, choices=("v8", "quickjs"))
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=25)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--baseline-source-sha256")
    parser.add_argument("--output", type=Path)
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
    }
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be executable: {path}")
    identities = {name: binary_info(path) for name, path in subjects.items()}
    if identities["baseline"]["sha256"] == identities["candidate"]["sha256"]:
        parser.error("baseline and candidate must be distinct builds")
    crate = "tenta-js-v8" if args.backend == "v8" else "tenta-js-qjs"
    source_paths = (
        "bench/cssom_wrapper_graph.py", "bench/process_metrics.py",
        "bench/workloads/cssom_wrapper_graph.rs", "bench/workloads/cssom_wrapper_graph.js",
        f"crates/{crate}/examples/cssom_wrapper_graph.rs",
    )
    sources = {name: file_sha256(ROOT / name) for name in source_paths}
    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {name: [] for name in subjects}
    for index in range(args.warmup + args.runs):
        order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
        for name in order:
            sample = run_json_process(
                subjects[name], args.timeout,
                lambda payload: validate_payload(payload, args.backend),
            )
            if index >= args.warmup:
                samples[name].append(sample)
    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    if any(file_sha256(ROOT / name) != digest for name, digest in sources.items()):
        raise RuntimeError("benchmark workload or driver changed during measurement")
    summaries = {
        name: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
        for name, values in samples.items()
    }
    deltas = {
        metric: 100 * (summaries["candidate"][metric]["p50"] /
                       summaries["baseline"][metric]["p50"] - 1)
        if summaries["baseline"][metric] and summaries["candidate"][metric]
        and summaries["baseline"][metric]["p50"] > 0 else None
        for metric in METRICS
    }
    report = {
        "scenario": SCENARIO,
        "backend": args.backend,
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "current_source_sha256": sources,
            "source_hash_scope": "Current checkout; preserved binary identities identify measured subjects",
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "runs_per_subject": args.runs,
        "process_warmups_per_subject": args.warmup,
        "interleaved_order": "AB,BA alternating",
        "measurement": {
            "workload": COUNTS,
            "read_ms": "Rust Instant around one evaluated task performing 8192 existing style reads, including task-end collection; 128 warmup reads excluded",
            "mutation_ms": "Rust Instant around one evaluated task performing 256 insert/read/delete pairs, including task-end collection",
            "trim_ms": "Rust Instant around three explicit trim rounds and native-failure checks; prior microtask checkpoint excluded",
            "automatic_collection": "Backends may collect during setup, allocation pressure or task end. Explicit trim alone does not measure total collection cost",
            "wall_ms": "whole process including engine initialization, setup, warmup, operations, collection and checks",
            "cpu_ms": "whole-process user plus system CPU; null without wait4",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null without wait4",
            "scope": "Real-engine CSSOM wrapper cost for 64 top-level rules and transient top-level mutation; native values and checksums must pass for both subjects. This does not stress large detached nested components or native marker-scan scaling. Separate behavior tests establish expando preservation and directed reachability",
            "noise_floor": "Changes within +/-10% do not support a performance claim",
        },
        "samples": samples,
        "summary": summaries,
        "delta_percent": deltas,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + f"-{SCENARIO}-{args.backend}.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(output)
    for metric in METRICS:
        baseline, candidate = summaries["baseline"][metric], summaries["candidate"][metric]
        if baseline and candidate:
            print(f"{metric}: p50/p95 {baseline['p50']:.3f}/{baseline['p95']:.3f} -> "
                  f"{candidate['p50']:.3f}/{candidate['p95']:.3f} "
                  f"({deltas[metric]:+.1f}% p50)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
