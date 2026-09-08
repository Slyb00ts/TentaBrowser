//! Link extraction (`docs/spec/04-dom-and-css.md` §5): absolute URLs resolved
//! against the document base, deduplicated, with `rel` and anchor text.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::extract::text::collapsed_text;

/// One outgoing link.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct Link {
    /// Absolute URL, resolved against `<base href>` or the document URL.
    pub url: String,
    /// Collapsed anchor text.
    pub text: String,
    /// The `rel` attribute, if present.
    pub rel: Option<String>,
    /// The `title` attribute, if present.
    pub title: Option<String>,
    /// The element the link came from.
    #[serde(skip)]
    pub node: NodeId,
}

/// Every `<a href>` and `<area href>` in the subtree, in tree order,
/// deduplicated by resolved URL. The first occurrence wins, except that a
/// later occurrence with text fills in an empty anchor text.
/// Links with `javascript:` URLs are omitted.
pub fn links(document: &Document, root: NodeId) -> Vec<Link> {
    let mut found: Vec<Link> = Vec::new();
    let mut seen: hashbrown::HashMap<String, usize> = hashbrown::HashMap::new();
    for node in document.descendants_inclusive(root) {
        let Some(tag) = document.tag(node) else {
            continue;
        };
        if tag != atoms::A && tag != atoms::AREA {
            continue;
        }
        let Some(href) = document.attribute(node, atoms::HREF) else {
            continue;
        };

        let Some(url) = document.resolve_url(href.trim()) else {
            continue;
        };
        // Skip javascript: URLs. The check is on the *resolved* scheme, not a
        // raw prefix: the WHATWG URL parser strips interior ASCII tab/CR/LF and
        // leading C0 controls, so `java\tscript:` and `\u{1}javascript:` parse to
        // the `javascript` scheme and a raw `starts_with` check misses them.
        if url.scheme() == "javascript" {
            continue;
        }
        let url = url.to_string();
        let text = if tag == atoms::A {
            collapsed_text(document, node)
        } else {
            document
                .attribute(node, atoms::ALT)
                .unwrap_or("")
                .to_owned()
        };
        if let Some(&index) = seen.get(&url) {
            if found[index].text.is_empty() && !text.is_empty() {
                found[index].text = text;
            }
            continue;
        }
        seen.insert(url.clone(), found.len());
        found.push(Link {
            url,
            text,
            rel: document.attribute(node, atoms::REL).map(str::to_owned),
            title: document.attribute(node, atoms::TITLE).map(str::to_owned),
            node,
        });
    }
    found
}

impl Document {
    /// Every link in the document.
    pub fn links(&self) -> Vec<Link> {
        links(self, self.document_node())
    }
}
