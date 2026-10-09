//! Real V8 coverage for DOMParser and XMLSerializer over arena documents.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;
use url::Url;

fn evaluate(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::with_url(
        Url::parse("https://example.test/page").expect("test URL"),
    )));
    let mut engine = Engine::with_document(document);
    engine.evaluate(source).expect("script evaluation")
}

#[test]
fn html_parse_is_inert_but_builds_the_full_tree() {
    assert_eq!(
        evaluate(
            r#"
            const parsed = new DOMParser().parseFromString(
              '<body><script>globalThis.parserRan = true</script><noscript><p id="inside">x</p></noscript></body>',
              'text/html'
            );
            [
              parsed instanceof Document,
              parsed.URL,
              parsed.contentType,
              parsed.characterSet,
              parsed.readyState,
              parsed.defaultView === null,
              parsed.body.querySelector('noscript p').id,
              typeof globalThis.parserRan
            ].join('|');
            "#,
        ),
        "true|https://example.test/page|text/html|UTF-8|complete|true|inside|undefined"
    );
}

#[test]
fn xml_mime_types_preserve_namespaces_and_return_parsererror_documents() {
    assert_eq!(
        evaluate(
            r#"
            const parser = new DOMParser();
            const types = ['text/xml', 'application/xml', 'application/xhtml+xml', 'image/svg+xml'];
            types.map(type => {
              const good = parser.parseFromString('<svg xmlns="http://www.w3.org/2000/svg"><clipPath/></svg>', type);
              const bad = parser.parseFromString('<svg>', type);
              return [
                good.contentType === type,
                good.documentElement.namespaceURI,
                good.documentElement.firstChild.localName,
                good.readyState,
                bad.contentType === type,
                bad.documentElement.namespaceURI,
                bad.documentElement.localName,
                bad.documentElement.textContent.length > 0
              ].join(',');
            }).join('|');
            "#,
        ),
        std::iter::repeat_n(
            "true,http://www.w3.org/2000/svg,clipPath,complete,true,http://www.mozilla.org/newlayout/xml/parsererror.xml,parsererror,true",
            4
        )
        .collect::<Vec<_>>()
        .join("|")
    );
}

#[test]
fn serializer_reads_minted_xml_nodes_across_document_slots() {
    assert_eq!(
        evaluate(
            r#"
            const parsed = new DOMParser().parseFromString(
              '<root xmlns:p="urn:example"><p:child a="&amp;">a &lt; b</p:child></root>',
              'application/xml'
            );
            const serializer = new XMLSerializer();
            [
              serializer.serializeToString(parsed),
              serializer.serializeToString(parsed.documentElement.firstChild)
            ].join('|');
            "#,
        ),
        "<root xmlns:p=\"urn:example\"><p:child a=\"&amp;\">a &lt; b</p:child></root>|<p:child xmlns:p=\"urn:example\" a=\"&amp;\">a &lt; b</p:child>"
    );
}

#[test]
fn invalid_mime_and_non_node_argument_throw_type_errors() {
    assert_eq!(
        evaluate(
            r#"
            const parser = new DOMParser();
            const serializer = new XMLSerializer();
            let mimeError = false;
            let nodeError = false;
            try { parser.parseFromString('<x/>', 'application/json'); }
            catch (error) { mimeError = error instanceof TypeError; }
            try { serializer.serializeToString({}); }
            catch (error) { nodeError = error instanceof TypeError; }
            [mimeError, nodeError].join('|');
            "#,
        ),
        "true|true"
    );
}

#[test]
fn empty_html_namespace_element_has_an_end_tag_in_xml_serialization() {
    assert_eq!(
        evaluate(
            r#"
            const parsed = new DOMParser().parseFromString('<div><span></span></div>', 'text/html');
            new XMLSerializer().serializeToString(parsed.body.firstChild);
            "#,
        ),
        "<div xmlns=\"http://www.w3.org/1999/xhtml\"><span></span></div>"
    );
}
