# V8 artifact setup

V8-enabled builds use the unpublished local `tenta-v8` bindings and a matching
reviewed native archive. Follow [the artifact setup and native source recipe](../../crates/tenta-v8/native/BUILDING.md)
before running V8 or combined workspace Cargo commands. Set
`TENTA_V8_ARTIFACT_DIR` for each command to the verified bundle in a durable local
cache; no global environment edit or committed absolute path is required.

The default macOS arm64 bundle is the only pinned configuration. Unsupported
targets/features fail rather than linking an upstream archive. Run the Temporal
lock validator after dependency resolution, keep licenses/notices with copied
artifacts, and preserve separate source/runtime/performance evidence. The native
classifier is a building block for structured cloning, not full clone support.

Desktop CI provisioning remains incomplete. Workspace Clippy and nextest build
`tenta-v8` even when CLI default features are empty. The workflow currently has
no bundle provisioning step, and Linux x86_64/Windows x86_64 have no reviewed
artifact pins. A local macOS cache does not satisfy these CI prerequisites.
Keep all three desktop gates: each needs its own verified native artifact and
target runtime validation. The SDK 26.5-produced macOS bundle has not yet been
executed on the macOS 14 CI image. Source reconstruction requires Python 3.12+;
the existing CI Python 3.11 setup only runs the lock validator and WPT tooling.

CI Cargo validation and mobile builds use `--locked`, and successful desktop
validation checks that `Cargo.lock` was not modified. These checks preserve the
reviewed dependency graph; they do not supply the missing native artifacts.
