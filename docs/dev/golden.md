# Golden-site API inventories

How to re-capture the golden-site API inventories (`golden/inventory/*.json`,
`docs/spec/15-testing-and-benchmarks.md` §7) and regenerate
`idl/MANIFEST.toml` from them. The capture is quarterly; the manifest is
regenerated whenever an inventory or `idl/phase-rules.toml` changes.

## What the tracer does

`golden/tracer/` is a small Node + Playwright harness:

- `inject.js` runs in every window realm before any page script
  (`context.addInitScript`, i.e. `Page.addScriptToEvaluateOnNewDocument`).
  It walks every interface object reachable from the global (`Reflect.ownKeys`
  over constructors and prototypes, the global's own `[Global]` members, the
  `CSS`, `console`, `WebAssembly` and `Intl` namespaces), replaces each method,
  getter and setter with a `Proxy` and counts `Interface.member` accesses by
  kind (get / set / call / construct). Semantic hooks record CSS property
  names (`CSSStyleDeclaration` setters and `getComputedStyle` reads,
  `insertRule` bodies, `CSS.supports`, `matchMedia`), a one-time walk of every
  reachable stylesheet (properties, selector pseudo-classes, at-rules, shadow
  roots included), media type probes (`canPlayType`, `isTypeSupported`,
  `decodingInfo`, EME key systems, MSE append volume), `RTCPeerConnection`
  configuration and ICE candidate types, worker / service worker / worklet
  script URLs, `AudioWorkletNode` processor options, Intl constructor
  locales and options, WebAssembly module sizes and streaming URLs, canvas
  and WebGL parameter probes, IndexedDB / Cache / BroadcastChannel names,
  `getHighEntropyValues` hints and `permissions.query` names.
- `tracer.js` launches headless Chromium (the full `chromium` channel, not
  the headless shell), injects the tracer, attaches CDP `Network` sessions to
  the main frame and to out-of-process iframes (HTTP protocol versions,
  content encodings, `alt-svc`, WebSocket frames), injects into dedicated and
  service workers when they attach, drives the scenario from `sites.js`,
  harvests every frame and worker at step boundaries and at
  `beforeunload` (the page flushes through `console.debug`), merges the
  dumps and writes one JSON sorted by count. `scrub.js` removes public IPs and
  the query strings of long redirect URLs from `meta` before writing.
- `sites.js` holds the four scenarios (DuckDuckGo JS + HTML endpoints,
  Google SERP with consent handling and block detection, YouTube home +
  one muted video played for 20 s, Teams pre-auth shell + login page) and
  a generic `url:<https://...>` scenario for ad-hoc captures.

Page-side evaluations made by the harness wrap their body in
`tracer.quiet(...)` so they are not counted as site usage; Playwright's own
locators run in the utility world and are never counted.

## Re-running the capture (quarterly)

```bash
cd golden/tracer
npm ci                              # playwright pinned in package-lock.json
npx playwright install chromium     # the Chromium build that Playwright version expects

# One run per site; each writes golden/inventory/<site>.json.
node tracer.js ddg     --out ../inventory/ddg.json     --scratch /tmp/golden --verbose
node tracer.js teams   --out ../inventory/teams.json   --scratch /tmp/golden --verbose
node tracer.js google  --out ../inventory/google.json  --scratch /tmp/golden --verbose
node tracer.js youtube --out ../inventory/youtube.json --scratch /tmp/golden --verbose

# Ad-hoc capture of any page (not committed):
node tracer.js url:https://example.org --out /tmp/example.json
```

`--scratch` receives screenshots (`<site>-<step>.png`) for eyeballing what
the site actually served; `--verbose` echoes page console output and
uncaught errors to stderr; `--headed` opens a visible browser.

Read `meta` before committing a capture:

- `meta.notes` says what happened (redirects, consent clicks, result counts,
  playback state); `meta.limitations` lists what the capture does not cover.
- `meta.page_errors` and `meta.tracer_errors` must not show the tracer
  breaking the site (a wrapper throwing, a custom element failing to
  upgrade). `meta.unwrappable` lists the `[LegacyUnforgeable]` members that
  cannot be traced.
