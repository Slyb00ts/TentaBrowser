# Render fixture corpus

Deterministic HTML fixtures with paired Chromium reference captures and
structural checks (`docs/spec/15-testing-and-benchmarks.md` §1,
`docs/spec/06-layout-and-render.md` §6). The corpus was imported from the
reference implementation's `render-repros/` (Apache-2.0, see
`ATTRIBUTION.md`); the captures are ours. The recipe to regenerate them is in
`docs/dev/render-fixtures.md`.

Each fixture directory contains:

| File | Origin | Purpose |
|------|--------|---------|
| `index.html` | imported | the fixture (self-contained; `data:` URLs or sibling files only) |
| `chromium.png` | captured | Chromium reference, 900x1000 viewport, DPR 1 (`captures.json` records the browser and settings) |
| `checks.json` | captured | structural checks: bounding rect of every element under `<body>` (`named: true` when it has an `id`/`data-check`) and the first client rect of every non-blank text node, CSS px at DPR 1 |
| `color-checks.json` | imported | behaviour assertions on solid-colour components (`x`, `y`, `width`, `height` within 1 px, optional `count`); anti-aliasing never decides pass/fail |

`tools/capture.mjs` produces the captured files; `tools/check.py` evaluates
`color-checks.json` and pixel-distance tripwires for an engine capture placed
next to the reference (`<name>/tentabrowser.png`).

## Fixtures

Block, inline and text (P3a):

| Fixture | Exercises |
|---------|-----------|
| `block-auto-margins` | `margin: auto` centring of fixed- and max-width blocks, one-sided auto margins, auto width |
| `block-whitespace-margin-collapse` | sibling margin collapsing across formatting whitespace |
| `mixed-content-block` | block children of a block that also has inline content stay full width |
| `legacy-center` | `<center>` and `align=center` legacy alignment, inheritance, tables resetting it |
| `list-indentation` | UA list indentation on the list container, `ex` padding |
| `font-metric-line-height` | `line-height: normal` from face metrics (sans, serif, mono), fractional explicit line heights |
| `font-shorthand` | `font` shorthand from custom properties, shorthand resetting longhands |
| `forced-line-breaks` | `<br>`-only line boxes, trailing breaks, inherited line height in flex items |
| `direction-rtl` | inherited `direction: rtl` in flex, grid and block under-constraint |
| `logical-borders` | `border-inline-*`/`border-block-*` mapping in RTL, logical versus physical cascade order |
| `relative-box-edges` | `vh`, `vw`, `vmin`, `vmax`, `em`, `rem` margins and padding |
| `contextual-css-math` | `min()`, `max()`, `clamp()`, `calc()`, `round()` with viewport and root font units |
| `cascade-priority` | `!important` and specificity |
| `supports-var-feature-query` | `@supports` with a `var()` value |
| `display-contents-cascade` | `display: contents` flattening into a flex parent |
| `background-shorthand-reset` | `background` shorthand resetting longhands, transparent padding |
| `box-sizing` | `content-box` and `border-box` with fixed and percentage sizes, inherited reset |
| `viewport-consistency` | width and height media queries and `innerWidth`/`innerHeight` agree with the capture viewport |
| `long-full-page-capture` | tall page for full-page capture (viewport capture only in this corpus) |

Floats and clearance (P3a):

| Fixture | Exercises |
|---------|-----------|
| `clear-float` | `clear: both` drops below a float at full width |
| `float-flow-bands` | right float bands, float-avoiding intro, inline atomic boxes sharing a line, clear |
| `float-bfc-continuation` | a float's band continuing through block wrappers |
| `mixed-float-run` | adjacent same-side floats and a trailing opposite float in one band |
| `nested-float-clearance` | shared-BFC endpoint of nested floats, clearfix |
| `opposing-header-floats` | left and right floats in a header, cleared navigation |
| `right-float-navigation` | source order versus visual order of right floats |
| `bootstrap-float-clearfix` | Bootstrap-style percentage float, `100vh` float, `::after` clearfix |

Flex, grid, alignment (P3b):

