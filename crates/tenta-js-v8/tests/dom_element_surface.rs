//! Tests for the Element-side DOM surface this slice added: the DOM boundary's
//! name validation, `Element.prefix` and the qualified `tagName`, the
//! namespace-aware attribute operations, `Element.attributes` /
//! `NamedNodeMap` / `Attr`, `getElementsByTagNameNS` and the ASCII-whitespace
//! rule `getElementsByClassName` splits on.
//!
//! Each test names the WPT file whose rule it pins, and each is written so that
//! deleting the implementation makes it fail: the assertions are on values read
//! back out of the tree or on an object identity the engine built, never on "no
//! exception was thrown".
//!
//! Two of the tables below are the WPT files' own tables, transcribed whole -
//! `dom/nodes/Document-createElement.html`'s `valid`/`invalid` name lists and
//! `dom/nodes/Document-createElementNS.js`'s namespace/qualified-name/expected-
//! exception rows (plus the rows `Document-createElementNS.html` appends
//! inline). They are here rather than only in the WPT run because they are the
//! whole specification of *validate* at this boundary, and a unit test that
//! reports the first mismatched row says which rule broke.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// `<html><head></head><body></body></html>`: the minimum a page's document
/// needs for `document.body` and the query operations to have somewhere to run.
fn document() -> Rc<RefCell<Document>> {
    let mut doc = Document::new();
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

fn eval(doc: Rc<RefCell<Document>>, source: &str) -> String {
    let mut engine = Engine::with_document(doc);
    engine
        .evaluate(source)
        .expect("the test script should evaluate without throwing")
}

/// The `valid` list of `dom/nodes/Document-createElement.html`: every name
/// `document.createElement` must accept.
const ELEMENT_VALID_NAMES: &str = r#"null, null, "foo", "f1oo", "foo1", "fெ", "fooெ", ":", ":foo", "f:oo", "foo:", "f:o:o", "f::oo", "f::oo:", "foo:0", "foo:_", "foo:ெ", "foo:fooெ", "fooெ:foo", "xml", "xmlns", "xmlfoo", "xml:foo", "xmlns:foo", "xmlfoo:bar", "svg", "math", "FOO", "marK", "İnput", "ınput", "̀foo", "f}oo", "foo}", "￿foo", "f￿oo", "foo￿", "f<oo", "̀""#;

/// The `invalid` list of the same file: every name it must refuse with an
/// `InvalidCharacterError`.
const ELEMENT_INVALID_NAMES: &str =
    r#""", "1foo", "1:foo", "fo o", "}foo", "<foo", "foo>", "<foo>", "-foo", ".foo""#;

/// `dom/nodes/Document-createElementNS.js` plus the rows
/// `Document-createElementNS.html` appends: `[namespace, qualifiedName,
/// expected exception name or null]`.
const CREATE_ELEMENT_NS_ROWS: &str = r#"
  [null, null, null],
  [null, null, null],
  [null, "foo", null],
  [null, "1foo", "INVALID_CHARACTER_ERR"],
  [null, "f1oo", null],
  [null, "foo1", null],
  [null, "ெfoo", null],
  [null, ";foo", null],
  [null, "}foo", "INVALID_CHARACTER_ERR"],
  [null, "f}oo", null],
  [null, "foo}", null],
  [null, "￿foo", null],
  [null, "f￿oo", null],
  [null, "foo￿", null],
  [null, "<foo", "INVALID_CHARACTER_ERR"],
  [null, "foo>", "INVALID_CHARACTER_ERR"],
  [null, "<foo>", "INVALID_CHARACTER_ERR"],
  [null, "f<oo", null],
  [null, "^^", "INVALID_CHARACTER_ERR"],
  [null, "fo o", "INVALID_CHARACTER_ERR"],
  [null, "-foo", "INVALID_CHARACTER_ERR"],
  [null, ".foo", "INVALID_CHARACTER_ERR"],
  [null, ":foo", "INVALID_CHARACTER_ERR"],
  [null, "f:oo", "NAMESPACE_ERR"],
  [null, "foo:", "INVALID_CHARACTER_ERR"],
  [null, "f:o:o", "NAMESPACE_ERR"],
  [null, ":", "INVALID_CHARACTER_ERR"],
  [null, "xml", null],
  [null, "xmlns", "NAMESPACE_ERR"],
  [null, "xmlfoo", null],
  [null, "xml:foo", "NAMESPACE_ERR"],
  [null, "xmlns:foo", "NAMESPACE_ERR"],
  [null, "xmlfoo:bar", "NAMESPACE_ERR"],
  [null, "null:xml", "NAMESPACE_ERR"],
  ["", null, null],
  ["", ":foo", "INVALID_CHARACTER_ERR"],
  ["", "f:oo", "NAMESPACE_ERR"],
  ["", "foo:", "INVALID_CHARACTER_ERR"],
  [null, null, null],
  [null, null, null],
  [null, "foo", null],
  [null, "1foo", "INVALID_CHARACTER_ERR"],
  [null, "f1oo", null],
  [null, "foo1", null],
  [null, ":foo", "INVALID_CHARACTER_ERR"],
  [null, "f:oo", "NAMESPACE_ERR"],
  [null, "foo:", "INVALID_CHARACTER_ERR"],
  [null, "f::oo", "NAMESPACE_ERR"],
  [null, "xml", null],
  [null, "xmlns", "NAMESPACE_ERR"],
  [null, "xmlfoo", null],
  [null, "xml:foo", "NAMESPACE_ERR"],
  [null, "xmlns:foo", "NAMESPACE_ERR"],
  [null, "xmlfoo:bar", "NAMESPACE_ERR"],
  ["http://example.com/", "foo", null],
  ["http://example.com/", "1foo", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "<foo>", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "fo<o", null],
  ["http://example.com/", "-foo", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", ".foo", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "f1oo", null],
  ["http://example.com/", "foo1", null],
  ["http://example.com/", ":foo", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "f:oo", null],
  ["http://example.com/", "f:o:o", null],
  ["http://example.com/", "foo:", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "f::oo", null],
  ["http://example.com/", "a:0", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "0:a", null],
  ["http://example.com/", "a:_", null],
  ["http://example.com/", "a:ெ", null],
  ["http://example.com/", "a:;", null],
  ["http://example.com/", "a:̀", null],
  ["http://example.com/", "ெ:a", null],
  ["http://example.com/", "̀:a", null],
  ["http://example.com/", ";:a", null],
  ["http://example.com/", "a:aெ", null],
  ["http://example.com/", "aெ:a", null],
  ["http://example.com/", "xml:test", "NAMESPACE_ERR"],
  ["http://example.com/", "xmlns:test", "NAMESPACE_ERR"],
  ["http://example.com/", "test:xmlns", null],
  ["http://example.com/", "xmlns", "NAMESPACE_ERR"],
  ["http://example.com/", "_:_", null],
  ["http://example.com/", "_:h0", null],
  ["http://example.com/", "_:test", null],
  ["http://example.com/", "l_:_", null],
  ["http://example.com/", "ns:_0", null],
  ["http://example.com/", "ns:a0", null],
  ["http://example.com/", "ns0:test", null],
  ["http://example.com/", "a.b:c", null],
  ["http://example.com/", "a-b:c", null],
  ["http://example.com/", "xml", null],
  ["http://example.com/", "XMLNS", null],
  ["http://example.com/", "xmlfoo", null],
  ["http://example.com/", "xml:foo", "NAMESPACE_ERR"],
  ["http://example.com/", "XML:foo", null],
  ["http://example.com/", "xmlns:foo", "NAMESPACE_ERR"],
  ["http://example.com/", "XMLNS:foo", null],
  ["http://example.com/", "xmlfoo:bar", null],
  ["http://example.com/", "prefix::local", null],
  ["http://example.com/", "namespaceURI:{", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:}", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:~", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:'", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:!", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:@", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:#", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:$", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:%", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:^", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:&", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:*", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:(", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:)", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:+", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:=", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:[", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:]", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:\\", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:/", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:;", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:`", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:<", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:>", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:,", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:a ", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", "namespaceURI:\"", "INVALID_CHARACTER_ERR"],
  ["/", "foo", null],
  ["/", "1foo", "INVALID_CHARACTER_ERR"],
  ["/", "f1oo", null],
  ["/", "foo1", null],
  ["/", ":foo", "INVALID_CHARACTER_ERR"],
  ["/", "f:oo", null],
  ["/", "foo:", "INVALID_CHARACTER_ERR"],
  ["/", "xml", null],
  ["/", "xmlns", "NAMESPACE_ERR"],
  ["/", "xmlfoo", null],
  ["/", "xml:foo", "NAMESPACE_ERR"],
  ["/", "xmlns:foo", "NAMESPACE_ERR"],
  ["/", "xmlfoo:bar", null],
  ["http://www.w3.org/XML/1998/namespace", "foo", null],
  ["http://www.w3.org/XML/1998/namespace", "1foo", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/XML/1998/namespace", "f1oo", null],
  ["http://www.w3.org/XML/1998/namespace", "foo1", null],
  ["http://www.w3.org/XML/1998/namespace", ":foo", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/XML/1998/namespace", "f:oo", null],
  ["http://www.w3.org/XML/1998/namespace", "foo:", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/XML/1998/namespace", "xml", null],
  ["http://www.w3.org/XML/1998/namespace", "xmlns", "NAMESPACE_ERR"],
  ["http://www.w3.org/XML/1998/namespace", "xmlfoo", null],
  ["http://www.w3.org/XML/1998/namespace", "xml:foo", null],
  ["http://www.w3.org/XML/1998/namespace", "xmlns:foo", "NAMESPACE_ERR"],
  ["http://www.w3.org/XML/1998/namespace", "xmlfoo:bar", null],
  ["http://www.w3.org/XML/1998/namespaces", "xml:foo", "NAMESPACE_ERR"],
  ["http://www.w3.org/xml/1998/namespace", "xml:foo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "foo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "1foo", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/2000/xmlns/", "f1oo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "foo1", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", ":foo", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/2000/xmlns/", "f:oo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "foo:", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/2000/xmlns/", "xml", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "xmlns", null],
  ["http://www.w3.org/2000/xmlns/", "xmlfoo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "xml:foo", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "xmlns:foo", null],
  ["http://www.w3.org/2000/xmlns/", "xmlfoo:bar", "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "foo:xmlns", "NAMESPACE_ERR"],
  ["foo:", "foo", null],
  ["foo:", "1foo", "INVALID_CHARACTER_ERR"],
  ["foo:", "f1oo", null],
  ["foo:", "foo1", null],
  ["foo:", ":foo", "INVALID_CHARACTER_ERR"],
  ["foo:", "f:oo", null],
  ["foo:", "foo:", "INVALID_CHARACTER_ERR"],
  ["foo:", "xml", null],
  ["foo:", "xmlns", "NAMESPACE_ERR"],
  ["foo:", "xmlfoo", null],
  ["foo:", "xml:foo", "NAMESPACE_ERR"],
  ["foo:", "xmlns:foo", "NAMESPACE_ERR"],
  ["foo:", "xmlfoo:bar", null],
  ["", "", "INVALID_CHARACTER_ERR"],
  [null, "", "INVALID_CHARACTER_ERR"],
  [null, "", "INVALID_CHARACTER_ERR"],
  ["http://example.com/", null, null],
  ["http://example.com/", "", "INVALID_CHARACTER_ERR"],
  ["/", null, null],
  ["/", "", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/XML/1998/namespace", null, null],
  ["http://www.w3.org/XML/1998/namespace", "", "INVALID_CHARACTER_ERR"],
  ["http://www.w3.org/2000/xmlns/", null, "NAMESPACE_ERR"],
  ["http://www.w3.org/2000/xmlns/", "", "INVALID_CHARACTER_ERR"],
  ["foo:", null, null],
  ["foo:", "", "INVALID_CHARACTER_ERR"],
"#;

// ------------------------------------------------------- name validation

#[test]
fn create_element_accepts_every_valid_name_of_the_wpt_table() {
    // `dom/nodes/Document-createElement.html`: the valid list, 39 entries,
    // including the ones the old ASCII-only check refused - U+0130 and U+0131
    // in `İnput`/`ınput`, `f}oo`, `foo}`, `f<oo` and the four
    // non-ASCII-leading names. Removing `validate_name`'s `is_name_start_char`
    // call makes the first of those throw; restoring the old ASCII-only first
    // character check makes most of the list throw. The table is the assertion.
    assert_eq!(
        eval(
            document(),
            &format!(
                r#"
                const names = [{ELEMENT_VALID_NAMES}];
                const bad = [];
                for (const name of names) {{
                    try {{ document.createElement(name); }}
                    catch (e) {{ bad.push(JSON.stringify(name) + ':' + e.name); }}
                }}
                bad.join(',') || 'all accepted';
                "#,
            ),
        ),
        "all accepted"
    );
}

#[test]
fn create_element_refuses_every_invalid_name_of_the_wpt_table() {
    // The same file's invalid list: the ten names that must raise
    // `InvalidCharacterError`. The table's `"foo>"` row is the one that pins the
    // *continuation* character rule: `createElement("f<oo")` is accepted while
    // `createElement("foo>")` is not, so a rule that only checks the first
    // character fails here, and so does one that also refuses `>` only first.
    assert_eq!(
        eval(
            document(),
            &format!(
                r#"
                const names = [{ELEMENT_INVALID_NAMES}];
                const wrong = [];
                for (const name of names) {{
                    try {{
                        document.createElement(name);
                        wrong.push(JSON.stringify(name) + ':accepted');
                    }} catch (e) {{
                        if (e.name !== 'InvalidCharacterError') {{
                            wrong.push(JSON.stringify(name) + ':' + e.name);
                        }}
                    }}
                }}
                wrong.join(',') || 'all refused';
                "#,
            ),
        ),
        "all refused"
    );
}

#[test]
fn create_element_ns_matches_every_row_of_the_wpt_table() {
    // `dom/nodes/Document-createElementNS.js` (and the rows
    // `Document-createElementNS.html` appends): 195 rows of
    // `[namespace, qualifiedName, expected]`. This is the whole of *validate and
    // extract* - the first-character rule, the empty-prefix/empty-local rule,
    // the fact that the local part may contain colons while the prefix is never
    // checked, and the four namespace rules. Any single rule removed shifts a
    // whole block of rows.
    //
    // The XML/XHTML halves of that WPT file are unreachable here (they read
    // `iframe.contentDocument`, and this engine has no subdocument loading), so
    // this runs the HTML-document column, which is the same table.
    assert_eq!(
        eval(
            document(),
            &format!(
                r#"
                const rows = [{CREATE_ELEMENT_NS_ROWS}];
                // The tables name the legacy constants; a `DOMException`'s
                // `name` is the modern spelling of the same thing.
                const NAMES = {{
                    INVALID_CHARACTER_ERR: 'InvalidCharacterError',
                    NAMESPACE_ERR: 'NamespaceError',
                }};
                const wrong = [];
                for (const [ns, qname, expected] of rows) {{
                    let got = null;
                    try {{ document.createElementNS(ns, qname); }}
                    catch (e) {{ got = e.name; }}
                    if (got !== (expected === null ? null : NAMES[expected])) {{
                        wrong.push(
                            JSON.stringify(ns) + ',' + JSON.stringify(qname)
                            + ': expected ' + expected + ', got ' + got
                        );
                    }}
                }}
                wrong.length ? wrong.slice(0, 5).join(' | ') : 'all ' + rows.length + ' rows match';
                "#,
            ),
        ),
        "all 195 rows match"
    );
}

#[test]
fn set_attribute_and_toggle_attribute_use_the_attribute_name_rule() {
    // `dom/nodes/attributes.html`: `invalid_names` is `[ "" ]`, so the empty
    // name must throw, and `valid_names` is
    // `["x", "X", ":", "a:0", "invalid^Name", "\\\\", "'", '"', "0", "0:a", ":a",
    // "x:y:x", "~"]`, every one of which `setAttribute` and `toggleAttribute`
    // must *accept* — no first-character rule can accept both `"0"` and `"~"`.
    // `dom/nodes/processing-instruction-attributes.html` supplies the other
    // half: `"="`, `">"`, `"/"` and whitespace are refused wherever they sit.
    // Removing `validate_attribute_name` makes the first two fields `accepted`;
    // keeping the old ASCII-only first-character check makes the five odd
    // names throw.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('div');
            const out = [];
            try { el.setAttribute('', 'v'); out.push('setAttribute-accepted'); }
            catch (e) { out.push('setAttribute:' + e.name); }
            try { el.toggleAttribute(''); out.push('toggleAttribute-accepted'); }
            catch (e) { out.push('toggleAttribute:' + e.name); }
            for (const name of ['0', '~', '\\', "'", '"', 'invalid^Name', 'a:0', 'x:y:x']) {
                try { el.setAttribute(name, 'v'); out.push(name + ':ok'); }
                catch (e) { out.push(name + ':' + e.name); }
            }
            for (const name of ['a=', '=x', 'b>', '>', '/x/', 'x/', 'x\t', '\tx', 'x\n']) {
                try { el.setAttribute(name, 'v'); out.push(JSON.stringify(name) + ':accepted'); }
                catch (e) { out.push(JSON.stringify(name) + ':' + e.name); }
            }
            out.join('|');
            "#,
        ),
        r#"setAttribute:InvalidCharacterError|toggleAttribute:InvalidCharacterError|"#.to_owned()
            + r#"0:ok|~:ok|\:ok|':ok|":ok|invalid^Name:ok|a:0:ok|x:y:x:ok|"#
            + r#""a=":InvalidCharacterError|"=x":InvalidCharacterError|"b>":InvalidCharacterError|">":InvalidCharacterError|"/x/":InvalidCharacterError|"x/":InvalidCharacterError|"x\t":InvalidCharacterError|"\tx":InvalidCharacterError|"x\n":InvalidCharacterError"#
    );
}

// ---------------------------------------- prefix, tagName, namespaceURI

#[test]
fn a_prefixed_element_reports_its_prefix_and_its_qualified_tag_name() {
    // `dom/nodes/Document-createElementNS.html`, "Lower-case HTML element with a
    // prefix": `createElementNS(HTMLNS, "html:span")` has `prefix` `"html"`,
    // `localName` `"span"` and `tagName` `"HTML:SPAN"` - the tag name is the
    // *qualified* name, uppercased for an HTML element. Before `Element.prefix`
    // existed the getter read `undefined` rather than the string or `null`, and
    // the tag name dropped the prefix entirely; removing either getter changes
    // one field of this line.
    assert_eq!(
        eval(
            document(),
            r#"
            const show = (v) => (v === null ? 'null' : v === undefined ? 'undefined' : String(v));
            const HTMLNS = 'http://www.w3.org/1999/xhtml';
            function describe(el) {
                return [show(el.prefix), el.localName, el.tagName, show(el.namespaceURI)].join('/');
            }
            [
                describe(document.createElementNS(HTMLNS, 'html:span')),
                describe(document.createElementNS(HTMLNS, 'span')),
                describe(document.createElementNS('test', 'html:span')),
                describe(document.createElementNS(null, 'span')),
                describe(document.createElement('div')),
            ].join(' ');
            "#,
        ),
        "html/span/HTML:SPAN/http://www.w3.org/1999/xhtml \
         null/span/SPAN/http://www.w3.org/1999/xhtml \
         html/span/html:span/test \
         null/span/span/null \
         null/div/DIV/http://www.w3.org/1999/xhtml"
    );
}

// -------------------------------------- namespace-aware attributes

#[test]
fn set_attribute_ns_partitions_the_attribute_list_by_namespace_and_local_name() {
    // `dom/nodes/attributes.html`: `setAttributeNS("ab", "attr", "fail")` then
    // `setAttributeNS("kl", "attr", "pass")` are *two* attributes - the pair
    // `(namespace, localName)` is the key, so a same-local-name attribute in
    // another namespace is neither found nor overwritten. Without
    // `setAttributeNS` the first call throws and nothing is stored; without the
    // namespace-aware `getAttributeNS` lookup the two reads below collapse.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('baz');
            el.setAttributeNS('ab', 'attr', 'fail');
            el.setAttributeNS('kl', 'attr', 'pass');
            el.setAttribute('plain', 'third');
            [
                el.attributes.length,
                el.getAttributeNS('ab', 'attr'),
                el.getAttributeNS('kl', 'attr'),
                el.getAttributeNS(null, 'plain'),
                el.hasAttributeNS('kl', 'attr'),
                el.hasAttributeNS('zz', 'attr'),
                el.hasAttributeNS(null, 'attr'),
            ].join('|');
            "#,
        ),
        "3|fail|pass|third|true|false|false"
    );
}

#[test]
fn set_attribute_ns_stores_the_prefix_and_local_name_verbatim() {
    // `dom/nodes/attributes.html`, "Based on a test by David Flanagan":
    // `setAttributeNS("foo", "foo:bar", "1")` stores `localName` `"bar"` with
    // `prefix` `"foo"` and *does not* ASCII-lowercase (`setAttribute`'s own
    // step does; `setAttributeNS` has no such step). The three fields are read
    // off the `Attr` the map hands back, so removing either the `NamedNodeMap`,
    // the `Attr` interface or the prefix storage changes one of them.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('baz');
            el.setAttributeNS('foo', 'foo:Bar', '1');
            const attr = el.attributes[0];
            [
                attr.localName,
                attr.prefix,
                attr.namespaceURI,
                attr.name,
                attr.value,
                attr.specified,
                attr.ownerElement === el,
                el.getAttributeNS('foo', 'Bar'),
            ].join('|');
            "#,
        ),
        "Bar|foo|foo|foo:Bar|1|true|true|1"
    );
}

#[test]
fn remove_attribute_ns_removes_only_the_namespace_it_names() {
    // `dom/nodes/Element-removeAttributeNS.html`: removing by namespace leaves a
    // same-named attribute in another namespace alone, and removing one that is
    // not there is silent. Removing the `remove_attribute_ns` lookup by
    // `(namespace, localName)` - for instance matching on the local name alone -
    // makes the first read below `null` instead of `"stays"`.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('div');
            el.setAttributeNS('ab', 'attr', 'goes');
            el.setAttributeNS('kl', 'attr', 'stays');
            el.removeAttributeNS('ab', 'attr');
            el.removeAttributeNS('ab', 'missing');
            [
                el.hasAttributeNS('ab', 'attr'),
                el.getAttributeNS('kl', 'attr'),
                el.attributes.length,
            ].join('|');
            "#,
        ),
        "false|stays|1"
    );
}

