#!/usr/bin/env python3
"""Benchmark harness: runs bench/scenarios.toml against TentaBrowser, the
reference implementation and headless Chromium, interleaved A/B/A/B, and
reports p50/p95/max wall time, CPU time, peak RSS and time to first request.

Python 3.11+ standard library only; `psutil`, when importable, adds the peak
RSS of the whole process tree (Chromium's renderer and GPU processes are
separate processes, which the kernel's per-process rusage does not include).

    python3 bench/run.py --scenario S-static-text-cli --compare reference
    python3 bench/run.py --scenario all --subjects reference,chromium --runs 20
    python3 bench/run.py --list
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import re
import resource
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from pathlib import Path

try:  # optional
    import psutil  # type: ignore
except ImportError:  # pragma: no cover - depends on the host
    psutil = None

ROOT = Path(__file__).resolve().parent.parent
SCENARIOS = ROOT / "bench" / "scenarios.toml"
RESULTS = ROOT / "bench" / "results"
FIXTURE_SERVER = ROOT / "target" / "release" / "fixture-server"
REQ_LINE = re.compile(rb"^REQ (\d+) (\S+) (\S+)")

NOT_IMPLEMENTED = "not-implemented"
UNAVAILABLE = "unavailable"


# --------------------------------------------------------------------------
# Machine and subjects


def machine_info() -> dict:
    info = {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "python": platform.python_version(),
        "psutil": getattr(psutil, "__version__", None),
    }
    if sys.platform == "darwin":
        for key, name in (("cpu", "machdep.cpu.brand_string"), ("memory_bytes", "hw.memsize"),
                          ("os_version", "kern.osproductversion")):
            try:
                value = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True,
                                       check=True).stdout.strip()
                info[key] = int(value) if key == "memory_bytes" else value
            except (subprocess.CalledProcessError, FileNotFoundError, ValueError):
                pass
    elif sys.platform.startswith("linux"):
        try:
            with open("/proc/meminfo") as f:
                for line in f:
                    if line.startswith("MemTotal:"):
                        info["memory_bytes"] = int(line.split()[1]) * 1024
            with open("/proc/cpuinfo") as f:
                for line in f:
                    if line.startswith("model name"):
                        info["cpu"] = line.split(":", 1)[1].strip()
                        break
        except OSError:
            pass
    return info


def expand(path: str) -> Path:
    p = Path(os.path.expanduser(path))
    return p if p.is_absolute() else ROOT / p


def subject_version(binary: Path, args: list[str]) -> str | None:
    try:
        out = subprocess.run([str(binary), *args], capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    text = (out.stdout or out.stderr).strip()
    return text.splitlines()[0] if text else None


# --------------------------------------------------------------------------
# Fixture server


class FixtureServer:
    """Runs target/release/fixture-server and records every `REQ` line it logs."""

    def __init__(self, rtt_ms: int, extra_args: list[str] | None = None, root: str | None = None):
        self.rtt_ms = rtt_ms
        self.extra_args = list(extra_args or [])
        # A scenario may serve a different tree: the search scenarios replay the
        # recorded SERPs from providers/fixtures/ (scenarios.toml: fixture_root).
        if root:
            self.extra_args = ["--root", str(expand(root)), *self.extra_args]
        self.requests: list[tuple[int, str, str]] = []
        self.lock = threading.Lock()
        self.proc: subprocess.Popen | None = None
        self.url = ""

    def __enter__(self) -> "FixtureServer":
        cmd = [str(FIXTURE_SERVER), "--rtt", str(self.rtt_ms), "--log-requests", *self.extra_args]
        self.proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert self.proc.stdout and self.proc.stderr
        line = self.proc.stdout.readline().decode()
        if not line.startswith("LISTENING "):
            raise RuntimeError(f"fixture-server did not start: {line!r}")
        self.url = line.split(" ", 1)[1].strip()
        threading.Thread(target=self._pump, daemon=True).start()
        return self

    def _pump(self) -> None:
        assert self.proc and self.proc.stderr
        for raw in self.proc.stderr:
            m = REQ_LINE.match(raw)
            if m:
                with self.lock:
                    self.requests.append((int(m.group(1)), m.group(2).decode(), m.group(3).decode()))

    def mark(self) -> int:
        with self.lock:
            return len(self.requests)

    def first_request_after(self, mark: int, deadline_s: float = 0.5) -> tuple[int, str, str] | None:
        # Log lines may still be in flight when the client exits; wait briefly.
        end = time.monotonic() + deadline_s
        while time.monotonic() < end:
            with self.lock:
                if len(self.requests) > mark:
                    return self.requests[mark]
            time.sleep(0.002)
        return None

    def __exit__(self, *_exc) -> None:
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def build_fixture_server() -> None:
    subprocess.run(["cargo", "build", "--release", "-p", "tenta-testutil", "--bin", "fixture-server"],
                   cwd=ROOT, check=True)


def measure_popen_floor() -> float:
    """Measure the median overhead of subprocess.Popen for /usr/bin/true.

    This is subtracted from TTFR measurements for S-cold-fetch to report
    the engine startup time separately from process creation overhead.
    Measures ~10 runs and returns median in milliseconds.
    """
    samples = []
    for _ in range(10):
        t0 = time.time_ns()
        proc = subprocess.Popen(["/usr/bin/true"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        proc.wait()
        elapsed_ns = time.time_ns() - t0
        samples.append(elapsed_ns / 1e6)
    return statistics.median(samples)


# --------------------------------------------------------------------------
# One measured run


class TreeSampler:
    """Peak RSS of a process and its descendants, sampled with psutil."""

    def __init__(self, pid: int, interval_s: float = 0.005):
        self.pid = pid
        self.interval_s = interval_s
        self.peak = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        try:
            root = psutil.Process(self.pid)
        except psutil.Error:
            return
        while not self._stop.is_set():
            total = 0
            try:
                for proc in [root, *root.children(recursive=True)]:
                    try:
                        total += proc.memory_info().rss
                    except psutil.Error:
                        pass
            except psutil.Error:
                break
            self.peak = max(self.peak, total)
            self._stop.wait(self.interval_s)

    def __enter__(self) -> "TreeSampler":
        self._thread.start()
        return self

    def __exit__(self, *_exc) -> None:
        self._stop.set()
        self._thread.join(timeout=1)


def rss_bytes(ru_maxrss: int) -> int:
    # macOS reports bytes, Linux kibibytes.
    return ru_maxrss if sys.platform == "darwin" else ru_maxrss * 1024


def run_once(cmd: list[str], timeout_s: float, server: FixtureServer, out_file: Path | None,
             marker: str | None, stdout_path: Path) -> dict:
    if out_file and out_file.exists():
        out_file.unlink()
    mark = server.mark()
    with open(stdout_path, "wb") as stdout, tempfile.TemporaryFile() as stderr:
        spawn_ns = time.time_ns()
        t0 = time.perf_counter()
        proc = subprocess.Popen(cmd, stdout=stdout, stderr=stderr, cwd=ROOT)
        killer = threading.Timer(timeout_s, lambda: proc.kill())
        killer.start()
        sampler = TreeSampler(proc.pid) if psutil else None
        if sampler:
            sampler.__enter__()
        try:
            if hasattr(os, "wait4"):
                _, status, ru = os.wait4(proc.pid, 0)
                proc.returncode = os.waitstatus_to_exitcode(status)
                cpu_s = ru.ru_utime + ru.ru_stime
                peak = rss_bytes(ru.ru_maxrss)
            else:  # pragma: no cover - Windows
                proc.wait()
                cpu_s = None
                peak = None
        finally:
            killer.cancel()
            if sampler:
                sampler.__exit__()
        wall_s = time.perf_counter() - t0
        stderr.seek(0)
        stderr_tail = stderr.read()[-2000:].decode(errors="replace")

    first = server.first_request_after(mark)
    ttfr_ms = (first[0] - spawn_ns) / 1e6 if first else None

    failure = None
    if proc.returncode != 0:
        failure = f"exit code {proc.returncode}"
    elif marker:
        with open(stdout_path, "rb") as f:
            if marker.encode() not in f.read():
                failure = f"stdout lacks marker {marker!r}"
    if not failure and out_file is not None:
        if not out_file.exists() or out_file.stat().st_size == 0:
            failure = f"output file {out_file.name} missing or empty"

    # Extract engine_overhead_ms from search JSON output (last line if present)
    engine_overhead_ms = None
    if not failure and marker and "engine_overhead_ms" in marker:
        with open(stdout_path, "rb") as f:
            content = f.read().decode(errors="replace")
            lines = content.strip().split("\n")
            if lines:
                try:
                    last_line = json.loads(lines[-1])
                    # engine_overhead_ms is nested in meta.timing.engine_overhead_ms
                    if "meta" in last_line and "timing" in last_line["meta"]:
                        engine_overhead_ms = last_line["meta"]["timing"].get("engine_overhead_ms")
                except (json.JSONDecodeError, KeyError, TypeError):
                    pass

    return {
        "wall_ms": wall_s * 1000,
        "cpu_ms": cpu_s * 1000 if cpu_s is not None else None,
        "peak_rss_bytes": peak,
        "peak_rss_tree_bytes": sampler.peak if sampler else None,
        "ttfr_ms": ttfr_ms,
        "engine_overhead_ms": engine_overhead_ms,
        "requests": server.mark() - mark,
        "failure": failure,
        "stderr_tail": stderr_tail if failure else None,
    }


# --------------------------------------------------------------------------
# Statistics and reporting


def percentile(sorted_values: list[float], pct: float) -> float:
    """Nearest-rank percentile over an already sorted list."""
    if not sorted_values:
        return float("nan")
    rank = max(1, min(len(sorted_values), int(-(-pct / 100 * len(sorted_values) // 1))))
    return sorted_values[rank - 1]


def summarize(values: list[float | None]) -> dict | None:
    present = sorted(v for v in values if v is not None)
    if not present:
        return None
    return {
        "n": len(present),
        "min": present[0],
        "p50": statistics.median(present),
        "p95": percentile(present, 95),
        "max": present[-1],
        "mean": statistics.fmean(present),
    }


def fmt_ms(summary: dict | None, key: str = "p50") -> str:
    if not summary:
        return "–"
    return f"{summary[key]:.1f}"


def fmt_mb(summary: dict | None, key: str = "p50") -> str:
    if not summary:
        return "–"
    return f"{summary[key] / (1024 * 1024):.1f}"


def markdown_table(rows: list[dict]) -> str:
    # Check if any row has engine_overhead_ms or ttfr_ms_adjusted
    has_engine_overhead = any("engine_overhead_ms" in r.get("summary", {}) and r.get("summary", {})["engine_overhead_ms"] for r in rows)
    has_adjusted_ttfr = any("ttfr_ms_adjusted" in r.get("summary", {}) and r.get("summary", {})["ttfr_ms_adjusted"] for r in rows)

    if has_engine_overhead and has_adjusted_ttfr:
        # Both metrics: engine overhead for search and adjusted TTFR for cold-fetch
        head = ("| Scenario | Subject | Status | n | Wall p50 (ms) | Wall p95 | Wall max | CPU p50 (ms) "
                "| Peak RSS p50 (MB) | Tree RSS p50 (MB) | TTFR/Engine p50 (ms) |")
        lines = [head, "|" + "---|" * 11]
        for r in rows:
            s = r.get("summary") or {}
            # Show ttfr_ms_adjusted for S-cold-fetch, engine_overhead_ms for search scenarios
            metric = fmt_ms(s.get('ttfr_ms_adjusted')) if s.get('ttfr_ms_adjusted') else fmt_ms(s.get('engine_overhead_ms'))
            lines.append(
                f"| {r['scenario']} | {r['subject']} | {r['status']} | {r.get('runs_ok', 0)} "
                f"| {fmt_ms(s.get('wall_ms'))} | {fmt_ms(s.get('wall_ms'), 'p95')} | {fmt_ms(s.get('wall_ms'), 'max')} "
                f"| {fmt_ms(s.get('cpu_ms'))} | {fmt_mb(s.get('peak_rss_bytes'))} "
                f"| {fmt_mb(s.get('peak_rss_tree_bytes'))} | {metric} |"
            )
    elif has_engine_overhead:
        head = ("| Scenario | Subject | Status | n | Wall p50 (ms) | Wall p95 | Wall max | CPU p50 (ms) "
                "| Peak RSS p50 (MB) | Tree RSS p50 (MB) | Engine Overhead p50 (ms) |")
        lines = [head, "|" + "---|" * 11]
        for r in rows:
            s = r.get("summary") or {}
            lines.append(
                f"| {r['scenario']} | {r['subject']} | {r['status']} | {r.get('runs_ok', 0)} "
                f"| {fmt_ms(s.get('wall_ms'))} | {fmt_ms(s.get('wall_ms'), 'p95')} | {fmt_ms(s.get('wall_ms'), 'max')} "
                f"| {fmt_ms(s.get('cpu_ms'))} | {fmt_mb(s.get('peak_rss_bytes'))} "
                f"| {fmt_mb(s.get('peak_rss_tree_bytes'))} | {fmt_ms(s.get('engine_overhead_ms'))} |"
            )
    elif has_adjusted_ttfr:
        head = ("| Scenario | Subject | Status | n | Wall p50 (ms) | Wall p95 | Wall max | CPU p50 (ms) "
                "| Peak RSS p50 (MB) | Tree RSS p50 (MB) | TTFR p50 (ms) | TTFR adj p50 (ms) |")
        lines = [head, "|" + "---|" * 12]
        for r in rows:
            s = r.get("summary") or {}
            lines.append(
                f"| {r['scenario']} | {r['subject']} | {r['status']} | {r.get('runs_ok', 0)} "
                f"| {fmt_ms(s.get('wall_ms'))} | {fmt_ms(s.get('wall_ms'), 'p95')} | {fmt_ms(s.get('wall_ms'), 'max')} "
                f"| {fmt_ms(s.get('cpu_ms'))} | {fmt_mb(s.get('peak_rss_bytes'))} "
                f"| {fmt_mb(s.get('peak_rss_tree_bytes'))} | {fmt_ms(s.get('ttfr_ms'))} | {fmt_ms(s.get('ttfr_ms_adjusted'))} |"
            )
    else:
        head = ("| Scenario | Subject | Status | n | Wall p50 (ms) | Wall p95 | Wall max | CPU p50 (ms) "
                "| Peak RSS p50 (MB) | Tree RSS p50 (MB) | TTFR p50 (ms) |")
        lines = [head, "|" + "---|" * 11]
        for r in rows:
            s = r.get("summary") or {}
            lines.append(
                f"| {r['scenario']} | {r['subject']} | {r['status']} | {r.get('runs_ok', 0)} "
                f"| {fmt_ms(s.get('wall_ms'))} | {fmt_ms(s.get('wall_ms'), 'p95')} | {fmt_ms(s.get('wall_ms'), 'max')} "
                f"| {fmt_ms(s.get('cpu_ms'))} | {fmt_mb(s.get('peak_rss_bytes'))} "
                f"| {fmt_mb(s.get('peak_rss_tree_bytes'))} | {fmt_ms(s.get('ttfr_ms'))} |"
            )
    return "\n".join(lines)


# --------------------------------------------------------------------------
# Driver


def load_config() -> dict:
    with open(SCENARIOS, "rb") as f:
        return tomllib.load(f)


def resolve_subjects(config: dict, names: list[str]) -> dict[str, dict]:
    resolved = {}
    for name in names:
        spec = config["subjects"].get(name)
        if spec is None:
            sys.exit(f"unknown subject {name!r}; known: {', '.join(config['subjects'])}")
        binary = expand(spec["bin"])
        version = subject_version(binary, spec.get("version_args", [])) if binary.exists() else None
        resolved[name] = {
            **spec,
            "name": name,
            "binary": binary,
            "version": version,
            "available": binary.exists() and version is not None,
        }
    return resolved


def render(args_template: list[str], url: str, out: Path | None, profile: Path) -> list[str]:
    return [a.replace("{url}", url).replace("{out}", str(out) if out else "").replace("{profile}", str(profile))
            for a in args_template]


def run_scenario(scenario: dict, subjects: dict[str, dict], opts: argparse.Namespace, profile: Path,
                 work: Path, popen_floor_ms: float | None = None) -> list[dict]:
    rows: list[dict] = []
    runnable: dict[str, list[str]] = {}
    rtt_ms = opts.rtt if opts.rtt is not None else scenario.get("rtt_ms", 0)

    for name, subject in subjects.items():
        row = {"scenario": scenario["name"], "subject": name, "version": subject["version"],
               "status": "ok", "runs_ok": 0, "runs_failed": 0, "samples": [], "summary": None}
        template = scenario.get("commands", {}).get(name)
        if name not in scenario["applicable_to"]:
            row["status"] = "not-applicable"
        elif scenario["mode"] != "cli" or template is None or subject.get("status") == NOT_IMPLEMENTED:
            row["status"] = NOT_IMPLEMENTED
        elif not subject["available"]:
            row["status"] = UNAVAILABLE
            row["reason"] = f"binary not found or not runnable: {subject['binary']}"
        else:
            runnable[name] = template
        rows.append(row)

    if not runnable:
        return rows

    with FixtureServer(rtt_ms, opts.server_args, scenario.get("fixture_root")) as server:
        url = server.url + scenario["path"]
        by_name = {r["subject"]: r for r in rows}
        for name, template in runnable.items():
            subject = subjects[name]
            out = work / f"{scenario['name']}-{name}.png" if scenario.get("expect_output_file") else None
            cmd = [str(subject["binary"]), *render(subject.get("common_args", []), url, out, profile),
                   *render(template, url, out, profile), *opts.extra_args.get(name, [])]
            by_name[name]["command"] = cmd
            by_name[name]["out_file"] = out
        marker = scenario.get("expect_stdout_contains")
        timeout_s = opts.timeout or scenario.get("timeout_s", 90)
        total = opts.warmup + opts.runs
        for i in range(total):
            for name in runnable:
                row = by_name[name]
                stdout_path = work / f"{scenario['name']}-{name}.stdout"
                sample = run_once(row["command"], timeout_s, server, row["out_file"], marker, stdout_path)
                if i < opts.warmup:
                    continue
                if sample["failure"]:
                    row["runs_failed"] += 1
                    row.setdefault("first_failure", sample)
                else:
                    row["runs_ok"] += 1
                    row["samples"].append({k: v for k, v in sample.items() if k not in ("failure", "stderr_tail")})
            if opts.verbose:
                done = i + 1 - opts.warmup
                print(f"  {scenario['name']}: round {done}/{opts.runs}" if done > 0 else
                      f"  {scenario['name']}: warmup {i + 1}/{opts.warmup}", file=sys.stderr)

        for row in rows:
            row.pop("out_file", None)
            if row["subject"] not in runnable:
                continue
            row["rtt_ms"] = rtt_ms
            row["url"] = url
            if row["runs_ok"] == 0:
                row["status"] = "failed"
                continue
            if row["runs_failed"]:
                row["status"] = f"ok ({row['runs_failed']} failed)"

            # Determine which keys to summarize based on the scenario
            summary_keys = ["wall_ms", "cpu_ms", "peak_rss_bytes", "peak_rss_tree_bytes", "ttfr_ms"]

            # For search scenarios, include engine_overhead_ms instead of ttfr_ms
            if scenario["name"].startswith("S-search-"):
                summary_keys = ["wall_ms", "cpu_ms", "peak_rss_bytes", "peak_rss_tree_bytes", "engine_overhead_ms"]

            row["summary"] = {
                key: summarize([s[key] for s in row["samples"] if key in s])
                for key in summary_keys
            }

            # For S-cold-fetch, also report popen-adjusted TTFR
            if scenario["name"] == "S-cold-fetch" and popen_floor_ms is not None:
                row["popen_floor_ms"] = popen_floor_ms
                # Calculate adjusted TTFR
                ttfr_samples = [s.get("ttfr_ms") for s in row["samples"] if "ttfr_ms" in s]
                adjusted_samples = [max(0, t - popen_floor_ms) for t in ttfr_samples if t is not None]
                if adjusted_samples:
                    row["summary"]["ttfr_ms_adjusted"] = summarize(adjusted_samples)
    return rows


def parse_extra_args(items: list[str]) -> dict[str, list[str]]:
    extra: dict[str, list[str]] = {}
    for item in items:
        subject, _, rest = item.partition("=")
        if not rest:
            sys.exit(f"--extra-args expects SUBJECT=ARG[,ARG...], got {item!r}")
        extra.setdefault(subject, []).extend(rest.split(","))
    return extra


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scenario", action="append", default=[],
                    help="scenario name (repeatable) or 'all'; default: all")
    ap.add_argument("--subjects", default=None,
                    help="comma-separated subjects to run (default: tentabrowser,reference,chromium)")
    ap.add_argument("--compare", default=None,
                    help="run tentabrowser against this one subject (shorthand for --subjects tentabrowser,X)")
    ap.add_argument("--runs", type=int, default=None, help="measured runs per subject (default from scenarios.toml)")
    ap.add_argument("--warmup", type=int, default=None, help="discarded warm-up runs per subject")
    ap.add_argument("--rtt", type=int, default=None, help="override the scenario's simulated RTT in ms")
    ap.add_argument("--timeout", type=float, default=None, help="per-run timeout in seconds")
    ap.add_argument("--extra-args", action="append", default=[], metavar="SUBJECT=ARG,ARG",
                    help="append arguments to one subject's command (repeatable)")
    ap.add_argument("--server-args", default="", help="extra arguments for fixture-server, e.g. '--delay-path /x/*.js=300'")
    ap.add_argument("--out-dir", default=str(RESULTS), help="where to write JSON results (default bench/results/)")
    ap.add_argument("--label", default=None, help="label stored in the result file and used in its name")
    ap.add_argument("--no-build", action="store_true", help="do not (re)build fixture-server")
    ap.add_argument("--list", action="store_true", help="list scenarios and exit")
    ap.add_argument("-v", "--verbose", action="store_true")
    opts = ap.parse_args()

    config = load_config()
    scenarios = config["scenario"]
    if opts.list:
        for s in scenarios:
            print(f"{s['name']:<22} {s['phase']:<4} {s['mode']:<10} {','.join(s['applicable_to']):<32} {s['definition']}")
        return 0

    wanted = opts.scenario or ["all"]
    if "all" not in wanted:
        known = {s["name"] for s in scenarios}
        for name in wanted:
            if name not in known:
                sys.exit(f"unknown scenario {name!r}; use --list")
        scenarios = [s for s in scenarios if s["name"] in wanted]

    defaults = config.get("defaults", {})
    opts.runs = opts.runs if opts.runs is not None else defaults.get("runs", 20)
    opts.warmup = opts.warmup if opts.warmup is not None else defaults.get("warmup", 1)
    opts.timeout = opts.timeout or defaults.get("timeout_s")
    opts.extra_args = parse_extra_args(opts.extra_args)
    opts.server_args = opts.server_args.split() if opts.server_args else []

    if opts.compare:
        subject_names = ["tentabrowser", opts.compare]
    elif opts.subjects:
        subject_names = [s for s in opts.subjects.split(",") if s]
    else:
        subject_names = list(config["subjects"])
    subjects = resolve_subjects(config, subject_names)

    if not opts.no_build or not FIXTURE_SERVER.exists():
        build_fixture_server()

    started = dt.datetime.now(dt.timezone.utc)

    # Measure subprocess.Popen overhead for TTFR baseline adjustment
    popen_floor_ms = None
    if any(s["name"] == "S-cold-fetch" for s in scenarios):
        if opts.verbose:
            print("Measuring Popen floor...", file=sys.stderr)
        popen_floor_ms = measure_popen_floor()
        if opts.verbose:
            print(f"  Popen floor median: {popen_floor_ms:.2f} ms", file=sys.stderr)

    all_rows: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="tenta-bench-") as tmp:
        work = Path(tmp)
        profile = work / "chromium-profile"
        profile.mkdir()
        for scenario in scenarios:
            if opts.verbose:
                print(f"{scenario['name']}: {scenario['definition']}", file=sys.stderr)
            all_rows.extend(run_scenario(scenario, subjects, opts, profile, work, popen_floor_ms))

    out_dir = Path(opts.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = started.strftime("%Y%m%dT%H%M%SZ")
    suffix = f"-{opts.label}" if opts.label else ""
    scenario_part = scenarios[0]["name"] if len(scenarios) == 1 else "multi"
    out_path = out_dir / f"{stamp}-{scenario_part}{suffix}.json"
    report = {
        "started_utc": started.isoformat(),
        "finished_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "label": opts.label,
        "machine": machine_info(),
        "runs": opts.runs,
        "warmup": opts.warmup,
        "interleaved": True,
        "popen_floor_ms": popen_floor_ms,
        "measurement": {
            "wall_ms": "time.perf_counter around Popen..wait4 of the subject process",
            "cpu_ms": "ru_utime + ru_stime from wait4 (user + system, subject process and its waited children)",
            "peak_rss_bytes": "ru_maxrss from wait4 (the subject process itself; same source as /usr/bin/time -l)",
            "peak_rss_tree_bytes": "psutil sampling of the whole process tree at 5 ms" if psutil else None,
            "ttfr_ms": "first REQ logged by fixture-server minus the subject's spawn time (same wall clock; includes Popen overhead)",
            "ttfr_ms_adjusted": "ttfr_ms minus popen_floor_ms (for S-cold-fetch only)",
            "engine_overhead_ms": "from the search query's --json meta line (S-search-* scenarios only)",
        },
        "subjects": {name: {"binary": str(s["binary"]), "version": s["version"], "available": s["available"]}
                     for name, s in subjects.items()},
        "results": all_rows,
    }
    with open(out_path, "w") as f:
        json.dump(report, f, indent=2, default=str)
    print(markdown_table(all_rows))
    print(f"\nresults: {out_path}")
    failed = [r for r in all_rows if r["status"] == "failed"]
    for r in failed:
        ff = r.get("first_failure") or {}
        print(f"\n{r['scenario']} / {r['subject']} failed: {ff.get('failure')}\n{ff.get('stderr_tail', '')}",
              file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
