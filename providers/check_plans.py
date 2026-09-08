#!/usr/bin/env python3
"""Apply every parse plan in providers/plans/ to its fixtures and print results.

Reference implementation of the plan format documented in providers/README.md.
Stdlib only. Exit status is non-zero when a plan extracts no results from a
fixture and does not recognise it as a block page either, so this doubles as
a fixture drift check in CI.

Usage:
    python3 providers/check_plans.py             # all plans, all fixtures
    python3 providers/check_plans.py bing        # one provider
    python3 providers/check_plans.py --json      # machine-readable output
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
import tomllib
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import serpdom  # noqa: E402

ROOT = Path(__file__).resolve().parent
PLANS = ROOT / "plans"
FIXTURES = ROOT / "fixtures"


def load_plan(path: Path) -> dict:
    with path.open("rb") as handle:
        plan = tomllib.load(handle)
    for key in ("provider", "version", "request", "results"):
        if key not in plan:
            raise ValueError(f"{path.name}: missing [{key}]")
    for key in ("item", "title", "url"):
        if key not in plan["results"]:
            raise ValueError(f"{path.name}: [results] needs {key}")
    return plan


def _first_text(node: serpdom.Node, selector: str | None) -> str:
    if not selector:
        return ""
    found = node.select_one(selector)
    return found.text() if found else ""


def _attr(node: serpdom.Node, selector: str, attr: str) -> str:
    found = node.select_one(selector)
    return (found.get(attr) or "") if found else ""


def unwrap_url(href: str, plan_results: dict, base: str) -> str:
    """Resolve a result href to the destination URL (redirect unwrapping)."""
    if href.startswith("//"):
        href = "https:" + href
    elif href.startswith("/"):
        href = urllib.parse.urljoin(base, href)
    param = plan_results.get("url_unwrap_param")
    if param:
        parsed = urllib.parse.urlsplit(href)
        values = urllib.parse.parse_qs(parsed.query).get(param)
        if values:
            value = values[0]
            if plan_results.get("url_unwrap_decode") == "bing-a1-base64url" and value.startswith("a1"):
                padded = value[2:] + "=" * (-len(value[2:]) % 4)
                return base64.urlsafe_b64decode(padded).decode("utf-8", errors="replace")
            return value
    pattern = plan_results.get("url_unwrap_regex")
    if pattern:
        match = re.search(pattern, href)
        if match:
            return urllib.parse.unquote(match.group(1))
    return href


def detect_block(plan: dict, root: serpdom.Node, html: str) -> str | None:
    block = plan.get("block", {})
    for selector in block.get("selectors", []):
        if root.select_one(selector):
            return f"selector {selector}"
    sample = html[:200_000]
    for pattern in block.get("patterns", []):
        if re.search(pattern, sample, re.I):
            return f"pattern {pattern!r}"
    return None


def apply_plan(plan: dict, html: str) -> dict:
    results_cfg = plan["results"]
    root = serpdom.parse(html)
    base = plan["request"]["endpoint"]
    scope = root
    container = results_cfg.get("container")
    if container:
        scope = root.select_one(container) or root
    excluded: set[int] = set()
    for selector in results_cfg.get("exclude", []):
        for node in scope.select(selector):
            excluded.add(id(node))
            for descendant in node.iter_elements():
                excluded.add(id(descendant))
    # Fields listed in `following` live outside the item element (table
    # layouts such as DuckDuckGo Lite): take the first match in document order
    # after the item and before the next item.
    following = set(results_cfg.get("following", []))
    items = [item for item in scope.select(results_cfg["item"]) if id(item) not in excluded]
    following_nodes = {field: scope.select(results_cfg[field]) for field in following if results_cfg.get(field)}
    results = []
    for index, item in enumerate(items):
        upper = items[index + 1].order if index + 1 < len(items) else float("inf")
        href = _attr(item, results_cfg["url"], results_cfg.get("url_attr", "href"))
        title = _first_text(item, results_cfg["title"])
        if not href or not title:
            continue
        url = unwrap_url(href, results_cfg, base)
        if not url.startswith(("http://", "https://")):
            continue
        fields = {}
        for field in ("display_url", "snippet"):
            if field in following:
                node = next((n for n in following_nodes.get(field, []) if item.order < n.order < upper), None)
                fields[field] = node.text() if node else ""
            else:
                fields[field] = _first_text(item, results_cfg.get(field))
        results.append({"rank": len(results) + 1, "title": title, "url": url, **fields})
    pagination = plan.get("pagination", {})
    next_page = None
    if pagination.get("next"):
        node = root.select_one(pagination["next"])
        if node is not None:
            next_page = node.get(pagination.get("attr", "href")) or node.text()
    return {
        "results": results,
        "blocked": detect_block(plan, root, html),
        "next_page": next_page,
        "selectors": {
            "item": results_cfg["item"],
            "title": results_cfg["title"],
            "url": results_cfg["url"] + "[@" + results_cfg.get("url_attr", "href") + "]",
            "snippet": results_cfg.get("snippet", "-"),
            "next_page": pagination.get("next", "-"),
            "exclude": ", ".join(results_cfg.get("exclude", [])) or "-",
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("providers", nargs="*", help="provider ids (default: all plans)")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--show", type=int, default=3, help="results to print per fixture")
    args = parser.parse_args()

    plan_paths = sorted(PLANS.glob("*.toml"))
    if args.providers:
        plan_paths = [p for p in plan_paths if p.stem in args.providers]
    failures = 0
    report = []
    for plan_path in plan_paths:
        plan = load_plan(plan_path)
        fixtures = sorted((FIXTURES / plan_path.stem).glob("*.html"))
        for fixture in fixtures:
            html = fixture.read_text(encoding="utf-8", errors="replace")
            outcome = apply_plan(plan, html)
            # A fixture that the plan recognises as a block page is a valid
            # outcome (the detector works); a silent zero is the failure.
            ok = bool(outcome["results"]) or bool(outcome["blocked"])
            failures += 0 if ok else 1
            entry = {"plan": plan_path.name, "fixture": str(fixture.relative_to(ROOT)), "count": len(outcome["results"]),
                     "blocked": outcome["blocked"], "next_page": outcome["next_page"], "ok": ok,
                     "results": outcome["results"][: args.show]}
            report.append(entry)
            if not args.json:
                status = "ok" if outcome["results"] else ("blocked" if ok else "FAIL")
                print(f"[{status}] {plan_path.name} <- {entry['fixture']}: {entry['count']} results"
                      + (f", blocked: {outcome['blocked']}" if outcome["blocked"] else "")
                      + (f", next: {outcome['next_page']}" if outcome["next_page"] else ""))
                for result in entry["results"]:
                    print(f"  {result['rank']}. {result['title']}\n     {result['url']}\n     {result['snippet'][:160]}")
    if args.json:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
