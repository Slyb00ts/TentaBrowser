//! Repository tooling. Run with `cargo xtask <command>`.
//!
//! Commands:
//! - `lint-deps`: enforce the crate dependency direction of
//!   `docs/spec/02-architecture.md` §2 by reading every crate manifest.
//! - `size-report`: build the release binary and print its size (the size
//!   budget table of `docs/spec/21-project-operations.md` §4 is filled from
//!   this output).
//! - `link-test`: build every rung of the `tests/linktest` feature ladder and
//!   fail if the binary contains more than one libcrypto
//!   (`docs/spec/03-network.md` §1.1).
//! - `hello-sizes`: measure stripped size and idle RSS of every rung and write
//!   `bench/baselines/hello-sizes.json` (`docs/spec/21-project-operations.md` §4).
//! - `notices`: regenerate `THIRD_PARTY_NOTICES.md` with `cargo about`
//!   (`docs/spec/21-project-operations.md` §1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

mod deps;
mod linktest;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let root = workspace_root()?;
    match command.as_str() {
        "lint-deps" => deps::lint(&root),
        "size-report" => size_report(&root),
        "notices" => notices(&root),
        "link-test" => linktest::link_test(&root),
        "hello-sizes" => linktest::hello_sizes(&root),
        "idl-manifest" => idl_manifest(&root),
        _ => {
            eprintln!(
                "usage: cargo xtask <lint-deps | size-report | notices | link-test | hello-sizes | idl-manifest>"
            );
            std::process::exit(2);
        }
    }
}

/// Checks that `idl/MANIFEST.toml` is up to date with the golden-site
/// inventories and `idl/phase-rules.toml` (`python3 idl/gen_manifest.py --check`).
fn idl_manifest(root: &Path) -> anyhow::Result<()> {
    let status = Command::new("python3")
        .args(["idl/gen_manifest.py", "--check"])
        .current_dir(root)
        .status()
        .context("running idl/gen_manifest.py")?;
    if !status.success() {
        bail!("idl/MANIFEST.toml is stale; run `python3 idl/gen_manifest.py`");
    }
    Ok(())
}

fn workspace_root() -> anyhow::Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .context("xtask must live one level below the workspace root")?;
    Ok(root.to_path_buf())
}

fn size_report(root: &Path) -> anyhow::Result<()> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "tentabrowser-cli"])
        .current_dir(root)
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("cargo build failed");
    }
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    let binary = root.join("target/release/tentabrowser");
    let binary = if binary.exists() {
        binary
    } else {
        root.join("target/release/tentabrowser.exe")
    };
    let len = std::fs::metadata(&binary)
        .with_context(|| format!("reading {}", binary.display()))?
        .len();
    sizes.insert("tentabrowser (release, unstripped)".to_string(), len);
    for (name, bytes) in &sizes {
        println!(
            "{name}: {} bytes ({:.2} MB)",
            bytes,
            *bytes as f64 / 1_048_576.0
        );
    }
    Ok(())
}

/// Regenerates `THIRD_PARTY_NOTICES.md` from `about.toml` and `about.hbs`.
/// Requires `cargo-about` (`cargo install cargo-about`).
fn notices(root: &Path) -> anyhow::Result<()> {
    let status = Command::new(env!("CARGO"))
        .args([
            "about",
            "generate",
            "about.hbs",
            "-o",
            "THIRD_PARTY_NOTICES.md",
        ])
        .current_dir(root)
        .status()
        .context("running cargo about (is cargo-about installed?)")?;
    if !status.success() {
        bail!("cargo about generate failed");
    }
    println!("wrote {}", root.join("THIRD_PARTY_NOTICES.md").display());
    Ok(())
}