// ------------------------------------- NamedNodeMap and Attr

#[test]
fn attributes_is_the_elements_live_attribute_list() {
    // `dom/nodes/Node-cloneNode.html:19` reads `nodeB.attributes.length`, and
    // `dom/nodes/attributes.js`'s `attributes_are` walks `el.attributes[i]`.
    // The map is `[SameObject]` and live: the same object twice, and a write
    // through `setAttribute` after it was obtained is visible through it.
    // Removing `Element.attributes` makes the first line throw; making the map
    // a snapshot (rather than deriving it from the element) makes the second
    // `length` stale.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('div');
            el.setAttribute('a', '1');
            const map = el.attributes;
            const same = map === el.attributes;
            const before = map.length;
            el.setAttribute('b', '2');
            el.removeAttribute('a');
            [same, before, map.length, map[0].name].join('|');
            "#,
        ),
        "true|1|1|b"
    );
}

#[test]
fn attribute_entries_report_their_value_and_owner() {
    // The `attr_is` helper of `dom/nodes/attributes.js` reads exactly these
    // seven fields, and `attributes_are` adds the `ownerElement` identity.
    // Removing the `Attr` interface's `value` getter (or making it read the
    // record's empty detached state) changes the first field.
    assert_eq!(
        eval(
            document(),
            r#"
            const show = (v) => (v === null ? 'null' : v === undefined ? 'undefined' : String(v));
            const el = document.createElement('div');
            el.setAttribute('id', 'x');
            const attr = el.getAttributeNode('id');
            [
                attr.value,
                attr.nodeValue,
                attr.textContent,
                attr.localName,
                show(attr.namespaceURI),
                show(attr.prefix),
                attr.name,
                attr.nodeName,
                attr.specified,
                attr.nodeType,
                attr.ownerElement === el,
            ].join('|');
            "#,
        ),
        "x|x|x|id|null|null|id|id|true|2|true"
    );
}

