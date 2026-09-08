# Developer recipes

Short, executable recipes. The specification in `docs/spec/` says what and
why; this directory says how on a workstation.

## Build and check

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo xtask lint-deps          # crate dependency direction (02-architecture.md §2)
cargo nextest run --workspace  # never `cargo test`
cargo deny check               # licenses, advisories, banned crates (one libcrypto)
```

The budget measurements of `docs/spec/04-dom-and-css.md` §8 are `#[ignore]`d,
because they only mean anything in a release build. They are part of the gate
and are run separately:

```bash
cargo nextest run -p tenta-dom -p tenta-html --release --run-ignored all
```

They print what they measured next to the budget, so a run is also the place
to read the current parse, query, extraction and per-element memory figures.
Run them on an otherwise idle machine: they assert absolute times, and nextest
already saturates the cores, so anything else competing for them turns the
gate into a load detector. The shape assertions — the tests that check a cost
grows linearly — compare two workloads timed in the same window and are safe
to run under any load.
`cargo bench -p tenta-html` times the parser into the owned test tree and
`cargo bench -p tenta-dom` times it into the arena `Document`, which is the
configuration the engine ships.

## Run the CLI

```bash
cargo build --release -p tentabrowser-cli          # target/release/tentabrowser
cargo run -q -p tentabrowser-cli -- fetch https://example.com/ --dump text
cargo run -q -p tentabrowser-cli -- search "rust browser engine" --json
cargo run -q -p tentabrowser-cli -- scrape --from urls.txt --concurrency 8
```

Against the local fixtures, which live on loopback and therefore need
`--allow-private-network` (`19-security.md` §2):

```bash
cargo run --release -p tenta-testutil --bin fixture-server -- --port 8080 &
target/release/tentabrowser fetch http://127.0.0.1:8080/static-article/index.html \
    --allow-private-network | tail -3
target/release/tentabrowser scrape http://127.0.0.1:8080/robots/index.html \
    --follow-links --same-host --allow-private-network --ordered
```

A search against a recorded SERP instead of a live provider, which is how the
search tests and the search benchmarks run:

```bash
cargo run --release -p tenta-testutil --bin fixture-server -- --port 8081 --root providers/fixtures &
target/release/tentabrowser search "rust browser engine" --provider ddg --json \
    --allow-private-network \
    --search-endpoint duckduckgo_html=http://127.0.0.1:8081/duckduckgo_html/rust-browser-engine.html
```

`--log debug` prints a `phases` line with the wall time of each stage of the
run (`engine`, `navigate`, `extract`, `write`, `close`); `--log trace` adds
the engine's spans. Both go to stderr, so stdout stays exactly the output.

## Repository tooling (`cargo xtask`)

| Command | What it does |
|---------|--------------|
| `lint-deps` | Reads every `crates/*/Cargo.toml` and fails on an edge that violates the foundation / feature / assembly layering. New crates must be added to `xtask/src/deps.rs` first. |
| `size-report` | Builds `tentabrowser-cli` in release and prints binary sizes. |
| `link-test` | Builds every rung of the `tests/linktest` feature ladder unstripped and fails if the binary contains more than one libcrypto (`docs/dev/size-baseline.md`). |
| `hello-sizes` | Measures stripped size and idle RSS of every rung and writes `bench/baselines/hello-sizes.json`. |
| `notices` | Regenerates `THIRD_PARTY_NOTICES.md` with `cargo about` from `about.toml` and `about.hbs` (needs `cargo install cargo-about`). |
| `idl-manifest` | Checks that `idl/MANIFEST.toml` matches the golden-site inventories and `idl/phase-rules.toml` (`python3 idl/gen_manifest.py --check`); regenerate with `python3 idl/gen_manifest.py`. |

## Benchmarks

`bench/run.py` drives TentaBrowser, the reference implementation and headless
Chromium through the scenarios of `bench/scenarios.toml`; results land in
`bench/results/` (gitignored) and reference baselines in `bench/baselines/`.

## Conformance

- `wpt.md`: running Web Platform Tests directories, host-name setup, adding a
  directory, updating expectations.
- `render-fixtures.md`: recapturing the render corpus references and checking
  an engine's output against them.
- `fingerprint.md`: capturing a browser's ClientHello and HTTP/2 preface with
  `fp-capture`, writing `profiles/*.toml` from it, and the JA4/Akamai
  conformance tests.

## Profiling

`--log debug` and the `phases` line are the first stop for a cold-start
regression; `bench/run.py` with `--runs 25` is the second. See
`docs/spec/13-performance.md` §9 for the rest of the toolkit.
