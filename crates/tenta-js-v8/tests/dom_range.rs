//! `AbstractRange`, `Range`, `StaticRange` and `Document.createRange` through
//! the generated V8 glue (`docs/spec/05-javascript.md` §3), plus the two
//! `Document` factories and the `ParentNode` inclusion this slice landed with
//! them.
//!
//! The stringifier test is a direct port of `wpt/checkout/dom/ranges/
//! Range-stringifier.html`: the same three `<div>`s with the same newline text
//! nodes between them, the same four cases and the same expected strings. The
//! other tests cover what that file does not reach — the four boundary-point
//! comparisons, the tree-order members, the refusals, and the `StaticRange`
//! constructor's copying semantics.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::Atom;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js::engine::PageEngine;
use tenta_js_v8::Engine;

/// A document of the same shape `Range-stringifier.html` uses:
/// `document -> doctype, html -> body -> div#test("Test div"), "\n",
/// div#another("Another div"), "\n", div#last("Last div")`.
///
/// Built node by node, like the WPT file's own source: the newline text nodes
/// between the divs are load-bearing, because they are what the stringifier's
/// third case reports.
fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let doctype = document.create_doctype("html", "", "");
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    for child in [doctype, html] {
        document
            .mutate(Mutation::Append {
                parent: root,
                child,
            })
            .expect("append a child of the document node");
    }
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    let entries = [
        ("test", "Test div"),
        ("another", "Another div"),
        ("last", "Last div"),
    ];
    let last = entries.len() - 1;
    for (index, (id, text)) in entries.into_iter().enumerate() {
        let div = document.create_html_element(atoms::DIV);
        document
            .mutate(Mutation::Append {
                parent: body,
                child: div,
            })
            .expect("append a div");
        if index != last {
            let newline = document.create_text("\n");
            document
                .mutate(Mutation::Append {
                    parent: body,
                    child: newline,
                })
                .expect("append the newline between the divs");
        }
        let name = QualName::new(atoms::EMPTY, Atom::new("id"));
        document
            .mutate(Mutation::SetAttribute {
                element: div,
                name,
                value: id.to_owned(),
            })
            .expect("set the div's id");
        let text = document.create_text(text);
        document
            .mutate(Mutation::Append {
                parent: div,
                child: text,
            })
            .expect("append the div's text");
    }
    Rc::new(RefCell::new(document))
}

/// Evaluates `source` against a fresh engine over the fixture.
fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("the script evaluates without throwing")
}

#[test]
fn setting_either_endpoint_in_another_root_collapses_the_range() {
    assert_eq!(
        eval(
            r#"
        const range = document.createRange();
        const a = document.createElement('div');
        const b = document.createElement('div');
        a.appendChild(document.createTextNode('alpha'));
        b.appendChild(document.createTextNode('beta'));
        const checks = [];
        range.setEnd(a, 1);
        checks.push(range.collapsed, range.startContainer === a, range.startOffset === 1);
        range.setStart(b.firstChild, 2);
        checks.push(range.collapsed, range.endContainer === b.firstChild, range.endOffset === 2);
        range.setEnd(a.firstChild, 0);
        checks.push(range.collapsed, range.commonAncestorContainer === a.firstChild);
        range.setStart(document.body, 0);
        checks.push(range.collapsed, range.endContainer === document.body);
        checks.every(Boolean)
    "#
        ),
        "true"
    );
}

#[test]
fn stringifier_does_not_duplicate_start_text_and_uses_utf16_offsets() {
    assert_eq!(
        eval(
            r#"
        const parent = document.createElement('div');
        const first = document.createTextNode('A😀B');
        const middle = document.createTextNode('middle');
        const last = document.createTextNode('XYZ');
        parent.appendChild(first);
        parent.appendChild(middle);
        parent.appendChild(last);
        const range = document.createRange();
        range.setStart(first, 0);
        range.setEnd(last, 2);
        const values = [String(range)];
        range.setStart(first, 1);
        values.push(String(range));
        range.setEnd(first, 3);
        values.push(String(range));
        range.setStart(first, 3);
        values.push(String(range));
        values.join('|')
    "#
        ),
        "A😀BmiddleXY|😀BmiddleXY|😀|"
    );
}

