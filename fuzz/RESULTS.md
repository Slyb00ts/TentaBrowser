# Fuzz Campaign Results

Continuous campaigns per target per release (target, tree, start/end, wall
time, executions, crashes). The P1 roadmap names 24 h clean per target; the
P1 campaigns below are shorter (memory on the development host was needed
for the gates) and are listed as unmet where they are.

| Target | Release | Tree | Runs (UTC) | Wall | Execs | Crashes | 24 h gate |
|---|---|---|---|---|---|---|---|
| tenta-html tokenizer | P1 | working tree 2026-09-07, uncommitted | 09:05–13:18, 13:30–16:24, 18:39–22:08 (2026-09-07) | ~10.6 h | 72.3 M | 0 | unmet |
| tenta-html xml | P1 | same | same three runs | ~10.6 h | 410 M | 0 | unmet |
| tenta-html tree_builder | P1 | same | 09:05 (crash after 27 min), 13:30 (crash at start), 15:52–16:24 and 18:39–22:08 on the fixed code | 4.0 h clean | 5.4 M clean | 2 found, both fixed (open-element stack cap; adoption agency moving subtrees past `MAX_DOM_DEPTH`; fixtures `crash1`–`crash4`) | unmet |
| tenta-net cookies | P1 | same | 09:22–16:21 (2026-09-07, ~70 min paused) | ~6.5 h | 270 M | 0 | unmet |
| tenta-net policy_headers | P1 | same | 09:22–16:21 (2026-09-07) | ~6.5 h | 275 M | 0 | unmet |

Corpora are replayed against every fix (`cargo +nightly fuzz run <target>
<corpus> -- -runs=0`); the last replay (2026-09-08, after the round-4 fixes)
ran tokenizer 30 124, xml 24 167 and tree_builder 24 609 inputs clean.
