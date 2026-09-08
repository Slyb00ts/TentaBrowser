# Render fixtures: capture and check recipe

`render-fixtures/` holds the render corpus (`render-fixtures/README.md` lists
the fixtures). This page is the reproducible recipe for the Chromium reference
captures and for checking an engine's output against them.

## Capture settings

The settings follow the reference corpus recipe (Obscura's
`render-repros/capture_chromium.py`), so the imported `color-checks.json`
geometry stays valid:

| Setting | Value |
|---------|-------|
| Browser | Google Chrome 144.0.7559.60 (stable channel, macOS) driven by Playwright 1.63.0 (`channel: "chrome"`, new headless mode) |
| Viewport | 900 x 1000 CSS px, `deviceScaleFactor: 1` |
| Locale, time zone, colour scheme | `en-US`, `UTC`, `light`; `prefers-reduced-motion: no-preference` |
| Load policy | `load` event, then a 2000 ms settle |
| Network | remote `http(s)` requests aborted; `data:` URLs and sibling files load |
| Screenshot | viewport only (not full page), caret hidden, animations left running |
| Checks | `getBoundingClientRect()` of every element under `<body>`; first `Range.getClientRects()` rect of every non-blank text node; rounded to 1/100 px |

The exact values used for the committed captures are recorded in
`render-fixtures/captures.json` (browser version, Playwright version, settings,
capture time, per-fixture check counts and page errors).

Captures use the platform's default fonts, so they are specific to the OS they
were taken on (macOS here). Font-dependent geometry (default serif/monospace
metrics, native form-control widths) differs on Linux and Windows; four
`color-checks.json` entries were re-baselined for that reason
(`render-fixtures/ATTRIBUTION.md`). When CI moves captures to Linux, recapture
on Linux and re-baseline in one commit. TentaBrowser itself renders with
embedded fonts (`docs/spec/06-layout-and-render.md` §6), so structural checks
on text positions are compared with tolerances, not exact equality.

## Recapture

```bash
cd render-fixtures/tools
npm install                      # Playwright, into render-fixtures/tools/node_modules (gitignored)
node capture.mjs                 # all fixtures -> <name>/chromium.png, <name>/checks.json, ../captures.json
node capture.mjs --fixture tables --fixture z-index
node capture.mjs --channel chromium          # Playwright's bundled Chromium instead of Google Chrome
node capture.mjs --executable /path/to/chrome
```

Google Chrome is found through Playwright's `chrome` channel
(`/Applications/Google Chrome.app` on macOS, `google-chrome` on Linux). To use
Playwright's pinned Chromium instead, run `npx playwright install chromium` in
`render-fixtures/tools` first and pass `--channel chromium`.

Only recapture when the corpus changes or Chrome is deliberately upgraded, and
say so in the commit (browser version from `captures.json`). Review the PNG
diffs; a recapture that changes many fixtures without a corpus change means the
browser or platform changed, which must be recorded in `captures.json`.

## Adding a fixture

1. Create `render-fixtures/<name>/index.html`. Keep it self-contained: inline
   CSS, `data:` URLs or files inside the fixture directory, no remote
   resources, no time-dependent output. Give the boxes you assert on an `id`
   or `data-check` attribute so `checks.json` names them.
2. Optionally add `color-checks.json`: a list of
   `{"name", "color": "#rrggbb", "x", "y", "width", "height", "count"}`
   entries describing solid-colour components the fixture must produce (any
   of the geometry keys may be omitted; `count` defaults to 1). Use colours
   that appear nowhere else in the fixture.
3. Run `node capture.mjs --fixture <name>` and commit `index.html`,
   `chromium.png`, `checks.json`, `color-checks.json` and the updated
   `captures.json`.
4. Add the fixture to the table in `render-fixtures/README.md`.

## Checking an engine

Place the engine's capture at `render-fixtures/<name>/<engine>.png` (same
viewport and DPR) and run:

```bash
python3 -m venv .venv && .venv/bin/pip install numpy scipy pillow   # once, outside the repository
.venv/bin/python render-fixtures/tools/check.py --engine tentabrowser --report /tmp/render-report.json
.venv/bin/python render-fixtures/tools/check.py --engine chromium     # self-check of the reference captures
```

`check.py` evaluates `color-checks.json` for both captures (the behaviour
assertions; exit status 1 on any failure) and prints pixel-distance metrics
(`rgb_mae`, `pixels_gt_50`, edge bounding-box delta, bidirectional edge
distance) as regression tripwires. Structural checks from `checks.json`
(element and text rects) are consumed by the Rust structural test in
`tenta-render` once P3 lands. Generated engine captures are not committed.