#[test]
fn live_ranges_follow_insertions_removals_and_subtree_removal() {
    assert_eq!(
        eval(
            r#"
        const parent = document.createElement('div');
        const nodes = [0, 1, 2].map(i => document.createElement('p'));
        nodes.forEach(node => parent.appendChild(node));
        const range = document.createRange();
        range.setStart(parent, 1);
        range.setEnd(parent, 3);
        const clone = range.cloneRange();
        parent.insertBefore(document.createElement('i'), nodes[1]);
        const values = [range.startOffset, range.endOffset, clone.endOffset];
        parent.removeChild(nodes[0]);
        values.push(range.startOffset, range.endOffset, clone.endOffset);
        const text = document.createTextNode('abcdef');
        nodes[1].appendChild(text);
        range.setStart(text, 2);
        range.setEnd(text, 5);
        const snapshot = new StaticRange({startContainer: text, startOffset: 2,
                                         endContainer: text, endOffset: 5});
        parent.removeChild(nodes[1]);
        values.push(range.startContainer === parent, range.endContainer === parent,
                    range.startOffset, range.endOffset, range.collapsed,
                    snapshot.startContainer === text, snapshot.startOffset, snapshot.endOffset);
        values.join('|')
    "#
        ),
        "1|4|4|0|3|3|true|true|1|1|true|true|2|5"
    );
}

#[test]
fn character_data_mutations_adjust_live_but_not_static_offsets() {
    assert_eq!(
        eval(
            r#"
        const text = document.createTextNode('A😀BCDEF');
        const range = document.createRange();
        range.setStart(text, 3);
        range.setEnd(text, 7);
        const snapshot = new StaticRange({startContainer: text, startOffset: 3,
                                         endContainer: text, endOffset: 7});
        const values = [];
        text.insertData(1, 'xy');
        values.push(range.startOffset, range.endOffset);
        text.replaceData(3, 3, 'z');
        values.push(range.startOffset, range.endOffset);
        text.deleteData(2, 0xffff_ffff);
        values.push(range.startOffset, range.endOffset);
        text.appendData('tail');
        values.push(range.startOffset, range.endOffset);
        text.data = 'replacement';
        values.push(range.startOffset, range.endOffset);
        values.push(snapshot.startOffset, snapshot.endOffset);
        values.join('|')
    "#
        ),
        "5|9|3|7|2|2|2|2|0|0|3|7"
    );
}

#[test]
fn replacing_children_and_self_insertions_keep_ranges_valid() {
    assert_eq!(
        eval(
            r#"
        const parent = document.createElement('div');
        const a = document.createElement('a');
        const b = document.createElement('b');
        const c = document.createElement('i');
        parent.append(a, b, c);
        const range = document.createRange();
        range.selectNodeContents(b);
        parent.insertBefore(b, b);
        const values = [parent.childNodes.length, parent.childNodes[1] === b,
                        range.startContainer === parent, range.startOffset];
        range.setStart(parent, 1);
        range.setEnd(parent, 3);
        parent.replaceChild(c, b);
        values.push(parent.childNodes.length, parent.lastChild === c,
                    range.startOffset, range.endOffset);
        values.join('|')
    "#
        ),
        "3|true|true|1|2|true|1|1"
    );
}

