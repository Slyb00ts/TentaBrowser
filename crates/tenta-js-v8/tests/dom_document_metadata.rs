//! Tests for d-7: the `Document` metadata members — `title`, `URL`,
//! `documentURI`, `characterSet`, `compatMode` and `referrer`.
//!
//! `document.title` is the interesting one: it is *not* a reflected attribute.
//! The HTML Standard's getter
//! (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-title>) reads
//! the *title element* — "the first `title` element in the document (in tree
//! order)" — and returns its child text content with ASCII whitespace stripped
//! and collapsed; the setter targets that element, or creates one in the head,
//! and writes the value by "string replace all". The WPT files under
//! `wpt/checkout/html/dom/documents/dom-tree-accessors/` pin every one of those
//! rules, and the tests below name the file each assertion comes from.
//!
//! Before this package the accessor did not exist, so `document.title = "x"`
//! silently created a plain own property: no error, no title. The
//! `assigning_document_title_leaves_no_own_property_behind` test is that
//! regression, and `the_title_getter_reads_a_title_element_from_markup` is the
//! one a plain property can never pass.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};
use tenta_js_v8::Engine;
use url::Url;

/// `<html><head></head><body></body></html>` over a document with `url` and
/// (optionally) a pre-set `encoding`, with no title element.
fn document(url: Option<&str>, encoding: Option<&str>) -> Rc<RefCell<Document>> {
    let mut doc = match url {
        Some(url) => Document::with_url(Url::parse(url).expect("the test URL parses")),
        None => Document::new(),
    };
    if let Some(encoding) = encoding {
        doc.set_encoding(encoding);
    }
    let root = doc.document_node();
    let html = doc.create_html_element(atoms::HTML);
    let head = doc.create_html_element(atoms::HEAD);
    let body = doc.create_html_element(atoms::BODY);
    for (parent, child) in [(root, html), (html, head), (html, body)] {
        doc.mutate(Mutation::Append { parent, child })
            .expect("append the element");
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

// ------------------------------------------------------------------- title

#[test]
fn setting_then_getting_document_title_round_trips() {
    assert_eq!(
        eval(
            document(None, None),
            r#"
            document.title = 'Hello, world';
            document.title;
        "#,
        ),
        "Hello, world"
    );
}

#[test]
fn the_title_getter_reads_a_title_element_from_markup() {
    // The case a plain JS property can never satisfy: the value lives in the
    // parsed tree, not on the wrapper. A pre-d-7 property would read back
    // `undefined` here, not "From markup".
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head><title>From markup</title></head>\
                 <body></body></html>"
            ),
            "document.title",
        ),
        "From markup"
    );
}

#[test]
fn the_title_getter_reads_the_first_title_element_in_tree_order() {
    // WPT `document.title-01.html` tests 3 and 4 and
    // `the-title-element/title-multiple-elements.html`: the first `title` in
    // tree order wins, and it need not be in the head.
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head><title>First</title><title>Second</title>\
                 </head><body></body></html>"
            ),
            "document.title",
        ),
        "First"
    );

    assert_eq!(
        eval(
            parse("<!doctype html><html><head></head><body></body></html>"),
            r#"
            const title = document.createElement('title');
            title.appendChild(document.createTextNode('In body'));
            document.body.appendChild(title);
            document.title;
        "#,
        ),
        "In body"
    );
}

#[test]
fn an_svg_title_is_not_the_document_title_element() {
    // WPT `document.title-not-in-html-svg.html`, "Root element not named svg":
    // only an HTML-namespace `title` is the title element, so an SVG `title`
    // first in tree order is ignored.
    assert_eq!(
        eval(
            parse("<!doctype html><html><head></head><body></body></html>"),
            r#"
            const svgTitle =
                document.createElementNS('http://www.w3.org/2000/svg', 'title');
            svgTitle.appendChild(document.createTextNode('svg'));
            document.body.appendChild(svgTitle);
            document.title;
        "#,
        ),
        ""
    );
}

#[test]
fn title_reads_the_empty_string_when_there_is_no_title_element() {
    // WPT `document.title-08.html`, "No title element".
    assert_eq!(eval(document(None, None), "document.title"), "");
}

