use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::{ElementFlags, QualName};

fn append(document: &mut Document, parent: NodeId, child: NodeId) {
    document
        .mutate(Mutation::Append { parent, child })
        .expect("append element");
}

#[test]
fn body_skips_an_earlier_non_html_body_child() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let document_node = document.document_node();
    append(&mut document, document_node, html);
    let svg_body = document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::BODY),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("SVG body-shaped element");
    append(&mut document, html, svg_body);
    assert_eq!(document.body(), None);

    let body = document.create_html_element(atoms::BODY);
    append(&mut document, html, body);
    assert_eq!(document.body(), Some(body));
}

#[test]
fn body_requires_an_html_html_root_but_not_an_html_document_type() {
    let mut svg_document = Document::new();
    let svg = svg_document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::SVG),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("SVG root");
    let document_node = svg_document.document_node();
    append(&mut svg_document, document_node, svg);
    let child = svg_document.create_html_element(atoms::BODY);
    append(&mut svg_document, svg, child);
    assert_eq!(svg_document.body(), None);

    let mut xhtml_document = Document::new();
    xhtml_document.set_content_type("application/xhtml+xml");
    let html = xhtml_document.create_html_element(atoms::HTML);
    let document_node = xhtml_document.document_node();
    append(&mut xhtml_document, document_node, html);
    let frameset = xhtml_document.create_html_element(atoms::FRAMESET);
    append(&mut xhtml_document, html, frameset);
    assert_eq!(xhtml_document.body(), Some(frameset));
}
