//! Scripts that script inserts.
//!
//! A parser runs the `<script>` elements it creates. Every other script
//! element runs when HTML's "prepare the script element" says so: when the
//! element becomes connected, or, already connected, is given a child or a
//! `src` (<https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element>).
//! The document records those moments ([`Document::take_ready_scripts`]);
//! [`prepare`] is the decision taken at each of them.
//!
//! What follows the decision is split by who can do it. An inline classic
//! script runs at once, inside the DOM call that made it ready, which only
//! the engine can do. Everything else needs the network or the module map, so
//! the engine hands it to the page as an [`InsertedScript`]
//! ([`crate::engine::PageEngine::take_inserted_scripts`]).

use tenta_core::atom::atoms;
use tenta_core::provenance::Origin;
use tenta_core::{Atom, NodeRef};
use tenta_dom::{Document, NodeKind};
use url::Url;

use crate::modules::is_javascript_mime_type;

/// What a `<script>` element's `type` makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptType {
    Classic,
    Module,
}

/// The type of a script element from its `type` and `language` attributes, or
/// `None` for an element that is not a script to run: a data block, or an
/// import map, which only a parser's script may be here.
pub fn script_type(kind: Option<&str>, language: Option<&str>) -> Option<ScriptType> {
    let essence = match (kind, language) {
        // An empty `type`, or none with an empty or absent `language`.
        (Some(""), _) | (None, None) | (None, Some("")) => return Some(ScriptType::Classic),
        (Some(kind), _) => kind.trim().to_ascii_lowercase(),
        (None, Some(language)) => format!("text/{}", language.to_ascii_lowercase()),
    };
    if essence == "module" {
        Some(ScriptType::Module)
    } else if !essence.contains(';') && is_javascript_mime_type(&essence) {
        Some(ScriptType::Classic)
    } else {
        None
    }
}

/// Where the text of an inserted script comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertedSource {
    /// The element's own text. Only a module script is handed over this way;
    /// an inline classic script has already run.
    Inline(String),
    /// The element's `src`, resolved against the document's base URL.
    External(Url),
    /// The element has a `src` that is empty or does not resolve: nothing is
    /// fetched, and an `error` event is owed to the element.
    Unusable,
}

/// A script element the page has to load and run for the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertedScript {
    /// The element: `document.currentScript` while a classic script runs, and
    /// the target of the `load` or `error` event afterwards.
    pub element: NodeRef,
    pub kind: ScriptType,
    pub source: InsertedSource,
    /// The script runs in the order it was inserted relative to the other
    /// ordered scripts of the document, instead of as soon as it has loaded:
    /// a script whose `async` IDL attribute was set to false.
    pub ordered: bool,
    /// The provenance of the task that inserted the element, which its fetch
    /// and its own run inherit: a script a background timer inserts long
    /// after the page loaded does not hold a settle open.
    pub origin: Origin,
}

/// What preparing a script element came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    /// The element does not run, now or because of this change.
    Skip,
    /// An inline classic script: run `source` now, with `element` as the
    /// current script.
    RunNow { element: NodeRef, source: String },
    /// The page loads and runs it.
    Request(InsertedScript),
}

/// HTML's "prepare the script element" for an element no parser inserted.
///
/// An element that is going to run is marked as started here, so that
/// nothing that happens while it runs or loads can run it a second time.
pub fn prepare(doc: &mut Document, script: NodeRef, modules: bool, origin: Origin) -> Prepared {
    let Ok(node) = doc.resolve(script) else {
        return Prepared::Skip;
    };
    if doc.tag(node) != Some(atoms::SCRIPT) || doc.script_started(node) {
        return Prepared::Skip;
    }
    let src = doc.attribute(node, atoms::SRC).map(str::to_owned);
    // The child text content: the element's own Text children, not its
    // descendants'.
    let source: String = doc
        .children(node)
        .filter(|child| matches!(doc.kind(*child), NodeKind::Text | NodeKind::CData))
        .filter_map(|child| doc.text_data(child))
        .collect();
    if src.is_none() && source.is_empty() {
        return Prepared::Skip;
    }
    if !doc.is_composed_connected(node) {
        return Prepared::Skip;
    }
    let Some(kind) = script_type(
        doc.attribute(node, atoms::TYPE),
        doc.attribute(node, Atom::from("language")),
    ) else {
        return Prepared::Skip;
    };
    if kind == ScriptType::Module && !modules {
        return Prepared::Skip;
    }
    doc.set_script_started(node, true);
    // `nomodule` keeps a classic script from running where module scripts do.
    if kind == ScriptType::Classic && modules && doc.has_attribute(node, atoms::NOMODULE) {
        return Prepared::Skip;
    }
    let ordered = !doc.has_attribute(node, atoms::ASYNC) && !doc.script_force_async(node);
    let Some(src) = src else {
        return match kind {
            ScriptType::Classic => Prepared::RunNow {
                element: script,
                source,
            },
            ScriptType::Module => Prepared::Request(InsertedScript {
                element: script,
                kind,
                source: InsertedSource::Inline(source),
                ordered,
                origin,
            }),
        };
    };
    let base = doc.base_url().or(doc.url()).cloned();
    let resolved = (!src.is_empty())
        .then(|| match &base {
            Some(base) => base.join(&src).ok(),
            None => Url::parse(&src).ok(),
        })
        .flatten();
    Prepared::Request(InsertedScript {
        element: script,
        kind,
        source: match resolved {
            Some(url) => InsertedSource::External(url),
            None => InsertedSource::Unusable,
        },
        ordered,
        origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_type_of_a_script_element() {
        for (kind, language, expected) in [
            (None, None, Some(ScriptType::Classic)),
            (Some(""), None, Some(ScriptType::Classic)),
            (Some(""), Some("vbscript"), Some(ScriptType::Classic)),
            (None, Some(""), Some(ScriptType::Classic)),
            (None, Some("JavaScript"), Some(ScriptType::Classic)),
            (None, Some("vbscript"), None),
            (Some("text/javascript"), None, Some(ScriptType::Classic)),
            (
                Some(" Application/JavaScript "),
                None,
                Some(ScriptType::Classic),
            ),
            (Some("text/javascript; charset=utf-8"), None, None),
            (Some("module"), None, Some(ScriptType::Module)),
            (Some(" MODULE "), None, Some(ScriptType::Module)),
            (Some("importmap"), None, None),
            (Some("application/json"), None, None),
            (Some("text/template"), None, None),
            (Some("text/x-handlebars"), Some("javascript"), None),
        ] {
            assert_eq!(
                script_type(kind, language),
                expected,
                "{kind:?} {language:?}"
            );
        }
    }
}
