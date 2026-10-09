#!/usr/bin/env python3
"""Compare two release builds of the render pipeline benchmark.

    cargo build --release -p tenta-render --example render_bench
    cp target/release/examples/render_bench /tmp/render-bench-before   # before the change
    # ... edit, rebuild ...
    python3 bench/render.py --baseline /tmp/render-bench-before \
        --candidate target/release/examples/render_bench

Each process runs one scenario and prints a JSON line per iteration (stage
times, total, RSS delta, fragment count, cold or warm text caches). Processes
of the two subjects alternate (A/B/A/B) for every scenario, after unmeasured
warm-up pairs, and the iterations of all processes are pooled per
(variant, cache, position): `first` is the first iteration of a process,
`steady` the rest. The pair order alternates (ABBA) to cancel drift.
Percentiles are nearest-rank (`run.summarize`).

Only a `cold` group is cold: the per-thread chunk cache survives a new
session, so session scenarios are cold in `first` only. `--fresh-process`
runs one iteration per process to sample that repeatedly. A change counts only
beyond max(10%, the observed split-half spread of either subject); comparing a
binary with itself reports that spread as measured A/A noise. `--smoke` allows
fewer runs and marks the report as not a claim.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import subprocess
import sys
from pathlib import Path

from run import ROOT, machine_info, summarize
from process_metrics import binary_info, command_output, run_json_process

STAGES = ("style_flush", "box_tree", "layout", "display_list", "raster", "encode")
NOISE_FLOOR_PERCENT = 10.0
LOAD_LIMIT = 8.0


def parse_lines(output: str) -> list[dict]:
    return [json.loads(line) for line in output.splitlines() if line.strip()]


def finite_number(value: object) -> bool:
    return type(value) in (int, float) and math.isfinite(value)


def validator(scenario: str):
    def validate(records: object) -> dict:
        if not isinstance(records, list) or not records:
            raise ValueError("no iteration records")
        for record in records:
            if not isinstance(record, dict) or record.get("scenario") != scenario:
                raise ValueError(f"record for another scenario: {record!r}")
            for key in ("variant", "cache", "iteration", "stages_ms", "total_ms", "detail"):
                if key not in record:
                    raise ValueError(f"record lacks {key!r}")
            if record["cache"] not in ("cold", "warm-text", "warm"):
                raise ValueError(f"unexpected cache state {record['cache']!r}")
            if not finite_number(record["total_ms"]) or record["total_ms"] < 0:
                raise ValueError(f"invalid total_ms: {record['total_ms']!r}")
            stages = record["stages_ms"]
            if set(stages) != set(STAGES):
                raise ValueError(f"unexpected stages: {sorted(stages)}")
            for name, value in stages.items():
                if value is not None and not finite_number(value):
                    raise ValueError(f"invalid {name}: {value!r}")
        return {"iterations": records}

    return validate


def list_scenarios(binary: Path) -> list[str]:
    out = subprocess.run([str(binary), "--list"], capture_output=True, text=True, check=True)
    return out.stdout.split()


def group_key(record: dict) -> str:
    position = "first" if record["iteration"] == 0 else "steady"
    return f"{record['variant']}/{record['cache']}/{position}"


def split_spread(runs: list[dict], key: str, metric: str) -> float | None:
    """Relative p50 difference in percent between the even and odd runs of
    one subject: the noise the machine shows when nothing changed."""
    halves = ([], [])
    for index, run in enumerate(runs):
        for record in run["iterations"]:
            if group_key(record) != key:
                continue
            value = record["total_ms"] if metric == "total_ms" else (
                record["stages_ms"].get(metric.removesuffix("_ms")))
            if value is not None:
                halves[index % 2].append(value)
    if not all(halves):
        return None
    a, b = (summarize(h)["p50"] for h in halves)
    return 100 * abs(a / b - 1) if b else None


def summarize_runs(runs: list[dict]) -> dict:
    """Per group: summaries of total, each measured stage, RSS delta; plus
    process-level wall time, CPU time and peak RSS."""
    pooled: dict[str, list[dict]] = {}
    for run in runs:
        for record in run["iterations"]:
            pooled.setdefault(group_key(record), []).append(record)
    groups = {}
    for key, records in pooled.items():
        metrics = {"total_ms": summarize([r["total_ms"] for r in records])}
        for stage in STAGES:
            values = [r["stages_ms"][stage] for r in records]
            if any(v is not None for v in values):
                metrics[f"{stage}_ms"] = summarize(values)
        metrics["rss_delta_kib"] = summarize([r.get("rss_delta_kib") for r in records])
        groups[key] = {
            "fragments": records[0].get("fragments"),
            "layout_covers_style_and_boxes": records[0].get("layout_covers_style_and_boxes"),
            "metrics": {k: v for k, v in metrics.items() if v},
            "split_spread_percent": {
                k: split_spread(runs, key, k) for k, v in metrics.items()
                if v and k != "rss_delta_kib"
            },
        }
    process = {
        metric: summarize([run[metric] for run in runs])
        for metric in ("wall_ms", "cpu_ms", "peak_rss_bytes")
    }
    return {"groups": groups, "process": process}


def deltas(baseline: dict, candidate: dict) -> dict:
    """Candidate p50 change in percent per group and metric. A change is
    outside noise only beyond max(10%, the split-half spread of either subject).
    Only metrics both subjects measured."""
    out = {}
    for key, base_group in baseline["groups"].items():
        cand_group = candidate["groups"].get(key)
        if not cand_group:
            continue
        for metric, base in base_group["metrics"].items():
            cand = cand_group["metrics"].get(metric)
            if not cand or not base["p50"] or metric == "rss_delta_kib":
                continue
            percent = 100 * (cand["p50"] / base["p50"] - 1)
            spreads = [base_group["split_spread_percent"].get(metric),
                       cand_group["split_spread_percent"].get(metric)]
            threshold = max([NOISE_FLOOR_PERCENT] + [v for v in spreads if v is not None])
            out.setdefault(key, {})[metric] = {
                "percent": percent,
                "threshold_percent": threshold,
                "within_noise": abs(percent) <= threshold,
            }
    return out


def print_scenario(scenario: str, summaries: dict, change: dict, same_binary: bool) -> None:
    print(f"\n{scenario}")
    print(f"  {'group':34s} {'subject':9s} {'total p50/p95 ms':>18s} {'layout p50':>10s}"
          f" {'raster p50':>10s} {'rss d p50 KiB':>13s}  change(total p50)")
    for key in summaries["baseline"]["groups"]:
        for name, summary in summaries.items():
            group = summary["groups"].get(key)
            if not group:
                continue
            m = group["metrics"]
            total = m["total_ms"]

            def p50(metric: str) -> str:
                return f"{m[metric]['p50']:.2f}" if metric in m else "-"

            rss = m.get("rss_delta_kib")
            verdict = ""
            if name == "candidate" and key in change and "total_ms" in change[key]:
                c = change[key]["total_ms"]
                if same_binary:
                    verdict = f"{c['percent']:+.1f}% (A/A measured noise)"
                else:
                    verdict = f"{c['percent']:+.1f}%" + (
                        f" (within noise, limit {c['threshold_percent']:.1f}%)"
                        if c["within_noise"] else f" (beyond {c['threshold_percent']:.1f}%)")
            print(f"  {key:34s} {name:9s} {total['p50']:8.2f}/{total['p95']:<8.2f}"
                  f" {p50('layout_ms'):>10s} {p50('raster_ms'):>10s}"
                  f" {(rss['p50'] if rss else float('nan')):13.0f}  {verdict}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--baseline", required=True, type=Path, help="pre-change render_bench")
    parser.add_argument("--candidate", required=True, type=Path, help="post-change render_bench")
    parser.add_argument("--scenario", action="append", help="scenario name; repeatable; default all")
    parser.add_argument("--runs", type=int, default=25, help="measured A/B process pairs per scenario")
    parser.add_argument("--warmup", type=int, default=2, help="unmeasured A/B process pairs")
    parser.add_argument("--iterations", type=int, help="iterations per phase inside a process "
                        "(default: the scenario's own)")
    parser.add_argument("--fresh-process", action="store_true",
                        help="one iteration per process: repeatable cold-text numbers (use more --runs)")
    parser.add_argument("--smoke", action="store_true",
                        help="allow fewer than 20 runs; the report is marked as not a claim")
    parser.add_argument("--timeout", type=float, default=300, help="seconds per process")
    parser.add_argument("--output", type=Path, help="JSON report path")
    args = parser.parse_args()
    if args.fresh_process:
        args.iterations = 1
    if args.runs < 1 or (args.runs < 20 and not args.smoke) or args.warmup < 0 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("runs must be >=20 (or pass --smoke), warmup nonnegative, timeout finite and positive")
    if args.iterations is not None and args.iterations < 1:
        parser.error("iterations must be positive")

    subjects = {name: path.expanduser().resolve() for name, path in
                (("baseline", args.baseline), ("candidate", args.candidate))}
    for name, path in subjects.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
    identities = {name: binary_info(path) for name, path in subjects.items()}
    known = list_scenarios(subjects["candidate"])
    scenarios = args.scenario or known
    for scenario in scenarios:
        if scenario not in known:
            parser.error(f"unknown scenario {scenario!r}; candidate offers {known}")

    load_start = os.getloadavg()
    started = dt.datetime.now(dt.timezone.utc)
    report_scenarios = {}
    for scenario in scenarios:
        extra = ("--scenario", scenario) + (
            ("--iterations", str(args.iterations)) if args.iterations else ())
        samples: dict[str, list[dict]] = {name: [] for name in subjects}
        for index in range(args.warmup + args.runs):
            order = list(subjects.items())
            if index % 2:
                order.reverse()
            for name, binary in order:
                result = run_json_process(binary, args.timeout, validator(scenario),
                                          extra, parse_lines)
                if index >= args.warmup:
                    samples[name].append(result)
        summaries = {name: summarize_runs(runs) for name, runs in samples.items()}
        change = deltas(summaries["baseline"], summaries["candidate"])
        report_scenarios[scenario] = {"summary": summaries, "delta_percent": change,
                                      "samples": samples}
        print_scenario(scenario, summaries, change,
                       identities['baseline']['sha256'] == identities['candidate']['sha256'])

    for name, path in subjects.items():
        if binary_info(path)["sha256"] != identities[name]["sha256"]:
            raise RuntimeError(f"{name} binary changed during measurement")
    load_end = os.getloadavg()
    loaded = max(load_start[0], load_end[0]) >= LOAD_LIMIT
    if loaded:
        print(f"\nWARNING: 1-minute load {load_start[0]:.1f} -> {load_end[0]:.1f} (limit {LOAD_LIMIT:g}); "
              "do not record these numbers as a baseline", file=sys.stderr)
    report = {
        "scenarios": scenarios,
        "smoke": args.smoke,
        "claim": not args.smoke and not loaded,
        "fresh_process": args.fresh_process,
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": machine_info(),
        "load_average_start": load_start,
        "load_average_end": load_end,
        "machine_loaded": loaded,
        "build": {
            "profile": "release",
            "identical_binaries": identities["baseline"]["sha256"] == identities["candidate"]["sha256"],
            "rustc": command_output("rustc", "--version"),
            "git_head": command_output("git", "rev-parse", "HEAD"),
            "git_status_short": command_output("git", "status", "--short"),
        },
        "subjects": identities,
        "runs_per_subject": args.runs,
        "process_warmups_per_subject": args.warmup,
        "iterations_override": args.iterations,
        "interleaved_order": "alternating pair order (baseline,candidate then candidate,baseline), per scenario",
        "measurement": {
            "stages_ms": "Instant around each stage inside the subject; null when the scenario does not time it. "
                         "Session scenarios report style flush, box tree and layout as `layout` "
                         "(layout_covers_style_and_boxes)",
            "total_ms": "wall time of the timed call, or the sum of the timed stages",
            "rss_delta_kib": "current RSS from ps after minus before one iteration, sampled outside the timed region",
            "cache": "cold: text new to the thread (first session capture of a process, or distinct text); "
                     "warm-text: a new session that kept the per-thread chunk cache; warm: repeated work",
            "position": "first = first iteration of a phase in a fresh process, steady = later iterations",
            "peak_rss_bytes": "ru_maxrss from wait4 for the whole subject process",
            "delta_percent": "100 * (candidate p50 / baseline p50 - 1); noise limit is max(10%, split-half spread of either subject)",
        },
        "results": report_scenarios,
    }
    output = args.output or ROOT / "bench" / "results" / (
        started.strftime("%Y%m%dT%H%M%S%fZ") + "-render.json")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as result_file:
        json.dump(report, result_file, indent=2)
        result_file.write("\n")
    print(f"\nresults: {output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"render benchmark failed: {exc}", file=sys.stderr)
        sys.exit(1)
