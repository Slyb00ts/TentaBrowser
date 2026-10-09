#!/usr/bin/env python3
"""WPT runner (docs/spec/15-testing-and-benchmarks.md §6).

Subcommands:
  checkout             clone web-platform-tests/wpt shallowly at the pinned commit
  manifest             (re)generate the WPT manifest for the checkout
  list --dir D         print the testharness URLs of a directory
  serve                run `./wpt serve` in the foreground (Ctrl-C stops it)
  run --product P --dir D [--dir D2 ...] [--max-minutes N] [--jobs N]
                       run directories against a product and write
                       wpt/results/<product>/<dir>.json, then judge that run
                       against wpt/expectations/<product>/
  compare --product P --dir D
                       re-judge the stored results file for a directory; no
                       checkout, no server, no driver (the way to exercise the
                       judge and the exit-code contract)
  candidate --product P --dir D
                       write a CANDIDATE expectation tree under
                       wpt/results/<product>/candidate/ from the last results
                       file, plus a review sheet; the live tree is untouched
  promote --product P --dir D [--by NAME]
                       copy a reviewed candidate over wpt/expectations/ and
                       restamp wpt/expectations/<product>/BASELINE.toml

  update-expectations  DEPRECATED alias for `candidate`

Products:
  chromium             Playwright's Chromium driven by tools/driver-chromium.mjs
  tentabrowser         tenta-wpt-driver with the explicit V8 backend
  tentabrowser-qjs     the same driver with the explicit QuickJS backend

Expectations are a trust boundary, not a measurement. `candidate` writes machine
output under the gitignored wpt/results/; only an explicit `promote` moves a
reviewed tree into wpt/expectations/. Every non-PASS entry carries `# bucket:`
and `# reason:`, and a reason of `unreviewed` is judged exactly like a mismatch,
so generating an expectation tree never turns a run green on its own. The review
unit is a bucket in wpt/expectations/<product>/BUCKETS.toml, never the .ini tree.

Exit codes:
  0  comparable run, every result matches a reviewed expectation
  1  comparable run, results differ (regression, improvement or unreviewed entry)
  2  tooling or usage error (never a statement about the engine)
  3  NOT COMPARABLE: truncated run, baseline-stamp mismatch, or a configured
     directory that yielded zero tests
  4  `promote` refused

Only the Python standard library is used. Node dependencies live in
wpt/tools/ (`npm install` there once).
"""

from __future__ import annotations

import argparse
import hashlib
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
from typing import NoReturn

WPT_DIR = Path(__file__).resolve().parent
CHECKOUT = WPT_DIR / "checkout"
TOOLS = WPT_DIR / "tools"
RESULTS = WPT_DIR / "results"
EXPECTATIONS = WPT_DIR / "expectations"
MANIFEST = CHECKOUT / "MANIFEST.json"
CONFIG = WPT_DIR / "config.toml"

HOSTS = ["web-platform.test", "not-web-platform.test"]
SUBDOMAINS = ["www", "www1", "www2", "xn--n8j6ds53lwwkrqhv28a", "xn--lve-6lad"]

# The exit-code contract. `wpt/run.py`'s exit status has to mean "the engine
# changed", never "the instrument changed"; these names are the contract, and
# `docs/dev/wpt.md` and `wpt/README.md` spell out what each one claims.
EXIT_OK = 0
EXIT_DIFFERS = 1
EXIT_TOOLING = 2
EXIT_NOT_COMPARABLE = 3
EXIT_REFUSED = 4

HARNESS_STATUSES = ("OK", "ERROR", "TIMEOUT", "CRASH", "PRECONDITION_FAILED", "NOTRUN")
SUBTEST_STATUSES = ("PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED")

# The sentinel a freshly generated entry carries. It is deliberately a value a
# reviewer can mass-replace, which is why the ledger (BUCKETS.toml) and not
# `grep -c unreviewed` is the review unit.
UNREVIEWED = "unreviewed"


class ToolError(Exception):
    """A problem with the tool or its inputs (exit 2), never with the engine.

    `raise SystemExit("no driver binary")` used to exit 1, which is the code a
    genuine regression takes: "no driver built yet" masqueraded as "the engine
    regressed". Everything that is not a statement about the engine raises this
    instead, and `main` turns it into `EXIT_TOOLING`.
    """


def tool_error(message: str) -> NoReturn:
    raise ToolError(message)


def load_config() -> dict:
    with CONFIG.open("rb") as handle:
        return tomllib.load(handle)


def log(message: str) -> None:
    print(f"[wpt] {message}", file=sys.stderr, flush=True)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


