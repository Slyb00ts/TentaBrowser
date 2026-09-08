# `tenta-net` fuzz targets

`docs/spec/15-testing-and-benchmarks.md` §1 lists the cookie parser among the
continuous fuzz targets. This crate is excluded from the workspace because
`cargo fuzz` drives it with its own nightly toolchain and sanitiser flags.

| Target | Parses |
| --- | --- |
| `cookies` | `Set-Cookie` values and cookie dates |
| `policy_headers` | robots.txt, `Cache-Control` / `Vary`, `Retry-After` |

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run cookies
cargo +nightly fuzz run policy_headers
```

Corpora and campaign results live in `fuzz/RESULTS.md` at the repository
root. A cheap deterministic version of the same corpus runs on every commit
as `policy::fuzz_lite` in the library's test suite.
