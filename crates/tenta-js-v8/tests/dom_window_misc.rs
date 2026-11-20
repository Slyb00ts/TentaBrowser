//! The `Window` interface object, dialogs, `screen`, the browsing-context
//! facts, `document.location` and user timing on the V8 host.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine_at(url: &str) -> (Engine, Rc<RefCell<Document>>) {
    let mut document = Document::new();
    document.set_url(Some(url.parse().expect("test URL")));
    let document = Rc::new(RefCell::new(document));
    (Engine::with_document(document.clone()), document)
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn the_window_is_an_instance_of_the_window_interface() {
    let (mut engine, _document) = engine_at("https://example.com/");
    assert_eq!(
        eval(
            &mut engine,
            "[typeof Window, window instanceof Window, window instanceof EventTarget, \
              Object.getPrototypeOf(window) === Window.prototype, \
              Object.getPrototypeOf(Window.prototype) === EventTarget.prototype, \
              Object.getPrototypeOf(Window) === EventTarget, \
              Object.prototype.toString.call(Window.prototype), \
              typeof window.addEventListener].join('|')",
        ),
        "function|true|true|true|true|true|[object Window]|function"
    );
    assert_eq!(
        eval(
            &mut engine,
            "(function () { try { new Window(); return 'made'; } catch (error) { \
               return error.name; } })()",
        ),
        "TypeError"
    );
}

#[test]
fn dialogs_are_dismissed_at_once() {
    let (mut engine, _document) = engine_at("https://example.com/");
    assert_eq!(
        eval(
            &mut engine,
            "[alert('saved'), confirm('delete it?'), prompt('name?', 'default'), alert(), \
              print(), stop(), focus(), blur()].map(String).join('|')",
        ),
        "undefined|false|null|undefined|undefined|undefined|undefined|undefined"
    );
}

#[test]
fn the_browsing_context_reports_itself() {
    let (mut secure, _document) = engine_at("https://example.com:8443/app");
    assert_eq!(
        eval(
            &mut secure,
            "[origin, isSecureContext, name === '', closed, length, opener, frameElement, \
              crossOriginIsolated, self === window, top === window, parent === window].join('|')",
        ),
        "https://example.com:8443|true|true|false|0|||false|true|true|true"
    );
    let (mut plain, _document) = engine_at("http://example.com/");
    assert_eq!(
        eval(&mut plain, "origin + '|' + isSecureContext"),
        "http://example.com|false"
    );
    let (mut local, _document) = engine_at("http://localhost:3000/");
    assert_eq!(eval(&mut local, "String(isSecureContext)"), "true");
}

#[test]
fn the_screen_is_the_viewport() {
    let (mut engine, _document) = engine_at("https://example.com/");
    assert_eq!(
        eval(
            &mut engine,
            "[screen.width === innerWidth, screen.height === innerHeight, \
              screen.availWidth === screen.width, screen.availHeight === screen.height, \
              screen.colorDepth, screen.pixelDepth, screen.availLeft, screen.availTop].join('|')",
        ),
        "true|true|true|true|24|24|0|0"
    );
}

#[test]
fn document_location_is_the_windows_location() {
    let (mut engine, document) = engine_at("https://example.com/app");
    assert_eq!(
        eval(
            &mut engine,
            "document.location = '/next'; \
             [document.location === window.location, document.location.href].join('|')",
        ),
        "true|https://example.com/app"
    );
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("assigning document.location navigates");
    assert_eq!(request.url.as_str(), "https://example.com/next");
}

#[test]
fn marks_and_measures_are_recorded() {
    let (mut engine, _document) = engine_at("https://example.com/");
    assert_eq!(
        eval(
            &mut engine,
            "var first = performance.mark('start'); performance.mark('end'); \
             var between = performance.measure('work', 'start', 'end'); \
             var sinceOrigin = performance.measure('all'); \
             var missing; try { performance.measure('x', 'no such mark'); missing = 'measured'; } \
               catch (error) { missing = error.name; } \
             var counts = [performance.getEntries().length, \
                           performance.getEntriesByType('mark').length, \
                           performance.getEntriesByType('measure').length, \
                           performance.getEntriesByName('work').length, \
                           performance.getEntriesByName('start', 'measure').length, \
                           performance.getEntriesByType('navigation').length].join(','); \
             performance.clearMarks('start'); \
             var afterNamed = performance.getEntriesByType('mark').length; \
             performance.clearMarks(); performance.clearMeasures(); \
             [first.name, first.entryType, first.duration, typeof first.startTime, \
              between.entryType, between.duration >= 0, sinceOrigin.startTime, missing, counts, \
              afterNamed, performance.getEntries().length].join('|')",
        ),
        // The document's navigation entry is always there: one of the five
        // entries, and the one that is left when the rest are cleared.
        "start|mark|0|number|measure|true|0|SyntaxError|5,2,2,1,0,1|1|1"
    );
    assert_eq!(
        eval(
            &mut engine,
            "[typeof performance.timing.navigationStart, \
              performance.timing.navigationStart === Math.round(performance.timeOrigin), \
              performance.timing.loadEventEnd === performance.timing.responseStart, \
              performance.navigation.type, typeof performance.now()].join('|')",
        ),
        "number|true|true|0|number"
    );
}

#[test]
fn a_measure_may_start_at_a_timing_milestone() {
    let (mut engine, _document) = engine_at("https://example.com/");
    assert_eq!(
        eval(
            &mut engine,
            "performance.mark('ready'); \
             var sinceNavigation = performance.measure('ttfb', 'navigationStart', 'ready'); \
             [sinceNavigation.startTime, sinceNavigation.duration >= 0, \
              navigator.javaEnabled()].join('|')",
        ),
        "0|true|false"
    );
}
