#!/usr/bin/env python3
"""Derive the CDP method/event checklist from the Puppeteer and Playwright sources.

Usage:
    extract.py <puppeteer-root> <playwright-root> [--out methods.toml] [--unclassified]

Scans
    puppeteer:  packages/puppeteer-core/src/cdp/**/*.ts   (plus the three files outside
                cdp/ that send raw CDP, see PUPPETEER_EXTRA)
    playwright: packages/playwright-core/src/server/chromium/**/*.ts

Every string literal of the shape 'Domain.name' is located and classified by
the call that precedes it:
    method  .send(', .send<T>(', ._send(', ._sendMayFail(', method: '
    event   .on(', .once(', .off(', .emit(', eventsHelper.addEventListener(session, ',
            fromEmitterEvent(client, ', object.method === ', and the
            [['Domain.event', handler], ...] table in Puppeteer's NetworkManager
Literals that match none of these are reported with --unclassified and are
skipped, so the output never contains a guess. Puppeteer's ExtensionTransport.ts
is excluded: it emulates a CDP server over chrome.debugger for the extension
build and does not send those names to a browser.

Playwright `connect_over_cdp` rule: every file under server/chromium/ except
chromium.ts and chromiumSwitches.ts is reachable after
`chromium.connectOverCDP()` (CRBrowser.connect builds the same CRBrowser,
CRPage, CRNetworkManager, ... objects for launched and connected browsers).
Uses inside chromium.ts are on the connectOverCDP path only if they sit in
`_connectOverCDPImpl`/`_connectOverCDPInternal`; the launch-only ones
(`_launchProcess`, `_attemptToGracefullyCloseBrowser` ... ) are not.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

LITERAL = re.compile(r"'([A-Z][A-Za-z0-9]*\.[a-z][A-Za-z0-9]*)'")

# Context that ends right before the literal (whitespace and newlines allowed).
METHOD_CTX = re.compile(
    r"(\.send(?:<[^>]*>)?\(|\._send\(|\._sendMayFail\(|\.sendCommand\(|\bmethod:)\s*$"
)
EVENT_CTX = re.compile(
    r"(\.(?:on|once|off|emit)\(|addEventListener\(\s*[\w.]+\s*,|fromEmitterEvent\(\s*[\w.]+\s*,"
    r"|\.method\s*===|\[)\s*$"
)

PUPPETEER_DIRS = ["packages/puppeteer-core/src/cdp"]
# Files outside cdp/ that still talk raw CDP (documented in docs/dev/cdp-checklist.md).
PUPPETEER_EXTRA = [
    "packages/puppeteer-core/src/api/Page.ts",
    "packages/puppeteer-core/src/node/ScreenRecorder.ts",
    "packages/puppeteer-core/src/common/util.ts",
]
PLAYWRIGHT_DIRS = ["packages/playwright-core/src/server/chromium"]

LAUNCH_ONLY_PW_FILES = {"chromium.ts", "chromiumSwitches.ts"}
EXCLUDED_FILES = {"ExtensionTransport.ts"}


def ts_files(root: Path, dirs: list[str], extra: list[str] = ()) -> list[Path]:
    out: list[Path] = []
    for d in dirs:
        for p in sorted((root / d).rglob("*.ts")):
            if p.name.endswith(".test.ts") or p.name.endswith(".d.ts") or p.name in EXCLUDED_FILES:
                continue
            out.append(p)
    for e in extra:
        p = root / e
        if p.exists():
            out.append(p)
    return out


def _regex_literal_start(out: list[str]) -> bool:
    """True when a "/" at this point can only begin a regex literal (not division)."""
    tail = "".join(out[-40:]).rstrip()
    if not tail:
        return True
    if tail[-1] in "(,=:[!&|?{};":
        return True
    return re.search(r"\b(return|typeof|case|in|of)$", tail) is not None


def strip_comments(src: str) -> str:
    """Blank out // and /* */ comments while keeping string literals and line structure.

    A naive regex would treat the "/*" inside a regex literal such as
    /(?:chrome|edge):\\/*([^/]+)/ as a comment start and swallow real code
    (crPage.ts loses half of its literals that way).
    """
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in ("'", '"', "`"):
            j = i + 1
            while j < n and src[j] != c:
                j += 2 if src[j] == "\\" else 1
            out.append(src[i : j + 1])
            i = j + 1
        elif c == "/" and not src.startswith(("//", "/*"), i) and _regex_literal_start(out):
            # Regex literal such as /^(?:chrome|edge):\\/*([^/?#]+)/i: skip to its closing
            # slash so the "/*" inside it is not taken for a comment start.
            j, in_class = i + 1, False
            while j < n and src[j] != "\n":
                ch = src[j]
                if ch == "\\":
                    j += 2
                    continue
                if ch == "[":
                    in_class = True
                elif ch == "]":
                    in_class = False
                elif ch == "/" and not in_class:
                    break
                j += 1
            out.append(src[i : j + 1])
            i = j + 1
        elif src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append("".join("\n" if ch == "\n" else " " for ch in src[i:j]))
            i = j
        else:
            out.append(c)
            i += 1
    return "".join(out)


def pw_connect_over_cdp(path: Path, src: str, pos: int) -> bool:
    if path.name not in LAUNCH_ONLY_PW_FILES:
        return True
    if path.name == "chromiumSwitches.ts":
        return False
    # chromium.ts: find the enclosing method by scanning backwards for the last
    # method definition header at two-space indentation.
    heads = list(re.finditer(r"(?m)^  (?:private |override |static )*(?:async )?(\w+)\(", src[:pos]))
    if not heads:
        return False
    return heads[-1].group(1) in {"_connectOverCDPImpl", "_connectOverCDPInternal", "connectOverCDP"}


def scan(client: str, root: Path, files: list[Path], unclassified: list[str]):
    """Yield (name, kind, relpath, connect_over_cdp)."""
    for path in files:
        src = strip_comments(path.read_text(encoding="utf-8"))
        rel = path.relative_to(root).as_posix()
        for m in LITERAL.finditer(src):
            name = m.group(1)
            before = src[max(0, m.start() - 200) : m.start()]
            if METHOD_CTX.search(before):
                kind = "method"
            elif EVENT_CTX.search(before):
                kind = "event"
            else:
                line = src.count("\n", 0, m.start()) + 1
                ctx = before[-60:].replace("\n", "\\n")
                unclassified.append(f"{client}:{rel}:{line}  {name}   ...{ctx}")
                continue
            coc = client == "playwright" and pw_connect_over_cdp(path, src, m.start())
            yield name, kind, rel, coc


def toml_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("puppeteer")
    ap.add_argument("playwright")
    ap.add_argument("--out", default=str(Path(__file__).with_name("methods.toml")))
    ap.add_argument("--unclassified", action="store_true", help="print skipped literals")
    args = ap.parse_args()

    pup = Path(args.puppeteer)
    pw = Path(args.playwright)
    commits = {}
    for client, root in (("puppeteer", pup), ("playwright", pw)):
        head = root / ".git" / "HEAD"
        commits[client] = "unknown"
        if head.exists():
            ref = head.read_text().strip()
            if ref.startswith("ref:"):
                ref_path = root / ".git" / ref.split(" ", 1)[1]
                packed = root / ".git" / "packed-refs"
                if ref_path.exists():
                    commits[client] = ref_path.read_text().strip()
                elif packed.exists():
                    for line in packed.read_text().splitlines():
                        if line.endswith(ref.split(" ", 1)[1]):
                            commits[client] = line.split()[0]
            else:
                commits[client] = ref

    entries: dict[tuple[str, str], dict] = {}
    unclassified: list[str] = []
    kinds_conflict: set[str] = set()

    def add(client, name, kind, rel, coc):
        key = (name, kind)
        e = entries.setdefault(key, {"used_by": set(), "files": set(), "count": 0, "coc": False})
        e["used_by"].add(client)
        e["files"].add(f"{client}:{rel}")
        e["count"] += 1
        e["coc"] = e["coc"] or coc

    for name, kind, rel, coc in scan("puppeteer", pup, ts_files(pup, PUPPETEER_DIRS, PUPPETEER_EXTRA), unclassified):
        add("puppeteer", name, kind, rel, coc)
    for name, kind, rel, coc in scan("playwright", pw, ts_files(pw, PLAYWRIGHT_DIRS), unclassified):
        add("playwright", name, kind, rel, coc)

    # A name seen as both method and event is almost always a classifier slip; report it.
    by_name = defaultdict(set)
    for name, kind in entries:
        by_name[name].add(kind)
    kinds_conflict = {n for n, k in by_name.items() if len(k) > 1}

    lines = [
        "# CDP methods and events used by Puppeteer and Playwright (Chromium backends).",
        "# Generated by crates/tenta-cdp/checklist/extract.py; do not edit by hand.",
        f"# puppeteer  commit {commits['puppeteer']}",
        f"# playwright commit {commits['playwright']}",
        "# Regenerate: python3 crates/tenta-cdp/checklist/extract.py <puppeteer-root> <playwright-root>",
        "#",
        "# kind             method | event",
        "# used_by          clients whose Chromium backend uses the name",
        "# files            source files, prefixed with the client name",
        "# count            occurrences across both clients",
        "# connect_over_cdp true when a Playwright use is reachable after chromium.connectOverCDP()",
        "",
    ]
    for (name, kind), e in sorted(entries.items()):
        lines.append("[[entry]]")
        lines.append(f"name = {toml_str(name)}")
        lines.append(f"kind = {toml_str(kind)}")
        lines.append("used_by = [" + ", ".join(toml_str(c) for c in sorted(e["used_by"])) + "]")
        lines.append("files = [" + ", ".join(toml_str(f) for f in sorted(e["files"])) + "]")
        lines.append(f"count = {e['count']}")
        if e["coc"]:
            lines.append("connect_over_cdp = true")
        lines.append("")
    Path(args.out).write_text("\n".join(lines), encoding="utf-8")

    methods = sum(1 for (_, k) in entries if k == "method")
    events = sum(1 for (_, k) in entries if k == "event")
    print(f"wrote {args.out}: {methods} methods, {events} events, {len(unclassified)} unclassified literals skipped")
    if kinds_conflict:
        print("WARNING names classified as both method and event: " + ", ".join(sorted(kinds_conflict)))
    if args.unclassified:
        for u in unclassified:
            print("  " + u)
    return 0


if __name__ == "__main__":
    sys.exit(main())
