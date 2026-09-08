#!/usr/bin/env python3
"""WPT runner skeleton (docs/spec/15-testing-and-benchmarks.md §6).

Subcommands:
  checkout             clone web-platform-tests/wpt shallowly at the pinned commit
  manifest             (re)generate the WPT manifest for the checkout
  list --dir D         print the testharness URLs of a directory
  serve                run `./wpt serve` in the foreground (Ctrl-C stops it)
  run --product P --dir D [--dir D2 ...] [--max-minutes N] [--jobs N]
                       run directories against a product and write
                       wpt/results/<product>/<dir>.json
  update-expectations --product P --dir D
                       rewrite wpt/expectations/<product>/<dir>/*.ini from the
                       last results file

Products:
  chromium             Playwright's Chromium driven by tools/driver-chromium.mjs
  tentabrowser         reserved: will drive `tentabrowser serve` (CDP); P0 only
                       defines the driver contract (see Driver below)

Only the Python standard library is used. Node dependencies live in
wpt/tools/ (`npm install` there once).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

WPT_DIR = Path(__file__).resolve().parent
CHECKOUT = WPT_DIR / "checkout"
TOOLS = WPT_DIR / "tools"
RESULTS = WPT_DIR / "results"
EXPECTATIONS = WPT_DIR / "expectations"
MANIFEST = CHECKOUT / "MANIFEST.json"
CONFIG = WPT_DIR / "config.toml"

HOSTS = ["web-platform.test", "not-web-platform.test"]
SUBDOMAINS = ["www", "www1", "www2", "xn--n8j6ds53lwwkrqhv28a", "xn--lve-6lad"]


def load_config() -> dict:
    with CONFIG.open("rb") as handle:
        return tomllib.load(handle)


def log(message: str) -> None:
    print(f"[wpt] {message}", file=sys.stderr, flush=True)


# --- checkout and manifest ---------------------------------------------------


def ensure_checkout(config: dict) -> None:
    commit = config["wpt"]["commit"]
    repository = config["wpt"]["repository"]
    if (CHECKOUT / ".git").is_dir():
        head = subprocess.run(
            ["git", "-C", str(CHECKOUT), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
        ).stdout.strip()
        if head == commit:
            return
        log(f"checkout is at {head[:12]}, moving to pinned {commit[:12]}")
    else:
        CHECKOUT.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init", "-q"], cwd=CHECKOUT, check=True)
        subprocess.run(["git", "remote", "add", "origin", repository], cwd=CHECKOUT, check=True)
    log(f"fetching {repository} at {commit} (shallow)")
    subprocess.run(["git", "fetch", "-q", "--depth", "1", "origin", commit], cwd=CHECKOUT, check=True)
    subprocess.run(["git", "checkout", "-q", "FETCH_HEAD"], cwd=CHECKOUT, check=True)
    if MANIFEST.exists():
        MANIFEST.unlink()


def ensure_manifest() -> dict:
    if not MANIFEST.exists():
        log("generating MANIFEST.json (about a minute on first run)")
        subprocess.run(["./wpt", "manifest", "--no-download", "-p", str(MANIFEST)], cwd=CHECKOUT, check=True)
    with MANIFEST.open("rb") as handle:
        return json.load(handle)


@dataclass
class TestCase:
    url: str
    timeout_ms: int


def list_tests(manifest: dict, directory: str, config: dict) -> list[TestCase]:
    """Testharness tests below `directory`, with their variants and timeouts."""
    runner = config["runner"]
    node = manifest["items"].get("testharness", {})
    for part in directory.strip("/").split("/"):
        node = node.get(part)
        if node is None:
            return []
    tests: list[TestCase] = []

    def walk(subtree: dict, prefix: str) -> None:
        for name, entry in subtree.items():
            path = f"{prefix}/{name}"
            if isinstance(entry, dict):
                walk(entry, path)
                continue
            for variant in entry[1:]:
                url, props = variant[0], variant[1] if len(variant) > 1 else {}
                timeout = runner["long_timeout"] if props.get("timeout") == "long" else runner["timeout"]
                url = url or path
                if not url.startswith("/"):
                    url = "/" + url
                tests.append(TestCase(url=url, timeout_ms=timeout * 1000))

    walk(node, "/" + directory.strip("/"))
    tests.sort(key=lambda t: t.url)
    return tests


# --- server ------------------------------------------------------------------


def hosts_file_configured() -> bool:
    try:
        return all(socket.gethostbyname(host) == "127.0.0.1" for host in HOSTS)
    except socket.gaierror:
        return False


def hosts_file_entries() -> str:
    names = []
    for host in HOSTS:
        names.append(host)
        names.extend(f"{sub}.{host}" for sub in SUBDOMAINS)
    return "\n".join(f"127.0.0.1 {name}" for name in names)


def port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


class Server:
    """`./wpt serve` with wpt/serve-config.json, bound to 127.0.0.1."""

    def __init__(self, config: dict, log_path: Path):
        self.serve_config = WPT_DIR / config["wpt"]["serve_config"]
        with self.serve_config.open("rb") as handle:
            ports = json.load(handle)["ports"]
        self.http_port = ports["http"][0]
        self.https_port = ports["https"][0]
        self.log_path = log_path
        self.process: subprocess.Popen | None = None

    @property
    def base(self) -> str:
        return f"http://web-platform.test:{self.http_port}"

    def __enter__(self) -> "Server":
        if port_open(self.http_port):
            raise SystemExit(f"port {self.http_port} is already in use; is another wpt serve running?")
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        self.log_handle = self.log_path.open("w")
        self.process = subprocess.Popen(
            ["./wpt", "serve", "--config", str(self.serve_config)],
            cwd=CHECKOUT,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise SystemExit(f"wpt serve exited early; see {self.log_path}")
            if port_open(self.http_port) and port_open(self.https_port):
                time.sleep(1.0)
                log(f"wpt serve ready on {self.base} (log: {self.log_path})")
                return self
            time.sleep(0.5)
        raise SystemExit(f"wpt serve did not come up within 120 s; see {self.log_path}")

    def __exit__(self, *_exc) -> None:
        if self.process and self.process.poll() is None:
            os.killpg(os.getpgid(self.process.pid), signal.SIGTERM)
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(os.getpgid(self.process.pid), signal.SIGKILL)
        self.log_handle.close()


# --- drivers -----------------------------------------------------------------


class Driver:
    """Contract every product driver implements.

    `run(plan)` receives
        {"base": "http://web-platform.test:8000", "resolver_rules": bool,
         "jobs": int, "deadline_epoch_ms": int | None,
         "tests": [{"url": "/dir/test.html", "timeout_ms": int}, ...]}
    and returns
        {"browser": {"name": str, "version": str},
         "tests": [{"test": url, "status": "OK|ERROR|TIMEOUT|CRASH|PRECONDITION_FAILED|NOTRUN",
                    "message": str | None, "duration_ms": int,
                    "subtests": [{"name": str, "status": "PASS|FAIL|TIMEOUT|NOTRUN|PRECONDITION_FAILED",
                                  "message": str | None}]}]}

    The driver must (a) load every test URL in a fresh page/context, (b) serve
    the completion hook in place of `/resources/testharnessreport.js` (or, for
    `tentabrowser`, inject it through the `Tenta.wptResult` binding), (c) accept
    self-signed certificates and private-network access, and (d) resolve the
    `web-platform.test` hosts to loopback when `resolver_rules` is true.
    """

    name = "abstract"

    def run(self, plan: dict) -> dict:
        raise NotImplementedError


class ChromiumDriver(Driver):
    name = "chromium"

    def __init__(self, channel: str | None, executable: str | None):
        self.channel = channel
        self.executable = executable

    def run(self, plan: dict) -> dict:
        if not (TOOLS / "node_modules" / "playwright").is_dir():
            raise SystemExit(f"run `npm install` in {TOOLS} first")
        if self.channel:
            plan["channel"] = self.channel
        if self.executable:
            plan["executable"] = self.executable
        plan_path = RESULTS / ".plan.json"
        out_path = RESULTS / ".driver-results.json"
        plan_path.write_text(json.dumps(plan))
        subprocess.run(["node", str(TOOLS / "driver-chromium.mjs"), str(plan_path), str(out_path)], check=True)
        with out_path.open("rb") as handle:
            return json.load(handle)


class TentaBrowserDriver(Driver):
    name = "tentabrowser"

    def run(self, plan: dict) -> dict:
        raise SystemExit(
            "the tentabrowser product is not implemented yet: it will start `tentabrowser serve`, "
            "connect over CDP and inject the completion hook through the `Tenta.wptResult` binding"
        )


def make_driver(args: argparse.Namespace) -> Driver:
    if args.product == "chromium":
        return ChromiumDriver(args.channel, args.executable)
    if args.product == "tentabrowser":
        return TentaBrowserDriver()
    raise SystemExit(f"unknown product {args.product}")


# --- expectations ------------------------------------------------------------


@dataclass
class Expectation:
    harness: str = "OK"
    subtests: dict[str, str] = field(default_factory=dict)


def expectation_path(product: str, url: str) -> Path:
    return EXPECTATIONS / product / (url.lstrip("/") + ".ini")


def parse_expectation(path: Path) -> Expectation:
    """Minimal parser for the wptrunner-like format documented in expectations/README.md."""
    expectation = Expectation()
    current: str | None = None
    depth = 0
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            depth = len(raw) - len(raw.lstrip())
            current = None if depth == 0 else line[1:-1].replace("\\]", "]")
            continue
        match = re.match(r"expected\s*:\s*(\w+)", line)
        if match:
            if current is None:
                expectation.harness = match.group(1)
            else:
                expectation.subtests[current] = match.group(1)
    return expectation


def load_expectations(product: str, tests: list[dict]) -> dict[str, Expectation]:
    loaded = {}
    for test in tests:
        path = expectation_path(product, test["test"])
        if path.exists():
            loaded[test["test"]] = parse_expectation(path)
    return loaded


def compare(tests: list[dict], expectations: dict[str, Expectation]) -> list[str]:
    unexpected = []
    for test in tests:
        expectation = expectations.get(test["test"], Expectation())
        if test["status"] != expectation.harness:
            unexpected.append(f"{test['test']}: harness {test['status']} (expected {expectation.harness})")
        for subtest in test["subtests"]:
            expected = expectation.subtests.get(subtest["name"], "PASS")
            if subtest["status"] != expected:
                unexpected.append(f"{test['test']} [{subtest['name']}]: {subtest['status']} (expected {expected})")
    return unexpected


def write_expectations(product: str, tests: list[dict]) -> int:
    written = 0
    for test in tests:
        path = expectation_path(product, test["test"])
        failing = [s for s in test["subtests"] if s["status"] != "PASS"]
        if test["status"] == "OK" and not failing:
            if path.exists():
                path.unlink()
            continue
        lines = [f"[{Path(test['test']).name}]"]
        if test["status"] != "OK":
            lines.append(f"  expected: {test['status']}")
        for subtest in failing:
            lines.append("")
            lines.append(f"  [{subtest['name'].replace(']', '\\]')}]")
            lines.append(f"    expected: {subtest['status']}")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n")
        written += 1
    return written


# --- commands ----------------------------------------------------------------


def results_path(product: str, directory: str) -> Path:
    return RESULTS / product / (directory.strip("/") + ".json")


def summarize(tests: list[dict]) -> dict:
    subtests = [s for t in tests for s in t["subtests"]]
    passed = sum(1 for s in subtests if s["status"] == "PASS")
    return {
        "tests": len(tests),
        "tests_ok": sum(1 for t in tests if t["status"] == "OK"),
        "tests_not_run": sum(1 for t in tests if t["status"] == "NOTRUN"),
        "subtests": len(subtests),
        "subtests_pass": passed,
        "subtest_pass_pct": round(100.0 * passed / len(subtests), 2) if subtests else 0.0,
    }


def cmd_run(args: argparse.Namespace) -> int:
    config = load_config()
    ensure_checkout(config)
    manifest = ensure_manifest()
    driver = make_driver(args)
    resolver_rules = not hosts_file_configured()
    if resolver_rules:
        log("hosts file does not map web-platform.test to 127.0.0.1; using browser resolver rules")
    deadline = int((time.time() + args.max_minutes * 60) * 1000) if args.max_minutes else None
    exit_code = 0
    with Server(config, RESULTS / f"{args.product}-serve.log") as server:
        for directory in args.dir:
            tests = list_tests(manifest, directory, config)
            if not tests:
                log(f"no testharness tests under {directory}")
                continue
            log(f"{directory}: {len(tests)} tests, {args.jobs} jobs")
            started = time.time()
            plan = {
                "base": server.base,
                "resolver_rules": resolver_rules,
                "jobs": args.jobs,
                "deadline_epoch_ms": deadline,
                "tests": [{"url": t.url, "timeout_ms": t.timeout_ms} for t in tests],
            }
            outcome = driver.run(plan)
            unexpected = compare(outcome["tests"], load_expectations(args.product, outcome["tests"]))
            summary = summarize(outcome["tests"])
            report = {
                "product": args.product,
                "browser": outcome["browser"],
                "wpt_commit": config["wpt"]["commit"],
                "directory": directory,
                "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
                "duration_s": round(time.time() - started, 1),
                "summary": summary,
                "unexpected": unexpected,
                "tests": outcome["tests"],
            }
            path = results_path(args.product, directory)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(report, indent=2) + "\n")
            print(
                f"{directory}: {summary['subtests_pass']}/{summary['subtests']} subtests pass "
                f"({summary['subtest_pass_pct']}%), {summary['tests_ok']}/{summary['tests']} files OK, "
                f"{summary['tests_not_run']} not run, {len(unexpected)} unexpected -> {path.relative_to(WPT_DIR.parent)}"
            )
            for line in unexpected[:50]:
                print(f"  unexpected: {line}")
            if len(unexpected) > 50:
                print(f"  ... {len(unexpected) - 50} more in the results file")
            if unexpected:
                exit_code = 1
    return exit_code


def cmd_list(args: argparse.Namespace) -> int:
    config = load_config()
    ensure_checkout(config)
    manifest = ensure_manifest()
    for directory in args.dir:
        for test in list_tests(manifest, directory, config):
            print(f"{test.url}\t{test.timeout_ms}")
    return 0


def cmd_serve(args: argparse.Namespace) -> int:
    config = load_config()
    ensure_checkout(config)
    if not hosts_file_configured():
        log("hosts file does not map web-platform.test; add these entries to reach the server by name:\n" + hosts_file_entries())
    with Server(config, RESULTS / "serve.log") as server:
        log(f"serving at {server.base}; press Ctrl-C to stop")
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            pass
    return 0


def cmd_update_expectations(args: argparse.Namespace) -> int:
    for directory in args.dir:
        path = results_path(args.product, directory)
        if not path.exists():
            raise SystemExit(f"no results at {path}; run the directory first")
        with path.open("rb") as handle:
            report = json.load(handle)
        written = write_expectations(args.product, report["tests"])
        print(f"{directory}: wrote {written} expectation files under {EXPECTATIONS / args.product}")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("checkout")
    sub.add_parser("manifest")
    sub.add_parser("serve")
    for name in ("list", "run", "update-expectations"):
        p = sub.add_parser(name)
        p.add_argument("--dir", action="append", required=True, help="WPT directory, e.g. dom/nodes (repeatable)")
        if name != "list":
            p.add_argument("--product", choices=["chromium", "tentabrowser"], required=True)
        if name == "run":
            p.add_argument("--jobs", type=int, default=load_config()["runner"]["jobs"])
            p.add_argument("--max-minutes", type=float, default=0, help="stop starting new tests after N minutes")
            p.add_argument("--channel", help="Playwright channel for chromium (e.g. chrome)")
            p.add_argument("--executable", help="browser executable for chromium")
    args = parser.parse_args(argv)
    if args.command == "checkout":
        ensure_checkout(load_config())
        return 0
    if args.command == "manifest":
        ensure_checkout(load_config())
        if MANIFEST.exists():
            MANIFEST.unlink()
        ensure_manifest()
        return 0
    if shutil.which("git") is None:
        raise SystemExit("git is required")
    return {"list": cmd_list, "run": cmd_run, "serve": cmd_serve, "update-expectations": cmd_update_expectations}[
        args.command
    ](args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
