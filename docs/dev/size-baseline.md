# "Hello" size baseline and single-libcrypto link test

P0 work package (b): the feature ladder of `docs/spec/21-project-operations.md`
§4, the process-idle row of `docs/spec/14-memory.md` §1 and the one-libcrypto
rule of `docs/spec/03-network.md` §1.1, measured on a real binary. The binary
lives in `tests/linktest/` (its own Cargo workspace, excluded from the root one
so the native builds never enter the main lock file). Two xtask commands drive
it:

```bash
cargo xtask link-test    # builds every rung unstripped and scans for a second libcrypto
cargo xtask hello-sizes  # builds every rung stripped, measures size and RSS, writes bench/baselines/hello-sizes.json
```

## The ladder

| Feature | Adds | What `hello` does with it |
|---------|------|---------------------------|
| `base` | tokio (`current_thread`), hyper 1, cloudflare `boring` 5 (BoringSSL), hickory-resolver (`tls-ring`, `https-ring`), `rcgen` (`ring`), `tenta-core` | loopback HTTPS: hyper server behind `tokio-boring`, one TLS 1.3 handshake from hyper's client against a self-signed `rcgen` certificate; resolves `localhost` with hickory |
| `rtc` | `base` + str0m 0.23 (`default-features = false`, `apple-crypto` on macOS/iOS, `wincrypto` on Windows, `rust-crypto` elsewhere) + rustls 0.23 (`default-features = false`, `ring`) | builds a rustls `ClientConfig` on the `ring` provider; creates an `Rtc` and an SDP offer with one video m-line |
| `codecs` | `rtc` + libvpx 1.16.0 (`shiguredo_libvpx`, `source-build`) + rav1d 1.1.0 (arm64 asm on) + libopus 1.6.1 (`opus` over `opusic-sys`, bundled cmake build) | opens a VP9 decoder, an Opus encoder and a dav1d decoder context |
| `v8` | `base` + rusty_v8 (`v8` crate, prebuilt, default features: custom libc++, no pointer compression) | platform init, one isolate from the default snapshot, evaluates `1+1` |
| `all` | `codecs` + `v8` | everything above |

Profile for the measurements (`tests/linktest/Cargo.toml`): `opt-level = 3`,
`lto = "thin"`, `codegen-units = 16`, `panic = "unwind"`, `strip = "symbols"`,
`debug = false`. The `linktest` profile is identical except `strip = "none"`
so that the symbol scan can read the symbol table.

## Measurements

Host: Mac16,10 (Apple M4, 16 GB), macOS 26.5.2, `rustc 1.98.0 (88d9e12ae 2026-08-18)`,
target `aarch64-apple-darwin`, measured 2026-09-06T13:40:41Z. Raw data with every run:
`bench/baselines/hello-sizes.json`.

| Rung | Stripped size | Δ size (MiB) | Idle max RSS, median of 10 | Δ RSS (MiB) | RSS min–max (MiB) | Startup (ms) | V8 platform init / first isolate (ms) |
|------|---------------|--------------|----------------------------|-------------|-------------------|--------------|----------------------------------------|
| `base` | 6,578,992 B (6.27 MiB) | — | 12,017,664 B (11.46 MiB) | — | 11.39–11.59 | 6.4 | — |
| `rtc` | 7,796,528 B (7.44 MiB) | +1.16 over `base` | 16,424,960 B (15.66 MiB) | +4.20 over `base` | 15.48–15.73 | 46.4 | — |
| `codecs` | 9,874,496 B (9.42 MiB) | +1.98 over `rtc` | 20,291,584 B (19.35 MiB) | +3.69 over `rtc` | 19.16–19.47 | 44.3 | — |
| `v8` | 49,086,272 B (46.81 MiB) | +40.54 over `base` | 20,889,600 B (19.92 MiB) | +8.46 over `base` | 19.83–20.00 | 7.0 | 0.35 / 0.58 |
| `all` | 52,381,184 B (49.95 MiB) | +40.54 over `codecs` | 27,090,944 B (25.84 MiB) | +6.48 over `codecs` | 25.23–28.14 | 49.0 | 0.46 / 0.68 |

