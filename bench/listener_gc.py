#!/usr/bin/env python3
"""Measure real-engine listener collection before and after a runtime change.

    python3 bench/listener_gc.py --baseline /tmp/listener_gc_v8_before
    python3 bench/listener_gc.py --baseline /tmp/listener_gc_v8_before \
        --candidate target/release/examples/listener_gc

Each process runs the same five isolated V8 or QuickJS phases. The driver
records a baseline alone or 25 interleaved A/B pairs, with two unmeasured
process warm-ups per subject. Cycle retention is an observed count, not a
passing condition: the pre-collector build is expected to retain cycles.
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

NODES = 256
GC_ROUNDS = 3
PHASES = (
    "connected",
    "self_capture",
    "cross_capture",
    "reachable_detached",
    "noncapturing_control",
)


def checked_int(value: object, label: str, minimum: int = 0) -> int:
    if type(value) is not int or value < minimum:
        raise ValueError(f"invalid {label}: {value!r}")
    return value


def validate_payload(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("example output must be a JSON object")
    if payload.get("scenario") != "S-listener-gc":
        raise ValueError(f"unexpected scenario: {payload.get('scenario')!r}")
    if payload.get("backend") not in ("v8", "quickjs"):
        raise ValueError(f"unexpected backend: {payload.get('backend')!r}")
    if payload.get("nodes_per_phase") != NODES or payload.get("gc_rounds") != GC_ROUNDS:
        raise ValueError("example workload size or GC round count changed")
    phases = payload.get("phases")
    if not isinstance(phases, dict) or set(phases) != set(PHASES):
        raise ValueError("example omitted or added a listener GC phase")

    for name in PHASES:
        phase = phases[name]
        if not isinstance(phase, dict):
            raise ValueError(f"{name} must be an object")
        created = checked_int(phase.get("created_nodes"), f"{name}.created_nodes")
        expected_created = NODES + 1 if name == "connected" else NODES
        if created != expected_created:
            raise ValueError(f"{name} created {created} nodes, expected {expected_created}")
        before = checked_int(phase.get("before_nodes"), f"{name}.before_nodes", 1)
        after = checked_int(phase.get("after_nodes"), f"{name}.after_nodes", 1)
        freed = checked_int(phase.get("freed_nodes"), f"{name}.freed_nodes")
        retained = checked_int(phase.get("retained_nodes"), f"{name}.retained_nodes")
        if before != created + 1 or after + freed != before or retained != after - 1:
            raise ValueError(f"{name} native node accounting is inconsistent")
        if name in ("connected", "reachable_detached") and (freed or retained != created):
            raise ValueError(f"{name} lost nodes that must remain reachable")
        if name == "noncapturing_control" and (freed != created or retained):
            raise ValueError("noncapturing detached control was not fully reclaimed")

        round_ns = phase.get("round_ns")
        if not isinstance(round_ns, list) or len(round_ns) != GC_ROUNDS:
            raise ValueError(f"{name} must report {GC_ROUNDS} GC rounds")
        rounds = [checked_int(value, f"{name}.round_ns", 1) for value in round_ns]
        safe_point_ns = checked_int(phase.get("safe_point_ns"), f"{name}.safe_point_ns", 1)
        if safe_point_ns != sum(rounds):
            raise ValueError(f"{name} safe-point total does not equal its round timings")
        hits = phase.get("dispatch_hits")
        if name in ("connected", "reachable_detached"):
            if type(hits) is not int or hits != NODES:
                raise ValueError(f"{name} live listener dispatch failed: {hits!r}")
        elif hits is not None:
            raise ValueError(f"{name} unexpectedly reported dispatch hits")
    return payload


def summarize_subject(samples: list[dict]) -> dict:
    return {
        "wall_ms": summarize([sample["wall_ms"] for sample in samples]),
        "cpu_ms": summarize([sample["cpu_ms"] for sample in samples]),
        "peak_rss_bytes": summarize([sample["peak_rss_bytes"] for sample in samples]),
        "phases": {
            name: {
                "safe_point_ms": summarize(
                    [sample["phases"][name]["safe_point_ns"] / 1_000_000 for sample in samples]
                ),
                "freed_nodes": summarize(
                    [sample["phases"][name]["freed_nodes"] for sample in samples]
                ),
                "retained_nodes": summarize(
                    [sample["phases"][name]["retained_nodes"] for sample in samples]
                ),
            }
            for name in PHASES
        },
    }


def source_metadata(backend: str) -> dict:
    crate = "tenta-js-v8" if backend == "v8" else "tenta-js-qjs"
    adapter = ROOT / "crates" / crate / "examples" / "listener_gc.rs"
    shared_runner = ROOT / "bench" / "workloads" / "listener_gc.rs"
    javascript = ROOT / "bench" / "workloads" / "listener_gc.js"
    return {
        "adapter_path": str(adapter.relative_to(ROOT)),
        "adapter_sha256": file_sha256(adapter),
        "shared_runner_sha256": file_sha256(shared_runner),
        "javascript_sha256": file_sha256(javascript),
        "scope": "Hashes of current checkout files; binary hashes identify the actual measured subjects",
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure real-engine listener GC across preserved release examples."
    )
    parser.add_argument("--baseline", required=True, type=Path, help="preserved release example")
    parser.add_argument("--candidate", type=Path, help="post-change release example")
    parser.add_argument("--runs", type=int, default=25, help="measured samples per subject")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured samples per subject")
    parser.add_argument("--timeout", type=float, default=60, help="seconds per subject process")
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

    subjects = {"baseline": args.baseline.expanduser().resolve()}
    if args.candidate:
        subjects["candidate"] = args.candidate.expanduser().resolve()
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
    identities = {name: binary_info(path) for name, path in subjects.items()}

    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {name: [] for name in subjects}
    for index in range(args.warmup + args.runs):
        # Alternate A/B then B/A to reduce monotonic heating/order effects.
        order = list(subjects)
        if index % 2:
            order.reverse()
        for name in order:
            sample = run_json_process(subjects[name], args.timeout, validate_payload)
            if index >= args.warmup:
                samples[name].append(sample)

    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    backends = {name: {sample["backend"] for sample in values} for name, values in samples.items()}
    if any(len(values) != 1 for values in backends.values()):
        raise RuntimeError(f"a subject changed backend during measurement: {backends}")
    backend = next(iter(backends["baseline"]))
    if any(values != {backend} for values in backends.values()):
        raise RuntimeError("A/B comparison requires the same engine backend")
    if "candidate" in samples:
        for sample in samples["candidate"]:
            for name in ("self_capture", "cross_capture"):
                if sample["phases"][name]["freed_nodes"] != NODES:
                    raise RuntimeError(
                        f"candidate did not reclaim all {name} nodes; "
                        "a GC cost comparison without cycle collection is invalid"
                    )

    summaries = {name: summarize_subject(values) for name, values in samples.items()}
    deltas = None
    if "candidate" in summaries:
        baseline = summaries["baseline"]
        candidate = summaries["candidate"]
        deltas = {
            "wall_ms": 100 * (candidate["wall_ms"]["p50"] / baseline["wall_ms"]["p50"] - 1),
            "cpu_ms": (
                100 * (candidate["cpu_ms"]["p50"] / baseline["cpu_ms"]["p50"] - 1)
                if candidate["cpu_ms"] and baseline["cpu_ms"] else None
            ),
            "peak_rss_bytes": (
                100 * (candidate["peak_rss_bytes"]["p50"] /
                       baseline["peak_rss_bytes"]["p50"] - 1)
                if candidate["peak_rss_bytes"] and baseline["peak_rss_bytes"] else None
            ),
            "phases": {
                name: 100 * (
                    candidate["phases"][name]["safe_point_ms"]["p50"] /
                    baseline["phases"][name]["safe_point_ms"]["p50"] - 1
                )
                for name in PHASES
            },
        }

    report = {
        "scenario": "S-listener-gc",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "backend": backend,
            "baseline_source_sha256": args.baseline_source_sha256,
            "current_source": source_metadata(backend),
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "runs_per_subject": args.runs,
        "process_warmups_per_subject": args.warmup,
        "interleaved_order": "AB,BA alternating" if args.candidate else None,
        "nodes_per_phase": NODES,
        "gc_rounds": GC_ROUNDS,
        "phase_order": PHASES,
        "measurement": {
            "safe_point_ns": "Rust Instant around trim_memory, native-failure check, Document::sweep_detached, trim_memory; summed over three rounds per fresh engine/document. Script setup, microtask checkpoint, final poison check and dispatch verification are outside the timer",
            "automatic_collection": "Backends may also collect at task end or during allocation pressure. Collection during setup or the preceding microtask checkpoint is excluded from safe_point_ns, so smaller explicit-trim timings alone do not establish reduced total GC cost. Whole-process CPU and wall time include that work",
            "retained_nodes": "Document live nodes after safe points minus its document node; baseline self/cross retention is reported, while an A/B candidate must reclaim every cycle node",
            "wall_ms": "perf_counter_ns around the whole child process, including engine initialization, all phase setup, safe points and dispatch checks",
            "cpu_ms": "wait4 user plus system CPU time for the whole child process; null where wait4 is unavailable",
            "peak_rss_bytes": "wait4 ru_maxrss for the whole child process; null where wait4 is unavailable, not per-phase live heap",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); differences within +/-10% are below the documented noise floor",
        },
        "samples": samples,
        "summary": summaries,
        "delta_percent": deltas,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-listener-gc.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as result_file:
        json.dump(report, result_file, indent=2)
        result_file.write("\n")

    print(f"backend: {backend}; samples per subject: {args.runs}")
    print("subject  phase                  GC+sweep ms p50/p95  freed p50  retained p50")
    for subject, summary in summaries.items():
        for name in PHASES:
            phase = summary["phases"][name]
            cost = phase["safe_point_ms"]
            print(f"{subject:9s}{name:23s}{cost['p50']:8.2f}/{cost['p95']:<8.2f}"
                  f"{phase['freed_nodes']['p50']:11.0f}{phase['retained_nodes']['p50']:14.0f}")
        wall = summary["wall_ms"]
        cpu = summary["cpu_ms"]
        rss = summary["peak_rss_bytes"]
        cpu_text = f"{cpu['p50']:.2f}/{cpu['p95']:.2f}" if cpu else "n/a"
        rss_text = f"{rss['p50']/1048576:.1f}/{rss['p95']/1048576:.1f}" if rss else "n/a"
        print(f"{subject}: wall ms {wall['p50']:.2f}/{wall['p95']:.2f}; "
              f"CPU ms {cpu_text}; peak RSS MiB {rss_text}")
    if deltas is not None:
        print("candidate p50 GC+sweep change: " + ", ".join(
            f"{name} {deltas['phases'][name]:+.1f}%" for name in PHASES
        ))
    print(f"results: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError) as exc:
        print(f"S-listener-gc failed: {exc}", file=sys.stderr)
        sys.exit(1)
