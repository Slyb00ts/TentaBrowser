# Search providers

Parse plans, recorded SERP fixtures and the provider survey for
`tenta-search` (`docs/spec/08-search.md`). Everything in this directory is
data or a stdlib Python tool; the Rust implementation (P1) reads the plans
and is tested against the fixtures.

```
providers/
├── plans/<provider>.toml          parse plans (this document describes the format)
├── fixtures/<provider>/<slug>.html raw SERPs recorded by the survey (nothing stripped)
├── survey.py                      live survey: one request per query per provider
├── survey-results.json            machine-readable survey record (headers, timing, blocks)
├── SURVEY.md                      generated report (tables per provider)
├── check_plans.py                 applies every plan to its fixtures; CI drift check
├── serpdom.py                     reference DOM + selector subset used by both scripts
└── tools/                         Node helper for Google through headless Chromium
```

Provider ids: `duckduckgo_html`, `duckduckgo_lite`, `bing`, `brave`,
`google_nojs` (detection-only), `google_chromium` (full path). No vendor
search API is surveyed or planned: `search` only ever uses the public pages
(`08-search.md` §2, O-7).

## Running

```bash
python3 providers/check_plans.py                    # offline, exit 1 on a silent zero
python3 providers/survey.py --report-only           # rebuild SURVEY.md from saved data
python3 providers/survey.py                         # live run (refreshes fixtures)
python3 providers/survey.py --only bing --query warszawa-pogoda
```

The live run is polite by construction: one request per query per provider,
a current Chrome desktop `User-Agent`, two seconds between the two queries of
a provider, no retries. Re-running `--only <provider>` keeps the earlier
attempt in `previous_attempts` so a transient reset or block stays visible.
Google through headless Chromium needs `cd providers/tools && npm install`
(`playwright-core`, pinned) and a Playwright Chromium build in the Playwright
cache; `TENTA_CHROMIUM=/path/to/chrome` overrides the binary. `node_modules/`
is gitignored.

## Plan format (v1)

A plan is one TOML file. Keys marked "template" accept `{language}`,
`{region}`, `{region_lower}` and `{accept_language}`, filled from the
`SearchRequest`'s locale. A template whose value the locale cannot supply
(`{region}` for a locale without one) drops its parameter, which is how a
provider's own default is selected. Unknown keys and unknown placeholders are
refused when the plan is loaded, so a typo is a load error and not a silent
zero. There are no API-key placeholders: `search` only uses the public pages
(O-7), and a plan with `path = "api"` is refused.

```toml
provider = "bing"          # id, equals the file stem and the fixture directory
version = 1                # bump on any selector change (ProviderMeta.plan_version)
path = "fast"              # fast | full  (08-search.md §2)
verified = true            # false when the selectors were not confirmed on a live SERP
notes = """free text: quirks observed by the survey"""

[request]
endpoint = "https://www.bing.com/search"
method = "GET"             # GET: params in the URL; POST: params as a form body
query_param = "q"
params = { source = "web" }                    # fixed extra parameters
locale = { setlang = "{language}", cc = "{region}" }   # templates
safe = { param = "safe", strict = "active", moderate = "active", off = "off" }
#   or safe = { header = "Cookie", strict = "SRCHHPGUSR=ADLT=STRICT", ... } when the
#   provider only takes the setting as a cookie
freshness = { param = "df", day = "d", week = "w", month = "m", year = "y" }
page = { param = "first", first = 1, step = 10 }   # offset parameter for page N
site = { param = "siteSearch" }                    # optional; otherwise "site:" is prefixed to q

[request.headers]          # sent verbatim after templating; the profile adds the rest
Accept-Language = "{accept_language}"

[cookies]
policy = "none"            # none: never send | session: keep the provider's cookies
                           # for the warm pool's lifetime | persist: through the session cache
preset = { kl = "pl-pl" }  # optional cookies sent on the first request

[consent]                  # full path only: interstitial to dismiss before extraction
selectors = ["form[action*=consent]"]
reject = "button#W0wltc"

[results]
container = "ol#b_results" # optional; extraction scope and the early-stop marker for
                           # the streaming parser (stop once this element closes)
item = "li.b_algo"         # one match per result, in document order
exclude = ["li.b_ad"]      # elements (and their subtrees) never treated as items
title = "h2 a"             # text
url = "h2 a"               # element carrying the destination
url_attr = "href"
url_unwrap_param = "u"     # optional: the real URL is in this query parameter of href
url_unwrap_decode = "bing-a1-base64url"   # optional decoder for that parameter
display_url = "div.b_attribution cite"
snippet = "div.b_caption p"
date = "time"              # optional; ISO 8601 only (SearchResult.published)
date_attr = "datetime"     # optional; read this attribute instead of the text
sitelinks = "div.b_vlist2col a"   # optional; each match is a Link (text + url_attr)
kind = { news = "div.news_badge", video = "a.video" }  # first match decides SearchResult.kind
drop_params = ["utm_source"]      # query parameters stripped from the destination URL
following = ["snippet"]    # fields that live *after* the item in document order
                           # (table layouts); the first match before the next item wins
                           # (snippet, display_url, date)

[related]                  # optional: the provider's "related searches"
item = "div.related a"     # text of each match, deduplicated in document order

[answer]                   # optional: the direct answer above the results
container = "div#answer"   # optional scope
text = "p.answer-text"
source = "a.answer-source"
source_attr = "href"       # default: href

[pagination]
kind = "link"              # link: follow `next`'s attr | form: submit the hidden
                           # fields of the form containing `next` | param: use [request.page]
next = "a.sb_pagN"
attr = "href"

[block]                    # any hit => SearchError::Blocked, escalation ladder (08 §4)
status = [403, 429, 503]
selectors = ["form#captcha-form"]
patterns = ["unusual traffic"]   # literal, ASCII-case-insensitive, first 200 KB of the body
class = "captcha"          # optional; otherwise derived from the signal that fired

[expect]
blocked = true             # the recorded fixtures are block pages (detection-only plan)
```