#[test]
fn the_title_setter_creates_a_title_element_in_the_head() {
    // WPT `document.title-06.html`: with no title element, the setter creates
    // one and appends it to the head, where `head.lastChild` finds it.
    assert_eq!(
        eval(
            document(None, None),
            r#"
            const headLengthBefore = document.head.children.length;
            document.title = 'Created';
            [
                document.head.lastChild.tagName,
                document.head.lastChild.namespaceURI,
                document.head.children.length - headLengthBefore,
                document.head.lastChild === document.querySelector('title'),
            ].join('|');
        "#,
        ),
        "TITLE|http://www.w3.org/1999/xhtml|1|true"
    );
}

#[test]
fn the_title_setter_is_a_no_op_with_neither_title_nor_head() {
    // WPT `document.title-01.html` test 2: with the head blown away there is
    // nowhere to put a new title element, so the write is ignored and the
    // getter still reads the empty string.
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head><title>original</title></head>\
                 <body></body></html>"
            ),
            r#"
            const head = document.getElementsByTagName('head')[0];
            head.parentNode.removeChild(head);
            document.title = 'FAIL';
            [document.title, document.querySelectorAll('title').length].join('|');
        "#,
        ),
        "|0"
    );
}

#[test]
fn the_title_setter_replaces_the_title_elements_children() {
    // WPT `document.title-06.html` / `document.title-08.html`: writing is
    // "string replace all", so an existing title ends up with exactly one text
    // node, and writing the empty string leaves it with no child at all rather
    // than an empty text node.
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head><title>old</title></head>\
                 <body></body></html>"
            ),
            r#"
            const title = document.querySelector('title');
            document.title = 'new';
            const afterSet = [title.childNodes.length, title.textContent].join(':');
            document.title = '';
            [
                afterSet,
                title.childNodes.length,
                document.title,
                document.querySelectorAll('title').length,
            ].join('|');
        "#,
        ),
        "1:new|0||1"
    );
}

#[test]
fn the_title_getter_strips_and_collapses_ascii_whitespace() {
    // WPT `document.title-03.html`: runs of ASCII whitespace collapse to one
    // space and the ends are stripped.
    assert_eq!(
        eval(
            parse(
                "<!doctype html><html><head><title>  a \t\n b </title></head>\
                 <body></body></html>"
            ),
            "document.title",
        ),
        "a b"
    );

    // WPT `document.title-05.html`: the wider Unicode whitespace set is *not*
    // touched, so U+00A0 survives verbatim. The text is built through
    // `createTextNode` rather than markup so that the assertion is about the
    // getter's collapse rule and not about the parser's byte decoding, and the
    // NBSP is spelled with `fromCharCode` so no invisible character appears in
    // this source file.
    assert_eq!(
        eval(
            parse("<!doctype html><html><head></head><body></body></html>"),
            r#"
            const title = document.createElement('title');
            const nbsp = String.fromCharCode(0x00a0);
            title.appendChild(document.createTextNode('a' + nbsp + 'b'));
            document.head.appendChild(title);
            document.title;
        "#,
        ),
        "a\u{a0}b"
    );
}

#[test]
fn the_title_round_trips_non_ascii_text() {
    // A title is UTF-16 text, not a byte string: the generated setter's
    // one-byte fast path must not mangle it.
    assert_eq!(
        eval(
            document(None, None),
            "document.title = 'caf\u{e9} \u{2014} \u{65e5}\u{672c}'; document.title",
        ),
        "caf\u{e9} \u{2014} \u{65e5}\u{672c}"
    );
}

#[test]
fn assigning_document_title_leaves_no_own_property_behind() {
    // The regression this package exists to fix: a plain assignment must reach
    // the accessor on the prototype, not define an own data property on the
    // wrapper. `Object.getOwnPropertyDescriptor(document, 'title')` staying
    // `undefined` proves the assignment went through the setter, and the
    // round-trip proves the setter actually ran.
    assert_eq!(
        eval(
            document(None, None),
            r#"
            document.title = 'through the accessor';
            [
                document.hasOwnProperty('title'),
                Object.getOwnPropertyDescriptor(document, 'title') === undefined,
                Object.getOwnPropertyNames(document).includes('title'),
                document.title,
            ].join('|');
        "#,
        ),
        "false|true|false|through the accessor"
    );
}