Crate versions: boring 5.2.0, boring-sys 5.2.0, hickory-resolver 0.26.2, hyper 1.11.1, opusic-sys 0.7.5, rav1d 1.1.0, rcgen 0.14.10, ring 0.17.14, rustls 0.23.43, shiguredo_libvpx 2026.1.0, str0m 0.23.1, str0m-apple-crypto 0.6.0, tokio 1.53.1, v8 152.2.0.

How the rows map onto the budget tables:

- `docs/spec/21-project-operations.md` §4 "Base binary": `base` = 6.27 MiB with
  `tenta-core` only (`tenta-net`, `tenta-html`, `tenta-dom` do not exist yet;
  re-measure when they land). "WebRTC": `rtc` − `base` = +1.16 MiB (str0m +
  rustls/ring, without TURN and audio processing). "Codecs": `codecs` − `rtc`
  = +1.98 MiB (libvpx VP8/VP9 with high bit depth, rav1d with arm64 asm,
  libopus; no rav1e). "V8": `v8` − `base` = +40.54 MiB (rusty_v8 152.2.0
  prebuilt, no pointer compression, ICU included).
- `docs/spec/14-memory.md` §1 "Process idle": `base` peak RSS 11.46 MiB
  (12.02 MB decimal) against the ≤ 12 MB ⓣ target. The measured process
  does more than "nothing loaded": it has completed one TLS 1.3 handshake
  through BoringSSL, built a hickory resolver from the system configuration
  and run one hyper request, so the idle floor of a CLI that skips those is
  below this number. `v8` adds +8.46 MiB for the platform plus one isolate
  from the default snapshot; `rtc` adds +4.20 MiB, most of it the Swift
  runtime and CryptoKit that `str0m-apple-crypto` loads (see findings).
- Startup: the `rtc` and `all` rungs take ~40 ms longer than `base`; the
  extra time coincides with the Swift/CryptoKit dynamic libraries appearing
  in `otool -L`. Not isolated further at P0.

## Method

- Size: `std::fs::metadata` of `tests/linktest/target/release/hello` after
  `cargo build --release --features <rung>`.
- Idle RSS: `/usr/bin/time -l ./hello`, "maximum resident set size" (bytes on
  macOS), 10 runs per rung, median. `hello` exits right after initialisation,
  so the peak RSS is the RSS after the handshake, the resolver, the codec
  contexts and (for `v8`) the first isolate. The `startup` column is the
  wall-clock time printed by the binary from process start to `ok`.
- V8 timings are printed by the binary from `std::time::Instant` around
  `V8::initialize_platform` + `V8::initialize` and around `Isolate::new`.

Exact commands:

```bash
cd tests/linktest
cargo build --release --features base      # then rtc, codecs, v8, all
/usr/bin/time -l ./target/release/hello    # x10, take the median RSS
cargo xtask hello-sizes                    # does the above and writes the JSON
```

## Single-libcrypto link test

Verdict on this host: **PASS on `aarch64-apple-darwin` for every rung (`base`, `rtc`, `codecs`, `v8`, `all`).**
`cargo xtask link-test` output for each rung, identical across rungs for the
crypto columns: 596 libcrypto-family symbols, each defined exactly once; 72
`ring_core_0_17_14_*` symbols; no `aws_lc_*`/`AWS_LC*` symbol; the only crypto
version string is `BoringSSL`; the only libcrypto archives linked are
`boring-sys-*/out/build/libcrypto.a` and `libssl.a`. `otool -L` lists only
system frameworks and libraries (plus CryptoKit and the Swift runtime for
rungs with str0m). Manual spot check on the `all` binary (75.9 MB unstripped):
`nm --defined-only` = 111,011 symbols, of which `SSL_` 71, `EVP_` 150, `CRYPTO_`
16, `OPENSSL_` 43, `BN_` 61, `EC_` 42, `RSA_` 31, `X509_` 120, `HMAC_` 9, `SHA256_`
4, `BIO_` 39, `ERR_` 10 (total = unique for each), `shiguredo_vpx_` 910,
`dav1d_` 603, `opus_` 8, `aws_lc` 0.