@dataclass(frozen=True)
class Paths:
    """Where expectations, results and candidates live.

    Injectable so the pure parts (`judge`, `generate_candidate`, `promote`) can
    be tested against a temporary directory with no checkout, no server, no
    driver and no fixed port.
    """

    expectations: Path = EXPECTATIONS
    results: Path = RESULTS

    def expectation_root(self, product: str) -> Path:
        return self.expectations / product

    def expectation(self, product: str, url: str) -> Path:
        return self.expectation_root(product) / (url.lstrip("/") + ".ini")

    def results_file(self, product: str, directory: str) -> Path:
        return self.results / product / (directory.strip("/") + ".json")

    def candidate_root(self, product: str) -> Path:
        return self.results / product / "candidate"

    def candidate_dir(self, product: str, directory: str) -> Path:
        return self.candidate_root(product) / directory.strip("/")

    def review_sheet(self, product: str, directory: str) -> Path:
        return self.candidate_root(product) / (directory.strip("/") + ".review.txt")

    def proposed_ledger(self, product: str, directory: str) -> Path:
        return self.candidate_root(product) / (directory.strip("/") + ".BUCKETS.proposed.toml")

    def ledger(self, product: str) -> Path:
        return self.expectation_root(product) / "BUCKETS.toml"

    def baseline(self, product: str) -> Path:
        return self.expectation_root(product) / "BASELINE.toml"


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
            tool_error(f"port {self.http_port} is already in use; is another wpt serve running?")
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
                tool_error(f"wpt serve exited early; see {self.log_path}")
            if port_open(self.http_port) and port_open(self.https_port):
                time.sleep(1.0)
                log(f"wpt serve ready on {self.base} (log: {self.log_path})")
                return self
            time.sleep(0.5)
        tool_error(f"wpt serve did not come up within 120 s; see {self.log_path}")

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
    the completion hook in place of `/resources/testharnessreport.js` (`chromium`
    routes it in the driver process; `tentabrowser` answers it from inside its
    own context, with no binding involved), (c) accept self-signed certificates
    and private-network access, and (d) resolve the `web-platform.test` hosts to
    loopback when `resolver_rules` is true.
    """

    name = "abstract"

    def run(self, plan: dict) -> dict:
        raise NotImplementedError

    def identity(self) -> dict:
        """`{"path": str, "sha256": str}` for the artefact that produced a run.

        `browser.version` is the crate version, so it is constant across driver
        commits and cannot stamp a baseline. This can, and it is the field a
        comparability failure names when the instrument moved under a run.
        """
        return {"path": "", "sha256": ""}


class ChromiumDriver(Driver):
    name = "chromium"

    def __init__(self, channel: str | None, executable: str | None):
        self.channel = channel
        self.executable = executable

    def identity(self) -> dict:
        script = TOOLS / "driver-chromium.mjs"
        return {"path": str(script), "sha256": sha256_file(script) if script.is_file() else ""}

    def run(self, plan: dict) -> dict:
        if not (TOOLS / "node_modules" / "playwright").is_dir():
            tool_error(f"run `npm install` in {TOOLS} first")
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

    def __init__(self, driver: Path, backend: str = "v8"):
        self.driver = driver
        self.backend = backend

    def identity(self) -> dict:
        return {"path": str(self.driver), "sha256": sha256_file(self.driver), "backend": self.backend}

    def run(self, plan: dict) -> dict:
        plan["backend"] = self.backend
        plan_path = RESULTS / ".plan.json"
        out_path = RESULTS / ".driver-results.json"
        plan_path.write_text(json.dumps(plan))
        subprocess.run([str(self.driver), str(plan_path), str(out_path)], check=True)
        with out_path.open("rb") as handle:
            outcome = json.load(handle)
        actual = outcome.get("browser", {}).get("backend")
        if actual != self.backend:
            tool_error(f"driver reported backend {actual!r}, requested {self.backend!r}; rebuild the driver")
        return outcome


def checked_driver(path: Path, source: str) -> Path:
    """`path`, or a `ToolError` saying which `source` named it and why it is unusable."""
    if not path.is_file():
        tool_error(f"{source} is not a file: {path}")
    if not os.access(path, os.X_OK):
        tool_error(f"{source} is not executable: {path}")
    return path


def resolve_driver(explicit: str | None) -> Path:
    """Where the `tenta-wpt-driver` binary is.

    `--driver` wins over `$TENTA_WPT_DRIVER`, which wins over the package's own
    build outputs (`target/release`, then `target/debug`). Nothing shells out to
    `cargo run`: a driver that rebuilt itself mid-run would make the measured
    wall clock meaningless.
    """
    if explicit:
        return checked_driver(Path(explicit), "--driver")
    from_env = os.environ.get("TENTA_WPT_DRIVER")
    if from_env:
        return checked_driver(Path(from_env), "$TENTA_WPT_DRIVER")
    target = WPT_DIR.parent / "target"
    for candidate in (target / "release" / "tenta-wpt-driver", target / "debug" / "tenta-wpt-driver"):
        if candidate.is_file():
            return checked_driver(candidate, str(candidate))
    tool_error(
        "no tenta-wpt-driver binary found; build one with "
        "`cargo build --release -p tenta-browser --features v8,quickjs --bin tenta-wpt-driver`, "
        "or name it with --driver / $TENTA_WPT_DRIVER"
    )


def make_driver(args: argparse.Namespace) -> Driver:
    if args.product == "chromium":
        return ChromiumDriver(args.channel, args.executable)
    if args.product in ("tentabrowser", "tentabrowser-qjs"):
        backend = "quickjs" if args.product == "tentabrowser-qjs" else "v8"
        return TentaBrowserDriver(resolve_driver(args.driver), backend)
    tool_error(f"unknown product {args.product}")


# --- expectations ------------------------------------------------------------
#
# The model is (status, bucket, reason) at both levels. `status` is the only
# field with comparison semantics; `bucket` and `reason` are review metadata, so
# a bad bucket derivation can never make a run green or red by itself. What
# DOES make a run red is `reason = unreviewed`: generation alone never blesses.

BUCKETS_FILE = "BUCKETS.toml"
BASELINE_FILE = "BASELINE.toml"
BUCKET_STATUSES = ("accepted", "must-fix", "forbidden")
DRIVER_TIMEOUT_MARKER = "the completion hook did not arrive"
DEADLINE_MARKER = "run deadline reached"
DUPLICATE_NAMES_RE = re.compile(r"^\d+ duplicate test names")
MISSING_MEMBER_RE = re.compile(r"\(reading '([^']+)'\)")
NOT_A_FUNCTION_RE = re.compile(r"([\w$.\[\]]+) is not a function")
NOT_DEFINED_RE = re.compile(r"([\w$.]+) is not defined")
ASSERT_IMPLEMENTS_RE = re.compile(r"assert_implements(?:_optional)?\s*:\s*(.+)")
ASSERT_ANY_RE = re.compile(r"assert_\w+\s*:")

BASELINE_FIELDS = (
    "wpt_commit",
    "driver_sha256",
    "plan_digest",
    "timeout_s",
    "long_timeout_s",
    "blessed_at",
    "blessed_by",
    "subtests",
    "subtests_pass",
    "files",
)


@dataclass
class Expected:
    """One reviewed expectation: a status plus the review that justifies it."""

    status: str
    bucket: str = ""
    reason: str = ""

    @property
    def reviewed(self) -> bool:
        """`unreviewed` is a sentinel, not a review: it judges like a mismatch."""
        return bool(self.reason) and self.reason != UNREVIEWED


@dataclass
class Expectation:
    harness: Expected = field(default_factory=lambda: Expected("OK"))
    subtests: dict[str, Expected] = field(default_factory=dict)

    def entries(self) -> list[tuple[str, Expected]]:
        """Every entry of this file as `(label, Expected)`; `""` labels the harness."""
        return [("", self.harness)] + sorted(self.subtests.items())


def expectation_path(product: str, url: str, paths: Paths | None = None) -> Path:
    return (paths or Paths()).expectation(product, url)


def parse_expectation(path: Path) -> Expectation:
    """Minimal parser for the wptrunner-like format documented in expectations/README.md.

    `# bucket:` and `# reason:` comments attach to the section they sit in, which
    is why the writer puts them directly above the `expected:` line they justify.
    A comment after `expected:` also lands, at the section's end.
    """
    expectation = Expectation()
    current: str | None = None
    pending_bucket = ""
    pending_reason = ""
    written: Expected | None = None

    def flush() -> None:
        nonlocal pending_bucket, pending_reason, written
        if written is not None:
            if pending_bucket:
                written.bucket = pending_bucket
            if pending_reason:
                written.reason = pending_reason
        pending_bucket = ""
        pending_reason = ""
        written = None

    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith("#"):
            match = re.match(r"#\s*bucket\s*:\s*(.*)", line)
            if match:
                pending_bucket = match.group(1).strip()
                continue
            match = re.match(r"#\s*reason\s*:\s*(.*)", line)
            if match:
                pending_reason = match.group(1).strip()
            continue
        if line.startswith("[") and line.endswith("]"):
            flush()
            depth = len(raw) - len(raw.lstrip())
            current = None if depth == 0 else line[1:-1].replace("\\]", "]")
            continue
        match = re.match(r"expected\s*:\s*(\w+)", line)
        if match:
            written = Expected(status=match.group(1), bucket=pending_bucket, reason=pending_reason)
            pending_bucket = ""
            pending_reason = ""
            if current is None:
                expectation.harness = written
            else:
                expectation.subtests[current] = written
            continue
    flush()
    return expectation


def render_expectation(url: str, harness: Expected, subtests: list[tuple[str, Expected]], stamp: str) -> str:
    """The on-disk form of one `.ini`, comments included."""
    lines = [f"# generated at {stamp}; review the bucket in BUCKETS.toml", f"[{Path(url).name}]"]
    if harness.status != "OK":
        if harness.bucket:
            lines.append(f"  # bucket: {harness.bucket}")
        if harness.reason:
            lines.append(f"  # reason: {harness.reason}")
        lines.append(f"  expected: {harness.status}")
    for name, record in subtests:
        lines.append("")
        lines.append(f"  [{name.replace(']', '\\]')}]")
        if record.bucket:
            lines.append(f"    # bucket: {record.bucket}")
        if record.reason:
            lines.append(f"    # reason: {record.reason}")
        lines.append(f"    expected: {record.status}")
    return "\n".join(lines) + "\n"


def load_expectation_tree(
    product: str,
    paths: Paths | None = None,
    root: Path | None = None,
    prefix: str = "",
) -> dict[str, Expectation]:
    """Every `.ini` under a root, keyed by test URL.

    The tree and not the results is the domain: judging needs to see an
    expectation file whose test no longer exists at all. `prefix` is the WPT
    directory a candidate root stands for, so a candidate tree keys to the same
    URLs as the live one.
    """
    base = root if root is not None else (paths or Paths()).expectation_root(product)
    tree: dict[str, Expectation] = {}
    if not base.is_dir():
        return tree
    for path in sorted(base.rglob("*.ini")):
        relative = path.relative_to(base).as_posix()[: -len(".ini")]
        tree[f"{prefix.rstrip('/')}/{relative}"] = parse_expectation(path)
    return tree


def candidate_relative(directory: str, url: str) -> str:
    """Where a test's candidate `.ini` sits, relative to the candidate root.

    The candidate root already is the directory (`wpt/results/<product>/candidate/<dir>/`),
    so the tree below it mirrors the WPT tree below `<dir>`, exactly as the live
    tree does.
    """
    relative = url.lstrip("/")
    prefix = directory.strip("/") + "/"
    if relative.startswith(prefix):
        relative = relative[len(prefix) :]
    return relative + ".ini"


def load_expectations(product: str, tests: list[dict], paths: Paths | None = None) -> dict[str, Expectation]:
    """The subset of the tree the run names (kept for callers that want it)."""
    tree = load_expectation_tree(product, paths)
    return {test["test"]: tree[test["test"]] for test in tests if test["test"] in tree}


# --- buckets -----------------------------------------------------------------
#
# Derivation is mechanical and message-driven, never a message-equality table:
# a message that gains a character upstream must not create a new bucket. The
# fallback is a per-file bucket, which is where the entries that actually cost
# review time land.


def normalise_message(message: str | None) -> str:
    """One line, collapsed whitespace, first 60 characters: a key, not a message."""
    return " ".join((message or "").split())[:60]


def harness_bucket(test: dict) -> str:
    status = test["status"]
    message = test.get("message") or ""
    if status == "OK":
        return "harness-ok"
    if status == "ERROR":
        return f"harness-error:{normalise_message(message)}"
    if status == "TIMEOUT":
        return "timeout-harness-report" if test.get("subtests") else "timeout-no-subtests"
    if status == "CRASH":
        return "crash"
    if status == "NOTRUN":
        return "harness-notrun" if DEADLINE_MARKER in message else "harness-notrun-unexplained"
    if status == "PRECONDITION_FAILED":
        return "harness-precondition-failed"
    return f"harness-unknown:{status}"


def subtest_bucket(test: dict, subtest: dict, parent: str) -> str:
    status = subtest["status"]
    message = subtest.get("message") or ""
    match = MISSING_MEMBER_RE.search(message)
    if match:
        return f"missing-member {match.group(1)}"
    match = NOT_A_FUNCTION_RE.search(message)
    if match:
        return f"missing-function {match.group(1)}"
    match = NOT_DEFINED_RE.search(message)
    if match:
        return f"missing-global {match.group(1)}"
    if "Illegal constructor" in message:
        return "illegal-constructor"
    match = ASSERT_IMPLEMENTS_RE.search(message)
    if match:
        return f"unsupported-feature {match.group(1).strip()}"
    if status == "NOTRUN":
        return f"notrun-after-{parent}"
    if status == "TIMEOUT":
        return "timeout-subtest"
    if status == "PRECONDITION_FAILED":
        return f"unsupported-feature {normalise_message(message) or '(no message)'}"
    if ASSERT_ANY_RE.search(message):
        return f"assert-mismatch:{Path(test['test']).name}"
    return f"per-file:{Path(test['test']).name}"


def test_buckets(test: dict) -> tuple[str, dict[str, str]]:
    """`(harness bucket, {subtest name: bucket})` for one result."""
    parent = harness_bucket(test)
    return parent, {s["name"]: subtest_bucket(test, s, parent) for s in test.get("subtests", [])}


def bucket_counts(tests: list[dict]) -> dict[str, int]:
    """Every non-pass entry of a run, counted by bucket.

    The harness entry counts once when it is not `OK`; each non-`PASS` subtest
    counts once. This is the number the ledger blesses and the number a run is
    compared against, in both directions: growth is a new defect, shrinkage is
    an improvement that has to be re-blessed rather than left stale.
    """
    counts: dict[str, int] = {}
    for test in tests:
        parent, subtests = test_buckets(test)
        if test["status"] != "OK":
            counts[parent] = counts.get(parent, 0) + 1
        for bucket in subtests.values():
            counts[bucket] = counts.get(bucket, 0) + 1
    return counts


def entry_count(tests: list[dict]) -> int:
    return sum(len([s for s in t.get("subtests", []) if s["status"] != "PASS"]) + (1 if t["status"] != "OK" else 0) for t in tests)


# --- the ledger --------------------------------------------------------------


@dataclass
class BucketRow:
    """One review unit: a bucket, who owns it, and how many entries it had."""

    key: str
    label: str = ""
    issue: str = ""
    status: str = "must-fix"
    entries: int = 0


def load_ledger(path: Path) -> dict[str, BucketRow]:
    if not path.is_file():
        return {}
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    rows: dict[str, BucketRow] = {}
    for raw in document.get("bucket", []):
        key = str(raw.get("key", ""))
        if not key:
            tool_error(f"{path}: a [[bucket]] row has no key")
        status = str(raw.get("status", "must-fix"))
        if status not in BUCKET_STATUSES:
            tool_error(f"{path}: bucket {key!r} has status {status!r}; expected one of {', '.join(BUCKET_STATUSES)}")
        rows[key] = BucketRow(
            key=key,
            label=str(raw.get("label", "")),
            issue=str(raw.get("issue", "")),
            status=status,
            entries=int(raw.get("entries", 0)),
        )
    return rows


def render_ledger(rows: dict[str, BucketRow]) -> str:
    lines = [
        "# The review unit for wpt/expectations/<product>/: one row per bucket.",
        "# `entries` is the count blessed for the directory set this ledger was",
        "# reviewed over; a run whose count differs (either way) is red and has to",
        "# be re-blessed by editing this file. `status` is accepted | must-fix |",
        "# forbidden; `promote` refuses `forbidden` outright and has no override.",
        "",
    ]
    for key in sorted(rows):
        row = rows[key]
        lines.append("[[bucket]]")
        lines.append(f"key = {json.dumps(row.key)}")
        lines.append(f"label = {json.dumps(row.label)}")
        lines.append(f"issue = {json.dumps(row.issue)}")
        lines.append(f"status = {json.dumps(row.status)}")
        lines.append(f"entries = {row.entries}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def proposed_ledger(run_buckets: dict[str, int], ledger: dict[str, BucketRow]) -> dict[str, BucketRow]:
    """The rows a reviewer should end up with for this run's buckets.

    An existing row keeps its label, issue and status -- the review act is the
    human's, not the generator's -- and a new bucket arrives as `must-fix` with
    no issue, which is what makes it a work order rather than a rubber stamp.
    """
    rows: dict[str, BucketRow] = {}
    for key, count in run_buckets.items():
        existing = ledger.get(key)
        if existing is not None:
            rows[key] = BucketRow(existing.key, existing.label, existing.issue, existing.status, count)
        else:
            rows[key] = BucketRow(key=key, label="", issue="", status="must-fix", entries=count)
    return rows


# --- never blessable ---------------------------------------------------------


def never_blessable(
    test: dict,
    *,
    reason: str = "",
    ledger_status: str | None = None,
    control_subtests: int | None = None,
) -> str | None:
    """Why this shape may not become an expectation, or `None` when it may.

    Four shapes, each of which would absorb a nameable defect behind a
    mechanical `expected:` line. `control_subtests` is the Chromium control's
    subtest count for the same URL, when one exists.
    """
    status = test["status"]
    message = test.get("message") or ""
    if status == "CRASH":
        return "CRASH is never blessable: a crashed page reports nothing about the engine"
    if status == "NOTRUN" and DEADLINE_MARKER in message:
        return (
            "harness NOTRUN (`run deadline reached`) is a truncation artefact: blessing it makes the "
            "next untruncated run report an unexpected pass forever"
        )
    if status == "TIMEOUT" and DRIVER_TIMEOUT_MARKER in message and not test.get("subtests") and (control_subtests or 0) > 0:
        return (
            f"driver-attributed TIMEOUT with no subtests while the chromium control reports "
            f"{control_subtests} subtests: a finding, not a driver artefact"
        )
    if status == "ERROR" and DUPLICATE_NAMES_RE.match(message.strip()):
        if not reason or reason == UNREVIEWED:
            return f"harness ERROR {message.strip()!r} needs a reason naming the interface-mapping defect it hides"
        if ledger_status != "must-fix":
            return f"harness ERROR {message.strip()!r} must sit in a `must-fix` ledger row (ledger says {ledger_status!r})"
    return None


# --- judging -----------------------------------------------------------------


@dataclass
class Issue:
    """One reason a run is not clean. `kind` is the finding, never the fix."""

    test: str
    kind: str
    entry: str = ""
    detail: str = ""

    def line(self) -> str:
        where = f"{self.test} [{self.entry}]" if self.entry else self.test
        return f"{where}: {self.detail}"


def judge(
    tests: list[dict],
    expectations: dict[str, Expectation],
    *,
    ledger: dict[str, BucketRow] | None = None,
    control: dict[str, int] | None = None,
) -> list[Issue]:
    """Every reason this run differs from the reviewed expectations.

    Pure over (results, expectation tree, ledger, control): no driver, no
    server, no checkout. `compare` exists so this can be exercised on a stored
    results file, and `wpt/tests/test_run.py` so it can be exercised on
    synthetic ones.

    Judgement order per test: stale expectation, harness status, subtest status,
    missing review, never-blessable shape. Bucket counts come last, over the
    whole run.
    """
    issues: list[Issue] = []
    ledger = ledger or {}
    control = control or {}
    seen = set()

    for test in tests:
        url = test["test"]
        seen.add(url)
        expectation = expectations.get(url)
        expected_harness = expectation.harness if expectation is not None else Expected("OK")
        if test["status"] != expected_harness.status:
            issues.append(Issue(url, "harness", "", f"harness {test['status']} (expected {expected_harness.status})"))
        for subtest in test.get("subtests", []):
            expected = expectation.subtests.get(subtest["name"], Expected("PASS")) if expectation is not None else Expected("PASS")
            if subtest["status"] != expected.status:
                issues.append(
                    Issue(url, "subtest", subtest["name"], f"{subtest['status']} (expected {expected.status})")
                )
        if expected_harness.status != "OK" and not expected_harness.reviewed:
            issues.append(
                Issue(url, "unreviewed", "", f"harness {expected_harness.status} was never reviewed ({reason_text(expected_harness)})")
            )
        if expectation is not None:
            for name, record in expectation.subtests.items():
                if record.status != "PASS" and not record.reviewed:
                    issues.append(Issue(url, "unreviewed", name, f"{record.status} was never reviewed ({reason_text(record)})"))
        parent, _ = test_buckets(test)
        row = ledger.get(parent)
        why = never_blessable(
            test,
            reason=expected_harness.reason,
            ledger_status=row.status if row is not None else None,
            control_subtests=control.get(url),
        )
        if why is not None:
            issues.append(Issue(url, "never-blessable", "", why))

    for url in sorted(set(expectations) - seen):
        issues.append(Issue(url, "stale", "", "stale expectation: no such test in the results"))

    for key, count in sorted(bucket_counts(tests).items()):
        row = ledger.get(key)
        if row is None or count == row.entries:
            continue
        direction = "grew" if count > row.entries else "shrank"
        issues.append(
            Issue("(ledger)", "bucket", key, f"bucket {key!r} {direction}: run has {count} entries, the ledger blessed {row.entries}")
        )
    return issues


def reason_text(record: Expected) -> str:
    return f"reason: {record.reason}" if record.reason else "no `# reason:` at all"


# --- comparability -----------------------------------------------------------


def plan_digest(plan: dict) -> str:
    """sha256 of the sorted `url\\ttimeout_ms` lines a run was asked for."""
    lines = sorted(f"{t['url']}\t{t['timeout_ms']}" for t in plan["tests"])
    return hashlib.sha256("\n".join(lines).encode()).hexdigest()


def plan_truncated(tests: list[dict], max_minutes: float | None) -> bool:
    """Whether the run stopped early, from the driver's own output.

    Two shapes, both of which make every later `unexpected` a statement about
    the clock rather than about the engine: the driver's own `run deadline
    reached` NOTRUN, and any harness NOTRUN at all under `--max-minutes`.
    """
    for test in tests:
        if test["status"] == "NOTRUN" and DEADLINE_MARKER in (test.get("message") or ""):
            return True
        if max_minutes and test["status"] == "NOTRUN":
            return True
    return False


def report_stamp(report: dict) -> dict[str, str]:
    return {
        "wpt_commit": str(report.get("wpt_commit", "")),
        "driver_sha256": str((report.get("driver") or {}).get("sha256", "")),
        "plan_digest": str(report.get("plan_digest", "")),
    }


def report_identity_problems(product: str, report: dict) -> list[str]:
    """Reject a misfiled or unlabelled report before it can bless another engine."""
    problems = []
    if report.get("product") != product:
        problems.append(f"product differs: requested {product!r}, report {report.get('product')!r}")
    backend = {"tentabrowser": "v8", "tentabrowser-qjs": "quickjs"}.get(product)
    if backend:
        for section in ("browser", "driver"):
            actual = (report.get(section) or {}).get("backend")
            if actual != backend:
                problems.append(f"{section}.backend differs: expected {backend!r}, report {actual!r}")
    return problems


def comparability(
    product: str,
    directory: str,
    report: dict,
    *,
    paths: Paths | None = None,
    baseline: dict[str, dict] | None = None,
) -> list[str]:
    """Why this run is not comparable to the blessed one, or `[]`.

    Names the field, because "not comparable" becoming the new "ignore the red"
    is the way this contract dies: the delta printed next to the pass percentage
    has to say which stamp moved.
    """
    paths = paths or Paths()
    problems = report_identity_problems(product, report)
    if report.get("truncated"):
        problems.append("the run was truncated (`--max-minutes` or a harness `run deadline reached`)")
    if baseline is None:
        baseline = load_baseline(paths.baseline(product))
    row = baseline.get(directory.strip("/"))
    if row:
        stamp = report_stamp(report)
        for field in ("wpt_commit", "driver_sha256", "plan_digest"):
            blessed = str(row.get(field, ""))
            if blessed and stamp[field] != blessed:
                problems.append(f"{field} differs: baseline {blessed} run {stamp[field] or '(missing)'}")
    return problems


def load_baseline(path: Path) -> dict[str, dict]:
    if not path.is_file():
        return {}
    with path.open("rb") as handle:
        return tomllib.load(handle)


def render_baseline(rows: dict[str, dict]) -> str:
    lines = [
        "# Provenance of the blessed expectations in this directory. A run whose",
        "# wpt_commit, driver_sha256 or plan_digest differs from the row for its",
        "# directory is NOT COMPARABLE (exit 3), not a regression.",
        "",
    ]
    for directory in sorted(rows):
        lines.append(f"[{json.dumps(directory)}]")
        for field_name in BASELINE_FIELDS:
            if field_name not in rows[directory]:
                continue
            value = rows[directory][field_name]
            lines.append(f"{field_name} = {json.dumps(value) if isinstance(value, str) else value}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def load_control(product: str, directory: str, paths: Paths | None = None) -> dict[str, int]:
    """URL -> subtest count from the Chromium control's stored run, when there is one."""
    paths = paths or Paths()
    path = paths.results_file(product, directory)
    if not path.is_file():
        return {}
    with path.open("rb") as handle:
        report = json.load(handle)
    return {test["test"]: len(test.get("subtests", [])) for test in report.get("tests", [])}


# --- candidate generation ----------------------------------------------------


@dataclass
class CandidateSummary:
    product: str
    directory: str
    root: Path
    review_sheet: Path
    proposal: Path
    stamp: str = ""
    files: int = 0
    files_clean: int = 0
    entries: int = 0
    unreviewed: int = 0
    dropped: int = 0
    buckets: dict[str, int] = field(default_factory=dict)
    new_buckets: list[str] = field(default_factory=list)
    never_blessable: list[tuple[str, str]] = field(default_factory=list)
    unreviewed_files: list[tuple[str, int]] = field(default_factory=list)


def candidate_stamp(report: dict) -> str:
    stamp = report_stamp(report)
    return f"{stamp['wpt_commit'] or '?'}/{stamp['driver_sha256'] or '?'}"


def generate_candidate(
    product: str,
    directory: str,
    report: dict,
    *,
    paths: Paths | None = None,
    stamp: str | None = None,
    ledger: dict[str, BucketRow] | None = None,
    control: dict[str, int] | None = None,
) -> CandidateSummary:
    """Write a candidate tree from the last results file. The live tree is untouched.

    REGENERATION IS A MERGE, not an overwrite: an entry whose status is unchanged
    and which exists in the live file re-emits verbatim, keeping its bucket and
    its reason; a new or changed entry emits `# reason: unreviewed`; an entry
    that no longer fails is dropped; a file with nothing left to expect has no
    candidate at all, so `promote` deletes it. After the first blessing a
    regeneration diff is only what actually changed.
    """
    paths = paths or Paths()
    directory = directory.strip("/")
    identity_problems = report_identity_problems(product, report)
    if identity_problems:
        tool_error("cannot generate candidate: " + "; ".join(identity_problems))
    root = paths.candidate_dir(product, directory)
    if root.exists():
        shutil.rmtree(root)
    if ledger is None:
        ledger = load_ledger(paths.ledger(product))
    if control is None:
        control = load_control("chromium", directory, paths)
    stamp = stamp or candidate_stamp(report)
    summary = CandidateSummary(
        product=product,
        directory=directory,
        root=root,
        review_sheet=paths.review_sheet(product, directory),
        proposal=paths.proposed_ledger(product, directory),
        stamp=stamp,
    )
    unreviewed_files: list[tuple[str, int]] = []

    for test in report.get("tests", []):
        url = test["test"]
        live = paths.expectation(product, url)
        old = parse_expectation(live) if live.is_file() else None
        failing = [s for s in test.get("subtests", []) if s["status"] != "PASS"]
        if test["status"] == "OK" and not failing:
            summary.files_clean += 1
            continue
        parent, _ = test_buckets(test)
        harness = Expected(test["status"])
        if test["status"] != "OK":
            previous = old.harness if old is not None else None
            if previous is not None and previous.status == test["status"] and previous.reason:
                harness.bucket, harness.reason = previous.bucket, previous.reason
            else:
                harness.bucket, harness.reason = parent, UNREVIEWED
        subtests: list[tuple[str, Expected]] = []
        for subtest in failing:
            previous = old.subtests.get(subtest["name"]) if old is not None else None
            if previous is not None and previous.status == subtest["status"] and previous.reason:
                record = Expected(subtest["status"], previous.bucket, previous.reason)
            else:
                record = Expected(subtest["status"], subtest_bucket(test, subtest, parent), UNREVIEWED)
            subtests.append((subtest["name"], record))
        if old is not None:
            remaining = {name for name, _ in subtests}
            summary.dropped += sum(1 for name in old.subtests if name not in remaining)
        target = root / candidate_relative(directory, url)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(render_expectation(url, harness, subtests, stamp))
        summary.files += 1
        summary.entries += len(subtests) + (1 if test["status"] != "OK" else 0)
        unreviewed_here = sum(1 for _, record in subtests if not record.reviewed) + (
            1 if test["status"] != "OK" and not harness.reviewed else 0
        )
        summary.unreviewed += unreviewed_here
        if unreviewed_here:
            unreviewed_files.append((url, unreviewed_here))

    summary.buckets = bucket_counts(report.get("tests", []))
    summary.new_buckets = sorted(key for key in summary.buckets if key not in ledger)
    for test in report.get("tests", []):
        parent, _ = test_buckets(test)
        row = ledger.get(parent)
        why = never_blessable(
            test,
            reason=_candidate_reason(root, directory, test["test"]),
            ledger_status=row.status if row is not None else None,
            control_subtests=control.get(test["test"]),
        )
        if why is not None:
            summary.never_blessable.append((test["test"], why))
    summary.unreviewed_files = unreviewed_files

    summary.review_sheet.parent.mkdir(parents=True, exist_ok=True)
    summary.review_sheet.write_text(render_review_sheet(summary, report, ledger, control))
    summary.proposal.write_text(render_ledger(proposed_ledger(summary.buckets, ledger)))
    log(
        f"{directory}: candidate {summary.files} files, {summary.entries} entries "
        f"({summary.unreviewed} unreviewed), {summary.files_clean} clean -> {summary.root}"
    )
    log(f"{directory}: review the buckets in {summary.review_sheet} (nothing is live until `promote`)")
    return summary


def _candidate_reason(root: Path, directory: str, url: str) -> str:
    path = root / candidate_relative(directory, url)
    return parse_expectation(path).harness.reason if path.is_file() else ""


def render_review_sheet(
    summary: CandidateSummary,
    report: dict,
    ledger: dict[str, BucketRow],
    control: dict[str, int],
) -> str:
    stamp = report_stamp(report)
    lines = [
        "WPT candidate review sheet",
        f"product:      {summary.product}",
        f"directory:    {summary.directory}",
        f"wpt commit:   {stamp['wpt_commit']}",
        f"driver:       {(report.get('driver') or {}).get('path', '?')} sha256 {stamp['driver_sha256']}",
        f"plan digest:  {stamp['plan_digest']}",
        f"truncated:    {'yes' if report.get('truncated') else 'no'}",
        "",
        f"This tree is a CANDIDATE at {summary.root}.",
        f"Nothing here is live until `promote` copies it to wpt/expectations/{summary.product}/{summary.directory}/.",
        "",
        "Review the BUCKETS below, not the .ini files. A blessed `dom` tree is ~265 files and",
        "~13 000 lines, and the whole P2 set projects to 30 000-60 000 entries: a tree that size",
        "cannot be reviewed entry by entry. The bucket table is the unit a human signs off on, and",
        "`entries` in the ledger is the count that re-blesses it.",
        "",
        f"files written:      {summary.files}",
        f"files now clean:    {summary.files_clean}   (no candidate file: `promote` deletes the live one)",
        f"entries:            {summary.entries}",
        f"entries unreviewed: {summary.unreviewed}   (each one judges exactly like a mismatch)",
        f"entries dropped:    {summary.dropped}   (were expected, no longer fail)",
        "",
        "== buckets (the review unit) ==",
    ]
    if not ledger:
        lines.append("  (no ledger yet: every bucket below is new, and `promote` refuses without a row)")
    header = f"{'entries':>7}  {'ledger':>7}  {'delta':>6}  {'status':<10}  key"
    lines.append(header)
    for key in sorted(summary.buckets, key=lambda k: (-summary.buckets[k], k)):
        row = ledger.get(key)
        blessed = row.entries if row is not None else 0
        status = row.status if row is not None else "(no row)"
        lines.append(f"{summary.buckets[key]:>7}  {blessed:>7}  {summary.buckets[key] - blessed:>+6}  {status:<10}  {key}")
    lines.append("")
    if summary.new_buckets:
        lines.append("== buckets with no ledger row (add a [[bucket]] row before promote) ==")
        for key in summary.new_buckets:
            lines.append(f"  {summary.buckets[key]:>6}  {key}")
    else:
        lines.append("== buckets with no ledger row: none ==")
    lines.append("")
    lines.append(f"A proposed ledger for exactly these buckets is at {summary.proposal}.")
    lines.append("Merge its rows into wpt/expectations/%s/BUCKETS.toml -- keep `entries` in step with the" % summary.product)
    lines.append("run, give every row a label and an issue link, and set `status` deliberately.")
    lines.append("")
    if summary.never_blessable:
        lines.append("== never blessable (promote refuses these, with no override) ==")
        for url, why in summary.never_blessable[:100]:
            lines.append(f"  {url}: {why}")
        if len(summary.never_blessable) > 100:
            lines.append(f"  ... and {len(summary.never_blessable) - 100} more")
    else:
        lines.append("== never blessable: none ==")
    lines.append("")
    lines.append("== files with entries that still need a reason ==")
    if summary.unreviewed_files:
        for url, count in summary.unreviewed_files[:200]:
            lines.append(f"  {url}  {count}")
        if len(summary.unreviewed_files) > 200:
            lines.append(f"  ... and {len(summary.unreviewed_files) - 200} more")
    else:
        lines.append("  (none)")
    lines.append("")
    return "\n".join(lines) + "\n"


# --- promotion ---------------------------------------------------------------


@dataclass
class PromoteResult:
    product: str
    directory: str
    exit_code: int = EXIT_OK
    refusals: list[str] = field(default_factory=list)
    files_written: int = 0
    files_removed: int = 0
    baseline: Path | None = None

    @property
    def refused(self) -> bool:
        return self.exit_code != EXIT_OK


UNREVIEWED_REFUSAL_CAP = 20


def review_promotion(
    product: str,
    directory: str,
    report: dict,
    *,
    paths: Paths | None = None,
    ledger: dict[str, BucketRow] | None = None,
    control: dict[str, int] | None = None,
    baseline: dict[str, dict] | None = None,
) -> list[str]:
    """The four refusals, in one list. Empty means the candidate may be promoted.

    1. not comparable: truncated, or a baseline stamp field that moved;
    2. a never-blessable shape (CRASH, deadline NOTRUN, driver TIMEOUT without
       subtests over a control that has some, duplicate-names without a reason
       and a `must-fix` row);
    3. a bucket problem: `forbidden`, no ledger row, count grew, count shrank;
    4. an entry whose reason is `unreviewed` or missing.
    """
    paths = paths or Paths()
    directory = directory.strip("/")
    if ledger is None:
        ledger = load_ledger(paths.ledger(product))
    if control is None:
        control = load_control("chromium", directory, paths)
    refusals: list[str] = []

    refusals.extend(comparability(product, directory, report, paths=paths, baseline=baseline))

    root = paths.candidate_dir(product, directory)
    if not root.is_dir():
        refusals.append(f"no candidate at {root}; run `candidate` first")
        return refusals
    candidate = load_expectation_tree(product, paths, root=root, prefix="/" + directory)
    stamps = {_candidate_header(path) for path in root.rglob("*.ini")}
    wanted = candidate_stamp(report)
    if stamps and stamps != {wanted}:
        refusals.append(
            f"the candidate was generated from another run; it carries {sorted(stamps)[0]!r}, this report stamps {wanted!r}"
        )

    for test in report.get("tests", []):
        url = test["test"]
        expectation = candidate.get(url)
        why = never_blessable(
            test,
            reason=expectation.harness.reason if expectation is not None else "",
            ledger_status=_row_status(ledger, test),
            control_subtests=control.get(url),
        )
        if why is not None:
            refusals.append(f"{url}: {why}")

    counts = bucket_counts(report.get("tests", []))
    for key in sorted(counts):
        row = ledger.get(key)
        if row is None:
            refusals.append(f"bucket {key!r} ({counts[key]} entries) has no row in {paths.ledger(product)}")
            continue
        if row.status == "forbidden":
            refusals.append(f"bucket {key!r} is `forbidden` in the ledger{': ' + row.label if row.label else ''}")
        elif counts[key] != row.entries:
            direction = "grew" if counts[key] > row.entries else "shrank"
            refusals.append(
                f"bucket {key!r} {direction}: this run has {counts[key]} entries, the ledger blessed {row.entries}; "
                f"re-bless it by editing `entries` in {paths.ledger(product)}"
            )

    unreviewed = 0
    named = 0
    for url in sorted(candidate):
        for label, record in candidate[url].entries():
            # A clean harness needs no reason, exactly as an absent subtest needs none.
            if record.status in ("OK", "PASS") or record.reviewed:
                continue
            unreviewed += 1
            if named < UNREVIEWED_REFUSAL_CAP:
                named += 1
                where = f"{url} [{label}]" if label else url
                refusals.append(f"{where}: {record.status} was never reviewed ({reason_text(record)})")
    if unreviewed > named:
        refusals.append(f"... and {unreviewed - named} more entries with no review")
    return refusals


def _candidate_header(path: Path) -> str:
    with path.open() as handle:
        first = handle.readline().strip()
    match = re.match(r"# generated at (.*?); review the bucket", first)
    return match.group(1) if match else first


def _row_status(ledger: dict[str, BucketRow], test: dict) -> str | None:
    row = ledger.get(test_buckets(test)[0])
    return row.status if row is not None else None


def apply_promotion(
    product: str,
    directory: str,
    report: dict,
    *,
    paths: Paths | None = None,
    blessed_by: str | None = None,
    timeout_s: int | None = None,
    long_timeout_s: int | None = None,
) -> PromoteResult:
    """Copy a reviewed candidate over the live tree and restamp BASELINE.toml."""
    paths = paths or Paths()
    directory = directory.strip("/")
    root = paths.candidate_dir(product, directory)
    live = paths.expectation_root(product) / directory
    result = PromoteResult(product=product, directory=directory)
    wanted = sorted(path.relative_to(root) for path in root.rglob("*.ini")) if root.is_dir() else []
    for relative in wanted:
        target = live / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text((root / relative).read_text())
        result.files_written += 1
    existing = sorted(path.relative_to(live) for path in live.rglob("*.ini")) if live.is_dir() else []
    for relative in existing:
        if relative in wanted:
            continue
        (live / relative).unlink()
        result.files_removed += 1
    _prune_empty_dirs(live)

    summary = report.get("summary") or {}
    stamp = report_stamp(report)
    rows = load_baseline(paths.baseline(product))
    rows[directory] = {
        "wpt_commit": stamp["wpt_commit"],
        "driver_sha256": stamp["driver_sha256"],
        "plan_digest": stamp["plan_digest"],
        "timeout_s": timeout_s if timeout_s is not None else report.get("timeout_s", 0),
        "long_timeout_s": long_timeout_s if long_timeout_s is not None else report.get("long_timeout_s", 0),
        "blessed_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "blessed_by": blessed_by or "",
        "subtests": summary.get("subtests", 0),
        "subtests_pass": summary.get("subtests_pass", 0),
        "files": summary.get("tests", 0),
    }
    result.baseline = paths.baseline(product)
    result.baseline.parent.mkdir(parents=True, exist_ok=True)
    result.baseline.write_text(render_baseline(rows))
    return result


def _prune_empty_dirs(root: Path) -> None:
    if not root.is_dir():
        return
    for path in sorted(root.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        if path.is_dir() and not any(path.iterdir()):
            path.rmdir()


def promote(
    product: str,
    directory: str,
    report: dict,
    *,
    paths: Paths | None = None,
    ledger: dict[str, BucketRow] | None = None,
    control: dict[str, int] | None = None,
    baseline: dict[str, dict] | None = None,
    blessed_by: str | None = None,
    timeout_s: int | None = None,
    long_timeout_s: int | None = None,
) -> PromoteResult:
    """`review_promotion` then `apply_promotion`. A refusal writes nothing."""
    paths = paths or Paths()
    refusals = review_promotion(
        product, directory, report, paths=paths, ledger=ledger, control=control, baseline=baseline
    )
    if refusals:
        return PromoteResult(product=product, directory=directory.strip("/"), exit_code=EXIT_REFUSED, refusals=refusals)
    return apply_promotion(
        product,
        directory,
        report,
        paths=paths,
        blessed_by=blessed_by,
        timeout_s=timeout_s,
        long_timeout_s=long_timeout_s,
    )


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


def load_report(path: Path) -> dict:
    if not path.is_file():
        tool_error(f"no results at {path}; run the directory first")
    with path.open("rb") as handle:
        return json.load(handle)


def judge_directory(product: str, directory: str, report: dict, *, paths: Paths | None = None):
    """`(issues, comparability problems, summary line)` for one stored run."""
    paths = paths or Paths()
    issues = judge(
        report.get("tests", []),
        load_expectation_tree(product, paths),
        ledger=load_ledger(paths.ledger(product)),
        control=load_control("chromium", directory, paths),
    )
    return issues, comparability(product, directory, report, paths=paths), summarize(report.get("tests", []))


def report_judgement(
    directory: str,
    report: dict,
    issues: list[Issue],
    problems: list[str],
    summary: dict,
    path: Path | None,
) -> None:
    where = f" -> {path.relative_to(WPT_DIR.parent)}" if path is not None else ""
    log(
        f"{directory}: {summary['subtests_pass']}/{summary['subtests']} subtests pass "
        f"({summary['subtest_pass_pct']}%), {summary['tests_ok']}/{summary['tests']} files OK, "
        f"{summary['tests_not_run']} not run, {len(issues)} unexpected{where}"
    )
    for problem in problems:
        log(f"  NOT COMPARABLE: {problem}; the pass percentage above is not a statement about the engine")
    for issue in issues[:50]:
        log(f"  unexpected: {issue.line()}")
    if len(issues) > 50:
        log(f"  ... {len(issues) - 50} more")


def cmd_run(args: argparse.Namespace) -> int:
    config = load_config()
    paths = Paths()
    ensure_checkout(config)
    manifest = ensure_manifest()
    driver = make_driver(args)
    identity = driver.identity()
    resolver_rules = not hosts_file_configured()
    if resolver_rules:
        log("hosts file does not map web-platform.test to 127.0.0.1; using browser resolver rules")
    deadline = int((time.time() + args.max_minutes * 60) * 1000) if args.max_minutes else None
    comparable = True
    differs = False
    with Server(config, RESULTS / f"{args.product}-serve.log") as server:
        for directory in args.dir:
            directory = directory.strip("/")
            tests = list_tests(manifest, directory, config)
            if not tests:
                log(
                    f"{directory}: the manifest yields no testharness tests; a configured directory that "
                    f"contributes nothing is NOT COMPARABLE (check the path in wpt/config.toml)"
                )
                comparable = False
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
            issues = judge(
                outcome["tests"],
                load_expectation_tree(args.product, paths),
                ledger=load_ledger(paths.ledger(args.product)),
                control=load_control("chromium", directory, paths),
            )
            summary = summarize(outcome["tests"])
            report = {
                "product": args.product,
                "browser": outcome["browser"],
                "wpt_commit": config["wpt"]["commit"],
                "directory": directory,
                "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
                "duration_s": round(time.time() - started, 1),
                "summary": summary,
                "driver": identity,
                "plan_digest": plan_digest(plan),
                "truncated": plan_truncated(outcome["tests"], args.max_minutes),
                "timeout_s": config["runner"]["timeout"],
                "long_timeout_s": config["runner"]["long_timeout"],
                "unexpected": [issue.line() for issue in issues],
                "tests": outcome["tests"],
            }
            path = results_path(args.product, directory)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(report, indent=2) + "\n")
            problems = comparability(args.product, directory, report, paths=paths)
            report_judgement(directory, report, issues, problems, summary, path)
            if problems:
                comparable = False
            elif issues:
                differs = True
    if not comparable:
        log("exit 3: at least one directory was not comparable")
        return EXIT_NOT_COMPARABLE
    return EXIT_DIFFERS if differs else EXIT_OK


def cmd_compare(args: argparse.Namespace) -> int:
    """Re-judge a stored results file: no checkout, no server, no driver."""
    paths = Paths()
    comparable = True
    differs = False
    for directory in args.dir:
        report = load_report(paths.results_file(args.product, directory))
        issues, problems, summary = judge_directory(args.product, directory, report, paths=paths)
        report_judgement(directory, report, issues, problems, summary, None)
        if problems:
            comparable = False
        elif issues:
            differs = True
    if not comparable:
        return EXIT_NOT_COMPARABLE
    return EXIT_DIFFERS if differs else EXIT_OK


def cmd_candidate(args: argparse.Namespace) -> int:
    paths = Paths()
    for directory in args.dir:
        report = load_report(paths.results_file(args.product, directory))
        generate_candidate(
            args.product,
            directory,
            report,
            paths=paths,
            ledger=load_ledger(paths.ledger(args.product)),
            control=load_control("chromium", directory, paths),
        )
    return EXIT_OK


def cmd_promote(args: argparse.Namespace) -> int:
    paths = Paths()
    config = load_config()
    reports = {directory.strip("/"): load_report(paths.results_file(args.product, directory)) for directory in args.dir}
    ledger = load_ledger(paths.ledger(args.product))
    baseline = load_baseline(paths.baseline(args.product))
    refused = False
    for directory, report in reports.items():
        refusals = review_promotion(
            args.product, directory, report, paths=paths, ledger=ledger, baseline=baseline
        )
        if not refusals:
            continue
        refused = True
        log(f"{directory}: promote REFUSED (exit 4), {len(refusals)} reasons; nothing was written")
        for refusal in refusals[:50]:
            log(f"  refuse: {refusal}")
        if len(refusals) > 50:
            log(f"  ... {len(refusals) - 50} more refusals")
    if refused:
        return EXIT_REFUSED
    for directory, report in reports.items():
        result = apply_promotion(
            args.product,
            directory,
            report,
            paths=paths,
            blessed_by=args.by,
            timeout_s=config["runner"]["timeout"],
            long_timeout_s=config["runner"]["long_timeout"],
        )
        log(
            f"{directory}: promoted {result.files_written} files "
            f"({result.files_removed} removed), restamped {result.baseline}"
        )
    return EXIT_OK


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
    """Deprecated alias for `candidate`: it must not touch the live tree."""
    log("`update-expectations` is deprecated and is now an alias for `candidate`; it writes a candidate")
    log("under wpt/results/<product>/candidate/ and nothing changes live until an explicit `promote`")
    return cmd_candidate(args)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("checkout")
    sub.add_parser("manifest")
    sub.add_parser("serve")
    for name in ("list", "run", "compare", "candidate", "promote", "update-expectations"):
        p = sub.add_parser(name)
        p.add_argument("--dir", action="append", required=True, help="WPT directory, e.g. dom/nodes (repeatable)")
        if name != "list":
            p.add_argument("--product", choices=["chromium", "tentabrowser", "tentabrowser-qjs"], required=True)
        if name == "run":
            p.add_argument("--jobs", type=int, default=load_config()["runner"]["jobs"])
            p.add_argument("--max-minutes", type=float, default=0, help="stop starting new tests after N minutes")
            p.add_argument("--channel", help="Playwright channel for chromium (e.g. chrome)")
            p.add_argument("--executable", help="browser executable for chromium")
            p.add_argument(
                "--driver",
                help="tenta-wpt-driver binary for either tentabrowser product "
                "(default: $TENTA_WPT_DRIVER, then target/release, then target/debug)",
            )
        if name == "promote":
            p.add_argument("--by", help="who signed the candidate off; recorded in BASELINE.toml")
    return parser


COMMANDS = {
    "list": cmd_list,
    "run": cmd_run,
    "compare": cmd_compare,
    "candidate": cmd_candidate,
    "promote": cmd_promote,
    "serve": cmd_serve,
    "update-expectations": cmd_update_expectations,
}


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "checkout":
        ensure_checkout(load_config())
        return EXIT_OK
    if args.command == "manifest":
        ensure_checkout(load_config())
        if MANIFEST.exists():
            MANIFEST.unlink()
        ensure_manifest()
        return EXIT_OK
    if shutil.which("git") is None:
        log("git is required")
        return EXIT_TOOLING
    try:
        return COMMANDS[args.command](args)
    except ToolError as error:
        log(f"error: {error}")
        return EXIT_TOOLING


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
