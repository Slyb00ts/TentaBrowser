# TentaBrowser

Headless browser engine written from scratch in Rust for the TentaFlow
agent platform. Target capabilities include JavaScript (V8 and QuickJS-ng),
its own DOM, CSS (Stylo), layout and paint, audio, video and WebRTC, a fast `search` mode,
Chrome DevTools Protocol for Puppeteer and Playwright, an MCP server, and a
library API for macOS, Linux, Windows, Android and iOS.

## Status

Status (2026-09-23): P0 foundation and the P1 network, HTML, DOM and search
implementation are present. The roadmap recorded the P0 exit locally; P1
exit-gate revalidation is not included in this status line. Tier 0/1 paths
include HTTP/1.1 and HTTP/2 with a Chrome fingerprint, cookies, cache and
session cache, a streaming HTML parser into an arena DOM, extraction, the
search fast path over DuckDuckGo, Bing and Brave, and the `tentabrowser` CLI.

P2 is in active development. Both V8 and QuickJS now execute pages with the
shared event loop, timers, watchdogs and generated DOM bindings. Fetch,
XHR, forms, Blob/File and XML parsing have partial implementations. Stylo
backs inline styles, CSS.supports and a native stylesheet model; live
stylesheet bindings and style/link loading remain unfinished. Focused local
Range and React 18/Vue 3 mount/render/click probes do not establish hydration
or the P2 exit gates. The in-process `S-js-xhr` probe has an initial measured
baseline, with complete budget validation still open.

Remaining P2 work includes QuickJS parity (including timezone, Intl and
Wasm), workers/SAB, storage, streams/WebSocket, Web Crypto, full CSSOM,
history/location, iframes/CSP/observers, Google Full search and the required
WPT, conformance and budget gates. See the
[implementation ledger](docs/dev/implementation-status.md) for package
status and the scope of validation evidence.

P3 and later work is still outstanding: layout, paint and render (P3), CDP,
MCP and FFI hosts (P4), audio/video (P5–P6), WebRTC (P7), and mobile,
hardening and 1.0 work (P8). A Tier 1 page still pays for none of the higher
tiers, so `fetch --dump text` remains independent of JavaScript and rendering.

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
`networkidle`. Exit codes: `0` ok, `1` transport or script execution failed
(`net::ERR_*` identifies transport errors on stderr), `2` usage or filesystem policy, `3` the
server answered 4xx or 5xx (`--allow-http-errors` accepts it).

### `fetch --eval`: JavaScript after navigation

```bash
cargo build --release -p tentabrowser-cli --features v8
tentabrowser fetch https://example.com/ --eval 'document.title'
tentabrowser fetch https://example.com/ --wait networkidle --eval '({title: document.title})'
```

`--eval` runs one expression in the loaded page's world after the chosen
`--wait` condition. Strings are printed directly; other JSON-serializable
values are printed as JSON. Cycles, BigInt and values without a JSON
representation produce a script error. It cannot be combined with `--dump`,
`--json` or `--main-content`. A build without `v8` refuses it with exit code
`5` before making a request. Fetches without `--eval` retain the Tier 1 path.

The expression shares the navigation timeout and the page's script budget.
Promise results are rejected by the CLI rather than awaited, and asynchronous
work started by the expression is not settled again. The library exposes this operation as
`Page::goto_with_eval`; the current runtime is released when navigation
returns, so this is not a persistent evaluation session.

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

### `screenshot`: one URL, one image

```bash
cargo build --release -p tentabrowser-cli --features render
tentabrowser screenshot https://example.com/ -o page.png --allow-write .
tentabrowser screenshot https://example.com/ --full-page -o full.png --allow-write .
tentabrowser screenshot https://example.com/ --selector main --format jpeg --quality 85 -o main.jpg --allow-write .
tentabrowser screenshot https://example.com/ --clip 0,0,800,600 --dpr 2 -o - > clip.png
```

Needs a build with `render` (the `full` feature set includes it); a build
without it exits `5` before making a request. `-o` takes a file, which needs
`--allow-write`, or `-` for stdout (refused on a terminal). The default file
is `screenshot.png`. `--full-page`, `--selector` and `--clip` are mutually
exclusive; without one of them the viewport is captured. The viewport is
`--width` by `--height` CSS pixels (default 1280 by 800) at `--dpr` device
pixels per CSS pixel (default 1). `--quality` (1 to 100, default 90) needs
`--format jpeg`. `--wait` takes the same values as `fetch`, and the global
`--timeout` (milliseconds) bounds the navigation. `--js on|off` decides whether
the page's scripts run before the capture; the default is `on` in a build with
`v8` and `off` otherwise, and `--js on` needs `v8`, so JavaScript pages need the
`full` build (exit `5` without it). `--dpr 2` doubles the output size. An
existing file at `-o` is overwritten without a prompt. `file://` URLs are not
supported yet; that is planned, and until it lands they fail with
`net::ERR_BLOCKED_BY_CLIENT` (exit `1`).

One line on stderr says what was written. Anything the rasteriser could not
draw is listed after it as a warning, and the output is still written; a page
taller than the size limits is cut and reported as such. `--json` prints that
line as a JSON object instead. Exit codes: `0` ok, `1` navigation or render
failed, `2` usage, filesystem policy, a selector that matches nothing or an
element without a box, `3` the server answered 4xx or 5xx
(`--allow-http-errors` accepts it), `5` the build has no render backend.

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

## DOM call benchmark

`bench/dom_calls.py` measures the real V8 backend's `S-dom-calls` scenario.
Build and preserve the baseline executable before changing the engine, then
build the candidate and compare them:

```bash
cargo build --release -p tenta-js-v8 --example dom_calls
cp target/release/examples/dom_calls /tmp/dom-calls-before
# Apply the change, then rebuild the example with the same command.
python3 bench/dom_calls.py --baseline /tmp/dom-calls-before \
  --candidate target/release/examples/dom_calls --runs 25
```

Each process warms both methods for 10,000 calls and measures one million
`hasAttribute` and `getAttribute` calls. Interleaved runs report p50/p95 of
the per-process mean call time, wall time and peak RSS, with raw samples and
binary hashes in the result artifact. This does not independently establish
which V8 dispatch path was selected; differences below 10% are noise.
