//! Dependency-direction lint (`docs/spec/02-architecture.md` §2).
//!
//! Foundation crates form a strict top-down order. Feature crates may depend
//! on foundation crates and on each other only along the listed edges.
//! Assembly crates may depend on anything above them in their own order.
//! `tenta-testutil` is a dev-dependency only; `tenta-idl` is a
//! build-dependency only (its engine compile checks may use native bindings).

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
    "tenta-quickjs-sys",
    "tenta-quickjs-core",
    "tenta-v8",
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
    #[serde(flatten)]
    dependencies: Dependencies,
    #[serde(default)]
    target: BTreeMap<String, Dependencies>,
}

#[derive(Default, Deserialize)]
struct Dependencies {
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct WorkspaceManifest {
    workspace: Workspace,
}

#[derive(Deserialize)]
struct Workspace {
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

fn is_internal(name: &str) -> bool {
    name.starts_with("tenta-") || name == "tentabrowser" || name == "tentabrowser-cli"
}

fn allowed(from: &str) -> BTreeSet<&'static str> {
    match from {
        "tenta-quickjs-sys" => return BTreeSet::new(),
        "tenta-quickjs-core" => return BTreeSet::from(["tenta-quickjs-sys"]),
        _ => {}
    }
    let mut set = layer_allowed(from);
    // Native engine bindings are consumed only by their backend and the
    // generated glue/compile checks, not by arbitrary higher-layer crates.
    set.remove("tenta-quickjs-sys");
    set.remove("tenta-quickjs-core");
    if matches!(from, "tenta-idl" | "tenta-js" | "tenta-js-qjs") {
        set.insert("tenta-quickjs-core");
    }
    set
}

fn layer_allowed(from: &str) -> BTreeSet<&'static str> {
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

/// Cargo aliases name an edge by `package`, including inherited declarations.
/// Inspect all target tables, since a platform-specific edge still has to obey
/// the architecture even when the lint runs on another platform.
fn dependency_errors(manifest: &Manifest, workspace: &Workspace) -> anyhow::Result<Vec<String>> {
    let name = manifest.package.name.as_str();
    let allowed = allowed(name);
    let mut errors = Vec::new();
    for dependencies in std::iter::once(&manifest.dependencies).chain(manifest.target.values()) {
        for (kind, table) in [
            ("normal", &dependencies.dependencies),
            ("build", &dependencies.build_dependencies),
            ("dev", &dependencies.dev_dependencies),
        ] {
            for (alias, declaration) in table {
                let optional =
                    declaration.get("optional").and_then(toml::Value::as_bool) == Some(true);
                let declaration = if declaration.get("workspace").and_then(toml::Value::as_bool)
                    == Some(true)
                {
                    workspace.dependencies.get(alias).with_context(|| {
                        format!("{name}: inherited dependency {alias} is missing from workspace.dependencies")
                    })?
                } else {
                    declaration
                };
                let dep = declaration
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias);
                if !is_internal(dep) {
                    continue;
                }
                if matches!(name, "tenta-quickjs-sys" | "tenta-quickjs-core")
                    && !allowed.contains(dep)
                {
                    errors.push(format!(
                        "{name}: {kind} dependency on {dep} violates native fork isolation"
                    ));
                    continue;
                }
                if kind == "normal"
                    && dep == "tenta-quickjs-core"
                    && matches!(name, "tenta-idl" | "tenta-js")
                    && !optional
                {
                    errors.push(format!(
                        "{name}: {dep} must be optional for backend-free builds"
                    ));
                }
                match kind {
                    "normal" if DEV_ONLY.contains(&dep) => {
                        errors.push(format!("{name}: {dep} may only be a dev-dependency"));
                    }
                    "normal" if BUILD_ONLY.contains(&dep) => {
                        errors.push(format!("{name}: {dep} may only be a build-dependency"));
                    }
                    "normal" if !allowed.contains(dep) => errors.push(format!(
                        "{name}: dependency on {dep} violates the crate layering (02-architecture.md §2)"
                    )),
                    "build" if dep != "tenta-idl" => errors.push(format!(
                        "{name}: build-dependency on {dep}; only tenta-idl is allowed"
                    )),
                    "dev" if !allowed.contains(dep) && !DEV_ONLY.contains(&dep) => {
                        errors.push(format!(
                            "{name}: dev-dependency on {dep} violates the crate layering"
                        ));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(errors)
}

pub fn lint(root: &Path) -> anyhow::Result<()> {
    let workspace: WorkspaceManifest = toml::from_str(
        &std::fs::read_to_string(root.join("Cargo.toml"))
            .context("reading workspace Cargo.toml")?,
    )
    .context("parsing workspace Cargo.toml")?;
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
        errors.extend(dependency_errors(manifest, &workspace.workspace)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check(source: &str, workspace: &str) -> anyhow::Result<Vec<String>> {
        dependency_errors(&toml::from_str(source)?, &toml::from_str(workspace)?)
    }

    #[test]
    fn renamed_internal_dependency_cannot_bypass_layering() {
        let errors = check(
            r#"
                [package]
                name = "tenta-core"
                [dependencies]
                innocent = { package = "tenta-browser", path = "../tenta-browser" }
                serde = "1"
            "#,
            "",
        )
        .unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("dependency on tenta-browser violates"));
    }

    #[test]
    fn workspace_alias_resolves_the_native_package() {
        let workspace = r#"
            [dependencies]
            rquickjs = { package = "tenta-quickjs-core", path = "crates/tenta-quickjs-core" }
        "#;
        let dependencies = r#"
            [dependencies]
            rquickjs = { workspace = true, optional = true }
        "#;
        assert!(
            check(
                &format!("[package]\nname = \"tenta-idl\"\n{dependencies}"),
                workspace,
            )
            .unwrap()
            .is_empty()
        );
        let errors = check(
            &format!("[package]\nname = \"tenta-core\"\n{dependencies}"),
            workspace,
        )
        .unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("dependency on tenta-quickjs-core violates"));
        assert!(
            check(
                r#"
                [package]
                name = "tenta-quickjs-core"
                [dependencies]
                rquickjs-sys = { package = "tenta-quickjs-sys", path = "../tenta-quickjs-sys" }
            "#,
                "",
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn aliases_preserve_dependency_kind_and_cross_target_checks() {
        let errors = check(
            r#"
                [package]
                name = "tenta-net"
                [dependencies]
                fixtures = { package = "tenta-testutil", path = "../tenta-testutil" }
                [build-dependencies]
                generate = { package = "tenta-idl", path = "../tenta-idl" }
                misplaced = { package = "tenta-core", path = "../tenta-core" }
                [target.'cfg(windows)'.dependencies]
                hidden = { package = "tenta-browser", path = "../tenta-browser" }
                [target.'cfg(windows)'.dev-dependencies]
                fixtures = { package = "tenta-testutil", path = "../tenta-testutil" }
            "#,
            "",
        )
        .unwrap();
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("tenta-testutil may only be a dev-dependency"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("build-dependency on tenta-core"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("dependency on tenta-browser violates"))
        );
    }

    #[test]
    fn missing_workspace_alias_is_an_error() {
        let error = check(
            r#"
                [package]
                name = "tenta-idl"
                [dependencies]
                rquickjs = { workspace = true }
            "#,
            "",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("inherited dependency rquickjs is missing")
        );
    }

    #[test]
    fn native_quickjs_routes_are_limited_to_their_consumers() {
        for from in [
            "tenta-js-v8",
            "tenta-layout",
            "tenta-browser",
            "tenta-quickjs-sys",
        ] {
            let errors = check(
                &format!(
                    "[package]\nname = {from:?}\n[dependencies]\nengine = {{ package = \"tenta-quickjs-core\", path = \"../tenta-quickjs-core\" }}"
                ),
                "",
            )
            .unwrap();
            assert_eq!(errors.len(), 1, "{from} must not consume QuickJS directly");
        }
        let errors = check(
            r#"
                [package]
                name = "tenta-quickjs-core"
                [dependencies]
                dom = { package = "tenta-dom", path = "../tenta-dom" }
            "#,
            "",
        )
        .unwrap();
        assert_eq!(
            errors.len(),
            1,
            "the native fork must not depend on the DOM"
        );
        let errors = check(
            r#"
                [package]
                name = "tenta-quickjs-sys"
                [build-dependencies]
                generate = { package = "tenta-idl", path = "../tenta-idl" }
                [dev-dependencies]
                fixtures = { package = "tenta-testutil", path = "../tenta-testutil" }
            "#,
            "",
        )
        .unwrap();
        assert_eq!(
            errors.len(),
            2,
            "dependency kinds cannot bypass fork isolation"
        );
        for from in ["tenta-js", "tenta-idl"] {
            let errors = check(
                &format!(
                    "[package]\nname = {from:?}\n[dependencies]\nengine = {{ package = \"tenta-quickjs-core\", path = \"../tenta-quickjs-core\" }}"
                ),
                "",
            )
            .unwrap();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("must be optional"));
        }
    }
}
