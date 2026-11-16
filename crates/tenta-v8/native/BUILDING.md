# Building with the reviewed V8 artifact

The local `tenta-v8` package uses a reviewed native bundle. It cannot download
an upstream archive or build native C++ implicitly. The committed dependency is
a repository-relative package path; the artifact directory is an explicit
per-command environment input. No global environment change is needed.

## Install a durable local bundle

Obtain the three files from the reviewed native30 build output. There is no
published download endpoint in this preparation. Copy them into a durable local
cache, not a temporary build directory. For the accepted macOS arm64 default
configuration, run the following with `bundle_source` set to that output:

```sh
bundle_source="/path/to/reviewed/native30-bundle"
artifact_cache="${XDG_CACHE_HOME:-$HOME/.cache}/tentabrowser/v8/152.2.0/aarch64-apple-darwin/default/f7393a5194ca5e83"
mkdir -p "$artifact_cache"
cp -n "$bundle_source/librusty_v8.a" "$bundle_source/src_binding.rs" "$bundle_source/provenance.json" "$artifact_cache/"
python3 crates/tenta-v8/native/validation/verify_temporal_lock.py --lockfile Cargo.lock
TENTA_V8_ARTIFACT_DIR="$artifact_cache" cargo nextest run -p tenta-v8 --test web_idl_legacy --test web_idl_order --test ordinary_object_brand --test termination_now --test build --locked --no-fail-fast
```

`cp -n` never overwrites an existing cache entry. The build guard validates each
private copy against these exact pins and checks the final archive used by the
linker. A missing, corrupt or mismatched entry fails the build; it does not select
an older file or upstream fallback. Use a new cache directory to repair a corrupt
entry and preserve the rejected bytes when investigating a failed build.

| File | SHA-256 |
| --- | --- |
| librusty_v8.a | f7393a5194ca5e8360b10c50b465944dad656fec071f7f82dddf01544aa6d8c3 |
| src_binding.rs | d6594abe610e3364c13c0b78a901c2eaa3a6422afaebcafdee9c8af6950315b1 |
| provenance.json | 29955d05d43e7c4d01181a9880ff6d99898c63851b6160302cfefa6e13bb8aa2 |

Pass the same `TENTA_V8_ARTIFACT_DIR` for any Cargo command that builds the V8
route, including workspace tests, Clippy and release examples. Unset upstream
`RUSTY_V8_*` overrides, `V8_FROM_SOURCE`, `GN_ARGS`, `EXTRA_GN_ARGS`, `DENO_TRYBUILD`
and `CXXSTDLIB` for the binding package. Its `.rustfmt.toml` preserves upstream
formatting. The accepted Temporal/Diplomat lock validator is a separate required
CI preflight; an exact direct temporal_capi pin alone does not constrain all
implementation/runtime dependencies.

Only `aarch64-apple-darwin` with default `use_custom_libcxx`, native release mode,
no pointer compression, no sandbox and no V8 checks has a reviewed pin in
`artifact_pins.rs`. A workstation can build the same configuration for its own
target and record it in a local pin (next section), which a build accepts only
when `TENTA_V8_LOCAL_PIN` names it. `x86_64-unknown-linux-gnu` was built that
way: on Fedora 43 (2026-11-16) its source build passed the same 30 native tests
and the binding-package route passed 58/58 (30 native and 28 build/guard
controls). It has no reviewed toolchain provenance. A target with neither pin,
other feature combinations, native debug and sanitizer settings deliberately
fail. The snapshot and generated binding are tied to the configuration. Neither
result establishes Windows, Android or iOS support.

## Build a bundle on a workstation

`./setup.sh` (repository root) does this when the cache has no bundle for the
host target; `--no-v8` skips it. The steps, which can also be run by hand:

```sh
build="${XDG_CACHE_HOME:-$HOME/.cache}/tentabrowser/v8/build"
python3 crates/tenta-v8/native/recipe/bootstrap_source.py --downloads "$build/downloads" --destination "$build/source/v8-152.2.0-native30"
cd "$build/source/v8-152.2.0-native30"
python3 tools/clang/scripts/update.py --output-dir "$build/target/release/clang" && python3 tools/rust_toolchain.py
env V8_FROM_SOURCE=1 LIBCLANG_PATH="$PWD/third_party/rust-toolchain/lib" RUSTY_V8_BINDGEN_RESOURCE_DIR="$("$build/target/release/clang/bin/clang" -print-resource-dir)" SOURCE_DATE_EPOCH=1790121600 GN_ARGS="symbol_level=0 treat_warnings_as_errors=false use_glib=false" CARGO_TARGET_DIR="$build/target" cargo +1.98.0 nextest run --release --locked --test web_idl_legacy --test web_idl_order --test ordinary_object_brand --test termination_now --no-fail-fast
python3 "$repository/crates/tenta-v8/native/install_bundle.py" install --target "$target" --source "$build/source/v8-152.2.0-native30" --release "$build/target/release" --native-tests "passed: ..." --pin
```

