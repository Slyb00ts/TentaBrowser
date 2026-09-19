//! Tests for d-2c: URL-valued attribute reflection (`href` on
//! `HTMLAnchorElement`, `src` on `HTMLImageElement` and `HTMLScriptElement`,
//! `action` on `HTMLFormElement`) and the `HTMLScriptElement` interface.
//!
//! The getter follows the HTML Standard's rule for a `USVString` attribute
//! whose content attribute is defined to contain a URL, as the WPT helper
//! states it (`wpt/checkout/html/dom/reflection.js`, the `"url"` reflection
//! case): an absent content attribute yields the empty string, a present one
//! is resolved against the element's node document (so a `<base href>` moves
//! the base), and a value that will not parse is returned verbatim rather than
//! throwing. The setter stores the raw value, which is what `getAttribute`
//! then reports.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};
use tenta_js_v8::Engine;
use url::Url;

fn append(doc: &mut Document, parent: NodeId, child: NodeId) {
    doc.mutate(Mutation::Append { parent, child })
        .expect("append the node");
}

/// `<html><head>[<base href>]</head><body></body></html>` over a document with
/// `url` (or no URL at all). The base is refreshed after insertion, since a
/// hand-built tree has no sink to do it.
fn document(url: Option<&str>, base_href: Option<&str>) -> Rc<RefCell<Document>> {
    let mut doc = match url {
        Some(url) => Document::with_url(Url::parse(url).expect("the test URL parses")),
        None => Document::new(),
    };
    let root = doc.document_node();
    let html = doc.create_html_element(atoms::HTML);
    let head = doc.create_html_element(atoms::HEAD);
    let body = doc.create_html_element(atoms::BODY);
    append(&mut doc, root, html);
    append(&mut doc, html, head);
    append(&mut doc, html, body);
    if let Some(href) = base_href {
        let base = doc.create_html_element(atoms::BASE);
        doc.mutate(Mutation::SetAttribute {
            element: base,
            name: QualName::new(atoms::EMPTY, atoms::HREF),
            value: href.to_owned(),
        })
        .expect("set the base href");
        append(&mut doc, head, base);
        doc.refresh_base_url();
    }
    Rc::new(RefCell::new(doc))
}

/// Parses `html` into a fresh document, the way a page without a URL is built.
fn parse(html: &str) -> Rc<RefCell<Document>> {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    Rc::new(RefCell::new(parser.finish()))
}

fn eval(doc: Rc<RefCell<Document>>, source: &str) -> String {
    let mut engine = Engine::with_document(doc);
    engine
        .evaluate(source)
        .expect("the test script should evaluate without throwing")
}

// ------------------------------------------------------------ URL getters

#[test]
fn a_url_getter_resolves_against_the_document_url() {
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), None),
            r#"
            const a = document.createElement('a');
            a.href = 'foo';
            a.href;
        "#,
        ),
        "https://example.com/dir/foo"
    );
}

#[test]
fn a_base_element_moves_the_resolution_base() {
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), Some("/assets/")),
            r#"
            const a = document.createElement('a');
            a.href = 'foo';
            const img = document.createElement('img');
            img.src = 'icon.png';
            const form = document.createElement('form');
            form.action = 'submit';
            [a.href, img.src, form.action].join('|');
        "#,
        ),
        "https://example.com/assets/foo\
         |https://example.com/assets/icon.png\
         |https://example.com/assets/submit"
    );
}

#[test]
fn an_absolute_url_is_left_alone() {
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), Some("/assets/")),
            r#"
            const a = document.createElement('a');
            a.href = 'https://other.example/x?q=1';
            a.href;
        "#,
        ),
        "https://other.example/x?q=1"
    );
}

#[test]
fn an_absent_url_attribute_is_the_empty_string() {
    // The document URL is deliberately set: a bogus getter returning it (or a
    // resolved `about:blank`) would break `if (img.src)` guards, so the empty
    // string is the assertion, not "some absolute URL".
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), Some("/assets/")),
            r#"
            const a = document.createElement('a');
            const img = document.createElement('img');
            const form = document.createElement('form');
            const script = document.createElement('script');
            [
                a.href === '',
                img.src === '',
                form.action === '',
                script.src === '',
            ].join('|');
        "#,
        ),
        "true|true|true|true"
    );
}

