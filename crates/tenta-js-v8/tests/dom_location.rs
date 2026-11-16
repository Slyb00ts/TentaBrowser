//! `window.location`, `atob` and `btoa` on the V8 host.
//!
//! Scripts of real pages read `location` before anything else. Writing to it
//! navigates: a change of the fragment stays in the document (URL, history
//! entry, `hashchange`), anything else is a request the page takes once the
//! script entry returns (`tenta_dom::NavigationRequest`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn document_at(url: Option<&str>) -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    document.set_url(url.map(|url| url.parse().expect("test URL")));
    Rc::new(RefCell::new(document))
}

fn eval_at(url: Option<&str>, source: &str) -> String {
    let mut engine = Engine::with_document(document_at(url));
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

const PAGE: &str = "https://user:pw@example.com:8443/a/b.html?x=1&y=2#top";

#[test]
fn location_reports_every_component_of_the_document_url() {
    assert_eq!(
        eval_at(
            Some(PAGE),
            "[location.href, location.origin, location.protocol, location.host, \
              location.hostname, location.port, location.pathname, location.search, \
              location.hash].join(' ')",
        ),
        "https://user:pw@example.com:8443/a/b.html?x=1&y=2#top https://example.com:8443 https: \
         example.com:8443 example.com 8443 /a/b.html ?x=1&y=2 #top"
    );
}

#[test]
fn location_stringifies_to_href() {
    assert_eq!(
        eval_at(
            Some(PAGE),
            "[String(location) === location.href, location.toString() === location.href, \
              ('' + location) === location.href, `${location}` === location.href].join(',')",
        ),
        "true,true,true,true"
    );
}

#[test]
fn a_document_without_a_url_is_at_about_blank() {
    assert_eq!(
        eval_at(
            None,
            "[location.href, location.origin, location.protocol, location.host, \
              location.pathname].join('|')",
        ),
        "about:blank|null|about:||blank"
    );
}

#[test]
fn a_realm_without_a_document_is_at_about_blank() {
    let mut engine = Engine::new();
    assert_eq!(
        engine.evaluate("location.href").expect("no document"),
        "about:blank"
    );
}

#[test]
fn location_follows_the_document_url() {
    let document = document_at(Some("https://example.com/first"));
    let mut engine = Engine::with_document(document.clone());
    assert_eq!(
        engine.evaluate("location.pathname").expect("first"),
        "/first"
    );
    document
        .borrow_mut()
        .set_url(Some("https://example.com/second?q#h".parse().expect("url")));
    assert_eq!(
        engine
            .evaluate("location.pathname + location.search + location.hash")
            .expect("second"),
        "/second?q#h"
    );
}

#[test]
fn every_alias_of_the_window_names_one_location_object() {
    assert_eq!(
        eval_at(
            Some(PAGE),
            "[window.location === location, self.location === location, \
              top.location === location, location === location].join(',')",
        ),
        "true,true,true,true"
    );
}

#[test]
fn location_is_an_unforgeable_instance_of_the_location_interface() {
    assert_eq!(
        eval_at(
            Some(PAGE),
            "(function () {
                var own = Object.getOwnPropertyDescriptor(window, 'location');
                var href = Object.getOwnPropertyDescriptor(location, 'href');
                var text = Object.getOwnPropertyDescriptor(location, 'toString');
                return [
                    typeof Location, location instanceof Location,
                    Object.prototype.toString.call(location),
                    typeof own.get, own.configurable, own.enumerable,
                    typeof href.get, href.configurable, href.enumerable,
                    text.writable, text.configurable,
                    delete window.location, delete location.href,
                    Object.keys(location).join('/'),
                ].join(',');
            })()",
        ),
        "function,true,[object Location],function,false,true,function,false,true,false,false,\
         false,false,\
         href/origin/protocol/host/hostname/port/pathname/search/hash/toString/assign/replace/reload"
    );
}

fn thrown(source: &str) -> String {
    eval_at(
        Some(PAGE),
        &format!(
            "(function () {{ try {{ {source}; return 'no exception'; }} \
             catch (e) {{ return e.name + ': ' + e.message; }} }})()"
        ),
    )
}

