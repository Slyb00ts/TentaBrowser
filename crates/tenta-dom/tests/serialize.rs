//! HTML fragment and XML serialisation (`docs/spec/04-dom-and-css.md` §3.5).

mod support;

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::{Attribute, ElementFlags, QualName};

fn body_of(document: &Document) -> tenta_core::NodeId {
    document.body().expect("the fixture has a body")
}

#[test]
fn round_trip_of_a_representative_fragment() {
    let source = "<p class=\"a\">Text <em>and</em> more</p><ul><li>one</li><li>two</li></ul>\
                  <img src=\"x.png\" alt=\"x\"><br><hr>";
    let document = support::parse(&format!("<html><body>{source}</body></html>"));
    let body = body_of(&document);
    let serialised = document.inner_html(body);
    assert_eq!(serialised, source);

    // Serialising the result again must be a fixed point.
    let again = support::parse(&format!("<html><body>{serialised}</body></html>"));
    assert_eq!(again.inner_html(body_of(&again)), serialised);
}

#[test]
fn void_elements_have_no_end_tag_and_raw_text_is_not_escaped() {
    let document = support::parse(
        "<html><body><input name=\"q\"><style>a > b { content: \"&\"; }</style>\
         <script>if (a < b && c) { x(); }</script></body></html>",
    );
    let html = document.inner_html(body_of(&document));
    assert!(html.contains("<input name=\"q\">"));
    assert!(!html.contains("</input>"));
    assert!(html.contains("a > b { content: \"&\"; }"), "got {html}");
    assert!(html.contains("if (a < b && c) { x(); }"), "got {html}");
}

#[test]
fn text_and_attribute_values_are_escaped() {
    let mut document = Document::new();
    let element = document
        .create_element(
            QualName::new(atoms::NS_HTML, atoms::DIV),
            vec![Attribute {
                name: QualName::new(atoms::EMPTY, Atom::new("title")),
                value: "a \"b\" & c\u{00A0}d".to_owned(),
            }],
            ElementFlags::default(),
        )
        .expect("create HTML element");
    let text = document.create_text("< & > \u{00A0} plain");
    document
        .mutate(Mutation::Append {
            parent: element,
            child: text,
        })
        .expect("append");
    assert_eq!(
        document.outer_html(element),
        "<div title=\"a &quot;b&quot; &amp; c&nbsp;d\">&lt; &amp; &gt; &nbsp; plain</div>"
    );
}

#[test]
fn template_contents_serialise_as_the_template_children() {
    let document =
        support::parse("<html><body><template id=\"t\"><p>inside</p></template></body></html>");
    let template = document.get_element_by_id("t").expect("the template");
    assert_eq!(document.inner_html(template), "<p>inside</p>");
    assert_eq!(
        document.outer_html(template),
        "<template id=\"t\"><p>inside</p></template>"
    );
    assert_eq!(
        document.children(template).count(),
        0,
        "template children live in the contents fragment, not in the element"
    );
}

#[test]
fn comments_and_doctype_serialise() {
    let document = support::parse("<!doctype html><html><body><!-- note --><p>x</p></body></html>");
    let html = document.inner_html(document.document_node());
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<!-- note -->"));
}

#[test]
fn xml_serialisation_declares_namespaces_and_self_closes() {
    let mut document = Document::new();
    document.set_content_type("application/xml");
    let svg = document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::SVG),
            vec![Attribute {
                name: QualName::new(atoms::EMPTY, Atom::new("viewBox")),
                value: "0 0 1 1".to_owned(),
            }],
            ElementFlags::default(),
        )
        .expect("create SVG element");
    let rect = document
        .create_element(
            QualName::new(atoms::NS_SVG, Atom::new("rect")),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("create SVG element");
    document
        .mutate(Mutation::Append {
            parent: svg,
            child: rect,
        })
        .expect("append");
    assert_eq!(
        document.to_xml(svg),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><rect/></svg>"
    );

    let text = document.create_text("a < b & c");
    document
        .mutate(Mutation::Append {
            parent: rect,
            child: text,
        })
        .expect("append");
    // Serialising the child on its own makes it the root of the output, so its
    // namespace has to be declared again or the round-trip would lose it.
    assert_eq!(
        document.to_xml(rect),
        "<rect xmlns=\"http://www.w3.org/2000/svg\">a &lt; b &amp; c</rect>"
    );
}

#[test]
fn cdata_and_processing_instructions_round_trip_in_xml() {
    let mut document = Document::new();
    let root = document
        .create_element(
            QualName::new(atoms::EMPTY, Atom::new("root")),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("create XML element");
    let cdata = document.create_cdata("raw < data");
    let pi = document.create_processing_instruction("target", "value");
    for child in [cdata, pi] {
        document
            .mutate(Mutation::Append {
                parent: root,
                child,
            })
            .expect("append");
    }
    assert_eq!(
        document.to_xml(root),
        "<root><![CDATA[raw < data]]><?target value?></root>"
    );
}

#[test]
fn noscript_content_round_trips_with_stored_scripting_flag() {
    // Parse with scripting enabled (the default): <noscript> content is
    // treated as raw text and not parsed as HTML.
    let source = "<noscript><p>&amp;</p></noscript>";
    let document = support::parse(&format!("<html><body>{source}</body></html>"));
    let body = body_of(&document);

    // Serialise: the stored scripting_enabled flag should match what was used
    // when parsing, so the serialiser preserves <noscript> as raw text.
    let serialised = document.inner_html(body);
    assert_eq!(serialised, source);

    // Round-trip: re-parsing the serialised output should produce the same tree.
    let again = support::parse(&format!("<html><body>{serialised}</body></html>"));
    assert_eq!(again.inner_html(body_of(&again)), serialised);
}
