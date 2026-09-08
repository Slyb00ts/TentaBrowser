# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
Semantic Versioning.

## [Unreleased]

### Added

- P0 foundation: workspace, 23 crate skeletons, `tenta-core` (ids, arenas,
  atoms, tiers, budgets, `FsPolicy`, tracing, embedder traits), `xtask`
  (`lint-deps`, `size-report`, `link-test`, `hello-sizes`, `notices`,
  `idl-manifest`), CI and nightly workflows, `cargo deny`/`cargo about`
  configuration and `THIRD_PARTY_NOTICES.md`.
- `tenta-idl`: WebIDL front end and generators (Rust traits, V8 and QuickJS
  glue, op tables, registration) with golden tests for a toy interface and a
  DOM excerpt.
- `tenta-testutil`: fixture HTTP server with RTT and per-path delay
  simulation, glob and client helpers; page fixtures under `tests/fixtures`.
- Benchmark harness (`bench/run.py`, `bench/scenarios.toml`) with recorded
  reference and Chromium baselines and the hello-size ladder
  (`bench/baselines/`).
- `tentabrowser-cli`: the `tentabrowser` binary. `fetch <url>` with
  `--dump {html,text,markdown,links,json,metadata,forms,snapshot}`,
  `--main-content`, `--wait {load,domcontent,networkidle}`, `--headers` and
  `-o <file>`; `search "<query>"` with `--provider`, `--limit`, `--page`,
  `--locale`, `--safe`, `--freshness`, `--site`, `--mode` and `--warm`,
  printing a table or JSON lines; `scrape <url>…` with `--from`,
  `--concurrency`, `--per-host`, `--delay`, `--max-pages`, `--follow-links
  --same-host --depth` and `--ordered`, honouring `robots.txt` by default
  (`--ignore-robots` opts out), per-host limits and `Retry-After`, and
  printing one JSON object per page. Global flags for the fingerprint
  profile, `User-Agent`, `Accept-Language`, proxy, ephemerality, private
  networks, the filesystem policy (`--allow-read`, `--allow-write`,
  `--downloads`, `--cache-dir`), thread counts and logging. Exit codes: 0 ok,
  1 transport (with the `net::ERR_*` name), 2 usage or policy, 3 HTTP ≥ 400,
  4 a blocked search provider, 5 unsupported. `--log debug` prints a `phases`
  line with the wall time of each stage of the run.
- `tentabrowser`: `Engine::search`, `Engine::warm_search` and
  `BrowserContext::search` over an internal search context that is created on
  first use, persistent under the id `search-warm`, and degrades to ephemeral
  when the filesystem policy has no writable session-cache directory. The
  search model (`SearchRequest`, `SearchResponse`, `Provider`, `Mode`,
  `SafeSearch`, `Freshness`, `Locale`, `SearchResult`, `SearchError`) is
  re-exported from the facade, as are the `robots` and `rate_limit` policies
  under `tentabrowser::net`.
- `tenta-browser`: type-keyed extension slots on `Engine` and
  `BrowserContext`, so an assembly crate above this one can attach state that
  lives and dies with them without an upward dependency.
- `tenta-testutil`: `/api/status?code=<code>&retry_after=<seconds|http-date>`
  on the fixture server, and the `robots/` fixture set behind a root
  `robots.txt` that disallows one path.
- Benchmarks: `tentabrowser` is a live subject in `bench/scenarios.toml` for
  `S-cold-fetch` and `S-static-text-cli`; `S-search-fast` and
  `S-search-cli-cold` replay a recorded DuckDuckGo SERP through the new
  `fixture_root` scenario key and the CLI's hidden `--search-endpoint`.
- `tenta-search`: the fast path of `docs/spec/08-search.md`. Parse plans from
  `providers/plans/` embedded at build time and loadable from a directory,
  validated and compiled at load; streaming extraction that stops the tree
  builder at the results container; block detection with the `reset`,
  `rate-limited`, `captcha`, `js-required` and `layout` classes and the
  ladder steps that need no JavaScript (reset retry, proxy rotation); a
  10-minute response cache whose identical concurrent queries coalesce;
  fan-out across providers merged by reciprocal rank fusion; the warm pool;
  and the JSON the CLI and the MCP tool print. Google stays `Unsupported`
  until `Mode::Full` lands in P2.
- Single-libcrypto link test ladder (`tests/linktest`), WPT runner skeleton
  (`wpt/`), render fixture corpus with Chromium captures
  (`render-fixtures/`), golden-site API inventories and the generated
  `idl/MANIFEST.toml`, search provider survey with SERP fixtures
  (`providers/`), CDP method checklist derived from Playwright and Puppeteer
  sources (`crates/tenta-cdp/checklist/`), developer recipes in `docs/dev/`.