| Fixture | Exercises |
|---------|-----------|
| `flex-flow` | `flex-flow` shorthand token order and longhand resets |
| `flex-whitespace-items` | whitespace between flex items, direct and `display: contents` children |
| `flex-post-ratio-cross-size` | `aspect-ratio` media item in a flex row, absolute child, following content |
| `inline-block-flex-items` | inline-block flex items keep block inner flow and shrink-wrap |
| `text-align-flex-items` | `text-align` does not move flex items but centres their inline content |
| `item-self-alignment` | `align-self`, `justify-self`, `place-self`, `place-items`, `align-content` in flex and grid |
| `grid-auto-repeat` | `repeat(auto-fill)` and `repeat(auto-fit)` |
| `grid-shorthand` | `grid` shorthand, also from a custom property |
| `contextual-grid-gap` | `gap` from `calc()`/`rem` with root and viewport context |
| `replaced-grid-order` | `order` in grid with a percentage-sized image in a wrapper |
| `percentage-height-indefinite-parent` | percentage heights through an indefinite chain inside a column flex |

Positioning, transforms, stacking (P3b):

| Fixture | Exercises |
|---------|-----------|
| `abs-fill` | absolutely positioned boxes with all four insets fill their containing block |
| `absolute-containing-block` | nearest positioned ancestor, padding-box insets, percentage insets, fixed to viewport |
| `absolute-static-position` | static position for `auto` insets, nested absolutes |
| `transform-containing-block` | `transform` (including identity) capturing absolute and fixed descendants |
| `modern-containing-block-triggers` | `filter`, `perspective`, `contain`, `will-change`, `content-visibility`, `container-type` as containing-block triggers |
| `z-index` | positive and negative `z-index` layers |
| `carousel-clip` | `overflow: hidden` clipping of a translated track |
| `box-shadow-outset` | outset `box-shadow` painting and clipping |
| `animation-fill-forwards` | finite `animation-fill-mode: forwards` final state |

Tables (P3b):

| Fixture | Exercises |
|---------|-----------|
| `tables` | `td width` fixing a whole column |
| `fixed-table-layout` | `table-layout: fixed` with collapsed and separate borders, spacing |
| `table-row-geometry` | row height as a minimum, rowspan distribution, line boxes taller than cells |
| `table-track-geometry` | auto, percentage and surplus column distribution, `cellpadding`, border spacing |

Replaced elements, form controls, media (P3b/P3c):

| Fixture | Exercises |
|---------|-----------|
| `img-density-and-alt` | `srcset` density and width descriptors with `sizes`, alt text, a fixed-border viewport |
| `inline-replaced-flow` | inline versus block images in line boxes, strut, mixed table cell content |
| `ratio-only-inline-images` | images with only an aspect ratio in inline, inline-block and block contexts |
| `replaced-block-wrapper` | percentage-width image inside an auto-width block wrapper |
| `max-height-replaced` | `max-height` clamping an image's aspect transfer |
| `video-poster` | `<video poster>` intrinsic size and `object-position` |
| `media-text-track` | `<track>` cue parsing exposed to the page |
| `form-control-geometry` | standards-mode intrinsic sizes of text inputs, checkbox, range, textarea, hidden inputs |
| `form-control-quirks` | quirks-mode form margins and border-box author sizes |
| `pdf-print-media` | `@import ... print` and print-only rules (oracle for PDF output) |

Scripting contracts (P2):

| Fixture | Exercises |
|---------|-----------|
| `eventtarget-lifecycle` | framework-style `EventTarget` subclass lifecycle |
| `modern-hydration-contracts` | DOM contracts modern hydration relies on (`template`, attributes, node identity) |
| `modern-streams` | streamed hydration through `ReadableStream`/`TextDecoderStream` |

Roadmap gates using this corpus: P3a (block/inline half at Obscura parity),
P3b (full corpus structural checks at or above Obscura's pass rate), P3c
(pixel distance at or below Obscura's on at least 90% of fixtures) in
`docs/spec/16-roadmap.md`.