- Google regularly serves the `/sorry/` reCAPTCHA page to headless traffic;
  the scenario records `blocked=true` and keeps the inventory of the
  interstitial. Do not retry in a loop and do not add evasion: re-run once
  from a different network later, or accept the block page for that quarter.
- Teams cannot be captured past the Microsoft login without a test tenant;
  the committed inventory covers the pre-auth shell (two workers, the
  service worker) and the login page only. Capturing the authenticated client
  needs a scenario extension that signs in with a test tenant (`07-media.md`
  §3.2), to be added when one exists.
- YouTube needs the consent dialog accepted and autoplay allowed; the
  scenario passes `--autoplay-policy=no-user-gesture-required`, mutes the
  video and checks that `currentTime` advances. Chromium without proprietary
  codecs is served VP9/AV1 + Opus, so H.264/AAC MSE paths do not appear.
- The user agent has `HeadlessChrome` replaced by `Chrome`; Playwright then
  derives a consistent `navigator.userAgentData`. Nothing else is changed.

Known limitations of the method (also stated in each file's
`meta.limitations`): workers are injected after they start (their first
synchronous statements are missed, and a worker that terminates before a
harvest loses the counts since the previous one); worklet global scopes are
not traced; cross-origin window property access bypasses the wrapped
prototypes; ECMAScript intrinsics other than Intl, WebAssembly,
SharedArrayBuffer and Atomics are not traced; `'X' in window` feature checks
are invisible to a Proxy on members.

Rate limit yourself to one run per site per capture. The sites are public
and the capture is read-only, but the runs are not cheap for them either.

## Validating the tracer offline

Before a quarterly capture, run the tracer against a local fixture (any page
that exercises custom elements, a worker with `SharedArrayBuffer`, a Wasm
module, canvas/WebGL, `AudioContext`, `RTCPeerConnection`, MSE probes,
Intl and storage; see the fixture used in P0 under `tests/fixtures/` once it
lands, or write a throwaway one) and check that `meta.page_errors` is empty
and the expected sections (`css`, `media`, `rtc`, `wasm`, `workers`,
`intl`) are populated. Playwright drops console messages emitted from
`pagehide` and `unload` of a navigated-away document; the flush must stay
on `beforeunload`.

## Regenerating the manifest

```bash
python3 idl/gen_manifest.py            # rewrites idl/MANIFEST.toml
python3 idl/gen_manifest.py --check    # CI: fails when the manifest is stale
```

The generator needs Python 3.11+ (`tomllib`) and nothing else. Assignment
rules live in `idl/phase-rules.toml`; unassigned interfaces are listed at the
end of the manifest with the reason (no matching rule, or a family the
roadmap does not schedule). To place one, add or edit a `[[rule]]` and
regenerate; never edit `MANIFEST.toml` by hand. The workflow is described in
`idl/README.md`.

## Reading an inventory

Top-level keys of `golden/inventory/<site>.json`:

| Key | Content |
|-----|---------|
| `meta` | site, URLs, date, Chromium and tracer versions, user agent, navigations, steps, notes, limitations, page errors, realm/worker counts, unwrappable members |
| `interfaces` | `Interface -> { total, total_top, member_count, realms, members: { member: { get, set, call, construct, static } } }`, sorted by total |
| `events` | `addEventListener` types per target interface |
| `elements`, `custom_elements` | `createElement` tag names, `customElements.define` names |
| `css` | property names set/read/computed/inserted, `CSS.supports` and `matchMedia` queries, the static stylesheet walk |
| `media`, `audio`, `canvas`, `webgl` | codec probes and MSE volume, AudioContext options and worklet nodes, canvas contexts, WebGL parameters and extensions |
| `workers`, `wasm`, `intl`, `rtc`, `storage`, `navigator`, `permissions`, `websockets`, `event_sources` | as named |
| `network` | request/response counts, HTTP versions (per host too), resource types, status codes, hosts, MIME types, content encodings, `alt-svc`, WebSockets, failures |
| `realms` | every window/worker realm that reported, with its access volume |

`total_top` counts accesses from the top-level document only; ad and
consent iframes report separately and are folded into `total`.
