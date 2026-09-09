//! Tests for Phase 4a-2b: the `navigator` and `performance` globals. Frameworks
//! read `navigator.userAgent`/`language` and prefer `performance.now()` for
//! scheduling; `performance.now()` reads the event loop's clock, so it advances
//! with the virtual scheduler under tests.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    Rc::new(RefCell::new(document))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn navigator_exposes_the_expected_shape() {
    assert_eq!(
        eval(
            r#"
            [
                typeof navigator,
                navigator.userAgent.includes('Chrome'),
                navigator.language,
                Array.isArray(navigator.languages),
                navigator.languages.join('|'),
                navigator.onLine,
                typeof navigator.hardwareConcurrency === 'number' && navigator.hardwareConcurrency >= 1,
                navigator.vendor,
                navigator.product,
            ].join(',');
        "#
        ),
        "object,true,en-US,true,en-US|en,true,true,Google Inc.,Gecko"
    );
}

#[test]
fn navigator_exposes_the_chrome_compatibility_shape() {
    // Unguarded probes real sites (and Teams-style compatibility checks) run:
    // an absent property here would throw a `TypeError` instead of reading a
    // present, Chrome-shaped value.
    assert_eq!(
        eval(
            r#"
            [
                navigator.webdriver,
                navigator.cookieEnabled,
                Array.isArray(navigator.plugins) && navigator.plugins.length,
                Array.isArray(navigator.mimeTypes) && navigator.mimeTypes.length,
                navigator.maxTouchPoints,
                navigator.appVersion.startsWith('5.0'),
                navigator.userAgent.endsWith('Mozilla/' + navigator.appVersion),
                navigator.productSub,
            ].join(',');
        "#
        ),
        "false,true,0,0,0,true,true,20030107"
    );
}

#[test]
fn hardware_concurrency_is_a_deterministic_value() {
    // Reported as a fixed profile value, not the real host core count.
    assert_eq!(eval("navigator.hardwareConcurrency"), "8");
}

#[test]
fn navigator_is_reachable_through_window() {
    assert_eq!(eval("navigator === window.navigator"), "true");
}

#[test]
fn performance_now_and_time_origin() {
    // On the virtual clock the loop origin is 0, so a synchronous read of
    // `performance.now()` is 0 and `timeOrigin` is 0.
    assert_eq!(
        eval(
            r#"
            [
                typeof performance,
                typeof performance.now,
                performance.now(),
                performance.timeOrigin,
                performance === window.performance,
            ].join(',');
        "#
        ),
        "object,function,0,0,true"
    );
}

#[test]
fn performance_now_advances_with_the_frame_clock() {
    // Ticking a frame advances the virtual clock by one 60fps interval
    // (~16.67 ms), which `performance.now()` reflects.
    let mut engine = Engine::with_document(test_document());
    let before = engine
        .evaluate("performance.now()")
        .expect("read performance.now before the frame");
    engine.tick_frame();
    let after = engine
        .evaluate("performance.now()")
        .expect("read performance.now after the frame");

    assert_eq!(before, "0");
    let after: f64 = after.parse().expect("performance.now returns a number");
    assert!(
        after > 16.0 && after < 17.0,
        "one frame should advance the clock by ~16.67 ms, got {after}"
    );
}

#[test]
fn performance_now_is_monotonically_non_decreasing_across_frames() {
    // The clock only moves forward, so successive reads never go backwards.
    let mut engine = Engine::with_document(test_document());
    let mut previous = 0.0_f64;
    for _ in 0..3 {
        let sample: f64 = engine
            .evaluate("performance.now()")
            .expect("read performance.now")
            .parse()
            .expect("performance.now returns a number");
        assert!(
            sample >= previous,
            "performance.now() must not go backwards: {sample} < {previous}"
        );
        previous = sample;
        engine.tick_frame();
    }
    assert!(
        previous > 0.0,
        "three frames should have advanced the clock past the origin, got {previous}"
    );
}