Selector subset (implemented in `serpdom.py`, mirrored in Rust by
`tenta-dom`'s selector engine): type, `#id`, `.class`,
`[attr]`, `[attr=v]`, `[attr^=v]`, `[attr$=v]`, `[attr*=v]`, `[attr~=v]`,
`:not(compound)`, `:has(compound)`, `:first-child`, descendant and child
combinators, comma lists. Nothing else is needed by the current plans; do not
widen the subset without a plan that needs it.

## Rust implementation (`crates/tenta-search`, P1)

`tenta-search` reads these plans; `providers/check_plans.py` stays the
reference for the format and the two are held to the same output by the
crate's fixture tests. The plans in `plans/` are embedded in the binary with
`include_str!`, so nothing has to be installed next to it;
`SearchOptions::plans_dir` loads a directory on top of them at runtime, which
is how a private provider or a plan under development is reached
(`Provider::Custom("<id>")`).

What the crate adds to the reference implementation:

- **Validation at load.** Selectors are compiled through `tenta-dom`, unknown
  keys, placeholders, block classes, decoders and `following` fields are
  refused with the reason, and the error names the plan.
- **Literal block patterns.** `check_plans.py` matches `patterns` with
  `re.search`; the fast path runs no regex (`08` §3.4), so patterns are
  literal ASCII-case-insensitive substrings and a pattern containing regex
  metacharacters is refused at load. `url_unwrap_regex` is refused for the
  same reason; use `url_unwrap_param`.
- **Block classes.** A `[block]` hit becomes a `BlockClass` (`reset`,
  `rate-limited`, `captcha`, `js-required`, `layout`) from the words in the
  signal that fired, unless the plan sets `class`. A 200 whose `item`
  selector matched nothing is `layout`, and its error names the fixture to
  diff.
- **Early stop.** `container` is also the streaming parser's stop marker: the
  tree builder stops when that element closes. A plan with `[related]` or
  `[answer]` parses the whole document instead, because providers put those
  after the results.
- **Pagination.** `[request.page]` is applied for page N. `[pagination]` with
  `kind = "link"` or `"form"` needs page 1's document, which the fast path
  does not have, so those providers report a typed pagination error instead
  of silently returning page 1 again.

### Adding a provider

1. Write `providers/plans/<id>.toml` and record a fixture with
   `python3 providers/survey.py --only <id>` (or by hand, into
   `providers/fixtures/<id>/<slug>.html`; never edit or minify a fixture).
2. `python3 providers/check_plans.py <id>` until the first results print
   correctly.
3. Add a case to `crates/tenta-search/tests/fixtures.rs` replaying that
   fixture through the loopback server, pinning the result count and the
   first results, and to `crates/tenta-search/tests/blocks.rs` if the plan
   detects a block page.
4. To ship the plan inside the binary, add it to the `EMBEDDED` table in
   `crates/tenta-search/src/plan.rs`; otherwise it is reached through
   `SearchOptions::plans_dir` alone. Then `cargo nextest run -p tenta-search`.

## Survey findings (2026-09-06)

See `SURVEY.md` for the full record. Points that affect the spec or P1:

- DuckDuckGo HTML and Lite: no JavaScript, 10 results per page, redirect
  wrapper links (`uddg`), pagination through a hidden POST form (`s`, `dc`,
  `vqd`). The very first request to `html.duckduckgo.com` got a TCP reset
  before any HTTP response; the retry succeeded. No cookies set. Locale via
  `kl` (`wt-wt` default); the survey did not exercise `kl`, only
  `Accept-Language`, which does not change the results.
- Bing: no JavaScript, stable `li.b_algo` markup, 10 results, `Accept-Language`
  alone switches the UI language (`html lang="pl"`); result hrefs are
  `bing.com/ck/a?…&u=a1<base64url>` wrappers that the plan decodes.
- Brave: no JavaScript for organic results (20 per page, direct hrefs,
  Svelte-hashed class names next to stable semantic ones). The second request
  (pl-PL) returned HTTP 429 with a "request has been flagged as suspicious …
  schedule a captcha" page from Python's TLS stack, confirming the
  fingerprint dependency noted in `08-search.md`.
- Google without JavaScript: HTTP 200, 93 KB, but the body is only a
  `<meta http-equiv="refresh">` to `/httpservice/retry/enablejs` plus
  `<noscript>` text. The fast path cannot serve Google; `google_nojs.toml`
  exists only to detect that page.
- Google through headless Chromium (Playwright, Chrome for Testing 151):
  the SERP's own script redirected both queries, on two attempts 15 minutes
  apart, to `/sorry/index` ("unusual traffic from your computer network",
  reCAPTCHA). No consent page appeared because the block came first. The
  `google_chromium.toml` selectors are therefore `verified = false`.
- Fixture coverage is 2 queries per provider; the `≥ 5 fixture SERPs per
  locale class` budget (`08` §6) needs the monthly survey to accumulate them.
