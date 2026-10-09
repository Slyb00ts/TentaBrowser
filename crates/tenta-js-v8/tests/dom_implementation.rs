//! Tests for the document-side surface `dom/nodes` was failing on:
//! `document.defaultView`, `document.doctype`,
//! `document.implementation` (+ `hasFeature`, `createDocumentType`,
//! `createDocument`, `createHTMLDocument`) and
//! `document.createProcessingInstruction`.
//!
//! The five failure buckets these cover, from the WPT run recorded in the
//! package brief, were (in order of how many subtests they cost) a
//! `document.implementation` that was `undefined`, an implementation without
//! `hasFeature` or `createHTMLDocument`, a `document.defaultView` that was
//! `undefined`, and a `createProcessingInstruction` that was not a function.
//!
//! Each test names the WPT file whose rule it pins, and each one is written so
//! that deleting the implementation makes it fail rather than pass vacuously:
//! the assertion is on a value read back out of the tree or on an identity
//! comparison between two objects the engine built, never on "no exception was
//! thrown".

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::{ParseStatus, Parser, ParserOptions};
use tenta_js_v8::Engine;
use url::Url;

/// `<html><head></head><body></body></html>` over a document with `url`.
fn document(url: Option<&str>) -> Rc<RefCell<Document>> {
    let mut doc = match url {
        Some(url) => Document::with_url(Url::parse(url).expect("the test URL parses")),
        None => Document::new(),
    };
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

/// Parses `html` into a fresh document, so a test can start from markup.
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

// ------------------------------------------------------------ defaultView

#[test]
fn the_default_view_of_a_page_document_is_the_window_object() {
    // WPT `dom/nodes/Document-defaultView.html` asserts through
    // `assert_equals(document.defaultView, window)`, and the whole point of the
    // member is that identity: a fresh object standing in for the window would
    // satisfy `typeof === 'object'` and fail this.
    assert_eq!(
        eval(document(None), "document.defaultView === window"),
        "true"
    );
    // The other aliases must agree too, because they are the same object.
    // (`String(...)` on the window is `[object EventTarget]` rather than
    // `[object Window]`, because this tree has no `Window` interface descriptor
    // to carry a `Symbol.toStringTag` — a separate, pre-existing gap; what this
    // test needs is that the alias resolves to the *global object*, which the
    // three identities above already prove.)
    assert_eq!(
        eval(
            document(None),
            r#"
            [
                document.defaultView === self,
                document.defaultView === globalThis,
                document.defaultView.document === document,
                document.defaultView === top,
            ].join('|');
        "#,
        ),
        "true|true|true|true"
    );
}

#[test]
fn a_minted_document_has_no_default_view() {
    // WPT `dom/nodes/DOMImplementation-createHTMLDocument.html` checks
    // `doc.defaultView === null` for the document it just created, and the DOM
    // Standard says the same: `defaultView` is "the window of the browsing
    // context this document is the active document of", and a document a
    // `DOMImplementation` built belongs to no browsing context at all.
    //
    // The engine answers this from the document's *slot*: slot 0 is the
    // document the wrapper world was built over (the navigated one) and every
    // minted document gets a slot of its own, so the test is "is this the
    // world's page document" rather than "is this document non-null". The
    // comparison with the same expression on the page's document is what makes
    // the assertion non-vacuous: a getter that returned `null` for everything
    // fails the previous test, and one that returned the window for everything
    // fails this one.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createHTMLDocument();
            [
                created.defaultView,
                String(created.defaultView),
                document.defaultView === null,
            ].join('|');
        "#,
        ),
        "|null|false"
    );
}

#[test]
fn a_document_created_by_create_document_also_has_no_default_view() {
    // The other constructor path to a detached document must agree, because
    // both go through `mint_document` and so both land on a non-zero slot.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createDocument(null, 'root', null);
            created.defaultView === null;
        "#,
        ),
        "true"
    );
}