`cargo xtask link-test` builds each rung with the unstripped `linktest`
profile and checks four things on the binary; all four must hold:

1. `otool -L` (`ldd` on Linux): no dynamic `libssl`/`libcrypto`.
2. `nm --defined-only`: no symbol containing `aws_lc_` (aws-lc-sys prefixes
   every symbol with `aws_lc_<version>_`), and every symbol of the libcrypto
   families `SSL_ EVP_ CRYPTO_ OPENSSL_ BN_ EC_ RSA_ X509_ HMAC_ SHA256_ BIO_
   ERR_` is defined exactly once. `SSL_new` and `EVP_aead_aes_128_gcm` must
   be present (BoringSSL is there). `ring` symbols are reported separately;
   they are prefixed `ring_core_0_17_…` and cannot collide.
3. Version strings in the image: `BoringSSL` must be present; `OpenSSL 1.`,
   `OpenSSL 3.`, `OpenSSL 4.` and `AWS-LC` must be absent.
4. Static archives produced by build scripts under `target/linktest/build`
   whose name contains `crypto` or `ssl` must all belong to `boring-sys`.

Dependency-graph evidence, run from `tests/linktest`:

```bash
cargo tree --features all -e features -i aws-lc-sys   # "nothing to print"
cargo tree --features all -e features -i aws-lc-rs    # "nothing to print"
cargo tree --features all -e features -i openssl-sys  # "did not match any packages"
cargo tree --features all -e features -i rustls | grep 'rustls feature'   # log, logging, ring, std, tls12 only
cargo deny --manifest-path tests/linktest/Cargo.toml --features all \
  --config "$PWD/deny.toml" check bans                 # from the repository root
```

Result on this host: `bans ok` for `--target aarch64-apple-darwin`,
`x86_64-pc-windows-msvc` and `aarch64-apple-ios`. **`bans FAILED` for
`x86_64-unknown-linux-gnu` and `aarch64-linux-android`** (and therefore for the
default all-targets graph): `aws-lc-rs 1.18.1` and `aws-lc-sys 0.45.0` are
reachable through `hello → str0m → str0m-rust-crypto 0.6.0 → dimpl 0.7.3
(features rust-crypto, rcgen) → aws-lc-rs`, because dimpl's `rcgen` feature is
defined as `["dep:rcgen", "aws-lc-rs"]`. See findings. The `wildcards`
check also requires the `tenta-core` path dependency to carry a version.

## rusty_v8 prebuilt availability

`v8` crate 152.2.0, GitHub release `denoland/rusty_v8` tag `v152.2.0`
(published 2026-08-20), static-library assets:

| Target | release | debug | `_ptrcomp` release | `_ptrcomp` debug |
|--------|---------|-------|--------------------|------------------|
| `aarch64-apple-darwin` | yes | yes | yes | yes |
| `x86_64-apple-darwin` | yes | yes | yes | yes |
| `x86_64-unknown-linux-gnu` | yes | yes | yes | yes |
| `aarch64-unknown-linux-gnu` | yes | yes | no | no |
| `x86_64-unknown-linux-musl` | yes | no | no | no |
| `aarch64-unknown-linux-musl` | yes | no | no | no |
| `riscv64gc-unknown-linux-gnu` | yes | yes | no | no |
| `x86_64-pc-windows-msvc` | yes (`.lib`) | no | no | no |
| `aarch64-pc-windows-msvc` | yes (`.lib`) | no | no | no |
| `aarch64-apple-ios` | yes | no | no | no |
| `aarch64-apple-ios-sim` | yes | no | no | no |
| `aarch64-linux-android`, `x86_64-linux-android` | none | none | none | none |

