//! XML serialisation round-trips through the XML parser
//! (`docs/spec/04-dom-and-css.md` §3.4, §5).
//!
//! `to_xml` writes namespace declarations from a scope stack rather than from
//! the prefixes the source happened to use, so the prefixes themselves may
//! differ. What must survive is the information: every element's namespace URI
//! and local name, every attribute's namespace URI, local name and value, and
//! the character data. The canonical form below records exactly that, and the
//! property is that parsing the serialisation again yields the same canonical
//! form and the same bytes.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, NodeKind};
use tenta_html::xml::{Parser as XmlParser, XmlOptions};

/// The fixtures. Each is well-formed namespace-aware XML.
const FIXTURES: &[(&str, &str)] = &[
    ("plain", r#"<root a="1" b="2">text</root>"#),
    ("nested", r#"<a><b><c/></b><b>t</b></a>"#),
    (
        "default namespace",
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect/></svg>"#,
    ),
    (
        "default namespace undeclared on a child",
        r#"<a xmlns="urn:one"><b xmlns=""><c/></b></a>"#,
    ),
    (
        "prefixed namespace",
        r#"<x:root xmlns:x="urn:one"><x:child>t</x:child></x:root>"#,
    ),
    (
        "prefix redefined on a descendant",
        r#"<x:a xmlns:x="urn:one"><x:b xmlns:x="urn:two"><x:c/></x:b><x:d/></x:a>"#,
    ),
    (
        "sibling scopes reuse a prefix for different URIs",
        r#"<r xmlns:p="urn:zero"><p:a xmlns:p="urn:one"/><p:b xmlns:p="urn:two"/><p:c/></r>"#,
    ),
    (
        "namespaced attributes",
        r#"<r xmlns:x="urn:one" xmlns:y="urn:two" x:a="1" y:b="2" c="3"><s x:a="4"/></r>"#,
    ),
    (
        "mixed default and prefixed",
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><svg xmlns="http://www.w3.org/2000/svg"><g><foreignObject><p xmlns="http://www.w3.org/1999/xhtml">t</p></foreignObject></g></svg></body></html>"#,
    ),
    (
        "xml prefix",
        r#"<r xml:lang="pl" xml:space="preserve">t</r>"#,
    ),
    (
        "escapes",
        r#"<r a="&lt;&amp;&quot;">a &lt; b &amp; c &gt; d</r>"#,
    ),
    ("cdata", r#"<r><![CDATA[raw < & data]]>tail</r>"#),
    (
        "processing instruction and comment",
        r#"<r><?target data?><!-- note --><c/></r>"#,
    ),
    (
        "empty and whitespace-only elements",
        "<r><a/><b> </b><c></c></r>",
    ),
    (
        "camelCase names",
        r#"<svg xmlns="http://www.w3.org/2000/svg"><clipPath id="cp"><rect/></clipPath><linearGradient/></svg>"#,
    ),
    ("raw CR in text", "<r>a\rb</r>"),
];

fn parse_xml(source: &str) -> Document {
    let mut parser = XmlParser::new(Document::new(), XmlOptions::default());
    parser.feed_str(source);
    parser.finish().expect("well-formed XML")
}

/// A description of the subtree that ignores prefixes but keeps every
/// namespace URI, name, value and character.
fn canonical(document: &Document, node: NodeId, out: &mut String) {
    match document.kind(node) {
        NodeKind::Element => {
            let name = document.element_name(node).expect("element name");
            out.push('<');
            out.push_str(name.ns.as_str());
            out.push('|');
            out.push_str(name.local.as_str());
            // Namespace declarations are attributes in the DOM, but the
            // serialiser derives them from the scope stack rather than
            // copying them, so they are not part of the information that has
            // to survive a round-trip.
            let mut attrs: Vec<String> = document
                .attributes(node)
                .filter(|(name, _)| name.ns != atoms::NS_XMLNS && name.local != atoms::XMLNS)
                .map(|(name, value)| {
                    format!(" {}|{}={value:?}", name.ns.as_str(), name.local.as_str())
                })
                .collect();
            attrs.sort();
            for attr in attrs {
                out.push_str(&attr);
            }
            out.push('>');
            for child in document.children(node) {
                canonical(document, child, out);
            }
            out.push_str("</>");
        }
        NodeKind::Text => {
            out.push_str("#text(");
            out.push_str(document.text_data(node).unwrap_or(""));
            out.push(')');
        }
        NodeKind::CData => {
            // A CDATA section is a serialisation detail of its characters, so
            // it is canonically the same as the text it contains.
            out.push_str("#text(");
            out.push_str(document.text_data(node).unwrap_or(""));
            out.push(')');
        }
        NodeKind::Comment => {
            out.push_str("#comment(");
            out.push_str(document.comment_data(node).unwrap_or(""));
            out.push(')');
        }
        NodeKind::ProcessingInstruction => {
            let (target, data) = document.processing_instruction(node).expect("PI");
            out.push_str(&format!("#pi({target},{data})"));
        }
        _ => {
            for child in document.children(node) {
                canonical(document, child, out);
            }
        }
    }
}

fn canonical_document(document: &Document) -> String {
    let mut out = String::new();
    canonical(document, document.document_node(), &mut out);
    out
}

#[test]
fn serialising_and_reparsing_preserves_the_tree() {
    for (name, source) in FIXTURES {
        let first = parse_xml(source);
        let root = first.document_element().expect("a document element");
        let serialised = first.to_xml(root);

        let second = parse_xml(&serialised);
        let second_root = second.document_element().unwrap_or_else(|| {
            panic!("{name}: the serialisation {serialised:?} did not parse back to a tree")
        });
        assert_eq!(
            canonical_document(&second),
            canonical_document(&first),
            "{name}: {source}\n  serialised as {serialised}"
        );

        // Serialisation is a fixed point after one pass: the second output is
        // byte-identical to the first.
        assert_eq!(
            second.to_xml(second_root),
            serialised,
            "{name}: serialisation is not stable"
        );
    }
}

#[test]
fn serialising_a_child_declares_the_namespaces_it_inherited() {
    // A subtree serialised on its own is the root of its output, so every
    // namespace it inherited has to be declared again or the round-trip loses
    // it.
    for (name, source) in FIXTURES {
        let document = parse_xml(source);
        for node in document.descendants(document.document_node()) {
            if !document.is_element(node) {
                continue;
            }
            let fragment = document.to_xml(node);
            let reparsed = parse_xml(&fragment);
            let reparsed_root = reparsed.document_element().unwrap_or_else(|| {
                panic!("{name}: fragment {fragment:?} did not parse back to a tree")
            });

            let mut expected = String::new();
            canonical(&document, node, &mut expected);
            let mut actual = String::new();
            canonical(&reparsed, reparsed_root, &mut actual);
            assert_eq!(actual, expected, "{name}: fragment {fragment}");
        }
    }
}
