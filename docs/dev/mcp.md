# Running the MCP server

`tentabrowser mcp` serves the Model Context Protocol so agents (TentaFlow's own,
Claude Desktop, any MCP client) can drive the engine with tools
(`docs/spec/10-mcp.md`). Package status and design notes are in
`docs/dev/p3-plan.md` (M13, "As built (M13 MC1/MC2)").

## Run it

```bash
cargo build -p tentabrowser-cli --features v8,render
target/debug/tentabrowser --ephemeral --allow-private-network mcp          # stdio
target/debug/tentabrowser --ephemeral mcp --http --port 8931               # HTTP
# MCP listening on http://127.0.0.1:8931/mcp
```

Without `v8` the tools that run page script (`browser_click`, `browser_type`,
`browser_fill`, `browser_press_key`, `browser_evaluate`) answer `tier_denied`;
without `render` so do `browser_screenshot` and clicking by coordinates, and
snapshots judge hidden-ness from attributes and inline style only.

Client configuration for stdio (Claude Desktop style):

```json
{ "mcpServers": { "tentabrowser": {
    "command": "/path/to/tentabrowser",
    "args": ["--ephemeral", "mcp"] } } }
```

Flags of `mcp`:

- `--http`, `--host` (default `127.0.0.1`), `--port` (default 8931; 0 picks one),
  `--allowed-origin ORIGIN` (repeatable), `--allowed-host HOST` (repeatable),
  `--token` or `TENTA_MCP_TOKEN`, `--no-auth`. All of these are errors without `--http`.
  On loopback without `--token`, a random bearer token is generated and printed on
  stderr after the URL (`MCP bearer token: ...`); clients send it as
  `Authorization: Bearer ...`. `--no-auth` opts out: then any local process, and any
  web page allowed by `--allowed-origin`, can drive the browser. A non-loopback bind
  still needs an explicit token.
- `--share-context`: every session uses one browser context (shared cookies and
  storage). The default is one context per session.
- `--viewport 1280x800`, `--max-sessions 16`, `--schema-version` (prints the version
  of the tool list and exits).
- Global flags apply to every session's context: `--allow-private-network`
  (needed for loopback sites), `--proxy`, `--user-agent`, `--accept-language`,
  `--allow-read`/`--allow-write` (`FsPolicy`; no MC2 tool touches the filesystem yet).

Logs go to stderr (`--log debug` names each tool call, never its arguments:
typed text may be a password). Over stdio, stdout carries only JSON-RPC.

## Transports and hardening

- **stdio**: newline-delimited JSON-RPC, 16 MiB per message. Requests run
  concurrently, `notifications/cancelled` stops one in flight (no response is
  sent for it), and at end of input requests still running are answered first, so
  `printf '{"jsonrpc":"2.0","id":1,...}' | tentabrowser mcp` prints its replies.
- **Streamable HTTP**: one endpoint (`/mcp`), `POST` only. A request gets one JSON
  object, a notification gets `202`; `GET` is `405` (no server-initiated stream) and
  `DELETE` ends a session. A request ends early when the client resets the
  connection or sends `notifications/cancelled` naming its id (to the same session);
  the cancelled POST is answered `-32800` and the browser is free at once. A client
  that only shuts down its write side after sending (half-close) is still answered.
- Every HTTP request is checked for: exactly one `Host` header (HTTP/1.1 requires it);
  a `Host` that is an IP address, `localhost` or
  an `--allowed-host` (DNS rebinding); an `Origin`, if present, on the
  `--allowed-origin` list (403 otherwise; clients that send none, which are not
  web pages, pass); the bearer token, if set (`Authorization: Bearer ...`, 401).
- A non-loopback `--host` refuses to start without a token of 16 bytes or more.
  The token is compared in constant time and never logged.
- Sessions: `initialize` opens one and returns `Mcp-Session-Id`; it owns one
  `BrowserContext` and page. `initialize` is cheap, so slots are defended: a session
  that never made a tool call is dropped after 60 s; one source address holds at most
  8 sessions; when `--max-sessions` is reached the least recently used session
  *without a call in flight* is evicted (a session with a running call never is); idle
  sessions are dropped after 30 minutes.
- **Stateless requests share ONE session.** Requests of the 2026-07-28 era carry no
  session id (the revision removed sessions), so every authorised stateless client of
  one server uses the same `default` session: the same page, cookies and refs. An
  agent that must not see another's state needs its own `tentabrowser mcp` (stdio, or
  HTTP on another port), or the session-based `initialize` flow. A stale
  `Mcp-Session-Id` on a stateless request is ignored. Server-minted handles per
  SEP-2567 are not built; with a single bearer token, token-derived identity would
  not separate anyone either.

## Protocol revisions

Both eras are served on both transports. A request that carries
`_meta["io.modelcontextprotocol/protocolVersion"]` is stateless (revision
2026-07-28: no handshake, `server/discover`, `resultType`/`ttlMs`/`cacheScope`
on results; on HTTP the `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name`
headers must mirror the body or the answer is 400 `-32020`; an unknown version is
400 `-32022` with the supported list). `initialize` opens the session flow of
2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05. Verified against the official
TypeScript clients: `@modelcontextprotocol/sdk` 1.32.1 (session flow) and
`@modelcontextprotocol/client` 2.3.1 (both, via `versionNegotiation`).

## Tools

