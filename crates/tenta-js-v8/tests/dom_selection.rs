//! `Selection` and the two `getSelection()` on the V8 host
//! (`tenta_js::selection`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// The tree every test selects in:
/// `<p id=a>alpha <b id=b>beta</b> gamma</p><p id=c>delta</p>`.
const TREE: &str = "var html = document.createElement('html'), body = document.createElement('body'); \
     document.appendChild(html); html.appendChild(body); \
     body.innerHTML = '<p id=\"a\">alpha <b id=\"b\">beta</b> gamma</p><p id=\"c\">delta</p>'; \
     var a = document.getElementById('a'), b = document.getElementById('b'), \
         c = document.getElementById('c'); \
     var at = a.firstChild, bt = b.firstChild, gt = a.lastChild, ct = c.firstChild; \
     var sel = getSelection(); \
     function thrown(f) { try { f(); return 'none'; } catch (e) { return e.name; } } \
     'ready'";

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    assert_eq!(eval(&mut engine, TREE), "ready");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn a_document_has_one_selection_and_it_starts_empty() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "[sel === getSelection(), sel === document.getSelection(), sel instanceof Selection, \
              Object.prototype.toString.call(sel), sel.rangeCount, sel.type, sel.isCollapsed, \
              sel.anchorNode, sel.focusNode, sel.anchorOffset, sel.focusOffset, sel.direction, \
              '[' + String(sel) + ']', typeof Selection, thrown(function () { new Selection(); }), \
              thrown(function () { sel.getRangeAt(0); }), \
              'modify' in sel, 'deleteFromDocument' in sel, \
              Object.keys(Selection.prototype).indexOf('addRange') >= 0].join('|')",
        ),
        "true|true|true|[object Selection]|0|None|true|||0|0|none|[]|function|TypeError|\
         IndexSizeError|false|false|true"
    );
}

#[test]
fn collapsing_makes_a_caret() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "sel.collapse(at, 2); \
             var caret = [sel.type, sel.rangeCount, sel.isCollapsed, sel.anchorNode === at, \
                          sel.focusNode === at, sel.anchorOffset, sel.focusOffset, sel.direction, \
                          '[' + sel + ']'].join(','); \
             sel.setPosition(bt); var moved = [sel.anchorNode === bt, sel.anchorOffset].join(','); \
             sel.collapse(null); \
             [caret, moved, sel.type, thrown(function () { sel.collapse(at, 99); }), \
              thrown(function () { sel.collapse('text'); }), \
              thrown(function () { sel.collapse(); })].join('|')",
        ),
        "Caret,1,true,true,true,2,2,none,[]|true,0|None|IndexSizeError|TypeError|TypeError"
    );
}

#[test]
fn a_selection_made_backwards_keeps_its_anchor_and_focus() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "sel.setBaseAndExtent(at, 0, bt, 2); \
             var forward = [sel.type, sel.direction, String(sel), sel.anchorNode === at, \
                            sel.focusNode === bt, sel.focusOffset].join(','); \
             sel.setBaseAndExtent(bt, 2, at, 1); \
             var range = sel.getRangeAt(0); \
             var backward = [sel.direction, String(sel), sel.anchorNode === bt, sel.anchorOffset, \
                             sel.focusNode === at, sel.focusOffset, range.startContainer === at, \
                             range.startOffset, range.endContainer === bt].join(','); \
             [forward, backward, thrown(function () { sel.setBaseAndExtent(at, 0, bt); }), \
              thrown(function () { sel.setBaseAndExtent(at, 0, bt, 9); })].join('|')",
        ),
        "Range,forward,alpha be,true,true,2|backward,lpha be,true,2,true,1,true,1,true|TypeError|\
         IndexSizeError"
    );
}

