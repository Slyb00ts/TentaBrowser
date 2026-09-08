//! `fetch --dump text` never touches V8, Stylo or fonts
//! (`docs/spec/13-performance.md` §1).
//!
//! Nothing above Tier 1 exists in the workspace yet, so a symbol check on the
//! linked binary would pass for the wrong reason: there would be no symbols to
//! find. What can be asserted today, and what keeps holding when P2 and P3
//! land their backends behind features, is the dependency graph: the default
//! build of `tentabrowser-cli` must not contain a script engine, a style
//! engine or a GPU stack. When a later phase adds `tenta-js-v8` behind the
//! `v8` feature, this test fails the moment the feature leaks into the
//! default set.

use std::process::Command;

/// Crate names that only a tier above 1 can need. Matched against the
/// dependency graph's crate names as whole words.
const FORBIDDEN: &[&str] = &[
    "v8",
    "rusty_v8",
    "stylo",
    "style",
    "wgpu",
    "quickjs",
    "rquickjs",
    "servo-fontconfig",
];

#[test]
fn the_default_cli_build_pulls_in_no_engine_above_tier_one() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--quiet",
            "-p",
            "tentabrowser-cli",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--no-dedupe",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8_lossy(&output.stdout);
    let crates: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert!(
        crates.contains(&"tentabrowser-cli"),
        "the tree does not name the crate under test"
    );
    assert!(
        crates.contains(&"tenta-net"),
        "Tier 0 is in the graph, so the check is looking at the right tree"
    );

    for forbidden in FORBIDDEN {
        assert!(
            !crates.contains(forbidden),
            "{forbidden} is in the default `tentabrowser-cli` graph; \
             `fetch --dump text` must pay for nothing above Tier 1"
        );
    }
}
