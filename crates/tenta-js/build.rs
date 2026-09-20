//! Generates the DOM binding glue from `idl/dom`, `idl/url`, `idl/fetch` and
//! `idl/encoding` with `tenta-idl`.
//!
//! The generator emits engine-independent output (`prelude.rs`, `types.rs`,
//! `*_ops.rs`, `*_register.rs`) plus per-engine glue (`*_v8.rs`, `*_qjs.rs`).
//! `src/bindings/mod.rs` includes the engine-independent files unconditionally
//! and the engine glue behind the matching `v8` / `quickjs` feature.
//!
//! Every directory is lowered as one unit and generated once: `generate`
//! writes `prelude.rs` and `types.rs` per model, so a second call would
//! overwrite the first. Each directory still gets its own
//! `cargo:rerun-if-changed`, so a change under `idl/url` reruns the build.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let idl_root = manifest_dir.join("..").join("..").join("idl");
    let directories = [
        idl_root.join("dom"),
        idl_root.join("url"),
        idl_root.join("fetch"),
        idl_root.join("encoding"),
    ];
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    for directory in &directories {
        println!("cargo:rerun-if-changed={}", directory.display());
    }

    tenta_idl::generate_directories(directories.iter().map(PathBuf::as_path), &out_dir)
        .unwrap_or_else(|error| panic!("generating DOM bindings from {directories:?}: {error}"));
}