// ----------------------------------------------------------- doctype

#[test]
fn doctype_is_the_documents_own_doctype() {
    // WPT `dom/nodes/Document-doctype.html`: `document.doctype` is the child
    // doctype, and its three members read the parsed values. The values come
    // from the parser, so a getter returning a fresh object would fail the
    // identity assertion below and a constant one would fail the name.
    assert_eq!(
        eval(
            parse("<!DOCTYPE html><html><head></head><body></body></html>"),
            r#"
            const doctype = document.doctype;
            [
                doctype.name,
                doctype.publicId,
                doctype.systemId,
                doctype.nodeType,
                doctype.ownerDocument === document,
                doctype === document.firstChild,
                String(doctype),
            ].join('|');
        "#,
        ),
        "html|||10|true|true|[object DocumentType]"
    );
}

#[test]
fn doctype_is_null_without_one_and_reads_public_and_system_ids() {
    assert_eq!(eval(document(None), "document.doctype"), "null");
    assert_eq!(
        eval(
            parse(
                r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html><head></head><body></body></html>"#
            ),
            r#"
            [
                document.doctype.name,
                document.doctype.publicId,
                document.doctype.systemId,
            ].join('|');
        "#,
        ),
        "html|-//W3C//DTD XHTML 1.0 Strict//EN|http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"
    );
}

// ------------------------------------------------------ implementation

#[test]
fn implementation_is_the_same_object_every_time() {
    // `[SameObject] readonly attribute DOMImplementation implementation` — the
    // attribute is declared `[SameObject]`, so two reads must hand back one
    // object. Two distinct readings are what a per-call allocation gives.
    assert_eq!(
        eval(
            document(None),
            r#"
            [
                document.implementation === document.implementation,
                typeof document.implementation.hasFeature,
                String(document.implementation),
            ].join('|');
        "#,
        ),
        "true|function|[object DOMImplementation]"
    );
}

#[test]
fn a_second_documents_implementation_is_a_different_object() {
    // WPT
    // `dom/nodes/DOMImplementation-createHTMLDocument-with-saved-implementation.html`
    // holds an implementation from before creating a document and uses it
    // afterwards; the identity rule underneath is that the implementation
    // belongs to *its* document. A session-global singleton would pass the
    // `[SameObject]` test above and fail this one.
    assert_eq!(
        eval(
            document(None),
            r#"
            const page = document.implementation;
            const other = document.implementation.createHTMLDocument();
            [
                page === other.implementation,
                document === other,
                String(other.implementation),
            ].join('|');
        "#,
        ),
        "false|false|[object DOMImplementation]"
    );
}

#[test]
fn has_feature_always_returns_true() {
    // WPT `dom/nodes/DOMImplementation-hasFeature.html` runs 130 argument lists
    // through `assert_equals(..., true)`, via
    // `hasFeature.apply(implementation, data)` where `data` has zero, one or two
    // elements. The DOM Standard therefore declares the method with *no*
    // parameters ("useless; always returns true"), so extra arguments are
    // simply ignored and a short call is not an arity error. Declaring the two
    // parameters the standard's prose mentions would turn the six 0- and
    // 1-argument rows into `TypeError`s, which is why this test calls it with
    // three different arities: it is the arity contract, not just the return
    // value, that the WPT file pins.
    assert_eq!(
        eval(
            document(None),
            r#"
            const impl = document.implementation;
            [
                impl.hasFeature(),
                impl.hasFeature('Core'),
                impl.hasFeature('HTML', '1.0'),
                impl.hasFeature.apply(impl, []),
                impl.hasFeature.apply(impl, ['XML']),
                impl.hasFeature.apply(impl, ['This is filler text.', '']),
                impl.hasFeature('', ''),
                impl.hasFeature('bogus', '99.9'),
                impl.hasFeature('Core', undefined),
                impl.hasFeature('Core', null),
                impl.hasFeature.length,
            ].join('|');
        "#,
        ),
        "true|true|true|true|true|true|true|true|true|true|0"
    );
}

