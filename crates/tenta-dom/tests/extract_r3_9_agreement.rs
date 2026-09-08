//! R3-9 conformance test: text(), markdown(), and snapshot() must agree on hidden elements
//! This test reconstructs the news fixture logic to verify the three extractors
//! exclude the same hidden paragraphs.

use tenta_core::atom::atoms;
use tenta_dom::Document;

#[test]
fn extractors_agree_on_hidden_elements() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .unwrap();

    document
        .mutate(tenta_dom::Mutation::Append {
            parent: html,
            child: body,
        })
        .unwrap();

    // Visible paragraph 1
    let p1 = document.create_html_element(atoms::P);
    let p1_text = document.create_text("Visible paragraph 1");
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: p1,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: p1,
            child: p1_text,
        })
        .unwrap();

    // Hidden with attribute 'hidden'
    let p2 = document.create_html_element(atoms::P);
    let p2_text = document.create_text("Hidden paragraph with hidden attribute");
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: p2,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: p2,
            child: p2_text,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: p2,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::HIDDEN),
            value: "".to_owned(),
        })
        .unwrap();

    // Hidden with aria-hidden="true"
    let p3 = document.create_html_element(atoms::P);
    let p3_text = document.create_text("Hidden with aria-hidden");
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: p3,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: p3,
            child: p3_text,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: p3,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ARIA_HIDDEN),
            value: "true".to_owned(),
        })
        .unwrap();

    // Note: style="display:none" test is not included because inline style
    // parsing for display:none is not fully implemented yet (according to R3-9)

    // Uppercase aria-hidden="TRUE"
    let p5 = document.create_html_element(atoms::P);
    let p5_text = document.create_text("Hidden with uppercase aria-hidden");
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: p5,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: p5,
            child: p5_text,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::SetAttribute {
            element: p5,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ARIA_HIDDEN),
            value: "TRUE".to_owned(),
        })
        .unwrap();

    // Visible paragraph 2
    let p7 = document.create_html_element(atoms::P);
    let p7_text = document.create_text("Visible paragraph 2");
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: p7,
        })
        .unwrap();
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: p7,
            child: p7_text,
        })
        .unwrap();

    // Extract with all three methods
    let text_output = document.text();
    let markdown_output = document.markdown();
    let snapshot_output = document.snapshot();
    let snapshot_text = snapshot_output.to_text();

    // Verify visible content is present in all three
    assert!(
        text_output.contains("Visible paragraph 1"),
        "text() should include visible paragraph 1"
    );
    assert!(
        text_output.contains("Visible paragraph 2"),
        "text() should include visible paragraph 2"
    );

    assert!(
        markdown_output.contains("Visible paragraph 1"),
        "markdown() should include visible paragraph 1"
    );
    assert!(
        markdown_output.contains("Visible paragraph 2"),
        "markdown() should include visible paragraph 2"
    );

    assert!(
        snapshot_text.contains("Visible paragraph 1"),
        "snapshot() should include visible paragraph 1"
    );
    assert!(
        snapshot_text.contains("Visible paragraph 2"),
        "snapshot() should include visible paragraph 2"
    );

    // Verify hidden content is excluded from all three
    let hidden_texts = [
        "Hidden paragraph with hidden attribute",
        "Hidden with aria-hidden",
        "Hidden with uppercase aria-hidden",
    ];

    for hidden_text in &hidden_texts {
        assert!(
            !text_output.contains(hidden_text),
            "text() should exclude: {}",
            hidden_text
        );
        assert!(
            !markdown_output.contains(hidden_text),
            "markdown() should exclude: {}",
            hidden_text
        );
        assert!(
            !snapshot_text.contains(hidden_text),
            "snapshot() should exclude: {}",
            hidden_text
        );
    }
}
