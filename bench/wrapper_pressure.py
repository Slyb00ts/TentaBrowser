#!/usr/bin/env python3
"""Measure a release wrapper-pressure example in fresh engine processes.

This new scenario reports candidate-only timings and boundedness, without a
pre-graph speedup claim. Existing DOM-call and listener scenarios provide
paired before/after measurements on compatible workloads.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path

from process_metrics import binary_info, command_output, file_sha256, run_json_process
from run import ROOT, machine_info, summarize

SHAPES = {"detached": 12000, "owners": 6000, "connected": 5000, "deep": 1200}
METRICS = ("task_ms", "wall_ms", "cpu_ms", "peak_rss_bytes", "nodes_after_task", "gc_epochs")


def validate_payload(payload: object, shape: str, backend: str) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("pressure output must be a JSON object")
    for name, expected in (
        ("scenario", "S-wrapper-pressure"), ("shape", shape), ("backend", backend),
        ("warmup_policy", "fresh_engine_no_warmup"),
    ):
        if payload.get(name) != expected:
            raise ValueError(f"unexpected {name}: {payload.get(name)!r}")
    iterations = SHAPES[shape]
    checksum = iterations * (iterations - 1) // 2
    for name, expected in (
        ("iterations", iterations), ("checksum", checksum), ("expected_checksum", checksum),
        ("nodes_before_task", 1), ("nodes_after_release_sweep", 1),
    ):
        if type(payload.get(name)) is not int or payload[name] != expected:
            raise ValueError(f"unexpected {name}: {payload.get(name)!r}")
    for name, minimum in (
        ("elapsed_ns", 1), ("gc_epoch_before", 0), ("gc_epoch_after", 0), ("nodes_after_task", 2),
    ):
        if type(payload.get(name)) is not int or payload[name] < minimum:
            raise ValueError(f"invalid {name}: {payload.get(name)!r}")
    epochs = payload["gc_epoch_after"] - payload["gc_epoch_before"]
    if epochs <= 1:
        raise ValueError("pressure did not collect during the uninterrupted task")
    nodes = payload["nodes_after_task"]
    if shape in ("detached", "owners"):
        if nodes >= 4096:
            raise ValueError("unreachable native nodes exceeded the workload bound")
    elif nodes != iterations + 2:
        raise ValueError("connected native node count changed")
    return {**payload, "task_ms": payload["elapsed_ns"] / 1_000_000, "gc_epochs": epochs}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--backend", required=True, choices=("v8", "quickjs"))
    parser.add_argument("--runs", type=int, default=25)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 20 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20 and timeout finite and positive")
    binary = args.binary.expanduser().resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"binary must be executable: {binary}")
    identity = binary_info(binary)
    adapter = (
        "crates/tenta-js-qjs/examples/wrapper_pressure_qjs.rs" if args.backend == "quickjs"
        else "crates/tenta-js-v8/examples/wrapper_pressure.rs"
    )
    source_paths = (
        "bench/wrapper_pressure.py", "bench/process_metrics.py",
        "bench/workloads/wrapper_pressure.rs", "bench/workloads/wrapper_pressure.js", adapter,
    )
    sources = {name: file_sha256(ROOT / name) for name in source_paths}
    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, list[dict]] = {shape: [] for shape in SHAPES}
    # Rotate shape order across rounds to distribute unrelated machine drift.
    shapes = list(SHAPES)
    for index in range(args.runs):
        offset = index % len(shapes)
        for shape in shapes[offset:] + shapes[:offset]:
            samples[shape].append(run_json_process(
                binary, args.timeout,
                lambda payload, expected=shape: validate_payload(payload, expected, args.backend),
                ("--shape", shape),
            ))
    if binary_info(binary)["sha256"] != identity["sha256"]:
        raise RuntimeError("pressure binary changed during measurement")
    summaries = {
        shape: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
        for shape, values in samples.items()
    }
    report = {
        "scenario": "S-wrapper-pressure",
        "backend": args.backend,
        "candidate_only": True,
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "subject": identity,
        "build": {
            "profile": "release",
            "current_source_sha256": sources,
            "source_hash_scope": "Current checkout; preserved binary identity identifies the measured build",
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "runs_per_shape": args.runs,
        "shape_order": "rotated once per sampling round",
        "warmup_policy": "fresh_engine_no_warmup",
        "measurement": {
            "task_ms": "Rust Instant around a single uninterrupted script and its task-end cleanup; engine initialization, function definition and final release/sweep verification excluded",
            "wall_ms": "whole child process including initialization and final cleanup",
            "cpu_ms": "whole-process user plus system CPU; null without wait4",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null without wait4, not sampled live heap",
            "nodes_after_task": "native document live count before releasing the retained root",
            "gc_epochs": "engine collection epochs during the task; more than one is required",
            "comparison": "No preserved pre-graph implementation of this scenario; no speedup or memory-reduction percentage is claimed",
        },
        "samples": samples,
        "summary": summaries,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + f"-S-wrapper-pressure-{args.backend}.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(output)
    for shape, values in summaries.items():
        task = values["task_ms"]
        print(f"{shape}: task p50/p95 {task['p50']:.3f}/{task['p95']:.3f} ms")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