// ------------------------------------------------ createDocumentType

#[test]
fn create_document_type_builds_a_real_doctype_node() {
    // WPT `dom/nodes/DOMImplementation-createDocumentType.html`: the returned
    // object is a `DocumentType` node owned by the implementation's document,
    // carrying the three strings it was given.
    assert_eq!(
        eval(
            document(None),
            r#"
            const doctype = document.implementation.createDocumentType(
                'html',
                '-//W3C//DTD HTML 4.01//EN',
                'http://www.w3.org/TR/html4/strict.dtd'
            );
            [
                doctype.name,
                doctype.publicId,
                doctype.systemId,
                doctype.nodeType,
                doctype.ownerDocument === document,
                doctype.parentNode,
                doctype instanceof DocumentType,
            ].join('|');
        "#,
        ),
        "html|-//W3C//DTD HTML 4.01//EN|http://www.w3.org/TR/html4/strict.dtd|10|true||true"
    );
}

#[test]
fn create_document_type_refuses_a_name_the_parser_could_not_read_back() {
    // WPT `dom/nodes/DOMImplementation-createDocumentType.html` expects an
    // `InvalidCharacterError` for a qualified name that contains a character
    // the doctype production cannot carry. The name is written with
    // `fromCharCode` so no raw control character appears in this file.
    assert_eq!(
        eval(
            document(None),
            r#"
            function throws(name) {
                try {
                    document.implementation.createDocumentType(name, '', '');
                    return 'no throw';
                } catch (e) {
                    return e.name;
                }
            }
            [
                throws('a' + String.fromCharCode(0x3e) + 'b'),
                throws('a' + String.fromCharCode(0) + 'b'),
                throws('a b'),
                throws(''),
            ].join('|');
        "#,
        ),
        "InvalidCharacterError|InvalidCharacterError|InvalidCharacterError|no throw"
    );
}

// --------------------------------------------------- createDocument

#[test]
fn create_document_builds_a_document_with_its_root_element() {
    // WPT `dom/nodes/DOMImplementation-createDocument.html`: the returned
    // document has the named root element in the given namespace, and a
    // document element identity of its own.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createDocument(
                'http://www.w3.org/1999/xhtml', 'root', null
            );
            [
                created.nodeType,
                created.documentElement.tagName,
                created.documentElement.localName,
                created.documentElement.namespaceURI,
                created.documentElement === document.documentElement,
                created.documentElement.ownerDocument === created,
                created.doctype,
                created.URL,
                created.contentType,
                created.firstChild === created.documentElement,
            ].join('|');
        "#,
        ),
        // `createDocument` returns an XML document. Even an element in the
        // HTML namespace keeps its qualified name's case in an XML document.
        "9|root|root|http://www.w3.org/1999/xhtml|false|true||about:blank|application/xhtml+xml|true"
    );
}

#[test]
fn create_document_with_a_null_name_has_no_document_element() {
    // WPT `dom/nodes/DOMImplementation-createDocument.html`, "null qualified
    // name": the root element is only created for a non-empty name, and the
    // document then has no document element at all.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createDocument(null, '', null);
            [created.documentElement, created.childNodes.length].join('|');
        "#,
        ),
        "|0"
    );
}

#[test]
fn create_document_with_missing_arguments_throws_a_type_error() {
    // WPT `dom/nodes/DOMImplementation-createDocument.html`, "createDocument
    // with missing arguments": `createDocument()` and `createDocument('')` must
    // throw a `TypeError` — the arity check, not a DOM exception. The third
    // field shows the other side of the same rule: `doctype` is `optional`, so
    // a two-argument call is accepted, which is what
    // `dom/nodes/name-validation.html` relies on when it calls
    // `createDocument('namespaceuri', name)`.
    assert_eq!(
        eval(
            document(None),
            r#"
            function throws() {
                try {
                    document.implementation.createDocument.apply(
                        document.implementation, arguments
                    );
                    return 'no throw';
                } catch (e) {
                    return e instanceof TypeError ? 'TypeError' : 'other';
                }
            }
            [
                throws(),
                throws(''),
                throws('http://example.com/', 'root'),
            ].join('|');
        "#,
        ),
        "TypeError|TypeError|no throw"
    );
}

