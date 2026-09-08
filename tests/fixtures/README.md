# Fixture pages

Offline pages served by the loopback fixture server in `crates/tenta-testutil`
(in-process from tests, or `cargo run --release -p tenta-testutil --bin
fixture-server` for `bench/run.py`). Each directory has a `README.md` saying
what it exercises and, where meaningful, an `expected.txt` with the settled
text output.

| Directory | Purpose |
|-----------|---------|
| `tiny/` | 200-byte page for `S-cold-fetch` and server tests |
| `static-article/` | ~1 MB static article, no JS: `S-static-text*`, `S-screenshot`, `S-fullpage` |
| `js-xhr/` | inline script, one XHR and one `fetch` mutating the DOM: `S-js-xhr` |
| `dynamic-scripts/` | chained dynamic `<script>` insertion: `S-dynamic-scripts` |
| `spa-placeholder/` | README only until the React 18 demo is imported in P2 |
| `encoding-cp1250/` | Polish page in `windows-1250` with a `<meta charset>`: encoding sniffing and transport-charset precedence |
| `robots/` | three pages behind the root `robots.txt`, one of them disallowed: `scrape` politeness |

`robots.txt` sits at the root of the fixture tree, because that is the only
place RFC 9309 lets a crawler look for it. It disallows `/robots/private/`.

Server endpoints available to every page: `/api/echo` (JSON echo of method,
path, query, headers and body), `/api/slow?ms=N`, `/api/never` (never
answers), `/api/sse` (an event every 100 ms),
`/api/redirect?to=<path>&n=<hops>&status=<code>` (a redirect chain),
`/api/cookie?set=<Set-Cookie value>` (sets a cookie and echoes the `Cookie`
header it received), and
`/api/status?code=<code>&retry_after=<seconds|http-date>` (any status with an
optional `Retry-After`, for the politeness paths of `scrape`).

Any file may be served with an explicit content type through
`?content_type=<value>`, which is how a fixture is fetched without a `charset`
parameter so its own `<meta charset>` decides.
