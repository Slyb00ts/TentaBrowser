# Bundled fonts

Fonts embedded by `tenta-layout` behind the `embedded-fonts` feature (default on)
through `include_bytes!`, per D-028 and `docs/dev/p3-plan.md` §2.

The files are unmodified upstream TTFs. Modifying them, including subsetting, is
not allowed until `xtask fonts` has checked the reserved-font-name terms of each
licence.

## Shipped files

| File | Upstream | SHA-256 |
|------|----------|---------|
| `LiberationSans-Regular.ttf` | Liberation Fonts 2.1.5 | `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8` |
| `LiberationSans-Bold.ttf` | Liberation Fonts 2.1.5 | `788abee4c806d660e8aee46689dd8540cd4bb98da03dcc9d171ce3efd99a9173` |
| `LiberationSerif-Regular.ttf` | Liberation Fonts 2.1.5 | `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74` |
| `LiberationSerif-Bold.ttf` | Liberation Fonts 2.1.5 | `d754ba427cfe0bca54ae052384baa8f842da5bd6550ad4da024ac441e7a7d5ce` |
| `LiberationMono-Regular.ttf` | Liberation Fonts 2.1.5 | `f2b83c763e8afd21709333370bed4774337fae82267937e2b5aea7e2fbd922c1` |
| `LiberationMono-Bold.ttf` | Liberation Fonts 2.1.5 | `bd62a0672d0b9b6710b01df434c80ad54fa5f0835207eb7b17b7a761463067bb` |
| `DejaVuSans.ttf` | DejaVu Fonts 2.37 | `7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954` |
| `LICENSE-Liberation.txt` | Liberation Fonts 2.1.5 `LICENSE` (SIL OFL 1.1) | `93fed46019c38bbe566b479d22148e2e8a1e85ada614accb0211c37b2c61c19b` |
| `LICENSE-DejaVu.txt` | DejaVu Fonts 2.37 `LICENSE` | `7a083b136e64d064794c3419751e5c7dd10d2f64c108fe5ba161eae5e5958a93` |

## Sources

- Liberation Fonts 2.1.5 (SIL OFL 1.1): release page
  <https://github.com/liberationfonts/liberation-fonts/releases/tag/2.1.5>, TTF
  archive <https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz>
  (SHA-256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`).
- DejaVu Fonts 2.37 (Bitstream Vera and DejaVu licence): release page
  <https://github.com/dejavu-fonts/dejavu-fonts/releases/tag/version_2_37>, Sans
  archive <https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-sans-ttf-2.37.zip>
  (SHA-256 `5c6e497a2f36552cb5ffb112c413a6af39c0f3c47653662b90b4fa6499822fd7`).

Neither release page publishes a digest for its assets. The archive hashes above
are the recorded identity of the download made on 2026-10-09. Each file hash was
taken from the archive's extracted members. Liberation's version string is 2.1.5
in the release tag and archive name, and the unit test in `src/embedded_fonts.rs`
checks family name, subfamily name, OS/2 weight class and `unitsPerEm` for every
shipped file.

## Italic faces

The Italic and BoldItalic faces of Liberation Sans, Serif and Mono are not
shipped. The full Latin set plus DejaVu is 5 116 240 bytes (4.88 MiB), above the
3 MiB threshold in `docs/dev/p3-plan.md` §2, so they were dropped. The shipped set
is 2 973 420 bytes (2.84 MiB). Italic text uses synthetic oblique from the regular
faces until a later package revisits this. Sizes and the decision are recorded in
`docs/dev/size-baseline.md`.
