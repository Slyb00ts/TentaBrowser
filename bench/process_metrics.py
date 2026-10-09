"""Process measurement shared by the real-engine benchmark drivers.

Payload validation belongs to each scenario. Process wall time and peak RSS
are distinct from the in-process timings returned by the subject executable.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import os
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable
from pathlib import Path

from run import ROOT, rss_bytes


def command_output(*args: str) -> str | None:
    try:
        result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def binary_info(path: Path) -> dict:
    digest = hashlib.sha256()
    with path.open("rb") as binary:
        for block in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(block)
    stat = path.stat()
    return {
        "path": str(path),
        "sha256": digest.hexdigest(),
        "size_bytes": stat.st_size,
        "mtime_utc": dt.datetime.fromtimestamp(stat.st_mtime, dt.timezone.utc).isoformat(),
    }


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_json_process(
    binary: Path,
    timeout_s: float,
    validate_payload: Callable[[object], dict],
    command_args: tuple[str, ...] = (),
) -> dict:
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        proc = subprocess.Popen(
            [str(binary), *command_args], cwd=ROOT, stdout=stdout, stderr=stderr
        )
        timed_out = threading.Event()

        def kill_on_timeout() -> None:
            timed_out.set()
            try:
                proc.kill()
            except ProcessLookupError:
                pass

        timer = threading.Timer(timeout_s, kill_on_timeout)
        timer.start()
        try:
            if hasattr(os, "wait4"):
                _, status, usage = os.wait4(proc.pid, 0)
                proc.returncode = os.waitstatus_to_exitcode(status)
                peak_rss_bytes = rss_bytes(usage.ru_maxrss)
                cpu_ms = (usage.ru_utime + usage.ru_stime) * 1000
            else:  # pragma: no cover - Windows lacks wait4
                proc.wait()
                peak_rss_bytes = None
                cpu_ms = None
        finally:
            timer.cancel()
        wall_ms = (time.perf_counter_ns() - started) / 1_000_000
        stdout.seek(0)
        stderr.seek(0)
        output = stdout.read().decode("utf-8", errors="replace").strip()
        error = stderr.read().decode("utf-8", errors="replace").strip()

    if timed_out.is_set():
        raise RuntimeError(f"{binary} exceeded {timeout_s:g} seconds; stderr: {error[-1000:]}")
    if proc.returncode != 0:
        raise RuntimeError(f"{binary} exited {proc.returncode}; stderr: {error[-1000:]}")
    try:
        payload = validate_payload(json.loads(output))
    except (json.JSONDecodeError, ValueError) as exc:
        raise RuntimeError(f"invalid output from {binary}: {exc}; stdout: {output[-1000:]}") from exc
    return {
        **payload,
        "wall_ms": wall_ms,
        "cpu_ms": cpu_ms,
        "peak_rss_bytes": peak_rss_bytes,
    }
