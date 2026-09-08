# Search provider survey and parse plans

How to refresh the provider survey, add a fixture and change a parse plan.
The format and the current findings are in `providers/README.md` and
`providers/SURVEY.md`; the model is `docs/spec/08-search.md`.

## Check plans against fixtures (offline, CI)

```bash
python3 providers/check_plans.py            # prints 3 results per fixture, exit 1 on drift
python3 providers/check_plans.py brave --show 10
python3 providers/check_plans.py --json     # for tooling
```

A fixture is "ok" when the plan extracts results, "blocked" when the plan's
`[block]` section recognises it, and "FAIL" when neither happens: that is a
provider layout change, and the fix is a plan bump (`version += 1`) plus a
fresh fixture, not a debugging session.

## Refresh the survey (live, monthly)

```bash
cd providers/tools && npm install && cd -          # once; playwright-core pinned
python3 providers/survey.py                         # all providers, both queries
python3 providers/survey.py --only duckduckgo_html --query rust-browser-engine
python3 providers/survey.py --report-only           # regenerate SURVEY.md only
```

The script issues exactly one request per query per provider with a Chrome
desktop User-Agent, writes the raw body to
`providers/fixtures/<provider>/<slug>.html`, records status, redirects,
headers (cookie values dropped), block signatures and timing in
`providers/survey-results.json`, then applies the plan to the fixture and
renders `providers/SURVEY.md`. Partial runs merge into the saved record and
keep earlier attempts under `previous_attempts`.

Google through headless Chromium uses `providers/tools/google_chromium.mjs`
(Playwright + a Chromium build from the Playwright cache, override with
`TENTA_CHROMIUM`). Expect `/sorry/` interstitials from consumer IPs; the
survey records them rather than retrying.

## The Rust side (`tenta-search`)

The plans are data for `crates/tenta-search`, which embeds `plans/*.toml` at
build time and can load more at runtime:

```bash
cargo nextest run -p tenta-search                    # plans, fixtures, blocks, cache, merge
cargo nextest run -p tenta-search --release --run-ignored all --no-capture   # S-search-fast
```

`crates/tenta-search/tests/fixtures.rs` replays every recorded SERP through
`tenta_testutil::FixtureServer` with the plan's own endpoint path
(`SearchOptions::endpoint_override` swaps the host), so a plan change that
breaks extraction fails there with the same numbers `check_plans.py` prints.
The differences between the Python reference and the Rust implementation
(literal block patterns, no `url_unwrap_regex`, refused API placeholders,
block classes, the early stop) are listed in `providers/README.md` under
"Rust implementation".

## Add a query or a provider

1. Add the query to `QUERIES` in `providers/survey.py` (slug, text,
   `Accept-Language`, language, region) or the provider to `HTTP_PROVIDERS`
   / `CHROMIUM_PROVIDERS`.
2. Write `providers/plans/<provider>.toml` following the format in
   `providers/README.md`; keep to the selector subset of `serpdom.py`.
3. Run the live survey for that provider only, inspect the fixture, then
   `check_plans.py <provider>` until the first results print correctly, and
   add the fixture to `crates/tenta-search/tests/fixtures.rs` (and to the
   `EMBEDDED` table in `crates/tenta-search/src/plan.rs` if the plan ships
   with the binary).
4. Commit plan, fixture and the regenerated `SURVEY.md` /
   `survey-results.json` together.

Fixtures are the recorded SERPs the Rust fast path is benchmarked against
(`08` §6: 300 KB SERP to results in ≤ 2.5 ms), so never edit or minify them.
