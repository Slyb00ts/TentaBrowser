//! Focus: `focus()`/`blur()`, `document.activeElement`, the four focus events
//! and the focus pseudo-classes through the generated V8 glue (HTML Standard
//! §6.6).

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};
use tenta_js::engine::{LifecycleEvent, PageEngine};
use tenta_js::geometry::{
    ClientMetrics, GeometryHost, OffsetMetrics, RectF64, ScrollMetrics, ViewportMetrics,
};
use tenta_js_v8::Engine;

use std::cell::RefCell;
use std::rc::Rc;

fn page() -> Rc<RefCell<Document>> {
    let html = "<!DOCTYPE html><html><body>\
        <button id=b1>one</button><button id=b2>two</button>\
        <input id=off disabled><input id=hid type=hidden>\
        <div id=plain></div><div id=tab tabindex=-1></div><div id=edit contenteditable>x</div>\
        <a id=link href='/x'>l</a><a id=nolink>n</a>\
        <section id=wrap><input id=inner></section>\
        <div hidden><button id=gone>g</button></div>\
        <svg><a id=sa tabindex=0></a><text id=st></text></svg>\
        </body></html>";
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    Rc::new(RefCell::new(parser.finish()))
}

fn run(script: &str) -> String {
    let mut engine = Engine::with_document(page());
    engine
        .evaluate(&format!("globalThis.log = []; {script}"))
        .expect("the script evaluates without throwing");
    engine
        .evaluate("log.join('|')")
        .expect("the log is readable")
}