#[test]
fn the_interface_cannot_be_constructed_and_its_getters_reject_other_receivers() {
    assert_eq!(thrown("new Location()"), "TypeError: Illegal constructor");
    assert_eq!(thrown("Location()"), "TypeError: Illegal constructor");
    assert_eq!(
        thrown("Object.getOwnPropertyDescriptor(location, 'href').get.call({})"),
        "TypeError: Illegal invocation"
    );
    assert_eq!(
        thrown("location.toString.call({})"),
        "TypeError: Illegal invocation"
    );
    assert_eq!(
        thrown("Object.getOwnPropertyDescriptor(window, 'location').get.call({})"),
        "TypeError: Illegal invocation"
    );
}

/// An engine over a document at `url`, and the document.
fn engine_at(url: &str) -> (Engine, Rc<RefCell<Document>>) {
    let document = document_at(Some(url));
    (Engine::with_document(document.clone()), document)
}

const APP: &str = "https://example.com/app/list?page=1";

#[test]
fn assigning_a_url_asks_the_page_to_navigate_and_leaves_the_document_alone() {
    for (script, expected, replace) in [
        (
            "location.href = '/next?x=1'",
            "https://example.com/next?x=1",
            false,
        ),
        (
            "location = 'item/7'",
            "https://example.com/app/item/7",
            false,
        ),
        (
            "window.location = 'https://other.example/'",
            "https://other.example/",
            false,
        ),
        ("location.assign('../up')", "https://example.com/up", false),
        (
            "location.replace('/gone')",
            "https://example.com/gone",
            true,
        ),
        (
            "location.search = '?page=2'",
            "https://example.com/app/list?page=2",
            false,
        ),
        (
            "location.pathname = '/other'",
            "https://example.com/other?page=1",
            false,
        ),
        (
            "location.hostname = 'other.example'",
            "https://other.example/app/list?page=1",
            false,
        ),
        ("location.reload()", APP, true),
    ] {
        let (mut engine, document) = engine_at(APP);
        let href = engine
            .evaluate(&format!("{script}; location.href"))
            .expect("the assignment does not throw");
        assert_eq!(
            href, APP,
            "{script}: the document keeps its URL until the page navigates"
        );
        let request = document
            .borrow_mut()
            .take_navigation_request()
            .unwrap_or_else(|| panic!("{script}: a navigation request"));
        assert_eq!(request.url.as_str(), expected, "{script}");
        assert_eq!(request.replace, replace, "{script}");
        assert_eq!(request.post, None, "{script}");
        assert_eq!(document.borrow_mut().take_navigation_request(), None);
    }
}

#[test]
fn the_last_request_of_a_script_wins() {
    let (mut engine, document) = engine_at(APP);
    engine
        .evaluate("location.assign('/one'); location.assign('/two'); 'done'")
        .expect("two assignments");
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("a request");
    assert_eq!(request.url.as_str(), "https://example.com/two");
}

#[test]
fn a_fragment_change_stays_in_the_document() {
    let (mut engine, document) = engine_at(APP);
    let seen = engine
        .evaluate(
            "globalThis.seen = []; \
             addEventListener('hashchange', function (event) { \
               seen.push(event.type + ' ' + event.oldURL + ' -> ' + event.newURL); \
             }); \
             location.hash = 'first'; \
             var afterFirst = location.href + '|' + history.length + '|' + history.state; \
             location.href = '#second'; \
             location.replace('#third'); \
             [afterFirst, location.hash, history.length, seen.length].join('|')",
        )
        .expect("fragment navigations");
    assert_eq!(
        seen,
        format!("{APP}#first|2|null|#third|3|0"),
        "the URL and the history change at once; `hashchange` is a task; `replace` adds no entry"
    );
    assert_eq!(
        document.borrow_mut().take_navigation_request(),
        None,
        "nothing was asked of the page"
    );
    engine.run_event_loop(16);
    assert_eq!(
        engine.evaluate("seen.join(' ; ')").expect("the events"),
        format!(
            "hashchange {APP} -> {APP}#first ; hashchange {APP}#first -> {APP}#second ; \
             hashchange {APP}#second -> {APP}#third"
        )
    );
    // Going back over a fragment entry restores its URL.
    engine.evaluate("history.back(); 'queued'").expect("back");
    engine.run_event_loop(16);
    assert_eq!(
        engine.evaluate("location.href").expect("the URL"),
        format!("{APP}#first")
    );
}

