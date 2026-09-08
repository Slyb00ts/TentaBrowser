# Attribution

The HTML fixtures in this directory (`*/index.html`, plus
`pdf-print-media/pdf-print-import.css`), the colour-component behaviour checks
(`*/color-checks.json`) and `tools/check.py` were imported from the
`render-repros/` directory of the reference implementation:

- Project: Obscura (local copy `../obscura-reference`, rebranded README;
  upstream `h4ckf0r0day/obscura`)
- License: Apache License, Version 2.0 (`../obscura-reference/LICENSE`)
- Source revision: `9bc9c4eb50f5bf1cdf66b1d45b96161c8774c396`
  (`git -C ~/repos/rust/obscura-reference log -1 --format=%H`; the local
  copy has no remote, so this is the only identifier available)
- Source path: `~/repos/rust/obscura-reference/render-repros/`
- Imported: 2026-09-06

The Apache-2.0 license text is in `/LICENSE` at the repository root (this
project uses the same license). The imported files were not modified except
as listed below.

## Modifications

- Each `<name>.html` became `<name>/index.html`; the reference's single
  `checks.json` was split into one `<name>/color-checks.json` per fixture,
  with identical content.
- Four checks were re-baselined to the fresh Chromium captures because they
  depend on the platform's default fonts (macOS Chrome 144 versus the
  reference's capture machine):
  - `font-metric-line-height` "normal serif lines use serif face metrics":
    `y` 160 -> 150
  - `form-control-geometry` "text input size attribute sets intrinsic inline
    width": `x` 503 -> 482
  - `form-control-geometry` "textarea intrinsic box comes from rows and
    cols": `x` 168 -> 181
  - `form-control-geometry` "rows attribute scales the textarea intrinsic
    height": `x` 518 -> 531
- `tools/check.py` is derived from `render-repros/check.py`: the component
  and pair-metric functions are unchanged; the command line and file layout
  were adapted to this directory structure.

## Not imported

`paired-corpus.py`, `probe_long_full_page_chromium.py`, `representative-suite/`,
`spot-sites.txt` and the reference's own capture script were not imported.
Captures (`*/chromium.png`), structural checks (`*/checks.json`),
`captures.json` and `tools/capture.mjs` are produced by this project.