#[test]
fn create_document_validates_the_root_element_name_like_create_element_ns() {
    // WPT `dom/nodes/name-validation.html`: the createDocument half of the
    // createElementNS test runs the same name set through both and expects the
    // same `InvalidCharacterError`s. The name here starts with a character that
    // is not an ASCII letter, `_` or `:`.
    assert_eq!(
        eval(
            document(None),
            r#"
            function throws(name) {
                try {
                    document.implementation.createDocument('namespaceuri', name);
                    return 'accepted';
                } catch (e) {
                    return e.name;
                }
            }
            [throws('7name'), throws('-name'), throws('a name'), throws('a:b')].join('|');
        "#,
        ),
        "InvalidCharacterError|InvalidCharacterError|InvalidCharacterError|accepted"
    );
}

#[test]
fn create_document_content_type_follows_the_namespace() {
    // WPT `dom/nodes/DOMImplementation-createDocument.html`'s metadata half:
    // the content type is the one the namespace names, and everything else is
    // `application/xml`.
    assert_eq!(
        eval(
            document(None),
            r#"
            const impl = document.implementation;
            [
                impl.createDocument(null, 'root').contentType,
                impl.createDocument('http://www.w3.org/1999/xhtml', 'root').contentType,
                impl.createDocument('http://www.w3.org/2000/svg', 'root').contentType,
                impl.createDocument('foo', 'root').contentType,
                impl.createDocument(null, 'root').compatMode,
                impl.createDocument(null, 'root').characterSet,
                impl.createDocument(null, 'root').createElement('DIV').localName,
            ].join('|');
        "#,
        ),
        "application/xml|application/xhtml+xml|image/svg+xml|application/xml\
         |CSS1Compat|UTF-8|DIV"
    );
}

#[test]
fn create_document_carries_the_given_doctype_over_to_the_new_document() {
    // WPT `dom/nodes/DOMImplementation-createDocument.html`, "doctype":
    // `doc.doctype` must be present, owned by the new document, first in the
    // tree, and `doc.childNodes.length` must count it.
    //
    // This engine has no adopt machinery, so the doctype is reproduced in the
    // new document's arena rather than moved — the third deviation recorded in
    // `idl/dom/dom.webidl`. The identity of the *object* therefore differs from
    // the argument, which is the one WPT assertion these rows cannot satisfy;
    // the fields below are the ones that must hold, and `doctype !==
    // created.doctype` is asserted explicitly so the deviation is pinned rather
    // than left implicit.
    assert_eq!(
        eval(
            document(None),
            r#"
            const doctype = document.implementation.createDocumentType('html', '', '');
            const created = document.implementation.createDocument(
                'http://www.w3.org/1999/xhtml', 'html', doctype
            );
            [
                created.doctype !== null,
                created.doctype.name,
                created.doctype.ownerDocument === created,
                created.doctype.ownerDocument === document,
                created.doctype === doctype,
                created.firstChild === created.doctype,
                created.firstChild.nextSibling === created.documentElement,
                created.childNodes.length,
            ].join('|');
        "#,
        ),
        "true|html|true|false|false|true|true|2"
    );
}