#[test]
fn extending_moves_the_focus_and_leaves_the_anchor() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var none = thrown(function () { sel.extend(at, 1); }); \
             sel.collapse(at, 3); sel.extend(gt, 2); \
             var forward = [String(sel), sel.direction, sel.anchorNode === at, sel.anchorOffset, \
                            sel.focusNode === gt, sel.focusOffset].join(','); \
             sel.extend(at, 1); \
             var backward = [String(sel), sel.direction, sel.anchorNode === at, sel.anchorOffset, \
                             sel.focusNode === at, sel.focusOffset].join(','); \
             sel.collapseToStart(); var start = [sel.type, sel.anchorOffset].join(','); \
             sel.extend(at, 4); sel.collapseToEnd(); \
             [none, forward, backward, start, sel.type, sel.anchorOffset].join('|')",
        ),
        "InvalidStateError|ha beta g,forward,true,3,true,2|lp,backward,true,3,true,1|Caret,1|\
         Caret|4"
    );
}

#[test]
fn selecting_a_nodes_children_and_asking_what_is_inside() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "sel.selectAllChildren(a); \
             var whole = [String(sel), sel.anchorNode === a, sel.anchorOffset, sel.focusOffset, \
                          sel.containsNode(b), sel.containsNode(bt), sel.containsNode(c), \
                          sel.containsNode(c, true)].join(','); \
             sel.setBaseAndExtent(at, 2, bt, 2); \
             [whole, sel.containsNode(b), sel.containsNode(b, true), sel.containsNode(at, true), \
              sel.containsNode(gt, true), thrown(function () { sel.containsNode(); })].join('|')",
        ),
        "alpha beta gamma,true,0,3,true,true,false,false|false|true|true|false|TypeError"
    );
}

#[test]
fn the_range_handed_to_add_range_is_the_selections_range() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var range = document.createRange(); range.selectNodeContents(c); \
             sel.addRange(range); \
             var other = document.createRange(); other.selectNodeContents(a); \
             sel.addRange(other); \
             var first = [sel.rangeCount, sel.getRangeAt(0) === range, String(sel)].join(','); \
             range.setStart(ct, 2); \
             var live = String(sel); \
             var wrong = thrown(function () { sel.removeRange(other); }); \
             sel.removeRange(range); \
             var outside = document.createRange(); \
             outside.selectNodeContents(document.createElement('div')); \
             sel.addRange(outside); \
             [first, live, wrong, sel.rangeCount, thrown(function () { sel.addRange({}); }), \
              sel.getComposedRanges().length].join('|')",
        ),
        "1,true,delta|lta|NotFoundError|0|TypeError|0"
    );
}

#[test]
fn a_node_outside_the_document_is_ignored() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var loose = document.createElement('div'); loose.textContent = 'loose'; \
             sel.collapse(at, 1); \
             sel.collapse(loose, 0); sel.selectAllChildren(loose); sel.extend(loose, 0); \
             sel.setBaseAndExtent(at, 0, loose, 0); \
             [sel.anchorNode === at, sel.anchorOffset, sel.type, \
              sel.containsNode(loose, true)].join('|')",
        ),
        "true|1|Caret|false"
    );
}

#[test]
fn the_selection_follows_the_tree() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "sel.selectAllChildren(b); var before = String(sel); b.remove(); \
             var composed = sel.getComposedRanges(); \
             [before, sel.rangeCount, sel.anchorNode === a, sel.anchorOffset, sel.isCollapsed, \
              composed.length, composed[0] instanceof StaticRange, \
              composed[0].startContainer === a].join('|')",
        ),
        "beta|1|true|1|true|1|true|true"
    );
}

#[test]
fn one_selectionchange_is_fired_for_the_changes_of_a_task() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "globalThis.changes = []; \
             document.addEventListener('selectionchange', function (event) { \
               changes.push([event.target === document, event.bubbles, sel.type].join(':')); \
             }); \
             sel.removeAllRanges(); \
             sel.collapse(at, 1); sel.extend(bt, 1); sel.selectAllChildren(c); \
             changes.length",
        ),
        "0"
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "changes.join(' ')"), "true:false:Range");
    assert_eq!(
        eval(&mut engine, "sel.empty(); sel.empty(); changes.length"),
        "1"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "changes.join(' ')"),
        "true:false:Range true:false:None"
    );
}