#[test]
fn live_and_static_ranges_retain_detached_endpoints_until_collection() {
    for static_range in [false, true] {
        let document = test_document();
        let baseline = document.borrow().len();
        let mut engine = Engine::with_document(document.clone());
        let make_range = if static_range {
            "new StaticRange({startContainer: text, startOffset: 1, endContainer: text, endOffset: 3})"
        } else {
            "document.createRange()"
        };
        engine
            .evaluate(&format!(
                r#"
            (() => {{
                const parent = document.createElement('section');
                const text = document.createTextNode('kept');
                parent.appendChild(text);
                globalThis.saved = {make_range};
                if (saved instanceof Range) {{ saved.setStart(text, 1); saved.setEnd(text, 3); }}
            }})();
        "#
            ))
            .expect("create retained range");
        engine.trim_memory();
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
        assert_eq!(document.borrow().len(), baseline + 2);
        assert_eq!(
            engine
                .evaluate(
                    "saved.startContainer.parentNode.nodeName + ':' + saved.endContainer.data"
                )
                .unwrap(),
            "SECTION:kept"
        );
        engine.evaluate("globalThis.saved = null").unwrap();
        engine.trim_memory();
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 2);
        assert_eq!(document.borrow().len(), baseline);
    }
}

#[test]
fn replacing_endpoints_releases_old_nodes_but_a_clone_keeps_them() {
    let document = test_document();
    let mut engine = Engine::with_document(document.clone());
    engine
        .evaluate(
            r#"
        (() => {
            const text = document.createTextNode('old');
            globalThis.range = document.createRange();
            range.selectNodeContents(text);
            globalThis.clone = range.cloneRange();
            range.selectNodeContents(document.body);
        })();
    "#,
        )
        .unwrap();
    engine.trim_memory();
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
    assert_eq!(engine.evaluate("String(clone)").unwrap(), "old");
    engine.evaluate("globalThis.clone = null").unwrap();
    engine.trim_memory();
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
}

#[test]
fn engine_teardown_releases_ranges_with_live_wrappers() {
    let document = test_document();
    let mut engine = Engine::with_document(document.clone());
    engine
        .evaluate(
            r#"
        const text = document.createTextNode('retained');
        globalThis.range = document.createRange();
        range.selectNodeContents(text);
        globalThis.snapshot = new StaticRange({startContainer: text, startOffset: 0,
                                              endContainer: text, endOffset: 8});
    "#,
        )
        .unwrap();
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
    drop(engine);
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
}

#[test]
fn document_append_validates_fragments_and_order_before_changing_the_tree() {
    assert_eq!(
        eval(
            r#"
        const doc = new Document();
        const errors = [];
        const attempt = f => { try { f(); errors.push('accepted'); } catch (e) { errors.push(e.name); } };
        attempt(() => doc.append('text'));
        const fragment = doc.createDocumentFragment();
        fragment.append(doc.createElement('a'), doc.createElement('b'));
        attempt(() => doc.append(fragment));
        errors.push(doc.childNodes.length, fragment.childNodes.length);
        const root = doc.createElement('root');
        doc.append(root);
        attempt(() => doc.append(doc.createElement('extra')));
        const type = doc.implementation.createDocumentType('root', '', '');
        attempt(() => doc.append(type));
        doc.insertBefore(type, root);
        const replacement = doc.createElement('newRoot');
        attempt(() => doc.insertBefore(replacement, type));
        attempt(() => doc.replaceChild(fragment, root));
        errors.push(doc.documentElement === root, fragment.childNodes.length);
        doc.replaceChild(replacement, root);
        errors.push(doc.documentElement === replacement, doc.doctype === type);
        errors.join('|')
    "#
        ),
        "HierarchyRequestError|HierarchyRequestError|0|2|HierarchyRequestError|HierarchyRequestError|HierarchyRequestError|HierarchyRequestError|true|2|true|true"
    );
}

#[test]
fn create_range_is_a_collapsed_range_at_the_document() {
    // `Document.createRange`'s two steps: both endpoints are `(this, 0)`. Without
    // the member the whole script throws, so this cannot pass vacuously.
    assert_eq!(
        eval(
            r#"
        const r = document.createRange();
        [typeof Range, typeof AbstractRange, typeof StaticRange,
         r instanceof Range, r instanceof AbstractRange,
         r.collapsed, r.startContainer === document, r.startOffset,
         r.endContainer === document, r.endOffset].join("|")
    "#
        ),
        "function|function|function|true|true|true|true|0|true|0"
    );
}