#[test]
fn a_url_that_will_not_parse_is_returned_verbatim() {
    // `http://[` starts an absolute URL with an unterminated IPv6 host, so the
    // parse fails against any base. The getter must echo the content attribute
    // rather than throw or collapse to the empty string.
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), Some("/assets/")),
            r#"
            const img = document.createElement('img');
            img.setAttribute('src', 'http://[');
            img.src;
        "#,
        ),
        "http://["
    );
}

#[test]
fn a_relative_value_without_a_document_url_falls_back_to_the_raw_value() {
    // With no document URL there is no base to resolve against, so the parse
    // fails and the raw value is the result, exactly as for a malformed URL.
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const a = document.createElement('a');
            a.setAttribute('href', 'foo');
            a.href;
        "#,
        ),
        "foo"
    );
}

#[test]
fn setting_a_url_attribute_reflects_the_raw_value() {
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html"), Some("/assets/")),
            r#"
            const a = document.createElement('a');
            a.href = '/raw';
            [a.getAttribute('href'), a.href].join('|');
        "#,
        ),
        "/raw|https://example.com/raw"
    );
}

// ----------------------------------------------------- HTMLScriptElement

#[test]
fn a_created_script_is_an_html_script_element() {
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const script = document.createElement('script');
            [
                script instanceof HTMLScriptElement,
                script.constructor.name,
                script instanceof HTMLElement,
                script instanceof Element,
            ].join('|');
        "#,
        ),
        "true|HTMLScriptElement|true|true"
    );
}

#[test]
fn a_parsed_script_reports_the_script_interface() {
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head>\
                 <script src=\"app.js\" defer></script>\
                 </head><body></body></html>"
            ),
            r#"
            const script = document.querySelector('script');
            [
                script instanceof HTMLScriptElement,
                script.constructor.name,
                script.getAttribute('src'),
                script.defer,
            ].join('|');
        "#,
        ),
        "true|HTMLScriptElement|app.js|true"
    );
}

#[test]
fn script_boolean_members_reflect_their_content_attributes() {
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const s = document.createElement('script');
            const initial = [s.async, s.defer, s.noModule].join(',');
            s.async = true;
            s.defer = true;
            s.noModule = true;
            const set = [
                s.async, s.defer, s.noModule,
                s.hasAttribute('async'),
                s.hasAttribute('defer'),
                s.hasAttribute('nomodule'),
                s.getAttribute('async'),
            ].join(',');
            s.async = false;
            const cleared = [s.async, s.hasAttribute('async')].join(',');
            [initial, set, cleared].join('|');
        "#,
        ),
        "false,false,false\
         |true,true,true,true,true,true,\
         |false,false"
    );
}

#[test]
fn script_boolean_members_are_true_for_any_present_value() {
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const s = document.createElement('script');
            s.setAttribute('nomodule', 'nomodule');
            s.setAttribute('defer', '');
            [String(s.noModule), String(s.defer)].join('|');
        "#,
        ),
        "true|true"
    );
}

#[test]
fn script_string_members_round_trip() {
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const s = document.createElement('script');
            const initial = [s.type, s.charset, s.crossOrigin, s.integrity].join(',');
            s.type = 'module';
            s.charset = 'utf-8';
            s.crossOrigin = 'anonymous';
            s.integrity = 'sha384-abc';
            const set = [
                s.type, s.charset, s.crossOrigin, s.integrity,
                s.getAttribute('type'),
                s.getAttribute('crossorigin'),
                s.getAttribute('integrity'),
            ].join(',');
            s.setAttribute('crossorigin', 'use-credentials');
            const read = s.crossOrigin;
            [initial, set, read].join('|');
        "#,
        ),
        ",,,\
         |module,utf-8,anonymous,sha384-abc,module,anonymous,sha384-abc\
         |use-credentials"
    );
}
