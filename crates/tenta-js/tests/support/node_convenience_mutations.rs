// Included by V8 and QuickJS adapters to exercise the same ParentNode and
// ChildNode convenience mutations.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

fn eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine.evaluate(source).expect("mutation script")
}

/// Declares `html(parent)` (the serialized children) and `thrown(fn)` (the
/// DOMException name a call raises, or `none`) for the scripts below.
const HELPERS: &str = "\
    const html = (n) => n.childNodes.length + ':' + \
        Array.from(n.childNodes).map((c) => c.nodeType === 3 ? '#' + c.data : \
        c.nodeType === 8 ? '<!--' + c.data + '-->' : '<' + c.localName + '>').join(''); \
    const thrown = (f) => { try { f(); return 'none'; } catch (e) { return e.name; } }; \
    const el = (name) => document.createElement(name); \
    const parentWith = (...kids) => { const p = el('div'); p.append(...kids); return p; };";

fn run(body: &str) -> String {
    eval(&format!("{HELPERS} {body}"))
}

#[test]
fn append_and_prepend_convert_strings_to_separate_unmerged_text_nodes() {
    assert_eq!(
        run("const p = parentWith(el('a')); \
             p.append('x', 'y', null, undefined); \
             const q = parentWith(el('a')); \
             q.prepend('x', 'y', el('b')); \
             [html(p), html(q), el('div').prepend() === undefined].join('|')"),
        "5:<a>#x#y#null#undefined|4:#x#y<b><a>|true"
    );
}

#[test]
fn before_and_after_skip_siblings_that_are_arguments() {
    assert_eq!(
        run("let x = el('x'), y = el('y'), z = el('z'), child = el('c'); \
             const p = parentWith(x, y, child, z); \
             child.before(x, y, 'T'); \
             const a = html(p); \
             [x, y, z, child] = [el('x'), el('y'), el('z'), el('c')]; \
             const q = parentWith(x, child, y, z); \
             child.after(y, z, 'U'); \
             [a, html(q)].join('|')"),
        "5:<x><y>#T<c><z>|5:<x><c><y><z>#U"
    );
}

#[test]
fn before_with_the_preceding_siblings_keeps_the_tree_in_argument_order() {
    assert_eq!(
        run("const x = el('x'), y = el('y'), child = el('c'); \
             const p = parentWith(x, y, child); \
             child.before(y, x); \
             html(p)"),
        "3:<y><x><c>"
    );
}

#[test]
fn child_node_methods_do_nothing_without_a_parent() {
    assert_eq!(
        run("const lone = el('a'), x = el('x'); \
             lone.before(x); lone.after(x); lone.replaceWith(x); \
             [x.parentNode === null, lone.parentNode === null].join()"),
        "true,true"
    );
}

#[test]
fn replace_with_handles_the_context_object_among_its_arguments() {
    assert_eq!(
        run("let x = el('x'), child = el('c'); \
             const p = parentWith(child, x, document.createTextNode('t')); \
             child.replaceWith(x, child); \
             const a = html(p); \
             const q = parentWith(child = el('c'), x = el('x')); \
             child.replaceWith('s', x, 'u'); \
             const lone = parentWith(child = el('c')); \
             child.replaceWith(); \
             [a, html(q), html(lone)].join('|')"),
        "3:<x><c>#t|3:#s<x>#u|0:"
    );
}

#[test]
fn replace_with_works_on_character_data() {
    assert_eq!(
        run("const t = document.createTextNode('t'), c = document.createComment('c'); \
             const p = parentWith(t, c); \
             t.after('a'); c.before('b'); t.replaceWith(el('r')); \
             html(p)"),
        "4:<r>#a#b<!--c-->"
    );
}

#[test]
fn replace_children_replaces_all_children_for_every_parent_kind() {
    assert_eq!(
        run("const p = parentWith(el('a'), el('b')); \
             p.replaceChildren('x', el('y')); \
             const f = document.createDocumentFragment(); \
             f.append(el('a')); f.replaceChildren(); \
             const kept = el('k'); const q = parentWith(el('a'), kept); \
             q.replaceChildren(kept); \
             [html(p), html(f), html(q), kept.parentNode === q].join('|')"),
        "2:#x<y>|0:|1:<k>|true"
    );
}

// The children being removed are ignored by the hierarchy check, as WPT
// `dom/nodes/ParentNode-replaceChildren.html` lines 126-150 expect.
#[test]
fn document_replace_children_ignores_the_children_it_removes() {
    assert_eq!(
        run("const doc = document.implementation.createHTMLDocument('t'); \
             const a = doc.createElement('a'); \
             doc.replaceChildren(a); \
             const afterElement = html(doc); \
             const d2 = document.implementation.createHTMLDocument('t'); \
             const dt = d2.doctype; \
             d2.replaceChildren(dt); \
             [afterElement, d2.childNodes.length, d2.firstChild === dt].join('|')"),
        "1:<a>|1|true"
    );
}

#[test]
fn hierarchy_errors_carry_the_standard_names_and_leave_the_tree_alone() {
    assert_eq!(
        run("const doc = document.implementation.createHTMLDocument('t'); \
             const outer = el('o'), inner = el('i'); outer.append(inner); \
             const lone = el('c'); const p = parentWith(lone); \
             [thrown(() => doc.replaceChildren('text')), \
              thrown(() => doc.replaceChildren(doc.createElement('a'), doc.createElement('b'))), \
              thrown(() => doc.append('text')), \
              thrown(() => doc.documentElement.after(doc.createElement('z'))), \
              thrown(() => inner.append(outer)), \
              thrown(() => outer.replaceChildren(outer)), \
              thrown(() => inner.replaceChildren(outer)), \
              thrown(() => lone.after(p)), \
              html(outer), html(p)].join('|')"),
        "HierarchyRequestError|HierarchyRequestError|HierarchyRequestError|\
         HierarchyRequestError|HierarchyRequestError|HierarchyRequestError|\
         HierarchyRequestError|HierarchyRequestError|1:<i>|1:<c>"
    );
}

#[test]
fn a_node_from_another_document_is_refused_by_name() {
    assert_eq!(
        run("const other = document.implementation.createHTMLDocument('t'); \
             const p = el('p'); \
             thrown(() => p.append(other.createElement('x')))"),
        "NotSupportedError"
    );
}

#[test]
fn arguments_must_be_nodes_or_stringifiable() {
    assert_eq!(
        run("const p = el('p'); p.append(1, true, { toString() { return 'obj'; } }); html(p)"),
        "3:#1#true#obj"
    );
}

#[test]
fn a_doctype_takes_part_in_child_node_mutations() {
    assert_eq!(
        run("const doc = document.implementation.createHTMLDocument('t'); \
             const dt = doc.doctype; \
             const before = typeof dt.before + typeof dt.after + typeof dt.replaceWith; \
             dt.remove(); \
             [before, doc.doctype === null, doc.childNodes.length].join('|')"),
        "functionfunctionfunction|true|1"
    );
}

#[test]
fn a_node_named_as_its_own_argument_stays_where_it_is() {
    assert_eq!(
        run("const a = el('a'), x = el('x'), b = el('b'); \
             const p = parentWith(a, x, b); \
             x.replaceWith(x); x.before(x); x.after(x); \
             html(p)"),
        "3:<a><x><b>"
    );
}
