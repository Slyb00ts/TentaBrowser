//! `window.history` on the V8 host: the session history of one document.
//!
//! Routers read `history.state` and call `pushState` or `replaceState` while
//! they start, so a missing `history` stops a single-page application before
//! it renders. Traversal covers the entries the document made; leaving the
//! document through `history` is not built.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

const PAGE: &str = "https://example.com/app/list?page=1";

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

/// Runs the tasks `go`, `back` and `forward` queued.
fn run_tasks(engine: &mut Engine) {
    engine.run_event_loop(16);
}

#[test]
fn history_is_one_object_of_the_history_interface() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "[history === window.history, history instanceof History, \
              Object.prototype.toString.call(history), typeof History, \
              Object.getOwnPropertyNames(History.prototype).sort().join(' ')].join('|')",
        ),
        "true|true|[object History]|function|\
         back constructor forward go length pushState replaceState scrollRestoration state"
    );
    assert_eq!(
        eval(
            &mut engine,
            "(function () { try { new History(); return 'constructed'; } \
              catch (error) { return error.name + ': ' + error.message; } })()",
        ),
        "TypeError: Illegal constructor"
    );
    assert_eq!(
        eval(
            &mut engine,
            "(function () { try { History.prototype.pushState.call({}, 1, ''); return 'ran'; } \
              catch (error) { return error.name; } })()",
        ),
        "TypeError"
    );
}

#[test]
fn a_new_document_has_one_entry_and_no_state() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "[history.length, history.state, history.scrollRestoration].join('|')",
        ),
        "1||auto"
    );
    assert_eq!(eval(&mut engine, "String(history.state === null)"), "true");
}

#[test]
fn push_state_adds_an_entry_and_rewrites_the_document_url() {
    let (mut engine, document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "history.pushState({ item: 7 }, '', 'item/7?tab=a#top'); \
             [history.length, history.state.item, location.href, location.pathname, \
              location.search, location.hash, document.URL].join('|')",
        ),
        "2|7|https://example.com/app/item/7?tab=a#top|/app/item/7|?tab=a|#top|\
         https://example.com/app/item/7?tab=a#top"
    );
    assert_eq!(
        document.borrow().url().map(|url| url.as_str().to_owned()),
        Some("https://example.com/app/item/7?tab=a#top".to_owned()),
        "the host sees the rewritten URL"
    );
    // Without a URL the entry keeps the document's.
    assert_eq!(
        eval(
            &mut engine,
            "history.pushState('text', ''); history.pushState(3, '', null); \
             [history.length, history.state, location.pathname].join('|')",
        ),
        "4|3|/app/item/7"
    );
}

#[test]
fn replace_state_changes_the_current_entry_only() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "history.replaceState({ a: 1 }, '', '/other'); \
             [history.length, history.state.a, location.pathname].join('|')",
        ),
        "1|1|/other"
    );
}

#[test]
fn the_state_is_a_structured_clone_of_the_argument() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "var given = { list: [1, 2], nested: { ok: true } }; \
             history.pushState(given, ''); given.list.push(3); \
             [history.state === given, history.state === history.state, \
              history.state.list.length, history.state.nested.ok].join('|')",
        ),
        "false|true|2|true"
    );
    assert_eq!(
        eval(
            &mut engine,
            "(function () { try { history.pushState(function () {}, ''); return 'pushed'; } \
              catch (error) { return error.name + '|' + history.length; } })()",
        ),
        "DataCloneError|2",
        "a value that cannot be cloned adds no entry"
    );
}

#[test]
fn a_url_the_document_cannot_have_is_a_security_error() {
    let (mut engine, _document) = engine_at(PAGE);
    for url in [
        "https://other.example/",
        "http://example.com/app",
        "//evil.example/x",
        "https://exa mple.com/",
    ] {
        assert_eq!(
            eval(
                &mut engine,
                &format!(
                    "(function () {{ try {{ history.pushState(null, '', '{url}'); return 'pushed'; }} \
                      catch (error) {{ return error.name + '|' + (error instanceof DOMException) + \
                      '|' + history.length + '|' + location.href; }} }})()"
                ),
            ),
            format!("SecurityError|true|1|{PAGE}"),
            "{url}"
        );
    }
}

