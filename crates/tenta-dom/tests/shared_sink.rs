//! Parsing into a document that script also holds
//! (`docs/spec/05-javascript.md` §6).
//!
//! These tests stand in for the page runtime: they pause the parse where a
//! `<script>` closes, reach into the very document the parser is writing to,
//! and resume. What they pin is the contract the runtime depends on — the
//! tree a script sees is exactly the markup before its own end tag, and the
//! parse continues into that same document afterwards.

use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::mutation::Mutation;
use tenta_dom::{SharedDocument, SharedSink};
use tenta_html::{ParseStatus, Parser, ParserOptions};

fn scripting_parser(sink: SharedSink) -> Parser<'static, SharedSink> {
    Parser::new(
        sink,
        ParserOptions {
            pause_on_script: true,
            ..ParserOptions::default()
        },
    )
}

/// The document's serialized body, read through the shared handle.
fn body_html(document: &SharedDocument) -> String {
    let document = document.borrow();
    let body = document.body().expect("a parsed document has a body");
    document.inner_html(body)
}

#[test]
fn a_paused_parse_shows_script_the_markup_up_to_its_own_end_tag() {
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = scripting_parser(sink);

    let status = parser.feed_str("<body><p>before</p><script>var x = 1;</script><p>after</p>");
    assert_eq!(status, ParseStatus::Script);

    // This is the moment the script would run. It sees everything before its
    // own end tag, and nothing after it.
    let paused = body_html(&document);
    assert!(paused.contains("<p>before</p>"), "got: {paused}");
    assert!(paused.contains("<script>"), "got: {paused}");
    assert!(
        !paused.contains("after"),
        "markup after the script must not be in the tree yet: {paused}"
    );
}

#[test]
fn the_paused_script_element_is_the_one_in_the_shared_document() {
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = scripting_parser(sink);
    assert_eq!(
        parser.feed_str("<script>var x = 1;</script>"),
        ParseStatus::Script
    );

    let script = parser.take_pending_script().expect("the paused script");
    let text = document.borrow().inner_html(script);
    assert_eq!(
        text, "var x = 1;",
        "the handle must address the script the parser stopped on"
    );
}

#[test]
fn the_parse_continues_into_the_same_document_after_the_script() {
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = scripting_parser(sink);
    assert_eq!(
        parser.feed_str("<body><p>before</p><script>var x = 1;</script><p>after</p>"),
        ParseStatus::Script
    );

    parser.take_pending_script();
    assert_eq!(parser.resume(), ParseStatus::Continue);
    let finished = parser.finish();

    assert!(
        Rc::ptr_eq(&finished, &document),
        "the parse must end in the document script was given, not a copy"
    );
    let html = body_html(&document);
    assert!(html.contains("<p>before</p>"), "got: {html}");
    assert!(html.contains("<p>after</p>"), "got: {html}");
}

#[test]
fn a_script_that_mutates_the_document_keeps_the_parse_consistent() {
    // The whole point of sharing: what the script writes stays, and the rest
    // of the document is built around it.
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = scripting_parser(sink);
    assert_eq!(
        parser.feed_str("<body><p>before</p><script>s</script><p>after</p>"),
        ParseStatus::Script
    );

    {
        // Stand-in for `document.body.appendChild(document.createElement('b'))`.
        let mut document = document.borrow_mut();
        let body = document.body().expect("body");
        let injected = document.create_html_element(atoms::B);
        document
            .mutate(Mutation::Append {
                parent: body,
                child: injected,
            })
            .expect("append");
    }

    parser.take_pending_script();
    while parser.resume() == ParseStatus::Script {
        parser.take_pending_script();
    }
    parser.finish();

    let html = body_html(&document);
    assert!(
        html.contains("<b></b>") && html.contains("<p>after</p>"),
        "the script's node and the markup after it must both survive: {html}"
    );
    let injected = html.find("<b></b>").expect("the appended node");
    let parsed_after = html
        .find("<p>after</p>")
        .expect("the markup after the script");
    assert!(
        injected < parsed_after,
        "the parse must continue after what the script appended: {html}"
    );
}

/// Markup chosen to reach the awkward corners of the sink, not just append:
/// a doctype (quirks mode), a comment, foster parenting out of a table
/// (`append_before_sibling` / `append_based_on_parent_node`), the adoption
/// agency (`remove_from_parent` / `reparent_children`), a repeated `<body>`
/// start tag (`add_attrs_if_missing`), a `<template>`
/// (`get_template_contents`), a processing instruction and a parse error.
const AWKWARD: &str = "<!DOCTYPE html><html><body class=one><!-- c -->\
     <table><tr><td>cell</td></tr>stray</table>\
     <b><p>adopted</b>tail\
     <body class=two id=x>\
     <template><i>t</i></template>\
     <p>end</p></body>";

#[test]
fn a_document_with_no_script_parses_exactly_as_an_owned_one() {
    // Delegation fidelity: whatever `impl TreeSink for Document` does, the
    // shared sink must do. Serializing the whole document (not just the body)
    // and comparing the quirks mode and the parse errors is what makes this
    // test able to fail if a single delegation is wrong.
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = scripting_parser(sink);
    assert_eq!(parser.feed_str(AWKWARD), ParseStatus::Continue);
    parser.finish();

    let mut owned = Parser::new(tenta_dom::Document::new(), ParserOptions::default());
    assert_eq!(owned.feed_str(AWKWARD), ParseStatus::Continue);
    let owned = owned.finish();

    let shared = document.borrow();
    assert_eq!(
        shared.outer_html(shared.document_element().expect("root")),
        owned.outer_html(owned.document_element().expect("root"))
    );
    assert_eq!(shared.quirks_mode(), owned.quirks_mode());
    assert_eq!(shared.parse_errors(), owned.parse_errors());
    assert_eq!(shared.parse_line(), owned.parse_line());
    assert!(
        !owned.parse_errors().is_empty(),
        "the fixture is meant to exercise the error path too"
    );
}

#[test]
fn a_stop_predicate_still_ends_a_shared_parse() {
    // `TreeSink::pop` calls a predicate that the document itself owns, from
    // inside a borrow of that same document. It must not re-enter the shared
    // handle, and it must still be able to stop the parse.
    let (sink, document) = SharedSink::with_new_document();
    document
        .borrow_mut()
        .set_stop_after(Some(Box::new(|document: &tenta_dom::Document, node| {
            document.element_name(node).map(|name| name.local) == Some(atoms::P)
        })));

    let mut parser = scripting_parser(sink);
    assert_eq!(
        parser.feed_str("<p>first</p><p>second</p>"),
        ParseStatus::Stopped
    );
    parser.finish();

    let document = document.borrow();
    assert!(
        document.parse_stopped(),
        "the predicate must have stopped it"
    );
    let body = document.body().expect("body");
    assert_eq!(document.inner_html(body), "<p>first</p>");
}