Asset names follow `librusty_v8_[ptrcomp_]{release,debug}_<target>.a.gz`
(`rusty_v8_release_<target>.lib.gz` on Windows) with a matching
`src_binding_[ptrcomp_]{release,debug}_<target>.rs`. Consequences:

- Pointer compression (`v8_enable_pointer_compression`) has prebuilts for the
  three desktop targets only; every other target, including both iOS targets,
  needs `V8_FROM_SOURCE=1` for a pointer-compressed build.
- iOS device and simulator have release prebuilts (no debug, no ptrcomp),
  matching `docs/spec/12-platforms.md` §3.
- Android has no prebuilt at all; `docs/spec/12-platforms.md` §1 already plans
  a from-source build in CI.

## Findings and blockers

1. **str0m `rust-crypto` pulls aws-lc-rs (Linux, Android).** `str0m-rust-crypto`
   0.6.0 depends on `dimpl` with `features = ["rust-crypto", "rcgen"]` and
   dimpl's `rcgen` feature enables `aws-lc-rs` (`rcgen` itself supports
   `ring`, dimpl just does not expose that). With cloudflare `boring` in the
   same process this is exactly the duplicate-BoringSSL link failure that
   `docs/spec/03-network.md` §1.1 forbids. The link test therefore **cannot
   pass on Linux or Android with str0m 0.23.1's `rust-crypto` backend**; the
   `apple-crypto` (macOS/iOS) and `wincrypto` (Windows) backends are clean
   (`cargo deny` verified per target, see above). Options for P1/P7: an
   upstream change to dimpl/str0m-rust-crypto (a `rcgen-ring` feature), a
   `[patch.crates-io]` fork of `dimpl` until then, or a str0m backend over
   `boring` itself. This needs a decision entry; the spec's §1.1 bullet
   "rust-crypto on Linux/Android" is not currently achievable as written.
2. **rav1d 1.0.0 and 1.1.0 packages omit `src/arm/asm-offsets.h`.** With the
   default `asm` feature every arm64 build fails with
   `src/arm/64/refmvs.S:28:10: fatal error: 'src/arm/asm-offsets.h' file not found`.
   `tests/linktest/shims/rav1d/` carries the unmodified upstream header and
   `tests/linktest/.cargo/config.toml` puts it on `CPATH` (see the shim's
   README). The same shim will be needed in `tenta-media` until rav1d
   publishes a fixed package; alternatively `default-features = false` drops
   the assembly (slower decode, not measured).
3. **libvpx has no CMake build.** No crates.io binding builds libvpx from
   source with cmake; `libvpx-native-sys`, `env-libvpx-sys` and `vpx-sys` only
   bind a system library through pkg-config. `shiguredo_libvpx` 2026.1.0 with
   `source-build` clones libvpx v1.16.0 at build time and runs its
   `configure`/`make` (`--disable-shared --enable-vp9-highbitdepth`), then
   prefixes every symbol with `shiguredo_vpx_` using `llvm-nm`/`llvm-objcopy`
   from `rustup component add llvm-tools` (installed on this host for the
   test; not a Homebrew package). Without `source-build` it downloads a
   prebuilt from GitHub releases. Either way the build needs network access
   and `git`; on x86-64 hosts libvpx's own build wants `nasm`/`yasm` (not
   needed on arm64, so not installed here).
4. **`str0m-apple-crypto` links Swift.** Its `apple-cryptokit-rs` dependency
   compiles a Swift shim with the Xcode toolchain and the binary then loads
   `CryptoKit.framework`, `Security.framework` and the `libswift*.dylib`
   runtime (visible in `otool -L`). This is the +4.2 MiB RSS and ~+40 ms
   startup of the `rtc` rung and a build dependency on Xcode (not just the
   command line tools) that `docs/spec/12-platforms.md` does not list.
