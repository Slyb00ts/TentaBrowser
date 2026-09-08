//! HTML fragment serialisation and an XML serialiser
//! (`docs/spec/04-dom-and-css.md` §3.5).
//!
//! Both are iterative: an explicit stack of open/close steps, so a tree built
//! a million nodes deep through the DOM API serialises without touching the
//! native stack (`docs/spec/02-architecture.md` §3). Past
//! [`crate::MAX_TRAVERSAL_DEPTH`] the output is truncated and carries
//! [`crate::TRUNCATION_MARKER`].

use std::collections::HashMap;
use std::fmt::Write as _;

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};

use crate::arena::NodeKind;
use crate::document::Document;
use crate::{MAX_TRAVERSAL_DEPTH, TRUNCATION_MARKER};

/// Elements serialised without an end tag.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Elements whose text children are written literally.
const RAW_TEXT_PARENTS: &[&str] = &[
    "style",
    "script",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "plaintext",
];

/// Void and raw-text status is a property of the *HTML* element with that
/// name. An SVG `<style>` holds escapable character data and an SVG element
/// named `img` would have an end tag, so both checks consult the namespace
/// rather than the local name alone.
fn is_void(ns: Option<Atom>, tag: Atom) -> bool {
    ns == Some(atoms::NS_HTML) && VOID_ELEMENTS.contains(&tag.as_str())
}

fn is_raw_text_parent(ns: Option<Atom>, tag: Atom) -> bool {
    ns == Some(atoms::NS_HTML) && RAW_TEXT_PARENTS.contains(&tag.as_str())
}

/// Options for the HTML serialiser.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SerializeOptions {
    /// Whether the scripting flag is set. It only changes `<noscript>`:
    /// with scripting enabled its text is written literally, as the HTML
    /// fragment serialisation algorithm requires.
    pub scripting_enabled: bool,
}