#[test]
fn the_same_fragment_again_changes_nothing() {
    let (mut engine, _document) = engine_at("https://example.com/app#here");
    engine
        .evaluate(
            "globalThis.fired = 0; addEventListener('hashchange', function () { fired++; }); \
             location.hash = '#here'; 'set'",
        )
        .expect("the same fragment");
    engine.run_event_loop(16);
    assert_eq!(
        engine
            .evaluate("fired + '|' + history.length")
            .expect("state"),
        "0|1"
    );
}

#[test]
fn javascript_urls_and_unparsable_urls_do_not_navigate() {
    let (mut engine, document) = engine_at(APP);
    assert_eq!(
        engine
            .evaluate(
                "location.href = 'javascript:void 0'; \
                 (function () { try { location.assign('https://exa mple.com/'); return 'ok'; } \
                   catch (error) { return error.name; } })()",
            )
            .expect("the script"),
        "SyntaxError"
    );
    assert_eq!(document.borrow_mut().take_navigation_request(), None);
    assert_eq!(
        thrown("location.assign()"),
        "TypeError: Failed to execute 'assign' on 'Location': 1 argument required, but only 0 present."
    );
}

#[test]
fn origin_cannot_be_assigned() {
    let (mut engine, document) = engine_at(APP);
    assert_eq!(
        engine
            .evaluate("location.origin = 'https://other.example'; location.origin")
            .expect("a sloppy assignment is ignored"),
        "https://example.com"
    );
    assert_eq!(document.borrow_mut().take_navigation_request(), None);
}

#[test]
fn btoa_and_atob_are_the_binary_string_codecs() {
    assert_eq!(
        eval_at(
            None,
            "[btoa('foobar'), atob('Zm9vYmFy'), atob(' Zm9v\\tYg '), btoa(''), atob(''), \
              btoa(null), btoa(String.fromCharCode(0, 255, 254)), \
              atob('AP/+').charCodeAt(1), atob('AP/+').length, \
              atob(btoa('caf\\u00e9')) === 'caf\\u00e9', btoa.length, atob.length].join(',')",
        ),
        "Zm9vYmFy,foobar,foob,,,bnVsbA==,AP/+,255,3,true,0,0"
    );
}

#[test]
fn btoa_and_atob_refuse_what_the_standard_refuses() {
    assert_eq!(
        thrown("btoa('\\u0100')"),
        "InvalidCharacterError: The string to be encoded contains characters outside of the \
         Latin1 range."
    );
    assert_eq!(
        thrown("btoa('\\ud83d')"),
        "InvalidCharacterError: The string to be encoded contains characters outside of the \
         Latin1 range."
    );
    for refused in ["Z", "Zm9vY", "Z=g=", "Zm-v", "Zg==="] {
        assert_eq!(
            thrown(&format!("atob('{refused}')")),
            "InvalidCharacterError: The string to be decoded is not correctly encoded.",
            "{refused}"
        );
    }
    assert_eq!(
        thrown(
            "try { atob('Z') } catch (e) { throw new Error(String(e instanceof DOMException) + e.code) }"
        ),
        "Error: true5"
    );
    assert_eq!(
        thrown("btoa()"),
        "TypeError: Failed to execute 'btoa' on 'Window': 1 argument required, but only 0 present."
    );
    assert_eq!(
        thrown("atob()"),
        "TypeError: Failed to execute 'atob' on 'Window': 1 argument required, but only 0 present."
    );
    assert_eq!(
        thrown("btoa({ toString() { throw new RangeError('mine') } })"),
        "RangeError: mine"
    );
}