#[test]
fn push_and_replace_need_two_arguments() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "['pushState', 'replaceState'].map(function (name) { \
               try { history[name]({}); return 'ran'; } catch (error) { return error.name; } \
             }).join('|') + '|' + history.length",
        ),
        "TypeError|TypeError|1"
    );
}

#[test]
fn back_forward_and_go_move_between_the_documents_entries_and_fire_popstate() {
    let (mut engine, _document) = engine_at(PAGE);
    eval(
        &mut engine,
        "globalThis.seen = []; \
         addEventListener('popstate', function (event) { \
           seen.push(event.type + ':' + JSON.stringify(event.state) + ':' + location.pathname + \
                     ':' + (event.state === history.state)); \
         }); \
         history.pushState({ n: 1 }, '', '/one'); history.pushState({ n: 2 }, '', '/two'); \
         history.back();",
    );
    assert_eq!(
        eval(&mut engine, "location.pathname + '|' + seen.length"),
        "/two|0",
        "the traversal is a task: nothing has changed when the script ends"
    );
    run_tasks(&mut engine);
    assert_eq!(
        eval(&mut engine, "seen.join(' ') + '|' + history.length"),
        "popstate:{\"n\":1}:/one:true|3"
    );

    eval(&mut engine, "seen.length = 0; history.go(-1);");
    run_tasks(&mut engine);
    assert_eq!(
        eval(&mut engine, "seen.join(' ') + '|' + location.href"),
        format!("popstate:null:/app/list:true|{PAGE}")
    );

    eval(&mut engine, "seen.length = 0; history.go(2);");
    run_tasks(&mut engine);
    assert_eq!(
        eval(&mut engine, "seen.join(' ')"),
        "popstate:{\"n\":2}:/two:true"
    );

    // A push from the middle drops the entries ahead.
    eval(&mut engine, "seen.length = 0; history.back();");
    run_tasks(&mut engine);
    assert_eq!(
        eval(
            &mut engine,
            "history.pushState({ n: 9 }, '', '/nine'); history.length + '|' + location.pathname",
        ),
        "3|/nine"
    );
    eval(&mut engine, "seen.length = 0; history.forward();");
    run_tasks(&mut engine);
    assert_eq!(
        eval(&mut engine, "seen.length + '|' + location.pathname"),
        "0|/nine",
        "there is no entry ahead any more"
    );
}

#[test]
fn a_step_out_of_the_document_and_a_reload_do_nothing() {
    let (mut engine, _document) = engine_at(PAGE);
    eval(
        &mut engine,
        "globalThis.fired = 0; addEventListener('popstate', function () { fired++; }); \
         history.back(); history.go(-3); history.forward(); history.go(5); history.go(0); \
         history.go();",
    );
    run_tasks(&mut engine);
    assert_eq!(
        eval(
            &mut engine,
            "fired + '|' + history.length + '|' + location.href"
        ),
        format!("0|1|{PAGE}")
    );
}

#[test]
fn scroll_restoration_accepts_its_two_values_only() {
    let (mut engine, _document) = engine_at(PAGE);
    assert_eq!(
        eval(
            &mut engine,
            "history.scrollRestoration = 'manual'; var first = history.scrollRestoration; \
             history.scrollRestoration = 'sideways'; \
             first + '|' + history.scrollRestoration",
        ),
        "manual|manual"
    );
}

#[test]
fn a_realm_without_a_document_has_a_history_at_about_blank() {
    let mut engine = Engine::new();
    assert_eq!(
        engine
            .evaluate("history.length + '|' + history.state + '|' + typeof history.pushState")
            .expect("no document"),
        "1|null|function"
    );
}