#[test]
fn new_range_is_the_same_range_as_create_range_returns() {
    // `new Range()` builds over the realm's document, not over any receiver, so
    // the two factories agree.
    assert_eq!(
        eval(
            r#"
        const a = new Range();
        const b = document.createRange();
        a.constructor === b.constructor && a.collapsed === b.collapsed &&
            a.startContainer === b.startContainer
    "#
        ),
        "true"
    );
}

#[test]
fn stringifier_matches_the_wpt_cases() {
    // The four expectations are `Range-stringifier.html`'s, verbatim.
    assert_eq!(
        eval(
            r#"
        const r = new Range();
        const testDiv = document.getElementById("test");
        const anotherDiv = document.getElementById("another");
        const lastDiv = document.getElementById("last");
        const out = [];
        r.selectNodeContents(testDiv);
        out.push(r.collapsed);
        out.push(r.toString());
        const textNode = testDiv.childNodes[0];
        r.setStart(textNode, 5);
        r.setEnd(textNode, 7);
        out.push(r.toString());
        r.setStart(testDiv, 0);
        r.setEnd(anotherDiv, 0);
        out.push(r.toString());
        r.setStart(textNode, 5);
        r.setEnd(lastDiv.childNodes[0], 4);
        out.push(r.toString());
        out.join("|")
    "#
        ),
        // `selectNodeContents(div)` is not collapsed and stringifies to the div's
        // text; the two-offset case is the substring between them, not the whole
        // text node; the two-`div` case stops at the second div's start and so
        // includes the newline between them; the last crosses three divs.
        "false|Test div|di|Test div\n|div\nAnother div\nLast"
    );
}

#[test]
fn the_boundary_point_comparisons_answer_from_the_live_tree() {
    // The ancestor branches of the boundary-point comparison, which
    // `Range-comparePoint-2.html` and `Range-isPointInRange.html` pin:
    // `(parent, 0)` is before `(parent.firstChild, 0)` and is not equal to it.
    // The four `how` values of `compareBoundaryPoints` are read off
    // `Range-compareBoundaryPoints.html`'s own expectation function.
    assert_eq!(
        eval(
            r#"
        const r = document.createRange();
        const div = document.getElementById("test");
        const other = document.getElementById("another");
        const text = div.childNodes[0];
        const out = [];
        r.setStart(div, 0);
        r.setEnd(div, 1);
        out.push(r.comparePoint(text, 0));
        out.push(r.comparePoint(div, 0));
        out.push(r.comparePoint(div, 1));
        out.push(r.comparePoint(document.body, 5));
        out.push(r.isPointInRange(text, 0));
        out.push(r.isPointInRange(div, 1));
        out.push(r.isPointInRange(document.body, 5));
        out.push(r.isPointInRange(other, 0));
        out.push(r.intersectsNode(text));
        out.push(r.intersectsNode(div));
        out.push(r.intersectsNode(other));
        out.push(r.commonAncestorContainer === div);
        r.collapse(true);
        out.push(r.collapsed, r.startOffset, r.endOffset);
        r.setEnd(div, 1);
        out.push(r.startOffset, r.endOffset);
        r.setStart(div, 1);
        out.push(r.startOffset, r.endOffset);
        r.setEnd(document.body, 5);
        out.push(r.startOffset, r.endOffset);
        out.push(r.compareBoundaryPoints(r.START_TO_START, r));
        out.push(r.compareBoundaryPoints(r.START_TO_END, r));
        out.push(r.compareBoundaryPoints(r.END_TO_END, r));
        out.push(r.compareBoundaryPoints(r.END_TO_START, r));
        // The case the ancestor arms turn on: a point *above* an endpoint whose
        // offset is exactly the index of the child that leads to it. `(div, 0)`
        // is before `(text, 1)` even though the offsets are equal, because the
        // offset is measured among div's children and not inside the text node.
        r.setStart(text, 1);
        r.setEnd(text, 3);
        out.push(r.comparePoint(div, 0));
        out.push(r.comparePoint(div, 1));
        out.push(r.isPointInRange(div, 0));
        out.push(r.isPointInRange(div, 1));
        out.push(r.comparePoint(text, 1));
        out.push(r.comparePoint(text, 4));
        out.push(r.collapsed);
        out.join("|")
    "#
        ),
        // `(text, 0)` is inside `(div, 0)..(div, 1)`, both endpoints are inside,
        // and `(body, 5)` — body's sixth boundary, past its five children — is
        // after the end. `intersectsNode` is true for the div and its text child
        // and false for the sibling outside the range. `collapse(true)` keeps the
        // start; setting a start onto the end drags the end with it, and setting
        // a later end does not drag the start. The four comparisons: start vs
        // start equal, end `(body, 5)` after start `(div, 1)`, end vs end equal,
        // start `(div, 1)` before end `(body, 5)`. The last block brackets
        // `(text, 1)..(text, 3)`: `(div, 0)` is before it and `(div, 1)` is after
        // it — both equal-offset pairs of the first arm — while the two points
        // inside the text node are at the start and past the end.
        "0|0|0|1|true|true|false|false|true|true|false|true|true|0|0|0|1|1|1|1|5|0|1|0|-1|\
         -1|1|false|false|0|1|false"
    );
}