#[test]
fn a_held_attr_reflects_a_later_write_through_the_element() {
    // The standard makes an attached `Attr` and its element one value, so a
    // `setAttribute` after the `Attr` was obtained is visible through it - and
    // the reverse, which is `Attr.value`'s setter. A snapshot-valued `Attr`
    // (rather than one that reads the element) fails the third and sixth
    // fields.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('div');
            el.setAttribute('a', '1');
            const attr = el.getAttributeNode('a');
            el.setAttribute('a', '2');
            const afterElementWrite = attr.value;
            attr.value = '3';
            [afterElementWrite, attr.value, el.getAttribute('a')].join('|');
            "#,
        ),
        "2|3|3"
    );
}

#[test]
fn create_attribute_mints_a_detached_attr_that_attaching_moves_onto_the_element() {
    // `dom/nodes/Document-createAttribute.html`: a `createAttribute` result has
    // no owner and an empty value, `setAttributeNode` gives it one and returns
    // the attribute it replaced (`null` when there was none), and the local name
    // is ASCII-lowercased in an HTML document — which is why the first attach
    // below lands on `id` while `createAttributeNS(null, "Id")` would keep the
    // case. Removing the record update in `attach_attribute_node` leaves
    // `ownerElement` null after the attach.
    assert_eq!(
        eval(
            document(),
            r#"
            const el = document.createElement('div');
            const attr = document.createAttribute('ID');
            const firstReplaced = el.setAttributeNode(attr);
            el.setAttribute('id', 'by-setAttribute');
            const second = document.createAttributeNS(null, 'Id');
            const secondReplaced = el.setAttributeNodeNS(second);
            [
                attr.localName,
                attr.ownerElement === el,
                attr.value,
                firstReplaced === null,
                el.getAttribute('id'),
                second.localName,
                secondReplaced === null,
                el.getAttributeNames().join(','),
            ].join('|');
            "#,
        ),
        // The two attributes are `id` (no namespace) and `Id` (no namespace):
        // different local names, so the second does *not* replace the first.
        // `attr.value` follows the element it was attached to, which is why it
        // reports the `setAttribute` write rather than the empty value the
        // detached attribute started with.
        "id|true|by-setAttribute|true|by-setAttribute|Id|true|id,Id"
    );
}