`bootstrap_source.py` downloads the crate archive and the 46 supplemental
inputs and reconstructs the native30 tree. Gitiles regenerates its tar.gz
archives on request, so a fresh download does not have the compressed bytes
the lock records. Each archive is therefore verified by the Git tree id in the
lock, computed over its members, and the ICU data file by its SHA-256. The
package checksum, both patch checksums and the before/after hash of every
changed file are checked exactly as the offline recipe below checks them.

The upstream build script then downloads the Clang, GN, Ninja, Rust and (on
Linux) sysroot packages that the source tree's own scripts pin, compiles V8
and generates the binding with bindgen. Bindgen parses V8's libc++ headers and
needs libclang 21.1 or newer, which most distributions do not ship. On Linux
`setup.sh` therefore fetches the Clang and Rust packages before Cargo starts
and points bindgen at the libclang inside Chromium's Rust toolchain
(`third_party/rust-toolchain/lib`, the build of the same LLVM revision as the
compiler) with `RUSTY_V8_BINDGEN_RESOURCE_DIR` set to that Clang's resource
directory. On macOS it uses Homebrew's `llvm`, as the accepted run did. An
exported `LIBCLANG_PATH` overrides both; `clang` must then be in the sibling
`bin` directory. The GN arguments are the accepted macOS ones;
`treat_warnings_as_errors=false` is part of them, and Chromium's Clang rejects
two enum conversions in the patched `keys.cc` without it. Linux adds
`use_glib=false`: GN otherwise asks the host's pkg-config for glib, which V8
as a library does not use. The command above is the Linux one; on macOS drop
the `update.py`/`rust_toolchain.py` line, `RUSTY_V8_BINDGEN_RESOURCE_DIR` and
`use_glib=false`, and set `LIBCLANG_PATH="$(brew --prefix llvm)/lib"`.

`install_bundle.py install` copies `librusty_v8.a`, `src_binding.rs` and a
`provenance.json` describing the build into
`.../tentabrowser/v8/152.2.0/<target>/default/<first 16 hex of the archive hash>`
and, with `--pin`, writes the local pin `local-pin.toml` beside that directory.
It refuses a tree that is not the one `recipe/native30-delta.json` describes,
never replaces an installed archive or binding with other bytes, and replaces
the description of an archive it reproduced only with `--replace-provenance`
(`./setup.sh --rebuild-v8`). A build that reproduces the reviewed archive
leaves that bundle as it is and writes no local pin.

A local pin is the machine's own record of what it built: the target and
configuration, the three hashes, and the SHA-256 of `recipe/native30-delta.json`,
so it stops matching when the source recipe changes. It is not in the
repository, and `artifact_pins.rs` stays the table of reviewed bundles. The
guard reads a local pin only when `TENTA_V8_LOCAL_PIN` names the file; it then
checks the same three hashes against it, on the copy it links, as it does for a
reviewed pin. A build without that variable accepts reviewed bundles alone,
which is what CI and release builds get.

```sh
eval "$(python3 crates/tenta-v8/native/install_bundle.py env)"
```

sets `TENTA_V8_ARTIFACT_DIR` to the installed directory a build accepts and
`TENTA_V8_LOCAL_PIN` to its local pin, or to nothing when the directory is a
reviewed bundle, which goes first. `install_bundle.py find` prints the
directory alone. Both take `--target`; the default is the host.

A local pin records a workstation build. It is not the reviewed acceptance run
of the macOS bundle: the toolchain packages are whatever the pinned source
scripts fetch, and no provenance of them is kept beyond the Clang and rustc
version lines.

The archive is reproducible on one machine and not across machines. A second
full build in the same directory on Fedora 43 gave the same archive, byte for
byte. The same source built on Fedora 43, Arch and Ubuntu 24.04 (2026-11-16,
each passing the 30 native tests) gave one binding and three archives that
differ in size by a few kilobytes. The x86_64 build uses no sysroot, so it
compiles against the host's C library headers, and the build directory differed
too; which of the two accounts for the difference was not established. That is
why each machine keeps its own pin instead of sharing one through the tracked
table. A bundle can still be moved: copy the bundle directory and
`local-pin.toml` to the same place in the other machine's cache.