#[test]
fn select_node_reports_the_parent_and_the_index() {
    assert_eq!(
        eval(
            r#"
        const r = new Range();
        const div = document.getElementById("another");
        r.selectNode(div);
        const out = [];
        out.push(r.startContainer === document.body);
        out.push(r.endContainer === document.body);
        out.push(r.startOffset);
        out.push(r.endOffset);
        out.push(r.collapsed);
        r.setStartBefore(div);
        out.push(r.startContainer === document.body);
        out.push(r.startOffset);
        out.push(r.collapsed);
        r.setEndAfter(div);
        out.push(r.endOffset);
        r.collapse(false);
        r.setEndBefore(div);
        out.push(r.startOffset, r.endOffset, r.collapsed);
        r.selectNodeContents(div);
        out.push(r.toString());
        out.join("|")
    "#
        ),
        // `#another` is body's third child — a newline text node lies between it
        // and `#test` — so `selectNode` brackets offsets 2 and 3,
        // `setStartBefore` lands on 2 and leaves the end where it is, and
        // `setEndAfter` lands on 3. Collapsing onto the end and then calling
        // `setEndBefore` is the other direction: the end lands on 2 *before* the
        // start, so the start follows it and the range comes out collapsed at 2.
        "true|true|2|3|false|true|2|false|3|2|2|true|Another div"
    );
}

#[test]
fn a_range_may_be_set_into_a_detached_tree() {
    // The member the standard deliberately does *not* guard with a root check:
    // `Range-commonAncestorContainer-2.html` moves a range onto a node of a
    // detached fragment, and `Range-set.html` moves a start onto a node of
    // another tree and expects the end to follow it.
    assert_eq!(
        eval(
            r#"
        const r = document.createRange();
        const detached = document.createElement("div");
        detached.appendChild(document.createTextNode("Opqrstuv"));
        const other = document.createElement("div");
        other.appendChild(document.createTextNode("Wxyzabcd"));
        detached.appendChild(other);
        r.setStart(detached, 0);
        r.setEnd(detached, 2);
        const out = [];
        out.push(r.toString());
        out.push(r.comparePoint(detached, 1));
        out.push(r.intersectsNode(other));
        out.push(r.isPointInRange(other.childNodes[0], 3));
        r.setStart(other, 0);
        r.setEnd(other, 1);
        out.push(r.startContainer === other);
        out.push(r.endContainer === other);
        out.push(r.toString());
        // A range collapsed exactly on a node's own start boundary still
        // intersects that node: the standard's step 2 is the ancestor test and
        // not the `(node, 0) < end` test, which a collapsed range fails.
        r.collapse(true);
        out.push(r.intersectsNode(other));
        out.push(r.intersectsNode(other.childNodes[0]));
        // And a range that *starts* on an empty element intersects it even
        // though the element's own zero-length span contains nothing: the
        // ancestor test is about the endpoints' nodes, not about a span.
        const empty = document.createElement("div");
        document.body.appendChild(empty);
        r.setStart(empty, 0);
        r.setEnd(document.body, document.body.childNodes.length);
        out.push(r.intersectsNode(empty));
        out.join("|")
    "#
        ),
        "OpqrstuvWxyzabcd|0|true|true|true|true|Wxyzabcd|true|false|true"
    );
}

