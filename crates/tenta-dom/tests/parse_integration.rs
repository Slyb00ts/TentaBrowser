//! End-to-end parse through `tenta_html::Parser` into the arena.
//!
//! The sink is the arena document itself (`docs/spec/04-dom-and-css.md` §3.3):
//! the parser writes elements and text straight into it, with no intermediate
//! tree. These tests check that the conformant parser and the temporary test
//! tokenizer in `tests/support` build the same tree for the markup the
//! fixtures use, and that the parser's own recovery rules reach the arena.

mod support;

use tenta_dom::Document;
use tenta_html::{ParseStatus, Parser, ParserOptions, QuirksMode};
use url::Url;

fn parse(html: &str) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn parse_with_url(html: &str, url: &str) -> Document {
    let document = Document::with_url(Url::parse(url).expect("url"));
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

#[test]
fn the_real_parser_builds_the_same_tree_as_the_test_tokenizer() {
    let html = "<!doctype html><html lang=\"en\"><head><title>t</title></head>\
                <body><p class=\"a\">Hello <em>world</em></p></body></html>";

    let expected = support::parse(html);
    let expected_html = expected.outer_html(expected.document_element().expect("root"));
    assert_eq!(
        expected_html,
        "<html lang=\"en\"><head><title>t</title></head>\
         <body><p class=\"a\">Hello <em>world</em></p></body></html>"
    );

    let actual = parse(html);
    let actual_html = actual.outer_html(actual.document_element().expect("root"));
    assert_eq!(actual_html, expected_html);
    assert_eq!(actual.quirks_mode(), QuirksMode::NoQuirks);
    assert_eq!(actual.text(), expected.text());
    // `<title>` is head content, which the text extractor skips.
    assert_eq!(actual.text(), "Hello world\n");
}

#[test]
fn the_parser_implies_the_tags_the_markup_left_out() {
    // No <html>, no <head>, no <body>, an unclosed <p>: the tree builder's
    // insertion modes have to produce them.
    let document = parse("<title>t</title><p>one<p>two");
    assert_eq!(
        document.outer_html(document.document_element().expect("root")),
        "<html><head><title>t</title></head><body><p>one</p><p>two</p></body></html>"
    );
    assert!(document.head().is_some());
    assert!(document.body().is_some());
    assert_eq!(document.text(), "one\ntwo\n");
}

#[test]
fn a_missing_doctype_puts_the_document_in_quirks_mode() {
    let document = parse("<p>no doctype</p>");
    assert_eq!(document.quirks_mode(), QuirksMode::Quirks);
    let standards = parse("<!doctype html><p>with one</p>");
    assert_eq!(standards.quirks_mode(), QuirksMode::NoQuirks);
}

#[test]
fn a_streamed_parse_is_the_same_tree_as_a_single_chunk() {
    let html = "<!doctype html><html><body><ul><li>alpha<li>beta</ul>\
                <p>Caf&eacute; &amp; cr&#232;me</p></body></html>";
    let whole = parse(html);

    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    for chunk in html.as_bytes().chunks(7) {
        assert_eq!(parser.feed(chunk), ParseStatus::Continue);
    }
    let streamed = parser.finish();

    let root =
        |document: &Document| document.outer_html(document.document_element().expect("root"));
    assert_eq!(root(&streamed), root(&whole));
    assert!(
        streamed.text().contains("Café & crème"),
        "character references decode across chunk boundaries: {:?}",
        streamed.text()
    );
}

#[test]
fn selectors_and_extraction_run_over_a_parsed_tree() {
    let document = parse_with_url(
        "<!doctype html><html lang=\"en\"><head><title>Doc</title>\
         <meta name=\"description\" content=\"about\"></head>\
         <body><article><h1>Heading</h1><p>Body text.</p>\
         <a href=\"page.html\">link</a></article></body></html>",
        "http://127.0.0.1/dir/index.html",
    );

    let anchor = document
        .query_selector(document.document_node(), "article > a[href]")
        .expect("valid selector")
        .expect("the anchor is there");
    assert_eq!(document.attribute_str(anchor, "href"), Some("page.html"));

    let links = document.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].url, "http://127.0.0.1/dir/page.html");

    let metadata = document.metadata();
    assert_eq!(metadata.title.as_deref(), Some("Doc"));
    assert_eq!(metadata.lang.as_deref(), Some("en"));
    assert_eq!(
        metadata.meta.get("description").map(String::as_str),
        Some("about")
    );
    assert!(document.markdown().contains("# Heading"));
}

#[test]
fn scripting_off_parses_noscript_content_as_markup() {
    // Tier 1 never runs scripts, so `<noscript>` holds real elements
    // (`docs/spec/02-architecture.md` §1).
    let mut parser = Parser::new(
        Document::new(),
        ParserOptions {
            scripting_enabled: false,
            ..ParserOptions::default()
        },
    );
    assert_eq!(
        parser.feed(b"<!doctype html><body><noscript><p>fallback</p></noscript>"),
        ParseStatus::Continue
    );
    let document = parser.finish();
    assert!(
        document
            .query_selector(document.document_node(), "noscript p")
            .expect("valid selector")
            .is_some(),
        "with scripting off the fallback is parsed as elements"
    );
    assert_eq!(
        document.text_content(document.body().expect("body")),
        "fallback",
        "the fallback is real text in the tree, even though block text \
         extraction skips <noscript>"
    );
}
