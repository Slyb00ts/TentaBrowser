# V8 artifact setup

V8-enabled builds use the unpublished local `tenta-v8` bindings and a matching
pinned native archive. `./setup.sh` provides one: it looks in the cache for a
bundle a build accepts and otherwise builds V8 from source, runs the native
tests and records the result in a local pin. [The artifact setup and native source recipe](../../crates/tenta-v8/native/BUILDING.md)
has the steps and what each one verifies. Select the bundle for each shell that
builds V8; no global environment edit or committed absolute path is required:

```bash
eval "$(python3 crates/tenta-v8/native/install_bundle.py env)"
cargo build --release -p tentabrowser-cli --features full
```

That sets `TENTA_V8_ARTIFACT_DIR`, the bundle directory, and
`TENTA_V8_LOCAL_PIN`, the local pin when the bundle was built on this machine.

Two kinds of pin exist. `crates/tenta-v8/artifact_pins.rs` is the tracked table
of reviewed bundles; the macOS arm64 one is there, with toolchain provenance
and the native acceptance run. A local pin is a file in the cache, beside the
bundle, that `install_bundle.py` writes for a workstation build: the source is
verified the same way and the native tests ran, but the toolchain is whatever
the pinned source scripts fetch and nobody else has looked at the result. The
build guard checks the hashes of either kind on the bytes it links. It reads a
local pin only when `TENTA_V8_LOCAL_PIN` names it, and a local pin names the
source recipe it was built from, so it stops matching when the recipe changes.
A build without the variable accepts reviewed bundles only.

Unsupported targets/features fail rather than linking an upstream archive. Run
the Temporal lock validator after dependency resolution, keep licenses/notices
with copied artifacts, and preserve separate source/runtime/performance
evidence. The native classifier is a building block for structured cloning,
not full clone support.

Archives are reproducible on one machine but not across machines (three Linux
distributions gave three archives and one binding), so each machine builds and
pins its own. `./setup.sh --rebuild-v8` builds again and moves the local pin to
the new build.

Desktop CI provisioning remains incomplete. Workspace Clippy and nextest build
`tenta-v8` even when CLI default features are empty. The workflow currently has
no bundle provisioning step, and Linux and Windows x86_64 have no reviewed
artifact pin. A local cache does not satisfy these CI prerequisites: a hosted
runner would need a reviewed bundle delivered to it, or the 10 to 60 minute
source build cached and its local pin named.
Keep all three desktop gates: each needs its own verified native artifact and
target runtime validation. The SDK 26.5-produced macOS bundle has not yet been
executed on the macOS 14 CI image. Source reconstruction requires Python 3.12+;
the existing CI Python 3.11 setup only runs the lock validator and WPT tooling.

CI Cargo validation and mobile builds use `--locked`, and successful desktop
validation checks that `Cargo.lock` was not modified. These checks preserve the
reviewed dependency graph; they do not supply the missing native artifacts.