#[test]
fn remove_attribute_node_returns_the_same_object_detached() {
    // `dom/nodes/Element-removeAttributeNode.html`: the operation returns the
    // *same* `Attr` it was given, now ownerless and keeping its value. Removing
    // the record update leaves `ownerElement` pointing at the element.
    assert_eq!(
        eval(
            document(),
            r#"
            const show = (v) => (v === null ? 'null' : v === undefined ? 'undefined' : String(v));
            const el = document.createElement('div');
            el.setAttribute('a', '1');
            const attr = el.getAttributeNode('a');
            let wrong = 'none';
            try { el.removeAttributeNode(document.createAttribute('b')); wrong = 'accepted'; }
            catch (e) { wrong = e.name; }
            const returned = el.removeAttributeNode(attr);
            [
                wrong,
                returned === attr,
                show(attr.ownerElement),
                attr.value,
                el.hasAttribute('a'),
            ].join('|');
            "#,
        ),
        "NotFoundError|true|null|1|false"
    );
}

#[test]
fn a_named_node_map_removal_throws_not_found_for_a_missing_name() {
    // `dom/nodes/NamedNodeMap-removeNamedItem` / the `attr_not_found` rule:
    // removing an attribute the element does not carry is a `NotFoundError`,
    // unlike `removeAttribute`, which is a silent no-op.
    assert_eq!(
        eval(
            document(),
            r#"
            const show = (v) => (v === null ? 'null' : v === undefined ? 'undefined' : String(v));
            const el = document.createElement('div');
            el.setAttributeNS('ns', 'p:a', 'v');
            const byName = el.attributes.getNamedItem('p:a');
            const byNs = el.attributes.getNamedItemNS('ns', 'a');
            // The same Attr object is returned by both lookup paths, and its
            // value remains available after removal detaches it.
            const attachedValue = byName && byName.value;
            const removedNs = el.attributes.removeNamedItemNS('ns', 'a');
            let missing = 'none';
            try { el.attributes.removeNamedItem('p:a'); missing = 'accepted'; }
            catch (e) { missing = e.name; }
            [
                attachedValue,
                byName === byNs,
                removedNs === byName,
                removedNs.localName,
                show(removedNs.ownerElement),
                removedNs.value,
                missing,
                el.attributes.length,
            ].join('|');
            "#,
        ),
        // Both lookups and removal return the same Attr object. `attr_not_found`
        // makes the last `removeNamedItem` throw rather than no-op.
        "v|true|true|a|null|v|NotFoundError|0"
    );
}