#[test]
fn another_documents_tree_is_refused_not_mismatched() {
    // `Range-compareBoundaryPoints.html` compares a page range against a range of
    // a `createHTMLDocument` document and expects `WrongDocumentError`. The check
    // has to be against the *range's* document: a page range and a foreign range
    // are two host records whose endpoints name two arenas, so a comparison by
    // node id alone would read the wrong tree.
    assert_eq!(
        eval(
            r#"
        const foreign = document.implementation.createHTMLDocument("");
        const foreignRange = foreign.createRange();
        foreignRange.selectNodeContents(foreign.body);
        const r = document.createRange();
        r.selectNodeContents(document.body);
        const out = [];
        try {
            r.compareBoundaryPoints(r.START_TO_START, foreignRange);
            out.push("no throw");
        } catch (e) {
            out.push(e.name);
        }
        try {
            r.compareBoundaryPoints(4, r);
            out.push("no throw");
        } catch (e) {
            out.push(e.name);
        }
        try {
            r.comparePoint(foreign.body, 0);
            out.push("no throw");
        } catch (e) {
            out.push(e.name);
        }
        out.push(r.isPointInRange(foreign.body, 0));
        out.push(r.intersectsNode(foreign.body));
        r.detach();
        out.push(r.comparePoint(document.body, 0));
        out.join("|")
    "#
        ),
        // A foreign range, an unnameable `how` and a foreign node all throw;
        // `isPointInRange` and `intersectsNode` answer false for a different root
        // rather than throwing; and `detach` is a no-op, so the range still
        // answers.
        "WrongDocumentError|NotSupportedError|WrongDocumentError|false|false|0"
    );
}

#[test]
fn a_doctype_boundary_point_is_refused_and_a_bad_offset_is_not() {
    // `Range-isPointInRange.html`'s two refusals, in the order the standard
    // applies them: the doctype check precedes the offset check.
    assert_eq!(
        eval(
            r#"
        const r = document.createRange();
        const div = document.getElementById("test");
        const out = [];
        for (const call of [
            () => r.setStart(document.doctype, 0),
            () => r.setEnd(document.doctype, 0),
            () => r.selectNodeContents(document.doctype),
            () => r.isPointInRange(document.doctype, 0),
            () => r.comparePoint(document.doctype, 0),
        ]) {
            try {
                call();
                out.push("no throw");
            } catch (e) {
                out.push(e.name);
            }
        }
        for (const call of [
            () => r.setStart(div, 2),
            () => r.isPointInRange(div, 2),
            () => r.comparePoint(div, 2),
        ]) {
            try {
                call();
                out.push("no throw");
            } catch (e) {
                out.push(e.name);
            }
        }
        out.push(r.startOffset, r.endOffset);
        out.join("|")
    "#
        ),
        // A doctype is an `InvalidNodeTypeError`; offset 2 past a one-child div is
        // an `IndexSizeError` — and neither of the two failed writes moved the
        // range.
        "InvalidNodeTypeError|InvalidNodeTypeError|InvalidNodeTypeError|\
         InvalidNodeTypeError|InvalidNodeTypeError|IndexSizeError|IndexSizeError|\
         IndexSizeError|0|0"
    );
}