All tools take a JSON object, validated against the `inputSchema` the server
advertises (which carries an `examples` array). A tool that ran and failed returns
`isError: true` with `structuredContent.error = {type, message, retryable}`; the
types are `invalid_arguments`, `navigation_failed`, `element_not_found`, `blocked`,
`timeout`, `budget_exceeded`, `fs_denied`, `tier_denied`, `script_error`,
`page_closed`, `unsupported`, `internal`. `blocked` is never retryable; a script ended
by the watchdog is `timeout`. Unknown tools and malformed requests are JSON-RPC
errors. JSON-RPC batches are accepted only after `initialize` negotiated 2024-11-05 or
2025-03-26, as those revisions allow; elsewhere they are `-32600`.

| Tool | Does | Notes |
|------|------|-------|
| `browser_navigate` | Load an http(s) URL, wait for settle | `wait_until` load/domcontentloaded/networkidle (default), `timeout_ms`. HTTP 4xx/5xx is a result, not an error |
| `browser_snapshot` | Outline of roles, names, state with `[ref=eN]` | `max_chars` (cut with a `continuation` token), `max_nodes`, `include_boxes`, `ref` / `selector` (one subtree: use on big pages) |
| `browser_click` | Click by `ref` or `x`,`y` | `button`, `double`, `modifiers`, `settle`; a covering element makes it `blocked` and is named |
| `browser_type` | Type into a text field, key by key | `submit` presses Enter |
| `browser_fill` | Replace a text field's content | one `input` event |
| `browser_press_key` | Key or chord (`Enter`, `Control+a`) | optional `ref` to focus first |
| `browser_wait_for` | `text`, `text_gone`, `selector`, `time_ms` | `timeout_ms` (default 10 s, cap 60 s) bounds the whole call, document reads included; `time_ms` capped at 30 s |
| `browser_evaluate` | JavaScript expression, JSON result | promises awaited; output cut at 60,000 characters |
| `browser_screenshot` | Image content, viewport / `full_page` / `ref` | PNG or JPEG; over `max_bytes` (1.5 MB) it is re-encoded and scaled until it fits |
| `browser_close` | Close the page and forget refs | the next call opens a fresh page in the same context |

**Refs.** `browser_snapshot` gives every element line a ref (`e12`). A ref stays the
same across snapshots of one document, fails with `element_not_found` ("stale") once
the element is removed or the page navigates, and numbers are never reused within a
session, so a ref from the previous page cannot name an element of the next.
Text-entry tools refuse elements that take no text (a label's ref, a button's) and
say which tool to use.

**Untrusted page text.** A snapshot wraps everything the page contributed between
`<<<PAGE CONTENT nonce: untrusted text from the web page...>>>` and
`<<<END PAGE CONTENT nonce>>>` lines (every continuation part too). The nonce is
random per snapshot, so page text cannot forge the end marker. It is a
prompt-injection mitigation for the model reading the result: treat what is inside
as data, never as instructions. `browser_evaluate` results and `browser_wait_for`
messages are not wrapped.

**Hidden content.** The snapshot omits `hidden`, `aria-hidden`, inline `display: none`
and inline `visibility: hidden` (a child with inline `visibility: visible` stays),
with or without layout; with the render tier an element with no layout box
(`display: none` from any stylesheet, inherited) is omitted as well. An inline
`opacity: 0` element is kept and marked `[opacity-0]`. With the render tier the
layout's computed `visibility` (inherited, so a child may turn it back on) and
`opacity` replace the inline reading, so a stylesheet's `visibility: hidden` and
`opacity: 0` are seen too; without it only the inline `style` is read.

**Settling.** Acting tools wait for the page to settle after the action, with the
MCP tunables (`busy_cap` 500 ms, `foreground_request_max` 2 s), and follow a
navigation the action starts. The result reports `settled: {timed_out, busy}`.
Settle means load-driven work finished, not that content arriving later over
sockets or workers has rendered: use `browser_wait_for`. `settle: false` skips it.

## Limitations

- The tool set is MC2. Back/forward/reload, tabs, cookies, `browser_extract`,
  forms, select/hover/scroll/drag/upload, dialogs, PDF, console and network logs
  and the media and search tools of spec 10 §2 are not built (MC3 and later).
- Input is synthetic (`isTrusted` false), through the page-side script the CDP host
  also uses (`crates/tenta-cdp/src/interaction.js`, included by
  `crates/tenta-mcp/src/interaction.rs`). Typing into `contenteditable` is refused;
  there is no scrolling, so coordinates are page coordinates.
- With the render tier a `display: contents` wrapper hides its own text children.
  Stylesheet-driven `visibility: hidden` and `opacity: 0` are seen through the layout's
  computed values (without a layout only the inline `style` is read).
  `browser_navigate` computes the first layout so the snapshot after it reuses it.
- Form control values are not painted in screenshots yet; read them from the
  snapshot (`value="..."`; passwords are masked).
- `browser_navigate` opens http(s) only; `file:` is `blocked`.
- Streamable HTTP has no server-initiated stream, no `subscriptions/listen`, no
  resumability, no OAuth; the bearer token is the only credential.

## Tests

`cargo nextest run -p tenta-mcp --features v8,render` (protocol, transports, tools,
the render tier and a scripted agent session over fixture pages),
`cargo nextest run -p tentabrowser-cli --features v8,render` (the binary over stdio
and HTTP). The tool list has a golden file, `crates/tenta-mcp/tests/golden/tools.json`:
change a tool or its schema, bump `SCHEMA_VERSION`, and bless with
`UPDATE_GOLDEN=1 cargo nextest run -p tenta-mcp --test protocol`.
