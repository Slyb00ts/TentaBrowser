#!/usr/bin/env python3
"""Measure SharedSink ownership and incremental parser text growth.

Run `cargo build --release -p tenta-dom --example shared_parse` before and
after the change, preserving each binary. This driver runs 25 alternating
AB/BA pairs for flat, template and deep trees plus a 256 KiB text node fed in
128-byte chunks, with process CPU and peak RSS.
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

SHAPES = {"flat": 16389, "template": 4101, "deep": 1030, "chunked-text": 6}
METRICS = ("parse_ms", "ns_per_byte", "wall_ms", "cpu_ms", "peak_rss_bytes")


def validate_payload(payload: object, shape: str) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("parser example output must be an object")
    if payload.get("scenario") != "S-shared-parse" or payload.get("shape") != shape:
        raise ValueError("parser example reported a different scenario or shape")
    for name, expected in (("nodes", SHAPES[shape]), ("iterations", 8), ("warmups", 2)):
        value = payload.get(name)
        if type(value) is not int or value != expected:
            raise ValueError(f"unexpected {name}: {value!r}")
    for name in ("source_bytes", "elapsed_ns"):
        value = payload.get(name)
        if type(value) is not int or value <= 0:
            raise ValueError(f"invalid {name}: {value!r}")
    return {
        **payload,
        "parse_ms": payload["elapsed_ns"] / payload["iterations"] / 1_000_000,
        "ns_per_byte": payload["elapsed_ns"] / payload["iterations"] / payload["source_bytes"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=25)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--baseline-source-sha256", help="preserved baseline source manifest hash")
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
    source_paths = (
        "bench/shared_parse.py", "bench/process_metrics.py", "bench/workloads/shared_parse.rs",
        "crates/tenta-dom/examples/shared_parse.rs", "crates/tenta-dom/src/shared_sink.rs",
    )
    sources = {name: file_sha256(ROOT / name) for name in source_paths}
    started = dt.datetime.now(dt.timezone.utc)
    samples: dict[str, dict[str, list[dict]]] = {}
    for shape in SHAPES:
        samples[shape] = {name: [] for name in subjects}
        source_bytes = None
        for index in range(args.warmup + args.runs):
            order = ("baseline", "candidate") if index % 2 == 0 else ("candidate", "baseline")
            for name in order:
                sample = run_json_process(
                    subjects[name], args.timeout,
                    lambda payload, expected=shape: validate_payload(payload, expected),
                    ("--shape", shape),
                )
                if source_bytes is not None and source_bytes != sample["source_bytes"]:
                    raise RuntimeError(f"{shape} source size changed between subject processes")
                source_bytes = sample["source_bytes"]
                if index >= args.warmup:
                    samples[shape][name].append(sample)
    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    summaries = {
        shape: {
            name: {metric: summarize([sample[metric] for sample in values]) for metric in METRICS}
            for name, values in by_subject.items()
        }
        for shape, by_subject in samples.items()
    }
    deltas = {
        shape: {
            metric: 100 * (values["candidate"][metric]["p50"] / values["baseline"][metric]["p50"] - 1)
            if values["baseline"][metric] and values["candidate"][metric]
            and values["baseline"][metric]["p50"] > 0 else None
            for metric in METRICS
        }
        for shape, values in summaries.items()
    }
    report = {
        "scenario": "S-shared-parse",
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "build": {
            "profile": "release",
            "baseline_source_sha256": args.baseline_source_sha256,
            "current_source_sha256": sources,
            "source_hash_scope": "Current checkout; preserved subject identities identify measured binaries",
            "rustc": command_output("rustc", "--version"),
            "cargo": command_output("cargo", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "runs_per_shape_per_subject": args.runs,
        "process_warmups_per_shape_per_subject": args.warmup,
        "interleaved_order": "AB,BA alternating",
        "measurement": {
            "shapes": "flat: 8192 sibling divs; template: 1024 template/span pairs; deep: 1024 nested div input tokens subject to the parser's existing 512-level DOM depth cap; chunked-text: 256 KiB ASCII body text fed in 128-byte Parser::feed chunks, producing one text node",
            "parse_ms": "Rust Instant around eight fresh SharedSink parses and complete parser/document teardown, divided by eight; generated input, one untimed chunked-text correctness parse (chunked-text only), and two in-process warmup parses excluded",
            "ns_per_byte": "same elapsed time divided by iterations and generated UTF-8 input size",
            "wall_ms": "whole process including input generation, one preflight correctness parse for chunked-text, warmup parses and checks",
            "cpu_ms": "whole-process user plus system CPU; null without wait4",
            "peak_rss_bytes": "whole-process wait4 peak RSS; null without wait4",
            "scope": "In-process Parser and SharedSink parsing/ownership with complete document teardown, including incremental growth of one text node; no JavaScript, page orchestration or pressure collection. Not a full page or phase budget gate",
            "noise_floor": "Changes within +/-10% do not support a performance claim",
        },
        "samples": samples,
        "summary": summaries,
        "delta_percent": deltas,
    }
    output = args.output or ROOT / "bench/results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-S-shared-parse.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(output)
    for shape, values in summaries.items():
        baseline, candidate = values["baseline"]["parse_ms"], values["candidate"]["parse_ms"]
        print(f"{shape}: parse p50/p95 {baseline['p50']:.3f}/{baseline['p95']:.3f} -> "
              f"{candidate['p50']:.3f}/{candidate['p95']:.3f} ms "
              f"({deltas[shape]['parse_ms']:+.1f}% p50)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
