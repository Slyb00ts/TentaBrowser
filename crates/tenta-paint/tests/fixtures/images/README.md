# Image fixtures

Binary samples for `tests/image_*.rs`. Everything else the image tests decode
is built in code (`tests/support/mod.rs`, `tests/image_budget.rs`): PNG, JPEG
(`jpeg-encoder`), lossless WebP (`image-webp`), GIF (`gif`) and every BMP and
ICO sample are generated at test time from first principles, so their pixels
are known independently of the decoders.

| Path | Provenance |
| --- | --- |
| `pngsuite/` | PngSuite by Willem van Schaik, public domain (`pngsuite/LICENSE`), unmodified. |
| `lossy.webp`, `lossy-alpha.webp`, `animated.webp` | Generated with libwebp 1.6.0 tools (`cwebp`; the animation as a short multi-frame file) from the 32x24 gradient of `tests/support/mod.rs`. The exact command lines were not recorded. |
| `lossy-1mpx.webp`, `lossy-alpha-1mpx.webp` | `cwebp -q 60 [-alpha_q 60]` (libwebp 1.6.0) from a 1024x1024 noisy gradient, opaque and with a horizontal alpha ramp. |
| `lossless-1mpx.webp`, `lossless-alpha-1mpx.webp`, `lossless-palette-1mpx.webp` | `cwebp -lossless -z 9` (libwebp 1.6.0) from the same gradient (opaque, with alpha) and from a 16-colour random palette image. |
| `progressive-444-1mpx.jpg`, `progressive-420-1mpx.jpg` | `cjpeg -progressive -quality 75 [-sample 1x1]` (libjpeg-turbo 3.2.0) from the opaque gradient as PPM; 4:4:4 and the default 4:2:0. |

The noisy gradient is `r = x/4`, `g = y/4`, `b = (x+y)/8` plus uniform noise in
[-12, 12] (Python `random.seed(7)`, one `randint` per pixel and channel); the
palette image uses `random.seed(3)`. The 1 Mpx files exist to measure peak
decode memory per format against the admission estimates
(`tests/image_budget.rs`); they are not pixel-exact references.
