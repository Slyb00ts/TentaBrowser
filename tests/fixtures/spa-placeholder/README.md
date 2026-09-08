# spa-placeholder

Placeholder for the reference single-page application used by `S-spa` and
`S-spa-t2` (`docs/spec/15-testing-and-benchmarks.md` §2): a pinned build of a
public React 18 demo, served locally, hydrated, one route clicked, settled.

The application is imported in **P2** (`docs/spec/16-roadmap.md`), when the
JavaScript engines exist to run it, and lands under `bench/apps/react-spa/`
with its upstream commit, build command and license recorded. Until then the
`S-spa*` rows in `bench/scenarios.toml` point here and `bench/run.py` reports
them as `not-implemented` for every subject.

What the scenario will exercise once it exists: script loading and module
evaluation, React hydration through the scheduler's `MessageChannel` chain,
`fetch` of route data, `pushState` navigation, and the Tier 2 (`S-spa-t2`,
geometry stubs) versus Tier 3 (`S-spa`, style and layout without paint)
memory delta.
