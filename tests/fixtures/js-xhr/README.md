# js-xhr

The `S-js-xhr` fixture (`docs/spec/15-testing-and-benchmarks.md` §2): an
inline script issues one `XMLHttpRequest` (`GET /api/echo?item=alpha&via=xhr`)
and one `fetch` (`POST /api/echo` with a JSON body) against the fixture
server's echo endpoint and writes each reply into a pre-existing `<li>`
slot. When both have completed it sets `#status` to
`js-xhr: done (2 requests)`, the title to `JS + XHR + fetch: done` and
`window.__settled = true`.

The two responses race, but the slots are fixed and only deterministic
fields of the echo (`query`, the parsed JSON body) are used, so the settled
DOM is identical on every run and every engine.

Exercises: inline script execution, XHR and `fetch` on the page's event loop,
JSON parsing, DOM mutation after network completion, settle-after-XHR (the
page must not be considered settled at `load` because both requests are
still in flight). The settle-machine test expects `busy: false` within one
task of the last mutation.

Expected settled text: `expected.txt`. Harness marker:
`js-xhr: done (2 requests)`.
