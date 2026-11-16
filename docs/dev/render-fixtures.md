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
distance) as regression tripwires. Generated engine captures are not committed.

## The corpus test and its ratchet

```bash
cargo nextest run -p tenta-render --test corpus
```

`crates/tenta-render/tests/corpus.rs` renders every fixture through the Tier-1
pipeline at 900x1000, DPR 1, writes `<name>/tentabrowser.png` and grades each
`color-checks.json` entry natively (the same component labelling and
thresholds as `check.py`, no Python needed). Fixtures in the `SCRIPTED` list of
`corpus.rs` need the JavaScript page path, which `tenta-render` cannot depend on,
and are skipped by name; the test fails if a fixture with `<script>` is not in
the list. One difference from `check.py`: it counts a component's area as the
colour pixels inside its bounding box (which can include other components),
the native check counts the component's own pixels. Only the 20 pixel noise
floor is affected.

- `EXPECTED.toml` maps every colour check to `pass`, `fail`,
  `deferred:<milestone>` or `informational`. The test fails when a `pass`
  check fails, or when a check is missing from the file. Checks that start
  passing are listed in the summary; moving one to `pass` is a deliberate edit.
  `UPDATE_EXPECTED` rewrites the file; existing `pass` entries always stay and
  new checks take their milestone from the gate file. `UPDATE_EXPECTED=1`
  promotes the passing `M1-GATE.toml` checks, `UPDATE_EXPECTED=all` every
  passing check, `UPDATE_EXPECTED=a,b` every passing check of fixtures `a`, `b`.
- `M1-GATE.toml` is frozen; the corpus test only reads it. The M1 exit is the
  test `m1_gate_is_green`, which runs by default.
- `RENDER_CORPUS_PYCHECK=1` also runs `tools/check.py` on our captures and
  fails on any per-check verdict difference. Missing numpy, scipy or pillow
  is a failure, never a skip; `RENDER_CORPUS_PYTHON` selects the interpreter.
- Output is `target/render-corpus/report.json` (per check verdicts and
  failure detail, element-rect agreement per tolerance class, pixel metrics)
  and `summary.txt`. The pixel metrics split the reference's text rects from
  the rest: `text_mae`, `non_text_mae`, `text_ink_ratio` (our ink over
  Chromium's, below 1 means lighter glyphs) and `text_centroid_dx/dy`
  (ink offset per text rect); these drive text contrast and positioning
  calibration.
- Element rects from `checks.json` use tolerance 0.02 px (integer-valued
  expectations), 0.05 px (others) and 1 px position / 2 px or 3% size for
  elements that hold inline text. They are reported, not graded.

## Tables: Chrome geometry

`crates/tenta-layout/tests/tables.rs` replays `tests/tables/chrome_cases.rs`: 249
`getBoundingClientRect` records from Chrome 154 for tables (structure, anonymous boxes,
column widths, table width, fixed layout, row heights and alignment, collapsed borders,
captions, tables among floats/flex/grid, shapes taken from real pages). The cases are
written in `render-fixtures/tools/table_cases.py` and recorded with
`python3 record_geometry.py table_cases.py ALL ../../crates/tenta-layout/tests/tables/chrome_cases.rs`
(about four seconds a case). Chrome is a reference only; the text uses the bundled
Liberation Sans through `@font-face`, as for the float cases.

## Replaced elements and images: Chrome geometry and pixel references

Two recorded references cover images, both recorded from Chrome 154 on macOS
and checked in as source:

- **Geometry** (`crates/tenta-render/tests/replaced.rs` and
  `tests/replaced/chrome_*.rs`, 85 cases). Each case loads SVG `data:` images of
  a known natural size through the session's own image pipeline
  (`RenderSession::load_inline_images`), lays the page out and compares every
  `id`'d border box with Chrome's. Cases are written in
  `render-fixtures/tools/replaced_cases.py` and recorded with
  `python3 record_geometry.py replaced_cases.py BLOCK ../../crates/tenta-render/tests/replaced/chrome_block.rs`
  (groups `BLOCK`, `INLINE`, `FLEX`, `GRID`, `ABS`, `MISC`; one Chrome run per case,
  a few seconds each). The recorder waits for every image to decode, then reads
  the rectangles. Avoid text whose width depends on the font in these cases
  (the machine's default font differs from the embedded one); a zero-width space
  gives a line its strut without a width. Floats are left out until M1.1.
- **Pixels** (`crates/tenta-browser/tests/images.rs`, `images_match_chrome_pixels_at_known_points`):
  a 28-cell page (`object-fit`, `object-position`, `image-rendering`, clipping by
  `border-radius`, opacity, backgrounds with size, position, `repeat`, `round`, `space`,
  origin and clip, gradients, JPEG, WebP, GIF, SVG) served by the fixture server and
  probed at 16 points per cell, away from the edges that resampling blurs. To
  re-record: `PARITY_DIR=/tmp/parity cargo nextest run -p tenta-browser --features render --test images --run-ignored only write_the_parity_page`,
  then `python3 render-fixtures/tools/chrome_shot.py /tmp/parity 490 860` and
  `python3 render-fixtures/tools/probe_table.py /tmp/parity`, and paste the
  printed table over `CHROME`. Tolerance is 5 per channel (12 for JPEG).
