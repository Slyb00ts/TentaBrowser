//! Event handler IDL attributes of the `GlobalEventHandlers` mixin on the V8
//! host: `element.onclick = fn`, on HTML and SVG elements and the document.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine
        .evaluate(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             var button = document.createElement('button'); body.appendChild(button); \
             globalThis.log = []; 'ready'",
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
fn elements_and_the_document_have_the_handler_attributes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); \
             ['onclick', 'oninput', 'onchange', 'onsubmit', 'onkeydown', 'onload', 'onerror', \
              'onpointerdown', 'ontransitionend'].map(function (name) { \
               return [name in button, name in document, name in svg, button[name]].join('/'); \
             }).join(' ')",
        ),
        ["true/true/true/"; 9].join(" "),
        "present everywhere and null until assigned"
    );
    assert_eq!(
        eval(
            &mut engine,
            "['ontouchstart' in button, 'onclick' in document.createTextNode('x'), \
              Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onclick') !== undefined, \
              Object.getOwnPropertyDescriptor(Element.prototype, 'onclick') === undefined].join('|')",
        ),
        "false|false|true|true"
    );
}

#[test]
fn an_assigned_handler_runs_with_the_event_and_the_element_as_this() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var handler = function (event) { \
               log.push([event.type, this === button, event.target === button].join(':')); \
             }; \
             button.onclick = handler; \
             var same = button.onclick === handler; \
             button.click(); \
             same + '|' + log.join(' ')",
        ),
        "true|click:true:true"
    );
}

#[test]
fn reassigning_replaces_and_null_removes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.onclick = function () { log.push('first'); }; \
             button.onclick = function () { log.push('second'); }; \
             button.click(); \
             button.onclick = null; \
             button.click(); \
             log.join(' ') + '|' + button.onclick",
        ),
        "second|null"
    );
}

#[test]
fn a_value_that_is_not_an_object_is_treated_as_null() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.onclick = function () {}; button.onclick = 'not a function'; \
             var afterString = button.onclick; \
             button.onclick = 7; \
             afterString + '|' + button.onclick",
        ),
        "null|null"
    );
}

#[test]
fn a_handler_keeps_the_place_of_its_first_assignment_among_listeners() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.addEventListener('click', function () { log.push('a'); }); \
             button.onclick = function () { log.push('handler-1'); }; \
             button.addEventListener('click', function () { log.push('b'); }); \
             button.onclick = function () { log.push('handler-2'); }; \
             button.click(); log.join(' ')",
        ),
        "a handler-2 b"
    );
}

#[test]
fn returning_false_cancels_the_event() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.onclick = function () { return false; }; \
             var cancelled = !button.dispatchEvent(new MouseEvent('click', { cancelable: true })); \
             button.onclick = function () { return true; }; \
             var kept = button.dispatchEvent(new MouseEvent('click', { cancelable: true })); \
             cancelled + '|' + kept",
        ),
        "true|true"
    );
}

#[test]
fn handlers_bubble_to_ancestors_and_the_document() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.onclick = function (event) { log.push('body:' + (event.target === button)); }; \
             document.onclick = function (event) { log.push('document:' + (this === document)); }; \
             button.click(); log.join(' ')",
        ),
        "body:true document:true"
    );
}

#[test]
fn input_events_reach_oninput_and_onchange() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var field = document.createElement('input'); body.appendChild(field); \
             field.oninput = function (event) { log.push(event.type); }; \
             field.onchange = function (event) { log.push(event.type); }; \
             field.dispatchEvent(new Event('input', { bubbles: true })); \
             field.dispatchEvent(new Event('change', { bubbles: true })); \
             log.join(' ')",
        ),
        "input change"
    );
}

// ---- event handler content attributes ------------------------------------

#[test]
fn a_content_attribute_is_compiled_when_the_event_is_dispatched() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onclick', \"log.push(event.type + ':' + (this === button))\"); \
             button.click(); button.click(); log.join(' ')",
        ),
        "click:true click:true"
    );
}

#[test]
fn reading_the_idl_attribute_compiles_the_content_attribute() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var before = button.onclick; \
             button.setAttribute('onclick', 'return 7;'); \
             [before, typeof button.onclick, button.onclick === button.onclick, \
              button.onclick.name, button.onclick.length, button.onclick(null)].join('|')",
        ),
        "|function|true|onclick|1|7"
    );
}

#[test]
fn the_handler_sees_the_element_its_form_and_its_document_in_scope() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.setAttribute('id', 'the-body'); \
             button.setAttribute('value', 'from-element'); \
             button.setAttribute('onclick', \
               \"log.push([id === '', getAttribute('value'), typeof createElement, \
                          typeof getElementById, event instanceof MouseEvent].join(':'))\"); \
             button.click(); log.join(' ')",
        ),
        "true:from-element:function:function:true",
        "`id` and `getAttribute` are the element's, `createElement` the document's"
    );
}

