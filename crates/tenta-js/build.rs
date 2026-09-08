//! Generates the DOM binding glue from `idl/dom` with `tenta-idl`.
//!
//! The generator emits engine-independent output (`prelude.rs`, `types.rs`,
//! `*_ops.rs`, `*_register.rs`) plus per-engine glue (`*_v8.rs`, `*_qjs.rs`).
//! `src/bindings/mod.rs` includes the engine-independent files unconditionally
//! and the engine glue behind the matching `v8` / `quickjs` feature.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let idl_dom = manifest_dir.join("..").join("..").join("idl").join("dom");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed={}", idl_dom.display());

    tenta_idl::generate_directory(&idl_dom, &out_dir)
        .unwrap_or_else(|error| panic!("generating DOM bindings from {idl_dom:?}: {error}"));
}
