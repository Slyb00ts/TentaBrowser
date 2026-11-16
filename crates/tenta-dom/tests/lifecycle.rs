//! The custom element lifecycle log (`tenta_dom::LifecycleRecord`).

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_dom::{Document, LifecycleRecord, Mutation};
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};

fn parse(html: &str) -> Document {
    let document = Document::with_url("https://example.com/".parse().expect("the test URL"));
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn attribute(local: &str) -> QualName {
    QualName::new(atoms::EMPTY, Atom::from(local))
}

#[test]
fn a_document_records_nothing_until_the_log_is_enabled() {
    let mut doc = parse("<!doctype html><body><x-card id=a></x-card>");
    let card = doc.get_element_by_id("a").expect("the card");
    doc.mutate(Mutation::SetAttribute {
        element: card,
        name: attribute("tone"),
        value: "warm".to_owned(),
    })
    .expect("set an attribute");
    doc.mutate(Mutation::Remove { child: card })
        .expect("remove the card");
    assert!(!doc.has_lifecycle_records());
    assert!(doc.take_lifecycle_log().is_empty());
}

#[test]
fn creation_connection_attributes_and_disconnection_are_recorded_in_order() {
    let mut doc = parse("<!doctype html><body><div id=host></div>");
    doc.enable_lifecycle_log();
    let host = doc.get_element_by_id("host").expect("the host");

    let card = doc.create_html_element(Atom::from("x-card"));
    let plain = doc.create_html_element(Atom::from("section"));
    let card_ref = doc.node_ref(card);
    let plain_ref = doc.node_ref(plain);

    // Detached: an attribute change is recorded, an insertion is not.
    doc.mutate(Mutation::SetAttribute {
        element: card,
        name: attribute("tone"),
        value: "warm".to_owned(),
    })
    .expect("set an attribute");
    doc.mutate(Mutation::Append {
        parent: plain,
        child: card,
    })
    .expect("append to a detached parent");
    // Connecting the parent connects the subtree; one record names its root.
    doc.mutate(Mutation::Append {
        parent: host,
        child: plain,
    })
    .expect("connect the subtree");
    doc.mutate(Mutation::SetAttribute {
        element: card,
        name: attribute("tone"),
        value: "cool".to_owned(),
    })
    .expect("change the attribute");
    // An element without a hyphen in its name has no attribute records.
    doc.mutate(Mutation::SetAttribute {
        element: plain,
        name: attribute("title"),
        value: "t".to_owned(),
    })
    .expect("set an attribute on a plain element");
    doc.mutate(Mutation::RemoveAttribute {
        element: card,
        name: attribute("tone"),
    })
    .expect("remove the attribute");
    doc.mutate(Mutation::Remove { child: plain })
        .expect("disconnect the subtree");

    assert!(doc.has_lifecycle_records());
    let tone = Atom::from("tone");
    assert_eq!(
        doc.take_lifecycle_log(),
        vec![
            LifecycleRecord::Created(card_ref),
            LifecycleRecord::Attribute {
                element: card_ref,
                name: tone,
                namespace: atoms::EMPTY,
                old_value: None,
            },
            LifecycleRecord::Connected(plain_ref),
            LifecycleRecord::Attribute {
                element: card_ref,
                name: tone,
                namespace: atoms::EMPTY,
                old_value: Some("warm".to_owned()),
            },
            LifecycleRecord::Attribute {
                element: card_ref,
                name: tone,
                namespace: atoms::EMPTY,
                old_value: Some("cool".to_owned()),
            },
            LifecycleRecord::Disconnected(plain_ref),
        ]
    );
    assert!(!doc.has_lifecycle_records());
}
