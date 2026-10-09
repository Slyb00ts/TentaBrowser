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
fn head_skips_an_earlier_non_html_head_child() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let document_node = document.document_node();
    append(&mut document, document_node, html);

    let svg_head = document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::HEAD),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("SVG head-shaped element");
    append(&mut document, html, svg_head);

    let head = document.create_html_element(atoms::HEAD);
    append(&mut document, html, head);

    assert_eq!(document.head(), Some(head));
}

#[test]
fn head_requires_an_html_document_root() {
    let mut document = Document::new();
    let svg = document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::SVG),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("SVG root");
    let document_node = document.document_node();
    append(&mut document, document_node, svg);

    let head = document.create_html_element(atoms::HEAD);
    append(&mut document, svg, head);

    assert_eq!(document.head(), None);
}

#[test]
fn head_requires_the_root_to_be_in_the_html_namespace() {
    let mut document = Document::new();
    let html = document
        .create_element(
            QualName::new(atoms::NS_SVG, atoms::HTML),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("SVG-namespace html-shaped root");
    let document_node = document.document_node();
    append(&mut document, document_node, html);

    let head = document.create_html_element(atoms::HEAD);
    append(&mut document, html, head);

    assert_eq!(document.head(), None);
}
