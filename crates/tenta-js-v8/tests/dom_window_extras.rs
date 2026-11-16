//! `matchMedia`, the scrolling methods and `requestIdleCallback` on the V8
//! host.

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

#[test]
fn match_media_evaluates_the_query_against_the_viewport() {
    let mut engine = engine();
    // The viewport of an engine without a page is what `innerWidth` reports.
    assert_eq!(
        eval(
            &mut engine,
            "var w = innerWidth, h = innerHeight; \
             [matchMedia('(min-width: ' + w + 'px)').matches, \
              matchMedia('(min-width: ' + (w + 1) + 'px)').matches, \
              matchMedia('(max-width: ' + w + 'px) and (max-height: ' + h + 'px)').matches, \
              matchMedia('(orientation: ' + (w >= h ? 'landscape' : 'portrait') + ')').matches, \
              matchMedia('screen').matches, matchMedia('print').matches, \
              matchMedia('(prefers-color-scheme: dark)').matches, \
              matchMedia('(prefers-color-scheme: light)').matches, \
              matchMedia('(hover: hover)').matches, matchMedia('not all').matches].join('|')",
        ),
        "true|false|true|true|true|false|false|true|true|false"
    );
}

#[test]
fn a_media_query_list_reports_its_serialised_query() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var list = matchMedia('  SCREEN and (MIN-WIDTH:600px)'); \
             [list.media, matchMedia('@!').media, matchMedia('@!').matches, \
              list instanceof MediaQueryList, list instanceof EventTarget, \
              Object.prototype.toString.call(list), typeof MediaQueryList, \
              Object.getPrototypeOf(MediaQueryList) === EventTarget, \
              matchMedia('screen') !== matchMedia('screen')].join('|')",
        ),
        "screen and (min-width: 600px)|not all|false|true|true|[object MediaQueryList]|function|\
         true|true"
    );
    assert_eq!(
        eval(
            &mut engine,
            "[function () { new MediaQueryList(); }, function () { matchMedia(); }, \
              function () { Object.getOwnPropertyDescriptor(MediaQueryList.prototype, 'matches') \
                .get.call({}); }, \
              function () { MediaQueryList.prototype.addListener.call(new EventTarget(), null); }] \
               .map(function (run) { try { run(); return 'ran'; } catch (error) { \
                 return error.name; } }).join('|')",
        ),
        "TypeError|TypeError|TypeError|TypeError"
    );
}

#[test]
fn listeners_can_be_added_the_old_and_the_new_way() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var list = matchMedia('(min-width: 1px)'); var seen = []; \
             function legacy(event) { seen.push('legacy:' + event.type); } \
             function modern(event) { seen.push('modern:' + (this === list)); } \
             function handler() { seen.push('handler'); } \
             list.addListener(legacy); list.addEventListener('change', modern); \
             list.onchange = handler; \
             var assigned = list.onchange === handler; \
             list.dispatchEvent(new Event('change')); \
             list.removeListener(legacy); list.onchange = null; \
             list.dispatchEvent(new Event('change')); \
             list.addListener(null); \
             [assigned, list.onchange, seen.join(' ')].join('|')",
        ),
        "true||legacy:change modern:true handler modern:true"
    );
}

#[test]
fn the_window_does_not_scroll() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "scrollTo(0, 500); scroll({ top: 100, behavior: 'smooth' }); scrollBy(0, 50); \
             window.scrollTo(); \
             [typeof scrollTo, typeof scroll, typeof scrollBy, scrollX, scrollY, \
              pageYOffset].join('|')",
        ),
        "function|function|function|0|0|0"
    );
}

#[test]
fn an_idle_callback_runs_as_a_task_and_can_be_cancelled() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "globalThis.seen = []; \
             var first = requestIdleCallback(function (deadline) { \
               seen.push([deadline.didTimeout, deadline.timeRemaining() > 0].join(':')); \
             }); \
             var second = requestIdleCallback(function () { seen.push('cancelled'); }, \
                                              { timeout: 100 }); \
             cancelIdleCallback(second); \
             var refused; try { requestIdleCallback('not a function'); refused = 'accepted'; } \
               catch (error) { refused = error.name; } \
             [typeof first, first !== second, seen.length, refused].join('|')",
        ),
        "number|true|0|TypeError"
    );
    engine.run_event_loop(16);
    assert_eq!(eval(&mut engine, "seen.join(' ')"), "false:true");
}
