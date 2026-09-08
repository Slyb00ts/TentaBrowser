//! Test-only helpers.
//!
//! [`parse`] and [`parse_with_url`] run the conformant parser
//! (`tenta_html::Parser`) straight into the arena `Document`, which is the
//! configuration the engine ships; the golden extraction, selector and
//! serialisation tests therefore assert on trees the product actually builds.
//!
//! [`mini_html`] holds a toy tokenizer kept only for the sink tests, which
//! need to drive `TreeSink` directly.

#![allow(dead_code)]

pub mod mini_html;

use tenta_core::atom::atoms;
use tenta_dom::Document;
use tenta_html::{Attribute, ElementFlags, ParseStatus, Parser, ParserOptions, QualName};

/// Parses a full document from a string through the conformant parser.
pub fn parse(html: &str) -> Document {
    parse_document(Document::new(), html)
}

/// Parses a document with a known URL, so relative links resolve.
pub fn parse_with_url(html: &str, url: &str) -> Document {
    parse_document(
        Document::with_url(url.parse().expect("valid test URL")),
        html,
    )
}

fn parse_document(document: Document, html: &str) -> Document {
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(
        parser.feed(html.as_bytes()),
        ParseStatus::Continue,
        "the test parser is not expected to stop early"
    );
    parser.finish()
}

/// Builds a synthetic tree of `elements` `<div>`s under a `<body>`, each with
/// an id, a class and a text child, for the query tests.
pub fn synthetic_document(elements: usize) -> Document {
    synthetic(elements, true)
}

/// The same tree without the text children, so per-element accounting is not
/// diluted by text nodes.
pub fn synthetic_elements_only(elements: usize) -> Document {
    synthetic(elements, false)
}

/// The same shape, two attributes per element, but with the attribute values
/// a page normally uses: shared, not unique per element. Every element having
/// its own id is a worst case for the id index and the value interner, so the
/// budget is reported against both shapes.
pub fn synthetic_shared_values(elements: usize) -> Document {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    for _ in 0..elements {
        let div = document
            .create_element(
                QualName::new(atoms::NS_HTML, atoms::DIV),
                vec![
                    Attribute {
                        name: QualName::new(atoms::EMPTY, atoms::CLASS),
                        value: "row".to_owned(),
                    },
                    Attribute {
                        name: QualName::new(atoms::EMPTY, atoms::TITLE),
                        value: "a cell".to_owned(),
                    },
                ],
                ElementFlags::default(),
            )
            .expect("create HTML element");
        document
            .mutate(tenta_dom::Mutation::Append {
                parent: body,
                child: div,
            })
            .expect("append div");
    }
    document
}

fn synthetic(elements: usize, with_text: bool) -> Document {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(tenta_dom::Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    for index in 0..elements {
        let div = document
            .create_element(
                QualName::new(atoms::NS_HTML, atoms::DIV),
                vec![
                    Attribute {
                        name: QualName::new(atoms::EMPTY, atoms::ID),
                        value: format!("n{index}"),
                    },
                    Attribute {
                        name: QualName::new(atoms::EMPTY, atoms::CLASS),
                        value: if index == elements / 2 {
                            "row x".to_owned()
                        } else {
                            "row".to_owned()
                        },
                    },
                ],
                ElementFlags::default(),
            )
            .expect("create HTML element");
        document
            .mutate(tenta_dom::Mutation::Append {
                parent: body,
                child: div,
            })
            .expect("append div");
        if with_text {
            let text = document.create_text("cell text for the synthetic document");
            document
                .mutate(tenta_dom::Mutation::Append {
                    parent: div,
                    child: text,
                })
                .expect("append text");
        }
    }
    document
}