#[test]
fn only_focusable_areas_take_focus() {
    assert_eq!(
        run(r#"
            const id = () => document.activeElement.id || document.activeElement.localName;
            for (const name of ['b1', 'off', 'hid', 'plain', 'tab', 'edit', 'link', 'nolink', 'inner', 'gone']) {
                document.getElementById(name).focus();
                log.push(name + '>' + id());
                document.activeElement.blur();
            }
        "#),
        "b1>b1|off>body|hid>body|plain>body|tab>tab|edit>edit|link>link|nolink>body|inner>inner|gone>body"
    );
}

#[test]
fn focus_moves_with_the_events_and_related_targets_browsers_fire() {
    assert_eq!(
        run(r#"
            const b1 = document.getElementById('b1'), b2 = document.getElementById('b2');
            for (const type of ['focus', 'blur', 'focusin', 'focusout']) {
                document.addEventListener(type, (e) => log.push('doc:' + type), true);
                for (const el of [b1, b2]) {
                    el.addEventListener(type, (e) => log.push(el.id + ':' + type + ':' +
                        (e.relatedTarget ? e.relatedTarget.id : 'null') + ':' + e.bubbles + ':' + (e instanceof FocusEvent) + ':' + e.isTrusted));
                }
            }
            b1.focus();
            log.push('--');
            b2.focus();
            log.push('--');
            b2.blur();
            log.push(document.activeElement.localName, document.hasFocus());
        "#),
        "doc:focus|b1:focus:null:false:true:true|doc:focusin|b1:focusin:null:true:true:true|--|\
         doc:blur|b1:blur:b2:false:true:true|doc:focusout|b1:focusout:b2:true:true:true|\
         doc:focus|b2:focus:b1:false:true:true|doc:focusin|b2:focusin:b1:true:true:true|--|\
         doc:blur|b2:blur:null:false:true:true|doc:focusout|b2:focusout:null:true:true:true|body|true"
    );
}

#[test]
fn focusing_the_focused_element_fires_nothing() {
    assert_eq!(
        run(r#"
            const b1 = document.getElementById('b1');
            b1.addEventListener('focus', () => log.push('focus'));
            b1.focus(); b1.focus();
            log.push(document.activeElement === b1);
        "#),
        "focus|true"
    );
}

#[test]
fn a_blur_listener_that_moves_focus_wins() {
    assert_eq!(
        run(r#"
            const b1 = document.getElementById('b1'), b2 = document.getElementById('b2'), link = document.getElementById('link');
            b1.addEventListener('blur', () => link.focus());
            b2.addEventListener('focus', () => log.push('b2 focus'));
            b1.focus();
            b2.focus();
            log.push(document.activeElement.id);
        "#),
        "link"
    );
}

#[test]
fn removing_the_focused_element_returns_focus_to_the_body() {
    assert_eq!(
        run(r#"
            const b1 = document.getElementById('b1');
            b1.focus();
            b1.remove();
            log.push(document.activeElement.localName);
        "#),
        "body"
    );
}

#[test]
fn focus_pseudo_classes_follow_the_focused_element() {
    assert_eq!(
        run(r#"
            const inner = document.getElementById('inner');
            const probe = () => [document.querySelector(':focus')?.id,
                                 document.querySelector(':focus-visible')?.id,
                                 document.querySelector('#wrap:focus-within') !== null,
                                 inner.matches(':focus')].join(',');
            log.push(probe());
            inner.focus();
            log.push(probe());
            inner.focus({focusVisible: false});
            inner.blur();
            inner.focus({focusVisible: false});
            log.push(probe());
            inner.blur();
            log.push(probe());
        "#),
        ",,false,false|inner,inner,true,true|inner,,true,true|,,false,false"
    );
}

#[test]
fn tab_index_reports_the_default_and_reflects_writes() {
    assert_eq!(
        run(r#"
            const el = (id) => document.getElementById(id);
            log.push(el('b1').tabIndex, el('plain').tabIndex, el('tab').tabIndex, el('link').tabIndex, el('nolink').tabIndex);
            el('plain').tabIndex = 5;
            log.push(el('plain').getAttribute('tabindex'), el('plain').tabIndex);
            el('plain').focus();
            log.push(document.activeElement.id);
        "#),
        "0|-1|-1|0|-1|5|5|plain"
    );
}

#[test]
fn svg_elements_share_the_focus_members() {
    assert_eq!(
        run(r#"
            const sa = document.getElementById('sa'), st = document.getElementById('st');
            log.push(sa instanceof SVGElement, typeof sa.focus, st.tabIndex, sa.tabIndex);
            st.focus();
            log.push(document.activeElement.localName);
            sa.focus();
            log.push(document.activeElement.id, document.querySelector(':focus').id);
            sa.blur();
            log.push(document.activeElement.localName);
        "#),
        "true|function|-1|0|body|sa|sa|body"
    );
}

fn parsed(html: &str) -> Rc<RefCell<Document>> {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    Rc::new(RefCell::new(parser.finish()))
}

#[test]
fn the_first_focusable_autofocus_element_takes_focus_when_the_document_is_ready() {
    let mut engine = Engine::with_document(parsed(
        "<!DOCTYPE html><body><input id=off autofocus disabled><div id=plain autofocus></div>\
         <input id=first autofocus><input id=second autofocus>\
         <script>document.addEventListener('focusin', (e) => (globalThis.got = e.target.id), true);</script>",
    ));
    assert_eq!(
        engine.evaluate("document.activeElement.localName").unwrap(),
        "body"
    );
    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    assert_eq!(
        engine
            .evaluate(
                "[document.activeElement.id, document.querySelector(':focus-visible').id].join()"
            )
            .unwrap(),
        "first,first"
    );
}

#[test]
fn autofocus_does_not_take_focus_from_an_element_that_already_has_it() {
    let mut engine = Engine::with_document(parsed(
        "<!DOCTYPE html><body><input id=mine><input id=auto autofocus>",
    ));
    engine
        .evaluate("document.getElementById('mine').focus()")
        .unwrap();
    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    assert_eq!(
        engine.evaluate("document.activeElement.id").unwrap(),
        "mine"
    );
}

#[test]
fn inert_subtrees_do_not_take_focus() {
    assert_eq!(
        run(r#"
            const wrap = document.createElement('div');
            wrap.setAttribute('inert', '');
            const inside = document.createElement('button');
            wrap.appendChild(inside);
            document.body.appendChild(wrap);
            inside.focus();
            log.push(document.activeElement === inside);
            wrap.removeAttribute('inert');
            inside.focus();
            log.push(document.activeElement === inside);
        "#),
        "false|true"
    );
}

#[test]
fn removing_the_focused_element_fires_blur_and_focusout_with_a_null_related_target() {
    assert_eq!(
        run(r#"
            const b1 = document.getElementById('b1');
            for (const type of ['blur', 'focusout'])
                b1.addEventListener(type, (e) => log.push(type + ':' + e.relatedTarget + ':' +
                    (document.activeElement === document.body)));
            b1.focus();
            b1.remove();
            log.push('after');
        "#),
        "blur:null:true|focusout:null:true|after"
    );
}

/// A layout stand-in: the listed nodes compute `display: none` or
/// `visibility: hidden`.
struct Unrendered {
    display_none: Vec<NodeId>,
    hidden: Vec<NodeId>,
}

impl GeometryHost for Unrendered {
    fn viewport(&self) -> ViewportMetrics {
        ViewportMetrics::default()
    }
    fn bounding_rect(&self, _: NodeId) -> Option<RectF64> {
        None
    }
    fn client_rects(&self, _: NodeId) -> Vec<RectF64> {
        Vec::new()
    }
    fn offset_metrics(&self, _: NodeId) -> Option<OffsetMetrics> {
        None
    }
    fn client_metrics(&self, _: NodeId) -> Option<ClientMetrics> {
        None
    }
    fn scroll_metrics(&self, _: NodeId) -> Option<ScrollMetrics> {
        None
    }
    fn computed_value(&self, node: NodeId, _: Option<&str>, property: &str) -> Option<String> {
        Some(match property {
            "display" if self.display_none.contains(&node) => "none".to_owned(),
            "display" => "block".to_owned(),
            "visibility" if self.hidden.contains(&node) => "hidden".to_owned(),
            _ => "visible".to_owned(),
        })
    }
    fn computed_property_names(&self) -> &'static [&'static str] {
        &[]
    }
    fn element_from_point(&self, _: f64, _: f64) -> Option<NodeId> {
        None
    }
}

#[test]
fn elements_layout_does_not_render_cannot_take_focus() {
    let document = parsed(
        "<!DOCTYPE html><body><div id=gone><input id=nested></div><input id=hid>\
         <input id=shown>",
    );
    let (gone, hid) = {
        let document = document.borrow();
        (
            document.get_element_by_id("gone").unwrap(),
            document.get_element_by_id("hid").unwrap(),
        )
    };
    let mut engine = Engine::with_document(document);
    engine.set_geometry_host(Rc::new(Unrendered {
        display_none: vec![gone],
        hidden: vec![hid],
    }));
    assert_eq!(
        engine
            .evaluate(
                "['nested', 'hid', 'shown'].map((id) => { \
                   document.getElementById(id).focus(); \
                   return id + ':' + (document.activeElement.id === id); }).join('|')",
            )
            .unwrap(),
        "nested:false|hid:false|shown:true"
    );
}
