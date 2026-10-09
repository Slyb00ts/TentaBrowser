# `tenta-paint` fuzz targets

One target per image decoder, each feeding raw bytes to `decode_format` with
tight limits and asserting that the result is a typed error or an image inside
those limits, and never a (contained) decoder panic. This crate is excluded from
the workspace because `cargo fuzz` drives it with its own nightly toolchain and
sanitiser flags; CI does not run it.

| Target | Decoder |
| --- | --- |
| `png` | PNG |
| `jpeg` | JPEG (baseline, progressive, EXIF) |
| `webp` | WebP |
| `gif` | GIF |
| `bmp` | BMP |
| `ico` | ICO and CUR, including embedded PNG |

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run png
```

A deterministic version runs on every commit as
`tests/image_mutation.rs`: 10 MiB of seeded mutations of valid samples.
