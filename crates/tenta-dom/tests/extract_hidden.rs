//! Tests for R3-9: hidden element detection across text(), markdown(), and snapshot()

use tenta_core::atom::atoms;
use tenta_dom::Document;

#[test]
fn hidden_attribute_excluded_from_all_extractors() {
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);
    let text_node = document.create_text("Hidden content");

    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: div,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: div,
            child: text_node,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: div,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::HIDDEN),
            value: "".to_owned(),
        })
        .unwrap();

    let text = document.text();
    let markdown = document.markdown();
    let snapshot = document.snapshot();

    // Hidden content should not appear in text extraction
    assert!(
        !text.contains("Hidden content"),
        "text() should exclude hidden attribute"
    );

    // Hidden content should not appear in markdown extraction
    assert!(
        !markdown.contains("Hidden content"),
        "markdown() should exclude hidden attribute"
    );

    // Snapshot should not emit text from hidden elements
    let snapshot_text = snapshot.to_text();
    assert!(
        !snapshot_text.contains("Hidden content"),
        "snapshot() should exclude hidden attribute"
    );
}

#[test]
fn aria_hidden_true_excluded_case_insensitively() {
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);
    let text_node = document.create_text("Hidden with aria");

    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: div,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: div,
            child: text_node,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: div,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ARIA_HIDDEN),
            value: "TRUE".to_owned(), // uppercase to test case-insensitivity
        })
        .unwrap();

    let text = document.text();
    let markdown = document.markdown();

    // aria-hidden="TRUE" should be treated as true (case-insensitive)
    assert!(
        !text.contains("Hidden with aria"),
        "text() should exclude aria-hidden=TRUE"
    );
    assert!(
        !markdown.contains("Hidden with aria"),
        "markdown() should exclude aria-hidden=TRUE"
    );
}

#[test]
fn javascript_urls_filtered_from_links() {
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let link1 = document.create_html_element(atoms::A);
    let link2 = document.create_html_element(atoms::A);

    // Append body to document first
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: document.document_node(),
            child: body,
        })
        .unwrap();

    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: link1,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: link2,
        })
        .unwrap();

    // javascript: URL (should be filtered)
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: link1,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::HREF),
            value: "javascript:alert(1)".to_owned(),
        })
        .unwrap();

    // valid absolute URL (should be included)
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: link2,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::HREF),
            value: "https://example.com/page".to_owned(),
        })
        .unwrap();

    let links = document.links();

    // Should have 1 link (not the javascript: one)
    assert_eq!(links.len(), 1, "javascript: URLs should be filtered out");

    // Check that https is present
    let has_https = links.iter().any(|l| l.url.contains("example.com"));
    assert!(has_https, "https: URLs should be included");
}