#[test]
fn returning_false_from_an_attribute_handler_cancels_the_event() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onclick', 'return false'); \
             String(button.dispatchEvent(new MouseEvent('click', { cancelable: true })))",
        ),
        "false"
    );
}

#[test]
fn changing_or_removing_the_attribute_changes_the_handler() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onclick', \"log.push('one')\"); button.click(); \
             button.setAttribute('onclick', \"log.push('two')\"); button.click(); \
             button.removeAttribute('onclick'); button.click(); \
             log.join(' ') + '|' + button.onclick",
        ),
        "one two|null"
    );
}

#[test]
fn script_assignment_and_the_attribute_override_each_other_in_order() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onclick', \"log.push('attribute')\"); \
             button.onclick = function () { log.push('assigned'); }; \
             button.click(); \
             var kept = button.getAttribute('onclick'); \
             button.setAttribute('onclick', \"log.push('attribute-2')\"); \
             button.click(); \
             button.onclick = null; \
             button.click(); \
             log.join(' ') + '|' + kept",
        ),
        "assigned attribute-2|log.push('attribute')"
    );
}

#[test]
fn an_attribute_handler_runs_for_events_that_bubble_from_descendants() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.setAttribute('onclick', \"log.push('body:' + (event.target === button))\"); \
             button.click(); log.join(' ')",
        ),
        "body:true"
    );
}

#[test]
fn a_body_that_does_not_parse_leaves_the_handler_null_and_dispatch_goes_on() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onclick', 'this is not ( javascript'); \
             button.addEventListener('click', function () { log.push('listener'); }); \
             button.click(); \
             log.join(' ') + '|' + button.onclick",
        ),
        "listener|null"
    );
}

#[test]
fn an_attribute_that_is_not_a_handler_is_not_compiled() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "button.setAttribute('onfrobnicate', \"log.push('no')\"); \
             button.dispatchEvent(new Event('frobnicate')); \
             log.length + '|' + ('onfrobnicate' in button)",
        ),
        "0|false"
    );
}

#[test]
fn elements_made_by_inner_html_carry_their_handlers() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r##"body.innerHTML =
                 '<p><a id="go" href="#" onclick="log.push(id + tagName); return false;">go</a></p>';
               var link = document.getElementById('go');
               var followed = link.dispatchEvent(
                 new MouseEvent('click', { cancelable: true, bubbles: true }));
               log.join(' ') + '|' + followed"##,
        ),
        "goA|false"
    );
}

// ---- the window ----------------------------------------------------------

#[test]
fn the_window_has_the_global_and_the_window_handlers() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "['onclick', 'onload', 'onresize', 'onscroll', 'onpopstate', 'onhashchange', \
              'onbeforeunload', 'onmessage', 'onunhandledrejection', 'onstorage'] \
               .map(function (name) { return (name in window) + '/' + window[name]; }).join(' ')",
        ),
        ["true/null"; 10].join(" ")
    );
    assert_eq!(
        eval(
            &mut engine,
            "['onpopstate' in button, 'onpopstate' in document, 'ontouchstart' in window].join('|')",
        ),
        "false|false|false"
    );
}

#[test]
fn a_window_handler_runs_for_events_that_reach_the_window() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "window.onclick = function (event) { \
               log.push('window:' + (this === window) + ':' + (event.target === button)); \
             }; \
             onkeydown = function (event) { log.push('global:' + event.type); }; \
             button.click(); \
             button.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true })); \
             log.join(' ')",
        ),
        "window:true:true global:keydown"
    );
}

#[test]
fn onpopstate_receives_history_traversals() {
    let mut engine = engine();
    eval(
        &mut engine,
        "window.onpopstate = function (event) { log.push('popstate:' + event.state); }; \
         history.pushState('one', '', '/one'); history.pushState('two', '', '/two'); \
         history.back();",
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "log.join(' ')"), "popstate:one");
}

#[test]
fn body_attributes_of_the_window_reflecting_set_are_the_windows_handlers() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.setAttribute('onload', \"log.push('load:' + (this === window) + ':' + tagName)\"); \
             body.setAttribute('onhashchange', \"log.push('hashchange')\"); \
             body.setAttribute('onclick', \"log.push('click:' + (this === body))\"); \
             var handler = typeof window.onload; \
             window.dispatchEvent(new Event('load')); \
             window.dispatchEvent(new Event('hashchange')); \
             button.click(); \
             handler + '|' + log.join(' ')",
        ),
        "function|load:true:BODY hashchange click:true",
        "`onload` and `onhashchange` on <body> are the window's, compiled with the body in \
         scope; `onclick` stays the element's"
    );
}

#[test]
fn a_window_handler_assigned_by_script_stands_over_the_body_attribute() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "body.setAttribute('onload', \"log.push('attribute')\"); \
             window.onload = function () { log.push('assigned'); }; \
             window.dispatchEvent(new Event('load')); \
             log.join(' ')",
        ),
        "assigned"
    );
}