#[test]
fn create_document_reproduces_a_doctype_that_already_has_a_parent() {
    // `DOMImplementation-createDocument.html` passes doctypes that are already
    // associated with a document — the page's own `document.doctype` among them
    // — and expects no exception: the standard's step is an *append*, which
    // adopts the node out of its old tree, and none of its steps are fallible.
    // This engine reproduces the doctype instead of moving it, so the page keeps
    // its own doctype as well; refusing the call, which an earlier revision of
    // this code did, would fail every assertion in those rows rather than the
    // single identity one.
    assert_eq!(
        eval(
            parse("<!DOCTYPE html><html><head></head><body></body></html>"),
            r#"
            const created = document.implementation.createDocument(
                null, 'root', document.doctype
            );
            [
                created.doctype.name,
                created.doctype.ownerDocument === created,
                created.doctype === document.doctype,
                document.doctype === null,
                created.documentElement.tagName,
                created.childNodes.length,
            ].join('|');
        "#,
        ),
        "html|true|false|false|root|2"
    );
}

// ------------------------------------------------ createHTMLDocument

#[test]
fn create_html_document_builds_the_html_tree() {
    // WPT `dom/nodes/DOMImplementation-createHTMLDocument.html`: the created
    // document is `<!doctype html><html><head></head><body>`, its URL is
    // `about:blank`, and its encoding is UTF-8 whatever the caller's is.
    assert_eq!(
        eval(
            parse("<!DOCTYPE html><html><head></head><body></body></html>"),
            r#"
            const created = document.implementation.createHTMLDocument();
            [
                created.nodeType,
                created.doctype.name,
                created.documentElement.tagName,
                created.documentElement.namespaceURI,
                created.head.tagName,
                created.body.tagName,
                created.title,
                created.characterSet,
                created.URL,
                created.documentElement === document.documentElement,
            ].join('|');
        "#,
        ),
        "9|html|HTML|http://www.w3.org/1999/xhtml|HEAD|BODY||UTF-8|about:blank|false"
    );
}

#[test]
fn create_html_documents_are_distinct_documents() {
    // The point of "a real document": two calls must not share a document
    // element, a head or a body, and neither may be the calling document. An
    // implementation returning the same document twice, or a shallow wrapper
    // over `document`, fails at least one of these.
    assert_eq!(
        eval(
            document(None),
            r#"
            const first = document.implementation.createHTMLDocument();
            const second = document.implementation.createHTMLDocument();
            [
                first === second,
                first.documentElement === second.documentElement,
                first.head === second.head,
                first.body === second.body,
                first.documentElement === document.documentElement,
                first.head === document.head,
                first.body === document.body,
                first.body.ownerDocument === first,
            ].join('|');
        "#,
        ),
        "false|false|false|false|false|false|false|true"
    );
}

#[test]
fn a_page_can_hold_more_documents_than_a_byte_of_slot_would_allow() {
    // The document slot in a handle was eight bits until this slice, so the
    // 256th document a page minted took the process down: the slot field
    // overflowed inside a V8 callback, where the panic could not unwind, and
    // the abort killed every test the WPT driver had in flight. The widest
    // `dom/nodes` file is what found it — `DOMImplementation-createDocument.html`
    // runs three `createDocument` calls for each of the 105 rows that expect no
    // exception, over four hundred documents in one page.
    //
    // The documents have to stay *usable* and not merely be returned: a wider
    // slot whose bits collided would resolve handles in the wrong arena, so
    // every document is asked for the node only it owns.
    assert_eq!(
        eval(
            document(None),
            r#"
            let usable = 0;
            for (let i = 0; i < 600; i++) {
                const doc = document.implementation.createHTMLDocument('doc ' + i);
                const title = doc.head.firstChild;
                if (doc.documentElement !== document.documentElement
                    && title.textContent === 'doc ' + i
                    && doc.body.ownerDocument === doc) {
                    usable++;
                }
            }
            usable;
        "#,
        ),
        "600"
    );
}