5. **rusty_v8 prebuilt download.** `v8` 152.2.0 fetches
   `librusty_v8_release_aarch64-apple-darwin.a.gz` (40.6 MB) from GitHub at
   build time; `RUSTY_V8_MIRROR`/`RUSTY_V8_ARCHIVE` exist for offline CI.
   Pointer compression was not measured (default features, as specified for
   this ladder); a `_ptrcomp` prebuilt exists for this host and is the
   obvious next size experiment.
6. **Root `cargo fmt --all --check` fails outside this package**:
   `failed to resolve mod codegen: crates/tenta-idl/src/codegen.rs does not exist`
   (in-progress work of package (c)). `cargo fmt --check` passes for `xtask`
   and for `tests/linktest`; `cargo clippy -p xtask -- -D warnings` and
   `cargo clippy --features all -- -D warnings` in `tests/linktest` pass.
7. Not done on this host: the other four targets of the link test
   (`docs/spec/16-roadmap.md` P0 (b) says "on all five targets"). This host
   has no Linux/Windows toolchain for the native builds, and for Linux and
   Android the result is already known from the dependency graph (finding 1).
   iOS and Android builds of the ladder are CI work (nightly, `12` §7).

## Fonts bundle (A0, D-028)

Measured 2026-10-09 on the A0 tree. Byte counts are the file sizes of
`crates/tenta-layout/fonts/`, and the threshold is the 3 MiB rule of
`docs/dev/p3-plan.md` §2 (3,145,728 bytes).

| Set | Files | Bytes | MiB |
|-----|-------|-------|-----|
| Full set: Liberation Sans, Serif and Mono in Regular, Bold, Italic and BoldItalic, plus DejaVu Sans | 13 | 5,116,240 | 4.88 |
| Dropped: the six Italic and BoldItalic faces | 6 | 2,142,820 | 2.04 |
| Shipped: the six Liberation Regular and Bold faces plus DejaVu Sans | 7 | 2,973,420 | 2.84 |

The full set exceeds 3 MiB, so the Italic and BoldItalic faces are not shipped and
italic text uses synthetic oblique (D-028 and the plan's rule). The shipped set is
under the threshold. Against the `21` §4 fonts budget (<= 6 MiB ⓣ, which includes
the Noto CJK subset and COLRv1 emoji not yet bundled), 2.84 MiB leaves about
3.16 MiB for those two.

Library level, `cargo build --release -p tenta-layout --locked` against
`--no-default-features` (`rustc 1.98.0`, `aarch64-apple-darwin`; the rlib is
the `libtenta_layout.rlib` file size):

| Build | rlib size |
|-------|-----------|
| `tenta-layout`, default (`embedded-fonts`) | 5,968,960 B (5.69 MiB) |
| `tenta-layout`, `--no-default-features` | 14,600 B |

The rlib carries the font bytes because `EMBEDDED_FONTS` is a `pub static`,
which every library build emits.

Binary level, `cargo build --release -p tentabrowser-cli --features render --locked`
(the release profile keeps debug line tables and does not strip, so this is not
comparable with the stripped ladder above):

| Build | Size | Shipped font bytes found in the binary |
|-------|------|----------------------------------------|
| `tentabrowser` CLI, `render`, default `embedded-fonts` | 17,971,008 B (17.14 MiB) | 0 of 7 (a 64-byte slice of each file, checked by byte search) |

The embedded-fonts delta in the binary is therefore 0 B at A0. Nothing in the
binary references `EMBEDDED_FONTS` outside the tests, and the linker drops
unreferenced data, so the bytes in the rlib do not reach the CLI. The "without
`embedded-fonts`" CLI arm cannot be built: `tenta-render` depends on
`tenta-layout` with its default features, and no feature path turns them off.
Re-measure when T1a makes the fonts reachable from the binary.
