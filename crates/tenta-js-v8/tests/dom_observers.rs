//! `ResizeObserver` and `IntersectionObserver` on the V8 host.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    Engine::with_document(Rc::new(RefCell::new(document)))
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

/// Both interface objects exist, are not enumerable on the global, and behave
/// as constructors: `new` and a callback are required, and the methods check
/// what they are called on and with.
#[test]
fn the_observer_interfaces_are_constructors() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var attempt = f => { try { return String(f()); } catch (e) { return e.name; } };
             var io = new IntersectionObserver(function () {}, {rootMargin: '10px', threshold: [0, 0.5]});
             var plain = new IntersectionObserver(function () {});
             var ro = new ResizeObserver(function () {});
             [typeof ResizeObserver, typeof IntersectionObserver,
              ResizeObserver === ResizeObserver, ResizeObserver.name, ResizeObserver.length,
              Object.keys(globalThis).includes('ResizeObserver'), 'IntersectionObserver' in globalThis,
              ro instanceof ResizeObserver, io instanceof IntersectionObserver,
              Object.prototype.toString.call(ro), Object.prototype.toString.call(io),
              String(io.root), io.rootMargin, io.thresholds.join(), plain.rootMargin, plain.thresholds.join(),
              io.takeRecords().length, typeof ro.takeRecords,
              attempt(() => ResizeObserver(function () {})), attempt(() => new ResizeObserver()),
              attempt(() => new IntersectionObserver(1)), attempt(() => ro.observe({})),
              attempt(() => ro.observe(null)), attempt(() => io.observe(document)),
              attempt(() => ro.observe.call({}, document.createElement('div'))),
              attempt(() => ro.unobserve(document.createElement('div'))), attempt(() => ro.disconnect()),
              String(ResizeObserver).includes('[native code]')].join('|')",
        ),
        "function|function|true|ResizeObserver|1|false|true|true|true|\
         [object ResizeObserver]|[object IntersectionObserver]|\
         null|10px|0,0.5|0px 0px 0px 0px|0|0|undefined|\
         TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|undefined|undefined|true"
    );
}

/// An `IntersectionObserver` tells its callback, once and in a task of its
/// own, that every target it was given is in view. A target that stopped
/// being observed first is not reported, and one observed twice is reported
/// once.
#[test]
fn an_intersection_observer_reports_each_target_as_in_view_once() {
    let mut engine = engine();
    eval(
        &mut engine,
        "globalThis.calls = [];
         var a = document.createElement('div'), b = document.createElement('img'),
             c = document.createElement('p');
         globalThis.io = new IntersectionObserver(function (entries, observer) {
           calls.push(entries.map(function (e) {
             return e.target.tagName + ':' + e.isIntersecting + ':' + e.intersectionRatio + ':' +
                    (typeof e.time) + ':' + (e.rootBounds === null);
           }).join(',') + ' ' + (observer === io) + ' ' + (this === io));
         });
         io.observe(a); io.observe(b); io.observe(a); io.observe(c); io.unobserve(c);
         globalThis.late = a; 'observing'",
    );
    assert_eq!(
        eval(&mut engine, "calls.length"),
        "0",
        "never inside observe()"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "calls.join('|')"),
        "DIV:true:1:number:true,IMG:true:1:number:true true true"
    );

    // A target added later gets its own notification; the old ones are not
    // reported again, and a disconnected observer reports nothing.
    eval(
        &mut engine,
        "var d = document.createElement('span'); io.observe(d); io.observe(late); 'more'",
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "calls.length + ' ' + calls[1]"),
        "2 SPAN:true:1:number:true true true"
    );
    eval(
        &mut engine,
        "io.observe(document.createElement('b')); io.disconnect(); 'gone'",
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "calls.length"), "2");
}

/// A `ResizeObserver` reports a target's box once, and — as the standard has
/// it — only when the box is not empty.
#[test]
fn a_resize_observer_reports_a_box_that_is_not_empty() {
    let mut engine = engine();
    eval(
        &mut engine,
        "globalThis.seen = [];
         var empty = document.createElement('div');
         var sized = document.createElement('div');
         sized.getBoundingClientRect = function () {
           return {x: 1, y: 2, width: 300, height: 150, top: 2, left: 1, right: 301, bottom: 152};
         };
         globalThis.ro = new ResizeObserver(function (entries, observer) {
           entries.forEach(function (e) {
             seen.push([e.target === sized, e.contentRect.width, e.contentRect.height,
                        e.borderBoxSize[0].inlineSize, e.contentBoxSize[0].blockSize,
                        e.devicePixelContentBoxSize.length, observer === ro].join(':'));
           });
         });
         ro.observe(empty); ro.observe(sized); 'observing'",
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "seen.join('|')"),
        "true:300:150:300:150:1:true"
    );
}