#[test]
fn exhausting_the_document_slots_is_a_catchable_error_and_not_an_abort() {
    // The other half of the same defect: the ceiling must be reachable only by
    // a page that asks for more documents than the field can name, and reaching
    // it must throw. A page that mints in a loop is a page, not a crash: the
    // engine's own `minted` retention is what has to stay bounded, and the page
    // gets a `DOMException` for the document past the ceiling.
    //
    // `MAX_DOCUMENT_SLOT` is the width of the field, so this asserts the exact
    // ceiling rather than "eventually throws": a layout change that quietly
    // halved the capacity would fail here.
    let ceiling = tenta_js::bindings::host::MAX_DOCUMENT_SLOT;
    let source = format!(
        r#"
        let made = 0;
        let refused = null;
        for (let i = 0; i < {budget}; i++) {{
            try {{
                document.implementation.createHTMLDocument('x');
                made++;
            }} catch (error) {{
                refused = error.name + ':' + (error instanceof DOMException);
                break;
            }}
        }}
        made + '|' + refused;
        "#,
        budget = u32::from(ceiling) + 8
    );
    assert_eq!(
        eval(document(None), &source),
        format!("{ceiling}|QuotaExceededError:true")
    );
}

#[test]
fn a_node_of_another_document_is_never_indexed_in_this_arena() {
    // The second half of the same defect. A handle names its document only
    // through its slot, so a node minted by `createHTMLDocument` carries a node
    // id that means something else entirely in the page's arena — or nothing at
    // all, when it is past the end. Either answer is unacceptable, and the one
    // the arena gave was a panic out of `Document::parent`, inside a V8
    // callback, which aborted the WPT driver's process:
    //
    //     panicked at crates/tenta-dom/src/document.rs:716:32:
    //     fatal runtime error: failed to initiate panic, error 5, aborting
    //
    // `contains` is the member that found it (`Node-contains-xml.xml` and every
    // `assert_true(node.contains(...))` over two documents): a node of another
    // tree is not an inclusive ancestor of anything here, so the answer is
    // `false` and the walk never starts.
    //
    // The tree is then checked for *corruption*, not just for a return value:
    // the page's body must have exactly the one child it started with, and that
    // child must still be the node it was. A wrong-node write inside the arena
    // would show up here and nowhere else.
    assert_eq!(
        eval(
            document(None),
            r#"
            const foreign = document.implementation.createHTMLDocument();
            const div = foreign.createElement('div');
            foreign.body.appendChild(div);
            const pageDiv = document.createElement('div');
            document.body.appendChild(pageDiv);
            const answers = [
                document.contains(div),
                pageDiv.contains(div),
                div.contains(pageDiv),
                document.body.contains(document.body),
                document.body.contains(pageDiv),
            ];
            const failures = [];
            for (const [name, call] of [
                ['appendChild', () => document.body.appendChild(div)],
                ['insertBefore', () => document.body.insertBefore(div, pageDiv)],
                ['replaceChild', () => document.body.replaceChild(div, pageDiv)],
                ['removeChild', () => document.body.removeChild(div)],
            ]) {
                try {
                    call();
                    failures.push(name + ':no throw');
                } catch (error) {
                    failures.push(name + ':' + error.name);
                }
            }
            [
                answers.join(','),
                failures.join(' '),
                document.body.childNodes.length,
                document.body.firstChild === pageDiv,
                div.parentNode === foreign.body,
                div.ownerDocument === foreign,
            ].join('|');
        "#,
        ),
        // `contains` answers false for every foreign pair and true for the two
        // that are this tree's.
        //
        // The insertions throw because the standard *adopts* the foreign node
        // into the parent's document as part of inserting it, and this engine
        // has no adoption: `dom/nodes/Node-mutation-adoptNode.html` asserts
        // `div.ownerDocument === document` after exactly the `appendChild`
        // above, which is the subtests this records as failing rather than
        // answering with a corrupted tree. `removeChild` is a plain
        // `NotFoundError`, which is the answer the standard gives too — the node
        // is not a child of this parent.
        "false,false,false,true,true|\
         appendChild:NotSupportedError insertBefore:NotSupportedError \
         replaceChild:NotSupportedError removeChild:NotFoundError|\
         1|true|true|true"
    );
}

