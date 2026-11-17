//! `TreeWalker`, `NodeIterator` and `NodeFilter` on the V8 host (the
//! traversal prelude).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// An engine with the preludes a page gets and a small tree:
/// `body > (div#a > (span#a1 "one", <!--note-->, b#a2 > i#a3), p#b "two", div#c)`.
fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
        .evaluate(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             body.innerHTML = '<div id=a><span id=a1>one</span><!--note--><b id=a2><i id=a3></i></b></div>' \
               + '<p id=b>two</p><div id=c></div>'; \
             var name = n => n === null ? 'null' : n.nodeType === 1 ? n.id || n.localName \
               : n.nodeType === 3 ? '\"' + n.data + '\"' : n.nodeType === 8 ? '<!' + n.data + '>' : n.nodeName; \
             var all = (walker, step) => { var out = [], n; while ((n = walker[step]())) out.push(name(n)); return out.join(' '); }; \
             var attempt = f => { try { return String(f()); } catch (e) { return e.name; } }; \
             'ready'",
        )
        .expect("the test tree");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn a_tree_walker_walks_forward_and_back_in_tree_order() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var walker = document.createTreeWalker(body); \
             var forward = all(walker, 'nextNode'); \
             var back = all(walker, 'previousNode'); \
             [forward, back, name(walker.currentNode), walker.root === body, walker.whatToShow, \
              String(walker.filter), String(walker), walker instanceof TreeWalker].join(' | ')"
        ),
        "a a1 \"one\" <!note> a2 a3 b \"two\" c | \"two\" b a3 a2 <!note> \"one\" a1 a body \
         | body | true | 4294967295 | null | [object TreeWalker] | true"
    );
}

/// Rejecting `a` leaves its subtree out; skipping it keeps the subtree.
#[test]
fn what_to_show_and_the_filter_choose_the_nodes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var elements = all(document.createTreeWalker(body, NodeFilter.SHOW_ELEMENT), 'nextNode'); \
             var text = all(document.createTreeWalker(body, NodeFilter.SHOW_TEXT), 'nextNode'); \
             var comments = all(document.createTreeWalker(body, 128), 'nextNode'); \
             var rejecting = all(document.createTreeWalker(body, 1, \
               n => n.id === 'a' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT), 'nextNode'); \
             var skipping = all(document.createTreeWalker(body, 1, \
               {acceptNode: n => n.id === 'a' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT}), 'nextNode'); \
             [elements, text, comments, rejecting, skipping].join(' | ')"
        ),
        "a a1 a2 a3 b c | \"one\" \"two\" | <!note> | b c | a1 a2 a3 b c"
    );
}

#[test]
fn a_tree_walker_moves_between_parents_children_and_siblings() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var walker = document.createTreeWalker(body, NodeFilter.SHOW_ELEMENT); \
             var steps = []; \
             var step = method => steps.push(method + ':' + name(walker[method]())); \
             step('firstChild'); step('firstChild'); step('nextSibling'); step('firstChild'); \
             step('nextSibling'); step('parentNode'); step('parentNode'); step('nextSibling'); \
             step('lastChild'); step('nextSibling'); step('previousSibling'); step('parentNode'); \
             step('parentNode'); step('lastChild'); \
             steps.join(' ') + ' | ' + name(walker.currentNode)"
        ),
        "firstChild:a firstChild:a1 nextSibling:a2 firstChild:a3 nextSibling:null parentNode:a2 \
         parentNode:a nextSibling:b lastChild:null nextSibling:c previousSibling:b parentNode:body \
         parentNode:null lastChild:c | c"
    );
}

/// What a template library does: one walker rooted at the document, pointed
/// at the content of each template in turn, which is a tree of its own.
#[test]
fn a_walker_pointed_outside_its_root_walks_that_tree_and_stops_at_its_end() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var template = document.createElement('template'); \
             template.innerHTML = '<div id=t1><span id=t2></span><!--part--></div><p id=t3></p>'; \
             var walker = document.createTreeWalker(document, 129); \
             walker.currentNode = template.content; \
             var first = all(walker, 'nextNode'); \
             var after = name(walker.nextNode()); \
             walker.currentNode = template.content; \
             var again = name(walker.nextNode()); \
             [first, after, again, attempt(() => { walker.currentNode = null; }), \
              attempt(() => { walker.currentNode = {}; })].join(' | ')"
        ),
        "t1 t2 <!part> t3 | null | t1 | TypeError | TypeError"
    );
}

#[test]
fn a_node_iterator_keeps_a_reference_and_a_side_of_it() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var iterator = document.createNodeIterator(body, NodeFilter.SHOW_ELEMENT); \
             var before = [name(iterator.referenceNode), iterator.pointerBeforeReferenceNode].join('/'); \
             var first = name(iterator.nextNode()); \
             var forward = all(iterator, 'nextNode'); \
             var end = [name(iterator.referenceNode), iterator.pointerBeforeReferenceNode].join('/'); \
             var back = all(iterator, 'previousNode'); \
             var start = [name(iterator.referenceNode), iterator.pointerBeforeReferenceNode].join('/'); \
             iterator.detach(); \
             [before, first, forward, end, back, start, String(iterator), \
              iterator instanceof NodeIterator, iterator.root === body].join(' | ')"
        ),
        "body/true | body | a a1 a2 a3 b c | c/false | c b a3 a2 a1 a body | body/true \
         | [object NodeIterator] | true | true"
    );
}

#[test]
fn the_interfaces_have_the_shape_the_standard_gives_them() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var busy = document.createTreeWalker(body, 1, () => busy.nextNode()); \
             [NodeFilter.FILTER_ACCEPT, NodeFilter.FILTER_REJECT, NodeFilter.FILTER_SKIP, \
              NodeFilter.SHOW_ALL, NodeFilter.SHOW_DOCUMENT_FRAGMENT, \
              attempt(() => new TreeWalker()), attempt(() => new NodeIterator()), \
              attempt(() => document.createTreeWalker()), attempt(() => document.createTreeWalker({})), \
              attempt(() => document.createTreeWalker(body, 1, 5)), \
              attempt(() => document.createNodeIterator(null)), \
              attempt(() => TreeWalker.prototype.nextNode.call({})), \
              attempt(() => busy.nextNode()), \
              attempt(() => document.createTreeWalker(body, 1, {}).nextNode()), \
              document.createTreeWalker.length, document.createNodeIterator.length, \
              Object.keys(TreeWalker.prototype).join(',')].join('|')"
        ),
        "1|2|3|4294967295|1024|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError\
         |InvalidStateError|TypeError|1|1\
         |root,whatToShow,filter,currentNode,parentNode,firstChild,lastChild,previousSibling,nextSibling,previousNode,nextNode"
    );
}
