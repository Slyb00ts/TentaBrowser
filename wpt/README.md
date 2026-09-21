# WPT runner

Runs Web Platform Tests directories against a product and reports subtest
pass percentages per directory (`docs/spec/15-testing-and-benchmarks.md` §6).
Phase gates are WPT thresholds per directory (`docs/spec/16-roadmap.md`);
`config.toml` lists the directories per phase.

| Path | Purpose |
|------|---------|
| `run.py` | runner: checkout, manifest, `wpt serve`, product drivers, results, expectations (stdlib only) |
| `config.toml` | pinned WPT commit, timeouts, directories per phase |
| `serve-config.json` | `./wpt serve` configuration (fixed ports, bound to 127.0.0.1) |
| `tools/driver-chromium.mjs` | `--product chromium` driver on Playwright's Chromium (`npm install` in `tools/`) |
| `expectations/<product>/` | known results, `.ini` per test (`expectations/README.md`) |
| `results/<product>/<dir>.json` | last run per directory (gitignored; gate numbers are recorded in this README) |
| `checkout/` | shallow WPT checkout at the pinned commit (gitignored) |

Quick start:

```bash
cd wpt/tools && npm install && npx playwright install chromium && cd ../..
python3 wpt/run.py run --product chromium --dir dom/nodes
```

See `docs/dev/wpt.md` for the full recipe, host-name setup, adding
directories and updating expectations.

## Products

- `chromium`: Playwright's pinned Chromium (P0). Serves as the control: a
  directory that Chromium fails is suspect before TentaBrowser is blamed.
- `tentabrowser`: the engine itself, driven by `tenta-wpt-driver` (a `[[bin]]`
  of the `tenta-browser` crate, built with `--features v8`). It replaces
  `resources/testharnessreport.js` on the wire with its own hook, and the hook
  posts the result back over `fetch()`. The driver contract both products
  implement is documented on the `Driver` class in `run.py`; this one is
  described in `docs/dev/wpt.md`.

## Pinned WPT commit

`b89af32bc8f42d678f444eb0703bca015ddcf240` (`web-platform-tests/wpt` master,
2026-09-06). Change it in `config.toml`; `run.py` re-fetches and regenerates
the manifest on the next run.

## Baseline: `dom/nodes/` on Chromium

Recorded 2026-09-06 with the pinned commit, Playwright 1.63.0 Chromium
153.0.8010.12 (`chromium-1243`), macOS, `--jobs 4`:

| Directory | Files | Files OK | Subtests | Pass | Pass % | Wall time |
|-----------|-------|----------|----------|------|--------|-----------|
| `dom/nodes` | 334 | 332 | 15 110 | 15 044 | 99.56% | 23 s |

Two files time out (`moveBefore/fullscreen-preserve.html` needs a user
gesture; `insertion-removing-steps/change-event.window.html` calls
`test_driver.send_keys`, and the P0 driver provides no testdriver back end).
The 66 failing subtests are Chromium's own (mostly namespace-validation edge
cases in `createElementNS`/`createDocument` and one XML doctype comment case);
they are candidates for `expectations/chromium/` once the P2 gate is enforced.
