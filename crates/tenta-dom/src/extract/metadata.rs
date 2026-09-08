//! Document metadata (`docs/spec/04-dom-and-css.md` §5): title, `<meta>`
//! names and properties including OpenGraph and Twitter cards, the canonical
//! URL, JSON-LD blocks and basic microdata.

use std::collections::BTreeMap;

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::extract::text::collapsed_text;

/// One microdata item (`itemscope`).
#[derive(Clone, PartialEq, Eq, Debug, Default, serde::Serialize)]
pub struct MicrodataItem {
    /// The `itemtype` attribute.
    pub item_type: Option<String>,
    /// The `itemid` attribute.
    pub item_id: Option<String>,
    /// `itemprop` name to values, in document order.
    pub properties: BTreeMap<String, Vec<String>>,
}

/// Everything the metadata extractor found.
#[derive(Clone, PartialEq, Eq, Debug, Default, serde::Serialize)]
pub struct Metadata {
    pub title: Option<String>,
    /// The `lang` attribute of the document element.
    pub lang: Option<String>,
    /// `<meta name=...>` and `<meta http-equiv=...>` values.
    pub meta: BTreeMap<String, String>,
    /// `og:*` properties without the prefix.
    pub open_graph: BTreeMap<String, String>,
    /// `twitter:*` properties without the prefix.
    pub twitter: BTreeMap<String, String>,
    /// `<link rel=canonical href>`, resolved.
    pub canonical: Option<String>,
    /// Raw `application/ld+json` blocks, parsed. Blocks that do not parse are
    /// dropped rather than failing the extraction.
    pub json_ld: Vec<serde_json::Value>,
    /// Top-level microdata items.
    pub microdata: Vec<MicrodataItem>,
}

impl Metadata {
    /// The description, preferring OpenGraph over `<meta name=description>`.
    pub fn description(&self) -> Option<&str> {
        self.open_graph
            .get("description")
            .or_else(|| self.meta.get("description"))
            .map(String::as_str)
    }
}

/// Extracts metadata from the whole document.
pub fn metadata(document: &Document) -> Metadata {
    let mut found = Metadata::default();
    if let Some(root) = document.document_element() {
        found.lang = document.attribute(root, atoms::LANG).map(str::to_owned);
    }
    for node in document.descendants(document.document_node()) {
        let Some(tag) = document.tag(node) else {
            continue;
        };
        if tag == atoms::TITLE {
            if found.title.is_none() {
                found.title = Some(collapsed_text(document, node));
            }
        } else if tag == atoms::META {
            let content = document.attribute(node, atoms::CONTENT).unwrap_or("");
            let key = document
                .attribute(node, atoms::NAME)
                .or_else(|| document.attribute(node, atoms::PROPERTY))
                .or_else(|| document.attribute(node, atoms::HTTP_EQUIV));
            let Some(key) = key else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            if let Some(rest) = key.strip_prefix("og:") {
                found.open_graph.insert(rest.to_owned(), content.to_owned());
            } else if let Some(rest) = key.strip_prefix("twitter:") {
                found.twitter.insert(rest.to_owned(), content.to_owned());
            } else {
                found.meta.insert(key, content.to_owned());
            }
        } else if tag == atoms::LINK {
            let is_canonical = document.attribute(node, atoms::REL).is_some_and(|rel| {
                rel.split_ascii_whitespace()
                    .any(|r| r == atoms::CANONICAL.as_str())
            });
            if is_canonical
                && found.canonical.is_none()
                && let Some(href) = document.attribute(node, atoms::HREF)
                && let Some(url) = document.resolve_url(href.trim())
            {
                found.canonical = Some(url.to_string());
            }
        } else if tag == atoms::SCRIPT {
            let is_ld_json = document.attribute(node, atoms::TYPE).is_some_and(|t| {
                t.trim()
                    .eq_ignore_ascii_case(atoms::APPLICATION_LD_JSON.as_str())
            });
            if is_ld_json {
                let raw = document.text_content(node);
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
                    found.json_ld.push(value);
                }
            }
        }
        if document.has_attribute(node, atoms::ITEMSCOPE) && !has_microdata_ancestor(document, node)
        {
            found.microdata.push(microdata_item(document, node));
        }
    }
    found
}

fn has_microdata_ancestor(document: &Document, node: NodeId) -> bool {
    document
        .ancestors(node)
        .any(|ancestor| document.has_attribute(ancestor, atoms::ITEMSCOPE))
}

fn microdata_item(document: &Document, scope: NodeId) -> MicrodataItem {
    let mut item = MicrodataItem {
        item_type: document
            .attribute(scope, atoms::ITEMTYPE)
            .map(str::to_owned),
        item_id: document.attribute(scope, atoms::ITEMID).map(str::to_owned),
        properties: BTreeMap::new(),
    };
    for node in document.descendants(scope) {
        // Properties of a nested item belong to that item.
        if document
            .ancestors(node)
            .take_while(|a| *a != scope)
            .any(|a| document.has_attribute(a, atoms::ITEMSCOPE))
        {
            continue;
        }
        let Some(prop) = document.attribute(node, atoms::ITEMPROP) else {
            continue;
        };
        let value = microdata_value(document, node);
        for name in prop.split_ascii_whitespace() {
            item.properties
                .entry(name.to_owned())
                .or_default()
                .push(value.clone());
        }
    }
    item
}

fn microdata_value(document: &Document, node: NodeId) -> String {
    let tag = document.tag(node);
    let resolved = |attribute| {
        document
            .attribute(node, attribute)
            .and_then(|raw| document.resolve_url(raw.trim()))
            .map(|url| url.to_string())
    };
    match tag {
        Some(t) if t == atoms::META => document
            .attribute(node, atoms::CONTENT)
            .unwrap_or("")
            .to_owned(),
        Some(t) if t == atoms::A || t == atoms::LINK || t == atoms::AREA => {
            resolved(atoms::HREF).unwrap_or_default()
        }
        Some(t)
            if t == atoms::IMG
                || t == atoms::AUDIO
                || t == atoms::VIDEO
                || t == atoms::SOURCE
                || t == atoms::IFRAME
                || t == atoms::EMBED
                || t == atoms::TRACK =>
        {
            resolved(atoms::SRC).unwrap_or_default()
        }
        Some(t) if t == atoms::TIME => document
            .attribute(node, atoms::DATETIME)
            .map(str::to_owned)
            .unwrap_or_else(|| collapsed_text(document, node)),
        Some(t) if t == atoms::DATA || t == atoms::METER => document
            .attribute(node, atoms::VALUE)
            .map(str::to_owned)
            .unwrap_or_else(|| collapsed_text(document, node)),
        _ => collapsed_text(document, node),
    }
}

impl Document {
    /// Metadata of the whole document.
    pub fn metadata(&self) -> Metadata {
        metadata(self)
    }
}
