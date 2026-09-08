#!/usr/bin/env python3
"""Search provider survey (docs/spec/08-search.md §2).

Issues one request per query per provider with a current Chrome desktop
User-Agent, saves the raw response as a fixture under
``providers/fixtures/<provider>/<query-slug>.html``, applies the provider's
parse plan (``providers/plans/<provider>.toml``) to the response and records
everything in ``providers/survey-results.json`` and ``providers/SURVEY.md``.

Usage:
    python3 providers/survey.py                 # live run, refreshes fixtures
    python3 providers/survey.py --only bing     # one provider
    python3 providers/survey.py --report-only   # rebuild SURVEY.md from the
                                                # saved JSON and fixtures

Be polite: one request per query per provider, no retries. Google through
headless Chromium needs ``providers/tools/node_modules`` (``npm install`` in
``providers/tools``) and a Playwright Chromium build; see providers/README.md.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import http.cookiejar
import json
import os
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import serpdom  # noqa: E402
from check_plans import apply_plan, load_plan  # noqa: E402

ROOT = Path(__file__).resolve().parent
FIXTURES = ROOT / "fixtures"
PLANS = ROOT / "plans"
RESULTS_JSON = ROOT / "survey-results.json"
SURVEY_MD = ROOT / "SURVEY.md"

CHROME_VERSION = "151"
USER_AGENT = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
    f"(KHTML, like Gecko) Chrome/{CHROME_VERSION}.0.0.0 Safari/537.36"
)
BASE_HEADERS = {
    "User-Agent": USER_AGENT,
    "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,"
    "image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
    "Accept-Encoding": "gzip, deflate",
    "Sec-Ch-Ua": f'"Chromium";v="{CHROME_VERSION}", "Google Chrome";v="{CHROME_VERSION}", '
    '"Not_A Brand";v="24"',
    "Sec-Ch-Ua-Mobile": "?0",
    "Sec-Ch-Ua-Platform": '"macOS"',
    "Sec-Fetch-Dest": "document",
    "Sec-Fetch-Mode": "navigate",
    "Sec-Fetch-Site": "none",
    "Sec-Fetch-User": "?1",
    "Upgrade-Insecure-Requests": "1",
    "Connection": "keep-alive",
}

QUERIES = [
    {"slug": "rust-browser-engine", "q": "rust browser engine", "accept_language": "en-US,en;q=0.9", "language": "en", "region": "US"},
    {"slug": "warszawa-pogoda", "q": "warszawa pogoda", "accept_language": "pl-PL,pl;q=0.9,en;q=0.8", "language": "pl", "region": "PL"},
]

# Endpoints as given by the survey brief; the plans may shape requests
# differently (for example DuckDuckGo HTML supports POST as well).
HTTP_PROVIDERS = {
    "duckduckgo_html": "https://html.duckduckgo.com/html/?q={q}",
    "duckduckgo_lite": "https://lite.duckduckgo.com/lite/?q={q}",
    "bing": "https://www.bing.com/search?q={q}",
    "brave": "https://search.brave.com/search?q={q}",
    "google_nojs": "https://www.google.com/search?q={q}",
}
CHROMIUM_PROVIDERS = {"google_chromium": "https://www.google.com/search?q={q}&hl={language}&gl={region}"}

# Interstitial signatures, matched against the final URL and the body. They
# are deliberately specific (a SERP's own JavaScript mentions "captcha" and
# "consent"); the plan's [block] section is the authoritative detector.
BLOCK_SIGNATURES = {
    "consent": [r"consent\.google\.com", r"Before you continue to Google", r"Zanim przejdziesz do Google"],
    "captcha": [r"/sorry/index", r"g-recaptcha", r"unusual traffic from your computer", r"nietypowy ruch",
                r"schedule a captcha", r"id=\"captcha-form\""],
    "js_challenge": [r"Just a moment\.\.\.", r"challenge-platform", r"_cf_chl_opt", r"Enable JavaScript and cookies to continue"],
    "js_required": [r"/httpservice/retry/enablejs", r"Turn on JavaScript", r"This page needs JavaScript",
                    r"enable javascript on your web browser"],
    "anomaly": [r"bots use DuckDuckGo too", r"anomaly-modal"],
}
COOKIE_HEADERS = {"set-cookie", "set-cookie2"}


class RedirectRecorder(urllib.request.HTTPRedirectHandler):
    def __init__(self) -> None:
        self.hops: list[dict] = []

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self.hops.append({"status": code, "from": req.full_url, "to": newurl})
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def decode_body(raw: bytes, encoding: str | None) -> bytes:
    if encoding == "gzip":
        return gzip.decompress(raw)
    if encoding == "deflate":
        try:
            return zlib.decompress(raw)
        except zlib.error:
            return zlib.decompress(raw, -zlib.MAX_WBITS)
    return raw


def scrub_headers(headers) -> dict[str, str]:
    out: dict[str, str] = {}
    cookie_names: list[str] = []
    for key, value in headers.items():
        lowered = key.lower()
        if lowered in COOKIE_HEADERS:
            cookie_names.append(value.split("=", 1)[0].strip())
        else:
            out[key] = value
    if cookie_names:
        out["Set-Cookie"] = "<" + ", ".join(cookie_names) + ">"
    return out


def detect_blocks(final_url: str, body: str) -> list[str]:
    found = []
    sample = body[:200_000]
    for kind, patterns in BLOCK_SIGNATURES.items():
        for pattern in patterns:
            if re.search(pattern, final_url, re.I) or re.search(pattern, sample, re.I):
                found.append(kind)
                break
    return found


def charset_of(headers, body: bytes) -> str:
    content_type = headers.get("Content-Type", "")
    match = re.search(r"charset=([\w-]+)", content_type, re.I)
    if match:
        return match.group(1)
    match = re.search(rb'<meta[^>]+charset=["\']?([\w-]+)', body[:4096], re.I)
    return match.group(1).decode() if match else "utf-8"


def fetch(url: str, headers: dict[str, str]) -> dict:
    recorder = RedirectRecorder()
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(recorder, urllib.request.HTTPCookieProcessor(jar))
    request = urllib.request.Request(url, headers=headers)
    started = time.perf_counter()
    record: dict = {"url": url, "request_headers": headers, "redirects": recorder.hops}
    try:
        with opener.open(request, timeout=30) as response:
            record["ttfb_ms"] = round((time.perf_counter() - started) * 1000, 1)
            raw = response.read()
            record["total_ms"] = round((time.perf_counter() - started) * 1000, 1)
            record["status"] = response.status
            record["final_url"] = response.url
            record["response_headers"] = scrub_headers(response.headers)
            body = decode_body(raw, response.headers.get("Content-Encoding"))
    except urllib.error.HTTPError as error:
        record["ttfb_ms"] = round((time.perf_counter() - started) * 1000, 1)
        raw = error.read()
        record["total_ms"] = round((time.perf_counter() - started) * 1000, 1)
        record["status"] = error.code
        record["final_url"] = error.url
        record["response_headers"] = scrub_headers(error.headers)
        body = decode_body(raw, error.headers.get("Content-Encoding"))
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        record["total_ms"] = round((time.perf_counter() - started) * 1000, 1)
        record["status"] = None
        record["error"] = str(error)
        record["body"] = ""
        return record
    record["cookies_set"] = sorted({cookie.name for cookie in jar})
    record["body_bytes"] = len(body)
    record["body"] = body.decode(charset_of(record["response_headers"], body), errors="replace")
    return record


def chromium_script() -> Path:
    return ROOT / "tools" / "google_chromium.mjs"


def fetch_with_chromium(url: str, query: dict) -> dict:
    command = [
        "node", str(chromium_script()),
        "--url", url,
        "--accept-language", query["accept_language"],
        "--locale", f"{query['language']}-{query['region']}",
        "--user-agent", USER_AGENT,
    ]
    started = time.perf_counter()
    completed = subprocess.run(command, capture_output=True, text=True, cwd=ROOT / "tools")
    elapsed = round((time.perf_counter() - started) * 1000, 1)
    if completed.returncode != 0:
        return {"url": url, "status": None, "error": completed.stderr.strip()[-2000:], "body": "", "total_ms": elapsed}
    record = json.loads(completed.stdout)
    record.setdefault("url", url)
    record["request_headers"] = {"User-Agent": USER_AGENT, "Accept-Language": query["accept_language"], "note": "remaining headers set by Chromium " + record.get("browser_version", "")}
    record["wall_ms"] = elapsed
    return record


def query_url(template: str, query: dict) -> str:
    return template.format(q=urllib.parse.quote_plus(query["q"]), language=query["language"], region=query["region"])


def slugify(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def run_provider(provider: str, template: str, via_chromium: bool, slugs: set[str] | None = None) -> list[dict]:
    records = []
    for query in QUERIES:
        if slugs and query["slug"] not in slugs:
            continue
        url = query_url(template, query)
        headers = dict(BASE_HEADERS, **{"Accept-Language": query["accept_language"]})
        print(f"[{provider}] {query['q']!r} -> {url}", file=sys.stderr)
        record = fetch_with_chromium(url, query) if via_chromium else fetch(url, headers)
        record.update({"provider": provider, "query": query["q"], "slug": query["slug"], "accept_language": query["accept_language"]})
        body = record.pop("body", "")
        fixture = FIXTURES / provider / f"{query['slug']}.html"
        fixture.parent.mkdir(parents=True, exist_ok=True)
        fixture.write_text(body, encoding="utf-8")
        record["fixture"] = str(fixture.relative_to(ROOT))
        record["fetched_at"] = dt.datetime.now(dt.UTC).isoformat(timespec="seconds")
        record["blocks"] = detect_blocks(record.get("final_url", url), body)
        records.append(record)
        time.sleep(2.0)  # spacing between the two queries of one provider
    return records


def analyse(record: dict) -> dict:
    """Apply the provider plan to the saved fixture (offline)."""
    fixture = ROOT / record["fixture"]
    body = fixture.read_text(encoding="utf-8") if fixture.exists() else ""
    plan_path = PLANS / f"{record['provider']}.toml"
    analysis = {"fixture_bytes": len(body.encode("utf-8")), "plan": None, "results": [], "count": 0,
                "html_lang": (re.search(r"<html[^>]*\slang=[\"']?([\w-]+)", body[:4096], re.I) or [None, None])[1]}
    if not plan_path.exists() or not body:
        return analysis
    plan = load_plan(plan_path)
    outcome = apply_plan(plan, body)
    analysis.update({"plan": plan_path.name, "results": outcome["results"][:3], "count": len(outcome["results"]),
                     "selectors": outcome["selectors"], "blocked_by_plan": outcome["blocked"], "next_page": outcome["next_page"]})
    return analysis


def write_report(records: list[dict], meta: dict) -> None:
    by_provider: dict[str, list[dict]] = {}
    for record in records:
        by_provider.setdefault(record["provider"], []).append(record)
    lines = ["# Search provider survey", "",
             f"Generated by `providers/survey.py` on {meta['date']} (UTC) from {meta['location']}.",
             f"User-Agent: `{meta['user_agent']}`.", "",
             "Fixtures under `providers/fixtures/<provider>/<query-slug>.html` are the raw bodies "
             "(after transfer decoding, nothing stripped). Extraction columns come from applying "
             "`providers/plans/<provider>.toml` with `providers/check_plans.py`; a `-` means no plan.", "",
             "## Summary", "",
             "| Provider | Query | Endpoint (final) | Status | JS required | Results | Blocking | Size | TTFB | Total |",
             "|---|---|---|---|---|---|---|---|---|---|"]
    for provider, provider_records in by_provider.items():
        for record in provider_records:
            analysis = record["analysis"]
            lines.append("| {p} | {q} | {u} | {s} | {js} | {n} | {b} | {size} | {ttfb} | {total} |".format(
                p=provider, q=record["query"], u=shorten(record.get("final_url") or record["url"]),
                s=record.get("status") if record.get("status") is not None else f"error: {record.get('error','')[:60]}",
                js="yes" if analysis["count"] == 0 and ("js_required" in record["blocks"] or record.get("js_required")) else "no",
                n=analysis["count"] if analysis["plan"] else "-",
                b=", ".join(record["blocks"]) or "none",
                size=f"{analysis['fixture_bytes'] / 1024:.0f} KB",
                ttfb=f"{record.get('ttfb_ms', 0):.0f} ms" if record.get("ttfb_ms") is not None else "-",
                total=f"{record.get('total_ms', 0):.0f} ms"))
    lines.append("")
    for provider, provider_records in by_provider.items():
        lines += [f"## {provider}", ""]
        plan_name = provider_records[0]["analysis"].get("plan")
        selectors = provider_records[0]["analysis"].get("selectors") or {}
        plan = load_plan(PLANS / plan_name) if plan_name else {}
        js_required = all(r["analysis"]["count"] == 0 for r in provider_records) and any(
            "js_required" in r["blocks"] or r.get("js_required") for r in provider_records)
        lines += ["| Field | Value |", "|---|---|",
                  f"| Endpoint | `{provider_records[0]['url'].split('?')[0]}` ({plan.get('path', '-')} path) |",
                  f"| Plan | `{('plans/' + plan_name) if plan_name else '-'}` |",
                  f"| JS required | {'yes' if js_required else 'no'} |",
                  "| Results per query | " + ", ".join(f"`{r['query']}`: {r['analysis']['count']}" for r in provider_records) + " |",
                  f"| Item selector | `{selectors.get('item', '-')}` |",
                  f"| Title / URL / snippet | `{selectors.get('title', '-')}` / `{selectors.get('url', '-')}` / `{selectors.get('snippet', '-')}` |",
                  f"| Next page | `{selectors.get('next_page', '-')}` |",
                  f"| Ads excluded by | `{selectors.get('exclude', '-')}` |",
                  "| Locale handling | plan params `" + ", ".join(f"{k}={v}" for k, v in plan.get("request", {}).get("locale", {}).items())
                  + "`; observed `html lang`: " + ", ".join(f"{r['analysis'].get('html_lang') or '-'} ({r['accept_language'].split(',')[0]})" for r in provider_records) + " |",
                  "| Blocking observed | " + "; ".join(f"`{r['query']}`: {', '.join(r['blocks']) or 'none'}" + (f" (HTTP {r['status']})" if r.get('status') not in (200, None) else "") for r in provider_records) + " |",
                  "| Size / TTFB | " + ", ".join(f"{r['analysis']['fixture_bytes'] / 1024:.0f} KB / {r.get('ttfb_ms') or 0:.0f} ms" for r in provider_records) + " |",
                  ""]
        for record in provider_records:
            analysis = record["analysis"]
            lines += [f"### `{record['query']}` (Accept-Language: `{record['accept_language']}`)", "",
                      f"- Request: `GET {record['url']}`" if not record.get("via") else f"- Request: {record['via']} `{record['url']}`",
                      f"- Status: {record.get('status')}; final URL: `{record.get('final_url', '')}`",
                      f"- Redirects: {format_redirects(record.get('redirects', []))}",
                      f"- Response headers: {format_headers(record.get('response_headers', {}))}",
                      f"- Cookies set: {', '.join(record.get('cookies_set', [])) or 'none'}",
                      f"- Blocking observed: {', '.join(record['blocks']) or 'none'}",
                      f"- Size: {analysis['fixture_bytes']} bytes; TTFB {record.get('ttfb_ms', '-')} ms; total {record.get('total_ms', '-')} ms",
                      f"- Results extracted by plan: {analysis['count'] if analysis['plan'] else '-'}"
                      + (f"; plan block detection: {analysis['blocked_by_plan']}" if analysis.get("blocked_by_plan") else "")
                      + (f"; next page: `{analysis['next_page']}`" if analysis.get("next_page") else ""),
                      ]
            if record.get("notes"):
                lines.append(f"- Chromium notes: {record['notes']}")
            if record.get("error"):
                lines.append(f"- Error: `{record['error']}`")
            for attempt in record.get("previous_attempts", []):
                lines.append(f"- Earlier attempt at {attempt.get('fetched_at')}: status {attempt.get('status')}"
                             + (f", error `{attempt.get('error')}`" if attempt.get("error") else "")
                             + (f", blocks {attempt.get('blocks')}" if attempt.get("blocks") else ""))
            for index, result in enumerate(analysis["results"], 1):
                lines.append(f"  {index}. **{result.get('title', '')}** — `{result.get('url', '')}`")
                if result.get("snippet"):
                    lines.append(f"     {result['snippet'][:200]}")
            lines.append("")
    SURVEY_MD.write_text("\n".join(lines), encoding="utf-8")


def shorten(url: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    return f"`{parsed.netloc}{parsed.path}`"


def format_redirects(hops: list[dict]) -> str:
    if not hops:
        return "none"
    return "; ".join(f"{hop['status']} -> `{hop['to']}`" for hop in hops)


def format_headers(headers: dict[str, str]) -> str:
    keep = ("content-type", "content-encoding", "server", "cache-control", "set-cookie", "content-security-policy",
            "x-frame-options", "strict-transport-security", "vary", "cf-ray", "x-cache", "content-language", "alt-svc")
    parts = [f"`{key}: {value[:80]}`" for key, value in headers.items() if key.lower() in keep]
    return ", ".join(parts) or "(none of interest)"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--only", action="append", help="provider id to run (repeatable)")
    parser.add_argument("--query", action="append", help="query slug to run (repeatable), default both")
    parser.add_argument("--report-only", action="store_true", help="rebuild SURVEY.md from survey-results.json and fixtures")
    parser.add_argument("--skip-chromium", action="store_true", help="skip the Google via headless Chromium provider")
    args = parser.parse_args()

    if args.report_only:
        data = json.loads(RESULTS_JSON.read_text())
        records, meta = data["records"], data["meta"]
    else:
        records = []
        partial = bool(args.only or args.query)
        existing = json.loads(RESULTS_JSON.read_text())["records"] if RESULTS_JSON.exists() and partial else []
        selected = set(args.only or list(HTTP_PROVIDERS) + list(CHROMIUM_PROVIDERS))
        slugs = set(args.query) if args.query else None
        for provider, template in HTTP_PROVIDERS.items():
            if provider in selected:
                records += run_provider(provider, template, via_chromium=False, slugs=slugs)
        for provider, template in CHROMIUM_PROVIDERS.items():
            if provider in selected and not args.skip_chromium:
                for record in run_provider(provider, template, via_chromium=True, slugs=slugs):
                    record["via"] = "headless Chromium (Playwright)"
                    records.append(record)
        fetched = {(r["provider"], r["slug"]) for r in records}
        for old in existing:
            if (old["provider"], old["slug"]) in fetched:
                # Keep the history of earlier attempts (for example a connection reset).
                new = next(r for r in records if (r["provider"], r["slug"]) == (old["provider"], old["slug"]))
                new["previous_attempts"] = old.get("previous_attempts", []) + [
                    {k: old.get(k) for k in ("fetched_at", "status", "error", "final_url", "blocks", "ttfb_ms", "total_ms")}]
        records = [r for r in existing if (r["provider"], r["slug"]) not in fetched] + records
        order = list(HTTP_PROVIDERS) + list(CHROMIUM_PROVIDERS)
        slug_order = [q["slug"] for q in QUERIES]
        records.sort(key=lambda r: (order.index(r["provider"]), slug_order.index(r["slug"])))
        meta = {"date": dt.datetime.now(dt.UTC).strftime("%Y-%m-%d %H:%M"), "user_agent": USER_AGENT,
                "location": os.environ.get("TENTA_SURVEY_LOCATION", "a consumer connection in Poland")}
    for record in records:
        record["analysis"] = analyse(record)
    RESULTS_JSON.write_text(json.dumps({"meta": meta, "records": records}, indent=2, ensure_ascii=False) + "\n")
    write_report(records, meta)
    print(f"wrote {RESULTS_JSON.relative_to(ROOT)} and {SURVEY_MD.relative_to(ROOT)}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
