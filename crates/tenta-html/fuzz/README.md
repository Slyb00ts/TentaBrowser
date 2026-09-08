# `tenta-html` fuzz targets

`docs/spec/15-testing-and-benchmarks.md` §1 lists the HTML parser among the
continuous fuzz targets, and `docs/spec/04-dom-and-css.md` §3.3 asks for
deeply nested input specifically. This crate is excluded from the workspace
because `cargo fuzz` drives it with its own nightly toolchain and sanitiser
flags.

```bash
cargo install cargo-fuzz
cargo fuzz run tokenizer
cargo fuzz run tree_builder
cargo fuzz run xml
```

A cheap deterministic version runs on every commit as the
`random_input_never_panics` tests in `tests/differential.rs`.
