#!/usr/bin/env python3
"""Compare preserved real-engine DOM mutation examples in AB/BA pairs."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path

from process_metrics import binary_info, command_output, file_sha256, run_json_process
from run import ROOT, machine_info, summarize

SCENARIO = "S-dom-mutation-hooks"
WORK = {
    "phase_order": "detached_then_connected",
    "warmup_iterations_per_phase": 128,
    "iterations_per_phase": 8192,
    "mutations_per_iteration": 5,
    "native_nodes": 9,
    "detached_checksum": 40960,
    "connected_checksum": 40960,
}
METRICS = ("detached_ms", "connected_ms", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object, backend: str) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("DOM mutation example output must be an object")
    if payload.get("scenario") != SCENARIO or payload.get("backend") != backend:
        raise ValueError("DOM mutation example reported another scenario or backend")
    for name, expected in WORK.items():
        value = payload.get(name)
        if type(value) is not type(expected) or value != expected:
            raise ValueError(f"unexpected {name}: {value!r}")
    for name in ("detached_elapsed_ns", "connected_elapsed_ns"):
        value = payload.get(name)
        if type(value) is not int or value <= 0:
            raise ValueError(f"invalid {name}: {value!r}")
    return {
        **payload,
        "detached_ms": payload["detached_elapsed_ns"] / 1_000_000,
        "connected_ms": payload["connected_elapsed_ns"] / 1_000_000,
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
    parser.add_argument("--candidate-source-sha256")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 20 or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20, warmup nonnegative, and timeout finite and positive")
    for name in ("baseline_source_sha256", "candidate_source_sha256"):
        digest = getattr(args, name)
        if digest is not None and (len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest)):
            parser.error(f"{name} must be a lowercase SHA-256 digest")
    if args.output is not None and args.output.exists():
        parser.error(f"refusing to overwrite {args.output}")
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
    example = "dom_mutation_hooks_v8.rs" if args.backend == "v8" else "dom_mutation_hooks_qjs.rs"
    source_paths = (
        "bench/dom_mutation_hooks.py", "bench/process_metrics.py", "bench/run.py",
        "bench/scenarios.toml",
        "bench/workloads/dom_mutation_hooks.rs", "bench/workloads/dom_mutation_hooks.js",
        f"crates/{crate}/examples/{example}",
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
        raise RuntimeError("benchmark source changed during measurement")
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
            "candidate_source_sha256": args.candidate_source_sha256,
            "workload_source_sha256": sources,
            "source_hash_scope": "current workload/driver files; preserved binary and build manifests identify production subjects",
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
            "workload": WORK,
            "detached_ms": "one warmed evaluated task with 8192 detached-node iterations; includes return conversion and task-end collection",
            "connected_ms": "one warmed evaluated task with 8192 connected-node iterations; includes return conversion and task-end collection",
            "phase_order": "detached always precedes connected within a process; compare each phase across builds, not modes to each other",
            "mutations": "each iteration sets/removes one attribute, replaces Text.data, and appends/removes one child; semantic reads/checks remain inside timing",
            "teardown": "final tree/native-node verification, document cleanup and engine teardown are outside phase timers; per-iteration semantic and native-health checks remain inside",
            "wall_ms": "whole process including engine creation, setup, warmup, timed tasks, checks and teardown",
            "cpu_ms": "whole-process user plus system CPU; null without wait4",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null without wait4",
            "scope": "real V8 or QuickJS DOM mutation routing with no MutationObserver registration or callback; this does not establish observer delivery conformance",
            "noise_floor": "changes within +/-10% do not support a performance claim",
        },
        "samples": samples,
        "summary": summaries,
        "delta_percent": deltas,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + f"-{SCENARIO}-{args.backend}.json"
    )
    if output.exists():
        raise FileExistsError(f"refusing to overwrite {output}")
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
