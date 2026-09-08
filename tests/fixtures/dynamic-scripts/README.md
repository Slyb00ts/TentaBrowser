# dynamic-scripts

The `S-dynamic-scripts` fixture: the reference implementation's "dynamic
scripts" benchmark case. Its upstream benchmark fixtures are not in the local
reference checkout; the shape mirrors the reference's regression test for
timer-inserted external scripts
(`crates/obscura-cdp/tests/dynamic_script_onload_fires.rs`: a `setTimeout`
callback creates `<script src>` elements, one appended directly and one
nested inside a new container) extended into a chain:

1. the inline script schedules `setTimeout(…, 0)` and inserts `stage1.js`;
2. `stage1.js` inserts `sidecar.js` (`async`) and `stage2.js`;
3. `stage2.js` creates a `<div id="box">`, inserts `stage3.js` into it and
   appends the box to the body (nested insertion);
4. `stage3.js` runs; when all four scripts have run, `#status` becomes
   `dynamic-scripts: done (3 stages + sidecar)`, the title
   `Dynamic scripts: done`, and `window.__settled = true`.

Each script writes into its own `<li>` slot with its run order number.
Dynamically inserted scripts are async, so the sidecar (inserted first)
normally runs before stage 2 (order 1, sidecar, 2, 3, as observed in both
the reference and Chromium); an engine that schedules them differently
still produces the same `#status` line, which is what the harness asserts.

Exercises: script insertion from timers and from other scripts, dynamic
`<script src>` fetch and execution order, nested insertion, `async`, and the
settle machine's tracking of a load chain that continues past `load`.

Expected settled text: `expected.txt` (the slot numbers there are the
common order; only the `#status` line is asserted). Harness marker:
`dynamic-scripts: done (3 stages + sidecar)`.
