# TentaBrowser

Headless browser engine written from scratch in Rust for the TentaFlow
agent platform: real JavaScript (V8 and QuickJS-ng), own DOM, CSS (Stylo),
layout and paint, audio, video and WebRTC, a lightning-fast `search` mode,
Chrome DevTools Protocol for Puppeteer and Playwright, an MCP server, and a
library API for macOS, Linux, Windows, Android and iOS.

## Status

P1 (network, DOM, search) is implemented: HTTP/1.1 and HTTP/2 with a Chrome
fingerprint, cookies, cache and session cache, a streaming HTML parser into
an arena DOM, extraction, the search fast path over DuckDuckGo, Bing and
Brave, and the `tentabrowser` CLI. JavaScript (P2), layout and paint (P3),
the protocol hosts (P4) and media (P5–P7) are not implemented yet: a page
declares its tier and nothing above it is initialised, so `fetch --dump text`
pays for none of them.

`docs/spec/00-index.md` is the specification, `docs/spec/16-roadmap.md` the
plan. Agents and contributors start with `AGENTS.md`.

## The CLI

```bash
cargo build --release -p tentabrowser-cli   # target/release/tentabrowser
```

### `fetch`: one URL, one extraction

```bash
tentabrowser fetch https://example.com/                    # text, the default
tentabrowser fetch https://example.com/ --dump markdown
tentabrowser fetch https://example.com/ --dump links       # JSON lines
tentabrowser fetch https://example.com/ --json             # everything, one JSON object
tentabrowser fetch https://example.com/ --main-content     # the article, not the chrome
tentabrowser fetch https://example.com/ --headers          # status and headers on stderr
tentabrowser fetch https://example.com/ -o page.txt --allow-write .
```

`--dump` takes `html`, `text`, `markdown`, `links`, `json`, `metadata`,
`forms` and `snapshot`; `--wait` takes `load` (default), `domcontent` or
`networkidle`. Exit codes: `0` ok, `1` the request never reached a server
(the `net::ERR_*` name is on stderr), `2` usage or filesystem policy, `3` the
server answered 4xx or 5xx (`--allow-http-errors` accepts it).

### `search`: the providers' public pages, no API keys

```bash
tentabrowser search "rust browser engine"
tentabrowser search "rust browser engine" --provider ddg,brave --limit 20 --json
tentabrowser search "pogoda warszawa" --locale pl-PL --freshness day
tentabrowser search "layout engine" --site servo.org --safe off
```

Providers: `ddg`, `ddg-lite`, `bing`, `brave`. Google needs JavaScript and
arrives in P2; asking for it exits `5` with that message. A provider that
blocks the query exits `4` with the block class. `--json` prints one JSON
object per result and a trailing metadata line with the timings.

### `scrape`: many URLs, politely

```bash
tentabrowser scrape https://example.com/a https://example.com/b
tentabrowser scrape --from urls.txt --concurrency 8 --per-host 2 --delay 250
tentabrowser scrape https://example.com/ --follow-links --same-host --depth 2 --max-pages 50
```

One JSON object per page on stdout: `url`, `final_url`, `status`, `title`,
the extraction you asked for with `--dump`, `timing_ms`, and `error` when the
page was not fetched. `robots.txt` is honoured by default (`--ignore-robots`
opts out), per-host concurrency and spacing are enforced, and `Retry-After`
on 429 or 503 is waited out before one retry. The `User-Agent` is the
profile's ordinary Chrome UA, here as everywhere.

### Everywhere

`--profile <name|path|none>`, `--user-agent`, `--accept-language`,
`--proxy`, `--ephemeral`, `--allow-private-network`, `--allow-read`,
`--allow-write`, `--downloads`, `--cache-dir`, `--timeout`, `--io-threads`,
`--page-threads`, `--log <level>`, `--log-unredacted`, `--json`.

The filesystem is closed by default: no `file://`, no output paths, no
downloads until `--allow-read`, `--allow-write` or `--downloads` opens a
directory. So are private networks, until `--allow-private-network`.

## The library

```rust
use tentabrowser::{ContextOptions, Engine, EngineSearch, SearchRequest, Tier};

let engine = Engine::builder().max_tier(Tier::Dom).build()?;
let ctx = engine.new_context(ContextOptions::default())?;

let doc = ctx.fetch("https://example.com").await?;
println!("{} {}", doc.status(), doc.text().len());

let results = engine.search(SearchRequest::new("rust browser engine")).await?;
println!("{}", results.to_json_lines());

engine.close();
```

`docs/spec/11-library-api.md` is the full API; `crates/tentabrowser` is the
crate that implements it.