fn escape_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '\u{00A0}' => out.push_str("&nbsp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn escape_attribute(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '\u{00A0}' => out.push_str("&nbsp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

/// The attribute's serialised name, per the fragment serialisation algorithm.
fn write_attribute_name(out: &mut String, ns: Atom, prefix: Option<Atom>, local: Atom) {
    let ns = ns.as_str();
    match ns {
        "" => out.push_str(local.as_str()),
        "http://www.w3.org/XML/1998/namespace" => {
            out.push_str("xml:");
            out.push_str(local.as_str());
        }
        "http://www.w3.org/1999/xlink" => {
            out.push_str("xlink:");
            out.push_str(local.as_str());
        }
        "http://www.w3.org/2000/xmlns/" => {
            if local.as_str() == "xmlns" {
                out.push_str("xmlns");
            } else {
                out.push_str("xmlns:");
                out.push_str(local.as_str());
            }
        }
        _ => {
            if let Some(prefix) = prefix {
                out.push_str(prefix.as_str());
                out.push(':');
            }
            out.push_str(local.as_str());
        }
    }
}

enum Step {
    Open(NodeId, usize),
    Close(Atom),
}

/// Pushes `node`'s children as `Open` steps in reverse, without a temporary
/// vector: the tail of the stack is filled forwards and then reversed.
fn push_children(document: &Document, stack: &mut Vec<Step>, node: NodeId, depth: usize) {
    let start = stack.len();
    let mut child = document.first_child(node);
    while child.is_some() {
        stack.push(Step::Open(child, depth));
        child = document.next_sibling(child);
    }
    stack[start..].reverse();
}

fn serialize_children(
    document: &Document,
    root: NodeId,
    options: SerializeOptions,
    out: &mut String,
) {
    let mut stack: Vec<Step> = Vec::new();
    push_children(document, &mut stack, root, 1);
    let mut truncated = false;
    let cap = document.slots().saturating_mul(2) + 16;
    let mut visited = 0usize;
    while let Some(step) = stack.pop() {
        visited += 1;
        if visited > cap {
            truncated = true;
            break;
        }
        match step {
            Step::Close(tag) => {
                out.push_str("</");
                out.push_str(tag.as_str());
                out.push('>');
            }
            Step::Open(node, depth) => {
                if depth > MAX_TRAVERSAL_DEPTH {
                    truncated = true;
                    continue;
                }
                match document.kind(node) {
                    NodeKind::Element => {
                        let tag = document.tag(node).unwrap_or(tenta_core::atom::atoms::EMPTY);
                        if !write_start_tag(document, node, tag, out) {
                            continue;
                        }
                        stack.push(Step::Close(tag));
                        // A <template>'s children are its contents fragment.
                        let container = document.template_contents(node).unwrap_or(node);
                        push_children(document, &mut stack, container, depth + 1);
                    }
                    NodeKind::Text | NodeKind::CData => {
                        let text = document.text_data(node).unwrap_or_default();
                        let parent = document.parent(node);
                        let parent_ns = document.namespace(parent);
                        let literal = document.tag(parent).is_some_and(|tag| {
                            is_raw_text_parent(parent_ns, tag)
                                || (options.scripting_enabled
                                    && parent_ns == Some(atoms::NS_HTML)
                                    && tag == atoms::NOSCRIPT)
                        });
                        if literal {
                            out.push_str(text);
                        } else {
                            escape_text(out, text);
                        }
                    }
                    NodeKind::Comment => {
                        out.push_str("<!--");
                        out.push_str(document.comment_data(node).unwrap_or_default());
                        out.push_str("-->");
                    }
                    NodeKind::ProcessingInstruction => {
                        if let Some((target, data)) = document.processing_instruction(node) {
                            let _ = write!(out, "<?{target} {data}>");
                        }
                    }
                    NodeKind::DocType => {
                        if let Some(doctype) = document.doctype(node) {
                            let _ = write!(out, "<!DOCTYPE {}>", doctype.name);
                        }
                    }
                    NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot => {
                        push_children(document, &mut stack, node, depth + 1);
                    }
                }
            }
        }
    }
    if truncated {
        out.push_str(TRUNCATION_MARKER);
    }
}

/// The children of `node`, serialised as an HTML fragment. For a
/// `<template>` these are the children of its contents fragment.
pub fn inner_html(document: &Document, node: NodeId, options: SerializeOptions) -> String {
    let mut out = String::new();
    let container = document.template_contents(node).unwrap_or(node);
    serialize_children(document, container, options, &mut out);
    out
}

/// `node` and its children, serialised as an HTML fragment.
pub fn outer_html(document: &Document, node: NodeId, options: SerializeOptions) -> String {
    if document.kind(node) != NodeKind::Element {
        let mut out = String::new();
        // Non-elements have no start tag: serialise them as a one-node
        // fragment by borrowing the child path above.
        serialize_one(document, node, options, &mut out);
        return out;
    }
    let tag = document.tag(node).unwrap_or(tenta_core::atom::atoms::EMPTY);
    let mut out = String::new();
    if !write_start_tag(document, node, tag, &mut out) {
        return out;
    }
    let container = document.template_contents(node).unwrap_or(node);
    serialize_children(document, container, options, &mut out);
    out.push_str("</");
    out.push_str(tag.as_str());
    out.push('>');
    out
}

/// Writes the start tag of `node` and returns whether it has children to
/// serialise (a void element has none). A `<pre>`, `<textarea>` or
/// `<listing>` whose text starts with a newline gets one more, since the
/// parser drops the first newline after the start tag (HTML serialisation
/// §13.3).
fn write_start_tag(document: &Document, node: NodeId, tag: Atom, out: &mut String) -> bool {
    out.push('<');
    out.push_str(tag.as_str());
    for (name, value) in document.attributes(node) {
        out.push(' ');
        write_attribute_name(out, name.ns, name.prefix, name.local);
        out.push_str("=\"");
        escape_attribute(out, value);
        out.push('"');
    }
    out.push('>');
    if is_void(document.namespace(node), tag) {
        return false;
    }
    if matches!(tag.as_str(), "pre" | "textarea" | "listing") {
        let container = document.template_contents(node).unwrap_or(node);
        let first_child = document.first_child(container);
        if first_child.is_some()
            && document.kind(first_child) == NodeKind::Text
            && document
                .text_data(first_child)
                .is_some_and(|text| text.starts_with('\n'))
        {
            out.push('\n');
        }
    }
    true
}

fn serialize_one(document: &Document, node: NodeId, options: SerializeOptions, out: &mut String) {
    match document.kind(node) {
        NodeKind::Text | NodeKind::CData => {
            escape_text(out, document.text_data(node).unwrap_or_default())
        }
        NodeKind::Comment => {
            out.push_str("<!--");
            out.push_str(document.comment_data(node).unwrap_or_default());
            out.push_str("-->");
        }
        NodeKind::ProcessingInstruction => {
            if let Some((target, data)) = document.processing_instruction(node) {
                let _ = write!(out, "<?{target} {data}>");
            }
        }
        NodeKind::DocType => {
            if let Some(doctype) = document.doctype(node) {
                let _ = write!(out, "<!DOCTYPE {}>", doctype.name);
            }
        }
        _ => serialize_children(document, node, options, out),
    }
}

// ------------------------------------------------------------------- XML

fn escape_xml_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

fn escape_xml_attribute(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

fn write_xml_name(out: &mut String, prefix: Option<Atom>, local: Atom) {
    if let Some(prefix) = prefix
        && !prefix.as_str().is_empty()
    {
        out.push_str(prefix.as_str());
        out.push(':');
    }
    out.push_str(local.as_str());
}

/// The namespace declarations in scope while the XML serialiser walks.
///
/// The serialiser used to write both a synthesised `xmlns` and the retained
/// `xmlns` attribute of the same element, which is a duplicate attribute and
/// makes the output fail to re-parse. Declarations now come from one place:
/// this stack decides whether a binding is already in scope, and a retained
/// `xmlns`/`xmlns:*` attribute is recorded here rather than written twice.
///
/// Per-prefix stacks (like the parser's `Bindings`) give O(1) lookup instead
/// of O(N) linear scan over all bindings.
#[derive(Default)]
struct NsScopes {
    /// Every declaration in declaration order; `order[i]` is the prefix whose
    /// stack the i-th declaration pushed onto.
    order: Vec<Atom>,
    /// Per prefix, the stack of URIs currently in scope. `EMPTY` is the
    /// default namespace.
    scopes: HashMap<Atom, Vec<Atom>>,
    /// Serial for generated prefixes, so a namespaced attribute with no
    /// prefix of its own still round-trips.
    generated: u32,
}

impl NsScopes {
    #[inline]
    fn len(&self) -> usize {
        self.order.len()
    }

    fn lookup(&self, prefix: Atom) -> Option<Atom> {
        self.scopes.get(&prefix)?.last().copied()
    }

    fn is_bound(&self, prefix: Atom, uri: Atom) -> bool {
        // The two reserved prefixes are bound for the life of a document and
        // must never be declared.
        reserved_prefix(prefix) || self.lookup(prefix) == Some(uri)
    }

    fn declare(&mut self, out: &mut String, prefix: Atom, uri: Atom) {
        out.push(' ');
        if prefix == atoms::EMPTY {
            out.push_str("xmlns");
        } else {
            out.push_str("xmlns:");
            out.push_str(prefix.as_str());
        }
        out.push_str("=\"");
        escape_xml_attribute(out, uri.as_str());
        out.push('"');
        self.order.push(prefix);
        self.scopes.entry(prefix).or_default().push(uri);
    }

    fn truncate(&mut self, len: usize) {
        while self.order.len() > len {
            let prefix = self.order.pop().expect("order is longer than len");
            if let Some(stack) = self.scopes.get_mut(&prefix) {
                stack.pop();
            }
        }
    }

    fn next_prefix(&mut self) -> Atom {
        self.generated += 1;
        Atom::new(&format!("ns{}", self.generated))
    }
}

/// A prefix that is always in scope and never declared.
fn reserved_prefix(prefix: Atom) -> bool {
    matches!(prefix.as_str(), "xml" | "xmlns")
}

enum XmlStep {
    Open(NodeId, usize),
    /// Close tag plus the binding-stack length to restore.
    Close(Option<Atom>, Atom, usize),
}

fn push_xml_children(document: &Document, stack: &mut Vec<XmlStep>, node: NodeId, depth: usize) {
    let start = stack.len();
    let mut child = document.first_child(node);
    while child.is_some() {
        stack.push(XmlStep::Open(child, depth));
        child = document.next_sibling(child);
    }
    stack[start..].reverse();
}

/// Serialises `node` and its subtree as XML.
///
/// Namespaces are declared where they enter scope and nowhere else, element
/// prefixes are preserved, and empty elements are written self-closing, so
/// `tenta_html::xml::Parser` reads back the tree it was given.
pub fn serialize_xml(document: &Document, node: NodeId) -> String {
    let mut out = String::new();
    let mut scopes = NsScopes::default();
    let mut stack: Vec<XmlStep> = vec![XmlStep::Open(node, 0)];
    let mut truncated = false;
    let cap = document.slots().saturating_mul(2) + 16;
    let mut visited = 0usize;
    while let Some(step) = stack.pop() {
        visited += 1;
        if visited > cap {
            truncated = true;
            break;
        }
        match step {
            XmlStep::Close(prefix, local, scope) => {
                out.push_str("</");
                write_xml_name(&mut out, prefix, local);
                out.push('>');
                scopes.truncate(scope);
            }
            XmlStep::Open(current, depth) => {
                if depth > MAX_TRAVERSAL_DEPTH {
                    truncated = true;
                    continue;
                }
                match document.kind(current) {
                    NodeKind::Element => {
                        let scope = scopes.len();
                        let local = document.tag(current).unwrap_or(atoms::EMPTY);
                        let prefix = document
                            .element_prefix(current)
                            .filter(|prefix| !reserved_prefix(*prefix));
                        let ns = document.namespace(current).filter(|ns| *ns != atoms::EMPTY);

                        // The element's own name is written first, then the
                        // declarations it needs, then its ordinary attributes.
                        let mut head = String::new();
                        head.push('<');
                        write_xml_name(&mut head, prefix, local);

                        let mut body = String::new();
                        // Retained declarations first: they are the source of
                        // truth for what the document meant, and recording
                        // them here stops the element declaring the same
                        // binding twice.
                        for (name, value) in document.attributes(current) {
                            let declared = declared_prefix(name.ns, name.local);
                            let Some(declared) = declared else { continue };
                            let uri = Atom::new(value);
                            if scopes.lookup(declared) == Some(uri) {
                                continue;
                            }
                            scopes.declare(&mut body, declared, uri);
                        }
                        // Then anything the names still need.
                        let element_prefix = prefix.unwrap_or(atoms::EMPTY);
                        match ns {
                            Some(uri) if !scopes.is_bound(element_prefix, uri) => {
                                scopes.declare(&mut body, element_prefix, uri)
                            }
                            None if prefix.is_none()
                                && scopes
                                    .lookup(atoms::EMPTY)
                                    .is_some_and(|uri| uri != atoms::EMPTY) =>
                            {
                                // Leaving a default namespace has to be said.
                                scopes.declare(&mut body, atoms::EMPTY, atoms::EMPTY)
                            }
                            _ => {}
                        }

                        let mut attributes = String::new();
                        for (name, value) in document.attributes(current) {
                            if declared_prefix(name.ns, name.local).is_some() {
                                continue;
                            }
                            let attr_prefix = attribute_prefix(&mut scopes, &mut body, &name);
                            attributes.push(' ');
                            write_xml_name(&mut attributes, attr_prefix, name.local);
                            attributes.push_str("=\"");
                            escape_xml_attribute(&mut attributes, value);
                            attributes.push('"');
                        }

                        out.push_str(&head);
                        out.push_str(&body);
                        out.push_str(&attributes);
                        if !document.first_child(current).is_some() {
                            out.push_str("/>");
                            scopes.truncate(scope);
                            continue;
                        }
                        out.push('>');
                        stack.push(XmlStep::Close(prefix, local, scope));
                        push_xml_children(document, &mut stack, current, depth + 1);
                    }
                    NodeKind::Text => {
                        escape_xml_text(&mut out, document.text_data(current).unwrap_or_default())
                    }
                    NodeKind::CData => {
                        // Parser-built CDATA never contains `]]>` and comments
                        // never contain `--`; API-created ones may, and there
                        // is no lossless escape inside either construct, so
                        // they are written verbatim like `XMLSerializer` does
                        // in browsers (the output is then not well-formed).
                        out.push_str("<![CDATA[");
                        out.push_str(document.text_data(current).unwrap_or_default());
                        out.push_str("]]>");
                    }
                    NodeKind::Comment => {
                        out.push_str("<!--");
                        out.push_str(document.comment_data(current).unwrap_or_default());
                        out.push_str("-->");
                    }
                    NodeKind::ProcessingInstruction => {
                        if let Some((target, data)) = document.processing_instruction(current) {
                            let _ = write!(out, "<?{target} {data}?>");
                        }
                    }
                    NodeKind::DocType => {
                        if let Some(doctype) = document.doctype(current) {
                            let _ = write!(out, "<!DOCTYPE {}>", doctype.name);
                        }
                    }
                    NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot => {
                        push_xml_children(document, &mut stack, current, depth + 1);
                    }
                }
            }
        }
    }
    if truncated {
        out.push_str(TRUNCATION_MARKER);
    }
    out
}

/// The prefix an `xmlns`/`xmlns:p` attribute declares, if it is one.
fn declared_prefix(ns: Atom, local: Atom) -> Option<Atom> {
    if ns != atoms::NS_XMLNS {
        return None;
    }
    if local.as_str() == "xmlns" {
        Some(atoms::EMPTY)
    } else {
        Some(local)
    }
}

/// The prefix a namespaced attribute is written with, declaring a binding for
/// it if none is in scope. Attributes never take the default namespace, so an
/// unprefixed one in a namespace gets a generated prefix.
fn attribute_prefix(
    scopes: &mut NsScopes,
    declarations: &mut String,
    name: &tenta_html::QualName,
) -> Option<Atom> {
    if name.ns == atoms::EMPTY {
        return None;
    }
    if name.ns == atoms::NS_XML {
        return Some(Atom::new("xml"));
    }
    let wanted = name
        .prefix
        .filter(|prefix| *prefix != atoms::EMPTY && !reserved_prefix(*prefix));
    if let Some(prefix) = wanted {
        if scopes.lookup(prefix) != Some(name.ns) {
            scopes.declare(declarations, prefix, name.ns);
        }
        return Some(prefix);
    }
    // Reuse a prefix already bound to this namespace when there is one.
    for (prefix, uris) in &scopes.scopes {
        if *prefix != atoms::EMPTY && uris.last().is_some_and(|uri| *uri == name.ns) {
            return Some(*prefix);
        }
    }
    let generated = scopes.next_prefix();
    scopes.declare(declarations, generated, name.ns);
    Some(generated)
}

impl Document {
    /// The children of `node` as an HTML fragment.
    pub fn inner_html(&self, node: NodeId) -> String {
        inner_html(
            self,
            node,
            SerializeOptions {
                scripting_enabled: self.scripting_enabled(),
            },
        )
    }

    /// `node` and its children as an HTML fragment.
    pub fn outer_html(&self, node: NodeId) -> String {
        outer_html(
            self,
            node,
            SerializeOptions {
                scripting_enabled: self.scripting_enabled(),
            },
        )
    }

    /// `node` and its subtree as XML.
    pub fn to_xml(&self, node: NodeId) -> String {
        serialize_xml(self, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tenta_core::Atom;

    #[test]
    fn text_escaping_follows_the_fragment_algorithm() {
        let mut out = String::new();
        escape_text(&mut out, "a<b>c&d\u{00A0}e\"f");
        assert_eq!(out, "a&lt;b&gt;c&amp;d&nbsp;e\"f");
    }

    #[test]
    fn attribute_escaping_escapes_angle_brackets() {
        let mut out = String::new();
        escape_attribute(&mut out, "a<b>c&d\u{00A0}e\"f");
        assert_eq!(out, "a&lt;b&gt;c&amp;d&nbsp;e&quot;f");
    }

    #[test]
    fn namespaced_attribute_names_use_their_conventional_prefix() {
        let mut out = String::new();
        write_attribute_name(
            &mut out,
            tenta_core::atom::atoms::NS_XLINK,
            None,
            Atom::new("href"),
        );
        assert_eq!(out, "xlink:href");

        let mut out = String::new();
        write_attribute_name(
            &mut out,
            tenta_core::atom::atoms::NS_XML,
            None,
            Atom::new("lang"),
        );
        assert_eq!(out, "xml:lang");

        let mut out = String::new();
        write_attribute_name(
            &mut out,
            tenta_core::atom::atoms::NS_XMLNS,
            None,
            Atom::new("xmlns"),
        );
        assert_eq!(out, "xmlns");

        let mut out = String::new();
        write_attribute_name(
            &mut out,
            tenta_core::atom::atoms::EMPTY,
            None,
            Atom::new("id"),
        );
        assert_eq!(out, "id");
    }

    #[test]
    fn void_and_raw_text_elements_are_recognised() {
        let html = Some(atoms::NS_HTML);
        let svg = Some(atoms::NS_SVG);
        assert!(is_void(html, Atom::new("br")));
        assert!(is_void(html, Atom::new("input")));
        assert!(!is_void(html, Atom::new("div")));
        assert!(
            !is_void(svg, Atom::new("img")),
            "void-ness is a property of the HTML element, not of the name"
        );
        assert!(is_raw_text_parent(html, Atom::new("script")));
        assert!(is_raw_text_parent(html, Atom::new("style")));
        assert!(
            !is_raw_text_parent(svg, Atom::new("style")),
            "an SVG <style> holds escapable character data"
        );
        assert!(
            !is_raw_text_parent(html, Atom::new("noscript")),
            "noscript is raw text only when scripting is enabled"
        );
    }

    #[test]
    fn xml_escaping_covers_attribute_whitespace() {
        let mut out = String::new();
        escape_xml_attribute(&mut out, "a\tb\nc\"d&e<f");
        assert_eq!(out, "a&#9;b&#10;c&quot;d&amp;e&lt;f");
    }
}