// ---------------------------------------- getElementsByTagNameNS

#[test]
fn get_elements_by_tag_name_ns_matches_on_namespace_and_case_sensitive_local_name() {
    // `dom/nodes/Document-Element-getElementsByTagNameNS.js`: `("*", ...)`
    // matches any namespace, `("", ...)` the null one, a name is matched
    // case-sensitively (`createElementNS(HTMLNS, "ABC")` is *not* found by
    // `"abc"`), and `("**", "*")` matches nothing. Removing the namespace arm of
    // `qualified_snapshot` makes the third field 2 instead of 1.
    assert_eq!(
        eval(
            document(),
            r#"
            const HTMLNS = 'http://www.w3.org/1999/xhtml';
            const host = document.createElement('div');
            document.body.appendChild(host);
            host.appendChild(document.createElementNS('test', 'body'));
            host.appendChild(document.createElementNS('', 'body'));
            host.appendChild(document.createElementNS(HTMLNS, 'ABC'));
            const count = (ns, local) => document.getElementsByTagNameNS(ns, local).length;
            [
                count('*', 'body'),
                count('test', 'body'),
                count('', '*'),
                count(HTMLNS, 'ABC'),
                count(HTMLNS, 'abc'),
                count('**', '*') < count('*', '*'),
                count(null, 'body'),
            ].join('|');
            "#,
        ),
        // `document.body` is itself a `<body>`, so the wildcard-namespace count
        // is the two script-created ones plus it.
        "3|1|1|1|0|true|1"
    );
}

