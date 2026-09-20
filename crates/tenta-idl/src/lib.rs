//! WebIDL parser and binding code generator used at build time by feature
//! crates (`docs/spec/02-architecture.md` §2.1, `docs/spec/05-javascript.md`
//! §2–3, `docs/dev/idl.md`).
//!
//! The crate has two halves. [`parse_webidl`] turns WebIDL source into a
//! resolved [`Model`]: typedefs substituted, partials and mixins merged,
//! overloads grouped, unions named, every member classified for the V8
//! fast-API path. [`codegen`] turns that model into Rust source: a `*Ops`
//! trait per interface, V8 glue, QuickJS glue, a registry entry point, one
//! shared type module and one prelude. Every generator is a pure function of
//! the model.
//!
//! ```no_run
//! # fn main() -> Result<(), tenta_idl::Error> {
//! let model = tenta_idl::parse_webidl("interface Foo { readonly attribute long bar; };")?;
//! for file in tenta_idl::generate(&model) {
//!     println!("{}: {} bytes", file.name, file.contents.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! `tenta-idl` depends on no other crate of the workspace, which
//! `cargo xtask lint-deps` enforces.

#![deny(missing_docs)]

pub mod codegen;
mod front;
pub mod model;
pub mod naming;

use std::path::{Path, PathBuf};

pub use front::{parse_webidl, parse_webidl_units};
pub use model::{Interface, Model, Namespace, Type};

/// The result type of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while reading or lowering WebIDL.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The source is not valid WebIDL.
    #[error("WebIDL syntax error: {0}")]
    Syntax(String),
    /// The source parses but does not form a consistent model.
    #[error("WebIDL model error: {0}")]
    Model(String),
    /// A WebIDL construct this generator does not implement yet.
    #[error("unsupported WebIDL construct: {0}")]
    Unsupported(String),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// One generated source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// The file name, e.g. `node_v8.rs`.
    pub name: String,
    /// The file contents.
    pub contents: String,
}

/// Parses every `.webidl` file of a directory as one compilation unit.
///
/// Files are read in sorted order so that the model, and therefore the
/// generated code, does not depend on directory iteration order.
pub fn parse_directory(directory: &Path) -> Result<Model> {
    parse_directories([directory])
}

/// Parses the `.webidl` files of several directories as one compilation unit.
///
/// The directories are visited in the order given and the files within each in
/// sorted order, so the model never depends on directory iteration order. The
/// result is *one* model, not one per directory: [`generate`] emits `prelude.rs`
/// and `types.rs` once per model, so a second directory generated on its own
/// would write a second `prelude.rs` over the first. Naming an interface of one
/// directory from another is part of the same point — `idl/url`'s `URL` declares
/// a `URLSearchParams` attribute, and `idl/dom` and `idl/url` are lowered
/// together.
///
/// Every named directory must hold at least one `.webidl` file, so a directory
/// that was renamed or emptied fails loudly instead of silently contributing
/// nothing.
pub fn parse_directories<'a>(directories: impl IntoIterator<Item = &'a Path>) -> Result<Model> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for directory in directories {
        paths.extend(webidl_paths(directory)?);
    }
    let mut sources = Vec::with_capacity(paths.len());
    for path in &paths {
        sources.push(std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?);
    }
    parse_webidl_units(sources.iter().map(String::as_str))
}

/// The `.webidl` files of `directory`, sorted by path.
fn webidl_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|e| Error::io(directory, e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "webidl"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(Error::Model(format!(
            "{} contains no .webidl files",
            directory.display()
        )));
    }
    Ok(paths)
}

/// Generates every output file for a model.
///
/// The result contains, in this order: `prelude.rs`, `types.rs` and then, per
/// interface and namespace, `<snake>_ops.rs`, `<snake>_v8.rs`,
/// `<snake>_qjs.rs` and `<snake>_register.rs`.
pub fn generate(model: &Model) -> Vec<GeneratedFile> {
    let mut files = vec![
        GeneratedFile {
            name: "prelude.rs".to_string(),
            contents: codegen::prelude(),
        },
        GeneratedFile {
            name: "types.rs".to_string(),
            contents: codegen::types(model),
        },
    ];
    for interface in &model.interfaces {
        let stem = naming::snake_stem(&interface.name);
        files.push(GeneratedFile {
            name: format!("{stem}_ops.rs"),
            contents: codegen::interface_ops(model, interface),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_v8.rs"),
            contents: codegen::interface_v8(model, interface),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_qjs.rs"),
            contents: codegen::interface_quickjs(model, interface),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_register.rs"),
            contents: codegen::interface_register(model, interface),
        });
    }
    for namespace in &model.namespaces {
        let stem = naming::snake_stem(&namespace.name);
        files.push(GeneratedFile {
            name: format!("{stem}_ops.rs"),
            contents: codegen::namespace_ops(model, namespace),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_v8.rs"),
            contents: codegen::namespace_v8(model, namespace),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_qjs.rs"),
            contents: codegen::namespace_quickjs(model, namespace),
        });
        files.push(GeneratedFile {
            name: format!("{stem}_register.rs"),
            contents: codegen::namespace_register(model, namespace),
        });
    }
    files
}

/// Generates every output file for `idl_dir` into `out_dir`.
///
/// This is what a feature crate's `build.rs` calls; see `docs/dev/idl.md`.
pub fn generate_directory(idl_dir: &Path, out_dir: &Path) -> Result<Vec<PathBuf>> {
    generate_directories([idl_dir], out_dir)
}

/// Generates every output file for several IDL directories into `out_dir`.
///
/// The directories are lowered as one unit (see [`parse_directories`]) and
/// generated once, so `prelude.rs` and `types.rs` are written exactly once no
/// matter how many directories contributed. The caller is responsible for
/// emitting one `cargo:rerun-if-changed` per directory.
pub fn generate_directories<'a>(
    directories: impl IntoIterator<Item = &'a Path>,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let model = parse_directories(directories)?;
    std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;
    let mut written = Vec::new();
    for file in generate(&model) {
        let path = out_dir.join(&file.name);
        let contents = format_source(&file.contents);
        std::fs::write(&path, contents).map_err(|e| Error::io(&path, e))?;
        written.push(path);
    }
    Ok(written)
}

/// Runs generated source through `prettyplease` when the `format` feature is
/// on, and returns it unchanged otherwise.
///
/// Formatting only changes whitespace, so golden files record the unformatted
/// output and stay stable whether or not the feature is enabled.
pub fn format_source(source: &str) -> String {
    #[cfg(feature = "format")]
    {
        if let Ok(parsed) = syn::parse_file(source) {
            return prettyplease::unparse(&parsed);
        }
    }
    source.to_string()
}