## Reconstruct native source outside the repository

The native engine source is reconstructed from the pinned v8 152.2.0 crate and
46 verified supplemental inputs. `recipe/prepare_source.py` verifies archive,
patch, input and before/after source hashes, then publishes a fresh source tree.
It rejects replacement of an existing destination. This command prepares source
only; it does not claim to provision compilers, SDKs or native artifacts.

```sh
python3 crates/tenta-v8/native/recipe/prepare_source.py --crate-archive "$verified_v8_crate" --inputs "$verified_supplemental_inputs" --destination "$native_cache/source/v8-152.2.0"
```

The combined native patch is SHA-256
`2091d2806100fb4ea488312f88fa31ecee871fa773ecd431b4f6da26561a32cd`.
Its 25 changed files were reconstructed byte-for-byte against the source that
passed the native23 controls. The unchanged upstream crate is SHA-256
`a10fe1a92da5c32c7c7f838ce36c0ccfcfd5edf0865b58bdde820aa64cea9888`.
The five hostile source-preparation controls also passed.

The native30 source layers `recipe/incremental-native30.patch` (SHA-256
`a688da8fe4174962bfd41242d95560ee271b17333197c40889cd904ad99e036a`)
on the accepted native23 recipe. It adds the callback-only synchronous
termination binding and the executed Symbol/termination controls; no V8 engine
source or GN configuration changes. Reconstruct the tested source with:

```sh
python3 crates/tenta-v8/native/recipe/reconstruct_native30.py --crate-archive "$verified_v8_crate" --inputs "$verified_supplemental_inputs" --destination "$native_cache/source/v8-152.2.0-native30"
```

This source-only reconstruction matched all 35 preserved native30 inputs. The
independent native run passed 30/30 tests and rebuilt only `binding.o` and the
archive. The reviewed native30 artifact is pinned above; no other target is
accepted by this result.

The public-Symbol StoreIC candidate can be reconstructed with the same command
and `--storeic`. This applies `recipe/symbol-storeic.patch` after native30 and
checks all 35 recorded inputs before and after that one-file change. The final
source reconstruction and six preparation tests passed; the native candidate
separately passed 30 controls. These results do not promote the candidate:
the active artifact pins above remain native30. Two 25-pair Storage comparisons
confirm the Symbol median improvement, but other-operation p95 increases under
external compiler load remain unresolved. See `docs/dev/implementation-status.md`
for the measurement scope and pending integration gates.

Native compilation remains an explicitly provisioned, separate operation.
Use the pinned DEPS Clang/LLD package `llvmorg-23-init-19482-g53d18800-1`, exact
GN/Ninja and Rust sub-toolchain identities recorded under `native/provenance` and hashed by the artifact provenance,
Rust/Cargo 1.98.0 and macOS SDK26.5/build25F70. The accepted run used Homebrew
libclang23 only as bindgen's parser. Temporal remained enabled. A canonical SDK
symlink must be inside the native GN output directory; both `SDKROOT` and
`mac_sdk_path` must select that same SDK. Do not silently use the system default.

The native30 command shape, in the tree produced by `reconstruct_native30.py`, is:

```sh
env V8_FROM_SOURCE=1 CLANG_BASE_PATH="$verified_clang" LIBCLANG_PATH="$verified_libclang" GN="$verified_gn" NINJA="$verified_ninja" SOURCE_DATE_EPOCH=1790121600 SDKROOT="$verified_sdk" CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR="$native_target" GN_ARGS="symbol_level=0 mac_sdk_path=\"$canonical_sdk_link\"" cargo +1.98.0 nextest run --release --locked --manifest-path "$native_cache/source/v8-152.2.0-native30/Cargo.toml" --test web_idl_legacy --test web_idl_order --test ordinary_object_brand --test termination_now --no-fail-fast
```

The variables are explicit provisioned inputs, not guessed paths or public
artifact URLs. Verify the seven Temporal/Diplomat versions in that source lock
before building. Record tool hashes/versions, resolved GN arguments, SDK,
features, source manifests, generated snapshot, binding, archive, exact exit and
full test identifiers. The native23 and native30 increments each rebuilt only `binding.o` and the
archive; source reconstruction also supports a clean rebuild, but portable
full toolchain bootstrap and other target builds remain unexecuted gates.

Use one native job initially on the measured 16GiB host and check free disk
before extraction/build. Keep native compilation idle during benchmark samples.
Cache by all source/configuration/toolchain identities and then by output hashes,
not merely upstream version. A rebuilt archive with different bytes must receive
new review and pins; never replace an accepted bundle under its old checksum.