#[test]
fn create_html_document_writes_the_title_it_was_given() {
    // WPT `dom/nodes/DOMImplementation-createHTMLDocument.html`, "title": the
    // argument becomes a `<title>` element in the head whose text child is the
    // argument verbatim — the standard's strip-and-collapse applies to the
    // `document.title` *getter*, not to this write.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createHTMLDocument('foo  bar baz');
            const title = created.head.firstChild;
            [
                title.tagName,
                title.namespaceURI,
                title.firstChild.data,
                title === created.head.lastChild,
                created.head.childNodes.length,
                created.firstChild === created.doctype,
            ].join('|');
        "#,
        ),
        "TITLE|http://www.w3.org/1999/xhtml|foo  bar baz|true|1|true"
    );
}

#[test]
fn create_html_document_omits_the_title_element_when_not_asked_for_one() {
    // "optional DOMString title": called with no argument, the head is empty.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createHTMLDocument();
            [created.head.childNodes.length, created.title].join('|');
        "#,
        ),
        "0|"
    );
}

#[test]
fn a_minted_document_is_fully_usable_as_a_document() {
    // These documents are not decoration: script operates on them. Creating
    // nodes inside one, appending them, and reading the tree back is what the
    // WPT files that saved an implementation and used it later do, and it is
    // what proves the minted arena is reached through the handle rather than
    // the operation silently running on the page's document.
    assert_eq!(
        eval(
            document(None),
            r#"
            const created = document.implementation.createHTMLDocument();
            const div = created.createElement('div');
            div.textContent = 'inside';
            created.body.appendChild(div);
            [
                created.body.firstChild === div,
                created.body.textContent,
                div.ownerDocument === created,
                document.body.textContent,
                created.querySelector('div') === div,
                created.getElementsByTagName('div').length,
            ].join('|');
        "#,
        ),
        "true|inside|true||true|1"
    );
}

// ---------------------------------- createProcessingInstruction

#[test]
fn create_processing_instruction_builds_a_real_node() {
    // WPT `dom/nodes/Document-createProcessingInstruction.html`: the returned
    // node is a `ProcessingInstruction` with the given target and data, it is
    // `CharacterData` (so `data`/`length` work), and it is *not* in the tree.
    assert_eq!(
        eval(
            document(None),
            r#"
            const pi = document.createProcessingInstruction('xml-stylesheet', 'href="a"');
            [
                pi.target,
                pi.data,
                pi.length,
                pi.nodeType,
                pi.nodeName,
                pi.ownerDocument === document,
                pi.parentNode,
                pi instanceof ProcessingInstruction,
                pi instanceof CharacterData,
                pi instanceof Node,
            ].join('|');
        "#,
        ),
        "xml-stylesheet|href=\"a\"|8|7|xml-stylesheet|true||true|true|true"
    );
}

#[test]
fn a_processing_instructions_data_is_mutable_through_character_data() {
    // The node kind already existed in the arena but its `data` was not
    // reachable: `CharacterData`'s accessors guarded on "is a text-ish node"
    // and a processing instruction was not one. Reading back a write is what
    // proves the six splice entry points reach the arena's `pis` slot rather
    // than silently doing nothing.
    //
    // A processing instruction is `CharacterData` *itself*, not a container, so
    // it has no child node: the `firstChild === null` field is the standard's
    // shape, not a missing feature.
    assert_eq!(
        eval(
            document(None),
            r#"
            const pi = document.createProcessingInstruction('target', 'before');
            pi.appendData('-after');
            const first = pi.data;
            pi.deleteData(0, 6);
            const second = pi.data;
            pi.insertData(0, 'X');
            const third = pi.data;
            [
                first,
                second,
                third,
                pi.length,
                pi.childNodes.length,
                String(pi.firstChild),
                pi.nodeValue,
            ].join('|');
        "#,
        ),
        "before-after|-after|X-after|7|0|null|X-after"
    );
}