// ------------------------------------------------------- URL / documentURI

#[test]
fn url_and_document_uri_report_the_document_url() {
    assert_eq!(
        eval(
            document(Some("https://example.com/dir/page.html?q=1"), None),
            r#"
            [
                document.URL,
                document.documentURI,
                document.URL === document.documentURI,
            ].join('|');
        "#,
        ),
        "https://example.com/dir/page.html?q=1\
         |https://example.com/dir/page.html?q=1\
         |true"
    );
}

#[test]
fn document_url_is_the_document_url_not_the_base_url() {
    // A `<base href>` moves URL resolution for elements, but not the document's
    // own URL: `document.URL` must stay the address the document was loaded
    // from. A getter wired to `base_url()` would report the base instead.
    let doc = document(Some("https://example.com/dir/page.html"), None);
    {
        let mut doc = doc.borrow_mut();
        let head = doc.head().expect("the head element");
        let base = doc.create_html_element(atoms::BASE);
        doc.mutate(Mutation::SetAttribute {
            element: base,
            name: QualName::new(atoms::EMPTY, atoms::HREF),
            value: "/assets/".to_owned(),
        })
        .expect("set the base href");
        doc.mutate(Mutation::Append {
            parent: head,
            child: base,
        })
        .expect("append the base element");
        doc.refresh_base_url();
    }

    assert_eq!(
        eval(
            doc,
            r#"
            const a = document.createElement('a');
            a.setAttribute('href', 'x');
            [document.URL, a.href].join('|');
        "#,
        ),
        "https://example.com/dir/page.html|https://example.com/assets/x"
    );
}

#[test]
fn document_url_is_empty_for_a_document_with_no_url() {
    assert_eq!(eval(document(None, None), "document.URL"), "");
}

// ------------------------------------------ characterSet and compatMode

#[test]
fn character_set_reports_the_document_encoding() {
    // The default a `Document` starts with, and the sniffed name the page
    // pipeline writes once the parse settles.
    assert_eq!(eval(document(None, None), "document.characterSet"), "UTF-8");
    assert_eq!(
        eval(
            document(None, Some("windows-1250")),
            "document.characterSet"
        ),
        "windows-1250"
    );
}

#[test]
fn compat_mode_tracks_the_document_quirks_mode() {
    // The tree really records this: the parser sets quirks mode from the
    // doctype, so a page without one reports "BackCompat" and a standards page
    // reports "CSS1Compat". A getter returning a constant would fail one half.
    assert_eq!(
        eval(
            parse("<!doctype html><html><head></head><body></body></html>"),
            "document.compatMode",
        ),
        "CSS1Compat"
    );
    assert_eq!(
        eval(
            parse("<html><head></head><body></body></html>"),
            "document.compatMode",
        ),
        "BackCompat"
    );
    // A document built by hand, never parsed, is not in quirks mode.
    assert_eq!(
        eval(document(None, None), "document.compatMode"),
        "CSS1Compat"
    );
}

// ------------------------------------------------------------- referrer

#[test]
fn referrer_is_present_and_empty_for_a_direct_navigation() {
    // A *partial* guard, and deliberately framed as one. The engine records no
    // referrer for a navigation — nothing writes one onto the `Document` — so
    // this getter returns the empty string for every navigation, which is the
    // right answer for a direct one and cannot be told apart from a wired one.
    // What this pins is only the part that is true today: the member exists on
    // the prototype (so `document.referrer.indexOf(...)` is a string call, not
    // a TypeError) and reports the empty string. When the navigation side starts
    // recording a referrer, this test must gain the non-empty case rather than
    // being deleted.
    assert_eq!(eval(document(None, None), "document.referrer"), "");
    assert_eq!(
        eval(
            document(None, None),
            "typeof document.referrer + '|' + String(document.referrer.length)",
        ),
        "string|0"
    );
}