#[test]
fn an_element_query_excludes_the_element_itself() {
    // `dom/nodes/Element-getElementsByTagNameNS.html`, "Matching the context
    // object": a query rooted at an element never returns that element, in
    // either namespace spelling. Passing `include_root` as `true` - or rooting
    // the query at the wrong node - makes both lengths 1.
    assert_eq!(
        eval(
            document(),
            r#"
            const HTMLNS = 'http://www.w3.org/1999/xhtml';
            const el = document.createElement('div');
            document.body.appendChild(el);
            el.appendChild(document.createElement('span'));
            [
                el.getElementsByTagNameNS('*', el.localName).length,
                el.getElementsByTagNameNS(HTMLNS, el.localName).length,
                el.getElementsByTagNameNS('*', 'span').length,
            ].join('|');
            "#,
        ),
        "0|0|1"
    );
}

// ---------------------------------------- getElementsByClassName

#[test]
fn get_elements_by_class_name_splits_on_ascii_whitespace_only() {
    // `dom/nodes/getElementsByClassName-whitespace-class-names.html`: each span
    // carries a single-character class name, and
    // `document.getElementsByClassName(thatCharacter)` must find it. The 19
    // characters that file lists are exactly the ones Rust's Unicode
    // `split_whitespace` calls whitespace and the DOM does not. With
    // `split_whitespace()` each of those queries collapses to zero tokens, the
    // filter matches nothing, and every length below is 0; with
    // `split_ascii_whitespace()` each is 1.
    //
    // The second list is the control: characters Rust does *not* call
    // whitespace, which both splits treat as one token. The last three fields
    // pin the other half of the rule - `"a\u00a0b"` is one class name, not two,
    // so `"a"` does not find it while the whole string does.
    //
    // The code points are written as numbers and built with `String.fromCharCode`
    // rather than as JavaScript escapes so that the test's own bytes stay ASCII.
    assert_eq!(
        eval(
            document(),
            r#"
            const unicodeWhitespace = [
                0x000b, 0x0085, 0x00a0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003,
                0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028,
                0x2029, 0x202f, 0x205f, 0x3000,
            ].map((code) => String.fromCharCode(code));
            const notWhitespace = [0x180e, 0x200b, 0x200c, 0x200d, 0x2060, 0xfeff]
                .map((code) => String.fromCharCode(code));
            const tokens = unicodeWhitespace.concat(notWhitespace);
            const host = document.createElement('div');
            document.body.appendChild(host);
            const spans = tokens.map((token) => {
                const span = document.createElement('span');
                span.setAttribute('class', token);
                host.appendChild(span);
                return span;
            });
            const found = tokens.filter((token, index) => {
                const got = document.getElementsByClassName(token);
                return got.length === 1 && got[0] === spans[index];
            });
            const twoNames = String.fromCharCode(0x0061, 0x00a0, 0x0062);
            const multi = document.createElement('span');
            multi.setAttribute('class', twoNames);
            host.appendChild(multi);
            [
                found.length,
                tokens.length,
                document.getElementsByClassName('a').length,
                document.getElementsByClassName(twoNames).length,
                document.getElementsByClassName('').length,
            ].join('|');
            "#,
        ),
        // All 26 tokens are found. `"a"` is *not* a token of a class of
        // `a`+U+00A0+`b` (U+00A0 is not whitespace to the DOM), and the whole
        // string is; the empty argument matches nothing.
        "26|26|0|1|0"
    );
}