#[test]
fn create_processing_instruction_refuses_a_data_containing_the_terminator() {
    // The DOM Standard: the data must not contain `?>`, because the XML
    // serialization would then produce a node the parser cannot read back.
    assert_eq!(
        eval(
            document(None),
            r#"
            try {
                document.createProcessingInstruction('target', 'a?>b');
                'no throw';
            } catch (e) {
                e.name + '|' + (e instanceof DOMException);
            }
        "#,
        ),
        "InvalidCharacterError|true"
    );
}

#[test]
fn create_processing_instruction_refuses_a_target_that_is_not_an_xml_name() {
    // The target rule is XML's `Name` production, and the cases below are the
    // ones `dom/nodes/Document-createProcessingInstruction.js` lists, split the
    // way that file splits them.
    //
    // Invalid: a leading digit, a leading hyphen, an embedded space, a form
    // feed, a backslash, U+00B7 in first position (a `NameChar` but not a
    // `NameStartChar`), U+00D7 anywhere (neither), and the number `0`, which
    // the DOMString conversion turns into the digit string.
    //
    // Valid, and the reason the rule cannot be "letters only": `xml:fail` — a
    // colon is a `NameStartChar` in XML 1.0's `Name` (it is `NCName` that
    // excludes it) — `A·A`, and `a0`.
    //
    // Every non-ASCII or control character is spelled with `fromCharCode` so
    // none of them appears raw in this file.
    assert_eq!(
        eval(
            document(None),
            r#"
            const B7 = String.fromCharCode(0x00b7);
            const D7 = String.fromCharCode(0x00d7);
            const FF = String.fromCharCode(0x0c);
            const BS = String.fromCharCode(0x5c);
            function throws(target) {
                try {
                    document.createProcessingInstruction(target, 'x');
                    return 'accepted';
                } catch (e) {
                    return e.name;
                }
            }
            function accepts(target) {
                const pi = document.createProcessingInstruction(target, 'x');
                return [pi.target === target, pi.data].join('=');
            }
            [
                throws(B7 + 'A'),
                throws(D7 + 'A'),
                throws('A' + D7),
                throws(BS + 'A'),
                throws(FF),
                throws(0),
                throws('0'),
                accepts('xml:fail'),
                accepts('A' + B7 + 'A'),
                accepts('a0'),
            ].join('|');
        "#,
        ),
        "InvalidCharacterError|InvalidCharacterError|InvalidCharacterError\
         |InvalidCharacterError|InvalidCharacterError|InvalidCharacterError\
         |InvalidCharacterError|true=x|true=x|true=x"
    );
}

#[test]
fn a_processing_instruction_survives_being_serialized() {
    // A processing instruction created by script and inserted into the tree is
    // part of the page: the serializer must emit it, which is the observable
    // consequence of the node being a real tree node rather than a lone host.
    //
    // The HTML fragment serialization algorithm spells a processing
    // instruction `<?` target SPACE data `>` — one `>`, no `?` — while the XML
    // algorithm (a different function in `tenta_dom::serialize`) spells it
    // `...?>`; the two are deliberately different, so this pins the HTML one.
    assert_eq!(
        eval(
            parse("<!DOCTYPE html><html><head></head><body></body></html>"),
            r#"
            const pi = document.createProcessingInstruction('xml-stylesheet', 'href="a"');
            document.documentElement.insertBefore(pi, document.documentElement.firstChild);
            [
                document.documentElement.outerHTML,
                String(pi.previousSibling),
                pi.nextSibling.tagName,
                document.documentElement.childNodes.length,
            ].join('|');
        "#,
        ),
        "<html><?xml-stylesheet href=\"a\"><head></head><body></body></html>|null|HEAD|3"
    );
}
