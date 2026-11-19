//! Which script elements a document reports as ready to be prepared
//! (`Document::take_ready_scripts`): the ones HTML runs although no parser
//! inserted them.

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};
use tenta_dom::{Document, Mutation};
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};

fn scripted(html: &str) -> Document {
    let mut document = Document::with_url("https://example.com/".parse().expect("the test URL"));
    document.set_scripting_enabled(true);
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn append(document: &mut Document, parent: NodeId, child: NodeId) {
    document
        .mutate(Mutation::Append { parent, child })
        .expect("append");
}

fn ready(document: &mut Document) -> Vec<NodeId> {
    document
        .take_ready_scripts()
        .into_iter()
        .map(|script| script.id)
        .collect()
}

fn body(document: &Document) -> NodeId {
    document.get_element_by_id("body").expect("the body")
}

fn script(document: &mut Document) -> NodeId {
    document.create_html_element(atoms::SCRIPT)
}

fn set_src(document: &mut Document, element: NodeId, value: &str) {
    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, Atom::from("src")),
            value: value.to_owned(),
        })
        .expect("set src");
}

const PAGE: &str = "<!doctype html><html><head><script>parsed()</script></head>\
                    <body id=body><div id=host></div><script src=late.js></script></body></html>";

#[test]
fn a_parser_s_scripts_are_not_reported() {
    let mut document = scripted(PAGE);
    assert!(!document.has_ready_scripts());
    assert_eq!(ready(&mut document), []);
    // Nor are the scripts of markup parsed into an element.
    let host = document.get_element_by_id("host").unwrap();
    document
        .set_inner_html(
            host,
            "<script>inner()</script><b><script src=x.js></script></b>",
        )
        .unwrap();
    assert_eq!(ready(&mut document), []);
}

#[test]
fn a_script_is_reported_when_it_becomes_connected() {
    let mut document = scripted(PAGE);
    let body = body(&document);
    let first = script(&mut document);
    let text = document.create_text("one()");
    append(&mut document, first, text);
    // Not while it is outside the document, however it is built.
    assert_eq!(ready(&mut document), []);
    append(&mut document, body, first);
    assert!(document.has_ready_scripts());
    assert_eq!(ready(&mut document), [first]);
    assert!(!document.has_ready_scripts());

    // A subtree that is connected reports every script in it, in tree order.
    let wrapper = document.create_html_element(atoms::DIV);
    let second = script(&mut document);
    let inner = document.create_html_element(atoms::SPAN);
    let third = script(&mut document);
    append(&mut document, wrapper, second);
    append(&mut document, wrapper, inner);
    append(&mut document, inner, third);
    assert_eq!(ready(&mut document), []);
    append(&mut document, body, wrapper);
    assert_eq!(ready(&mut document), [second, third]);

    // Moving a script that has not started reports it again; one that has
    // started is never reported.
    append(&mut document, body, second);
    assert_eq!(ready(&mut document), [second]);
    document.set_script_started(third, true);
    append(&mut document, body, third);
    assert_eq!(ready(&mut document), []);
}

#[test]
fn a_connected_script_is_reported_when_it_gets_content_or_a_source() {
    let mut document = scripted(PAGE);
    let body = body(&document);
    let empty = script(&mut document);
    append(&mut document, body, empty);
    assert_eq!(ready(&mut document), [empty]);
    let text = document.create_text("later()");
    append(&mut document, empty, text);
    assert_eq!(ready(&mut document), [empty]);

    let external = script(&mut document);
    append(&mut document, body, external);
    assert_eq!(ready(&mut document), [external]);
    set_src(&mut document, external, "a.js");
    assert_eq!(ready(&mut document), [external]);
    // Changing a source it already has is not getting one.
    set_src(&mut document, external, "b.js");
    assert_eq!(ready(&mut document), []);

    // Neither matters while the element is outside the document.
    let detached = script(&mut document);
    set_src(&mut document, detached, "c.js");
    let more = document.create_text("x()");
    append(&mut document, detached, more);
    assert_eq!(ready(&mut document), []);
}

#[test]
fn a_document_that_runs_no_script_reports_nothing() {
    let mut document = Document::with_url("https://example.com/".parse().unwrap());
    // What a page does for a context without JavaScript, and what a document
    // made by `DOMParser` is.
    document.set_scripting_enabled(false);
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(PAGE.as_bytes()), ParseStatus::Continue);
    document = parser.finish();
    let body = body(&document);
    let element = script(&mut document);
    append(&mut document, body, element);
    assert_eq!(ready(&mut document), []);
}

#[test]
fn a_clone_has_started_when_its_original_has() {
    let mut document = scripted(PAGE);
    let body = body(&document);
    let original = script(&mut document);
    append(&mut document, body, original);
    document.set_script_started(original, true);
    ready(&mut document);

    let copy = document.clone_node(original, true).unwrap();
    assert!(document.script_started(copy));
    append(&mut document, body, copy);
    assert_eq!(ready(&mut document), []);

    let fresh = script(&mut document);
    let fresh_copy = document.clone_node(fresh, false).unwrap();
    assert!(!document.script_started(fresh_copy));
}

#[test]
fn the_scripts_of_a_contextual_fragment_are_reported_when_it_is_inserted() {
    let mut document = scripted(PAGE);
    let body = body(&document);
    let fragment = document
        .create_contextual_fragment(
            body,
            "<p>text</p><script>one()</script><i><script src=two.js></script></i>",
        )
        .unwrap();
    assert_eq!(ready(&mut document), []);
    let scripts: Vec<NodeId> = document
        .descendants(fragment)
        .filter(|node| document.tag(*node) == Some(atoms::SCRIPT))
        .collect();
    append(&mut document, body, fragment);
    assert_eq!(ready(&mut document), scripts);
    assert_eq!(scripts.len(), 2);
}