#[test]
fn static_range_copies_its_endpoints_and_validates_their_type() {
    assert_eq!(
        eval(
            r#"
        const div = document.getElementById("test");
        const out = [];
        const r = new StaticRange({startContainer: div, startOffset: 1,
                                   endContainer: div, endOffset: 2});
        out.push(r instanceof StaticRange, r instanceof AbstractRange);
        out.push(r instanceof Range);
        out.push(r.startContainer === div, r.startOffset, r.endOffset);
        out.push(r.collapsed);
        const collapsed = new StaticRange({startContainer: div, startOffset: 0,
                                           endContainer: div, endOffset: 0});
        out.push(collapsed.collapsed);
        const long = new StaticRange({startContainer: div, startOffset: 0,
                                      endContainer: div, endOffset: 15});
        out.push(long.endOffset);
        for (const init of [
            {startContainer: document.doctype, startOffset: 0,
             endContainer: document.doctype, endOffset: 0},
            {startContainer: div.getAttributeNode("id"), startOffset: 0,
             endContainer: div.getAttributeNode("id"), endOffset: 0},
            undefined,
            {startOffset: 0, endContainer: div, endOffset: 0},
            {startContainer: div, startOffset: 0, endContainer: div},
            {startContainer: null, startOffset: 0, endContainer: div, endOffset: 0},
        ]) {
            try {
                new StaticRange(init);
                out.push("no throw");
            } catch (e) {
                out.push(e.name);
            }
        }
        out.join("|")
    "#
        ),
        // `StaticRange-constructor.html`'s own last two blocks: the four members
        // are copied and are *not* checked against the tree (offset 15 past a
        // one-child div is accepted here alone), while a `DocumentType` and an
        // `Attr` are `InvalidNodeTypeError` and a missing, null or absent member
        // is a `TypeError` — including `new StaticRange()` itself.
        "true|true|false|true|1|2|false|true|15|\
         InvalidNodeTypeError|InvalidNodeTypeError|TypeError|TypeError|TypeError|TypeError"
    );
}

#[test]
fn append_converts_nodes_and_strings_into_one_value() {
    // `ParentNode.append` on all three inclusions: `Element`, `DocumentFragment`
    // and `Document`.
    assert_eq!(
        eval(
            r#"
        const host = document.getElementById("test");
        const fragment = document.createDocumentFragment();
        const out = [];
        fragment.append("a", document.createElement("b"), "c");
        out.push(fragment.childNodes.length);
        out.push(fragment.textContent);
        const single = document.createElement("i");
        host.append(single);
        out.push(host.lastChild === single);
        host.append("tail");
        out.push(host.lastChild.data);
        const footer = document.createElement("footer");
        document.body.append(footer);
        out.push(document.body.lastChild === footer);
        out.join("|")
    "#
        ),
        // Conversion preserves the two Text nodes around the empty element.
        "3|ac|true|tail|true"
    );
}

#[test]
fn cdata_sections_exist_only_on_a_non_html_document() {
    assert_eq!(
        eval(
            r#"
        const out = [];
        try {
            document.createCDATASection("abc");
            out.push("no throw");
        } catch (e) {
            out.push(e.name);
        }
        const xml = new Document();
        const section = xml.createCDATASection("abc");
        out.push(section.nodeType);
        out.push(section.data);
        out.push(section instanceof CDATASection);
        out.push(section instanceof Text);
        out.push(section instanceof CharacterData);
        try {
            xml.createCDATASection("a]]>b");
            out.push("no throw");
        } catch (e) {
            out.push(e.name);
        }
        const range = xml.createRange();
        range.selectNodeContents(section);
        out.push(range.toString());
        out.join("|")
    "#
        ),
        // `NotSupportedError` on the HTML page; the section is node type 4 with
        // the `CDATASection -> Text -> CharacterData` chain; `]]>` is an
        // `InvalidCharacterError`; and a range over the new document's section
        // stringifies its data, which is the case `dom/common.js` builds the
        // whole `dom/ranges` fixture out of.
        "NotSupportedError|4|abc|true|true|true|InvalidCharacterError|abc"
    );
}
