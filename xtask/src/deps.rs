//! Dependency-direction lint (`docs/spec/02-architecture.md` §2).
//!
//! Foundation crates form a strict top-down order. Feature crates may depend
//! on foundation crates and on each other only along the listed edges.
//! Assembly crates may depend on anything above them in their own order.
//! `tenta-testutil` is a dev-dependency only; `tenta-idl` is a
//! build-dependency only and depends on no internal crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

const FOUNDATION: &[&str] = &[
    // Vendored fork of `h2` (docs/spec/02-architecture.md §2): it depends on
    // no internal crate, so it sits above everything else in the order.
    "tenta-h2",
    "tenta-core",
    "tenta-net",
    "tenta-html",
    "tenta-dom",
    "tenta-css",
    "tenta-idl",
    "tenta-js",
    "tenta-js-v8",
    "tenta-js-qjs",
];

const FEATURE: &[&str] = &[
    "tenta-layout",
    "tenta-paint",
    "tenta-render",
    "tenta-media",
    "tenta-webrtc",
];

/// The only permitted edges between feature crates.
const FEATURE_EDGES: &[(&str, &str)] = &[
    ("tenta-paint", "tenta-layout"),
    ("tenta-render", "tenta-layout"),
    ("tenta-render", "tenta-paint"),
    ("tenta-webrtc", "tenta-media"),
];

const ASSEMBLY: &[&str] = &[
    "tenta-page",
    "tenta-browser",
    "tenta-search",
    "tenta-cdp",
    "tenta-mcp",
    "tentabrowser",
    "tenta-ffi",
    "tentabrowser-cli",
];

const DEV_ONLY: &[&str] = &["tenta-testutil"];
const BUILD_ONLY: &[&str] = &["tenta-idl"];

#[derive(Deserialize)]
struct Manifest {
    package: Package,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

fn is_internal(name: &str) -> bool {
    name.starts_with("tenta-") || name == "tentabrowser" || name == "tentabrowser-cli"
}

fn allowed(from: &str) -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    if let Some(pos) = FOUNDATION.iter().position(|c| *c == from) {
        set.extend(FOUNDATION[..pos].iter().copied());
        // Engine backends are siblings: neither depends on the other.
        set.remove("tenta-js-v8");
        set.remove("tenta-js-qjs");
        set.remove("tenta-idl");
        return set;
    }
    if FEATURE.contains(&from) {
        set.extend(FOUNDATION.iter().copied());
        set.remove("tenta-idl");
        set.remove("tenta-js-v8");
        set.remove("tenta-js-qjs");
        set.extend(
            FEATURE_EDGES
                .iter()
                .filter(|(a, _)| *a == from)
                .map(|(_, b)| *b),
        );
        return set;
    }
    if let Some(pos) = ASSEMBLY.iter().position(|c| *c == from) {
        set.extend(FOUNDATION.iter().copied());
        set.remove("tenta-idl");
        set.extend(FEATURE.iter().copied());
        set.extend(ASSEMBLY[..pos].iter().copied());
        // Only the browser assembly selects engine backends.
        if from != "tenta-browser" && from != "tentabrowser" {
            set.remove("tenta-js-v8");
            set.remove("tenta-js-qjs");
        }
        return set;
    }
    if DEV_ONLY.contains(&from) {
        set.extend(FOUNDATION.iter().copied());
        set.remove("tenta-idl");
        return set;
    }
    set
}

pub fn lint(root: &Path) -> anyhow::Result<()> {
    let crates_dir = root.join("crates");
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).context("reading crates/")? {
        let path = entry?.path().join("Cargo.toml");
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let manifest: Manifest =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            manifests.push(manifest);
        }
    }
    manifests.sort_by(|a, b| a.package.name.cmp(&b.package.name));

    let known: BTreeSet<&str> = FOUNDATION
        .iter()
        .chain(FEATURE)
        .chain(ASSEMBLY)
        .chain(DEV_ONLY)
        .copied()
        .collect();

    let mut errors = Vec::new();
    for manifest in &manifests {
        let name = manifest.package.name.as_str();
        if !known.contains(name) {
            errors.push(format!(
                "{name}: not listed in xtask/src/deps.rs; add it to the crate map first"
            ));
            continue;
        }
        let allowed = allowed(name);
        for dep in manifest.dependencies.keys().filter(|d| is_internal(d)) {
            if DEV_ONLY.contains(&dep.as_str()) {
                errors.push(format!("{name}: {dep} may only be a dev-dependency"));
            } else if BUILD_ONLY.contains(&dep.as_str()) {
                errors.push(format!("{name}: {dep} may only be a build-dependency"));
            } else if !allowed.contains(dep.as_str()) {
                errors.push(format!(
                    "{name}: dependency on {dep} violates the crate layering (02-architecture.md §2)"
                ));
            }
        }
        for dep in manifest
            .build_dependencies
            .keys()
            .filter(|d| is_internal(d))
        {
            if dep != "tenta-idl" {
                errors.push(format!(
                    "{name}: build-dependency on {dep}; only tenta-idl is allowed"
                ));
            }
        }
        for dep in manifest.dev_dependencies.keys().filter(|d| is_internal(d)) {
            if !allowed.contains(dep.as_str()) && !DEV_ONLY.contains(&dep.as_str()) {
                errors.push(format!(
                    "{name}: dev-dependency on {dep} violates the crate layering"
                ));
            }
        }
    }

    for crate_name in &known {
        if !manifests.iter().any(|m| m.package.name == *crate_name) {
            errors.push(format!(
                "{crate_name}: listed in the crate map but missing from crates/"
            ));
        }
    }

    if errors.is_empty() {
        println!(
            "lint-deps: {} crates, dependency direction OK",
            manifests.len()
        );
        Ok(())
    } else {
        for error in &errors {
            eprintln!("error: {error}");
        }
        bail!("{} dependency-direction violation(s)", errors.len());
    }
}
