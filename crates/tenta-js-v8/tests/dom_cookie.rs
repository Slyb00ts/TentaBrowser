//! `document.cookie` (P2 slice d-4b b-1), offline and JS-visible.
//!
//! The member is the HTML Standard's cookie getter and setter
//! (<https://html.spec.whatwg.org/multipage/dom.html#dom-document-cookie>),
//! which delegate to RFC 6265bis's "cookie-string" and "receive a
//! set-cookie-string" with the non-HTTP API flag set. Both directions therefore
//! reduce to two calls on the browsing context's jar, and the tests below pin
//! the three things a binding can get wrong on its own: the *join* the getter
//! returns, the non-HTTP API flag (which is what hides an `HttpOnly` cookie
//! from script and refuses a script write to one), and the cookie-averse rule.
//!
//! Before this package `document.cookie` did not exist, so
//! `document.cookie = 'x'` silently created a plain own property: no error, no
//! cookie. `assigning_a_cookie_leaves_no_own_property_behind` is that
//! regression, and every test that reads the write back out of the jar is the
//! proof that the property is an accessor over shared storage rather than a
//! JS-side shadow.
//!
//! `crates/tenta-browser/tests/cookie_js.rs` is the end-to-end half — the same
//! jar reaching `tenta-net` over a real socket. This file holds the engine half
//! against a jar only Rust can see.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js_v8::Engine;
use tenta_net::CookieJar;
use tenta_net::policy::cookies::{SameSiteContext, Site, StoreOutcome};
use url::Url;

/// `<html><head></head><body></body></html>` over a document with `url`, or over
/// a document with no URL at all when `url` is `None` — the first cookie-averse
/// case.
fn document(url: Option<&str>) -> Rc<RefCell<Document>> {
    let mut doc = match url {
        Some(url) => Document::with_url(Url::parse(url).expect("the test URL parses")),
        None => Document::new(),
    };
    let root = doc.document_node();
    let html = doc.create_html_element(atoms::HTML);
    let head = doc.create_html_element(atoms::HEAD);
    let body = doc.create_html_element(atoms::BODY);
    for (parent, child) in [(root, html), (html, head), (html, body)] {
        doc.mutate(Mutation::Append { parent, child })
            .expect("append the element");
    }
    Rc::new(RefCell::new(doc))
}

/// A URL for a test, parsed.
fn url(text: &str) -> Url {
    Url::parse(text).expect("the test URL parses")
}

/// An engine over `doc` sharing `jar`, the way a page hands its context's jar to
/// the engine. The clone is a clone of the handle, not of the store, so the same
/// jar is readable from Rust after the script has run.
fn engine(doc: Rc<RefCell<Document>>, jar: &CookieJar) -> Engine {
    Engine::with_document_on_clock(doc, Clock::virtual_at(MonoTime::ZERO), None, jar.clone())
}

/// Evaluates `source` in an engine over `doc` sharing `jar`, panicking on a
/// throw.
fn eval(doc: Rc<RefCell<Document>>, jar: &CookieJar, source: &str) -> String {
    engine(doc, jar)
        .evaluate(source)
        .expect("the test script should evaluate without throwing")
}

/// The cookies `is_http = true` sees for `page` — what the network stack would
/// send, so a cookie stored by script is visible here.
fn sent_to(jar: &CookieJar, page: &Url) -> Vec<tenta_net::policy::cookies::Cookie> {
    jar.cookies_for(
        page,
        Site::from_url(page).as_ref(),
        true,
        SameSiteContext::same_site(),
        SystemTime::now(),
    )
}

#[test]
fn the_cookie_getter_returns_the_cookie_string() {
    // Two cookies, one path, one creation time, so the only ordering left is the
    // name tie-break: the result is `a=1; b=2` and both the separator and the
    // order are what the assertion pins. This is RFC 6265bis's "cookie-string",
    // which `document.cookie` returns verbatim.
    let jar = CookieJar::new();
    let page = url("https://example.com/page.html");
    let now = SystemTime::now();
    assert!(jar.set_cookie(b"a=1", &page, true, now).is_some());
    assert!(jar.set_cookie(b"b=2", &page, true, now).is_some());

    assert_eq!(
        eval(
            document(Some("https://example.com/page.html")),
            &jar,
            "document.cookie",
        ),
        "a=1; b=2",
    );
}

#[test]
fn an_httponly_cookie_is_invisible_to_script() {
    // The security assertion of the slice. The jar holds one `HttpOnly` cookie
    // and one normal cookie; the getter's `is_http = false` is what makes
    // `cookies_for` drop the first. The non-vacuous half is that the *name* of
    // the HttpOnly cookie is absent — a getter that returned the whole jar, or
    // an unfiltered join, fails on exactly that.
    let jar = CookieJar::new();
    let page = url("https://example.com/page.html");
    let now = SystemTime::now();
    jar.set_cookie(b"secret=1; HttpOnly", &page, true, now);
    jar.set_cookie(b"visible=2", &page, true, now);

    let got = eval(
        document(Some("https://example.com/page.html")),
        &jar,
        "document.cookie",
    );
    assert!(
        got.contains("visible=2"),
        "the normal cookie is visible to script: {got:?}"
    );
    assert!(
        !got.contains("secret"),
        "an HttpOnly cookie's name must be absent from the cookie string: {got:?}"
    );
}

#[test]
fn script_cannot_overwrite_an_httponly_cookie() {
    // The same flag on the setter side: `CookieJar::store` refuses
    // `!is_http && existing.http_only` with `RejectedHttpOnly` (RFC 6265bis
    // §5.7 step 21), and the binding discards that outcome silently, as the
    // standard's setter does. Read the value back with `is_http = true`, so the
    // assertion is about the stored value and not about the getter's filter.
    let jar = CookieJar::new();
    let page = url("https://example.com/page.html");
    assert_eq!(
        jar.set_cookie(b"secret=1; HttpOnly", &page, true, SystemTime::now()),
        Some(StoreOutcome::Stored),
    );

    eval(
        document(Some("https://example.com/page.html")),
        &jar,
        "document.cookie = 'secret=2'",
    );

    let stored = sent_to(&jar, &page);
    let secret = stored
        .iter()
        .find(|cookie| cookie.name == "secret")
        .expect("the HttpOnly cookie is still stored");
    assert_eq!(
        secret.value, "1",
        "a non-HTTP API assignment must not overwrite an HttpOnly cookie"
    );
}

#[test]
fn the_setter_stores_a_cookie_the_jar_can_see() {
    // The write must reach the shared storage rather than a JS-side shadow: the
    // jar is read here from Rust, and it is the same handle a browsing context
    // hands its network client.
    let jar = CookieJar::new();
    let page = url("https://example.com/page.html");

    eval(
        document(Some("https://example.com/page.html")),
        &jar,
        "document.cookie = 'c=3'",
    );

    let stored = sent_to(&jar, &page);
    assert!(
        stored
            .iter()
            .any(|cookie| cookie.name == "c" && cookie.value == "3"),
        "the assignment reached the jar the network stack reads: {stored:?}"
    );
}

#[test]
fn the_setter_honours_attributes() {
    // The test a naive implementation fails. Splitting at the first `=` and
    // treating the rest as the value would store `d` with the value `4;
    // Path=/sub` at the default path; the standard's parser splits at the first
    // `;` into the name-value pair and the attribute list, so `d` is stored as
    // `4` with path `/sub`.
    let jar = CookieJar::new();

    eval(
        document(Some("https://example.com/page.html")),
        &jar,
        "document.cookie = 'd=4; Path=/sub'",
    );

    let stored = jar.cookies();
    let d = stored
        .iter()
        .find(|cookie| cookie.name == "d")
        .expect("the cookie is stored");
    assert_eq!(d.value, "4", "the attribute list is not part of the value");
    assert_eq!(d.path, "/sub", "the Path attribute is honoured from script");
}

#[test]
fn an_assignment_without_an_equals_sign_is_ignored() {
    // The standard's storage algorithm ignores a set-cookie-string that has no
    // `=` before the first `;`, and the setter is silent about it. `eval`
    // panics on a throw, so this also pins that no exception escapes.
    let jar = CookieJar::new();

    eval(
        document(Some("https://example.com/page.html")),
        &jar,
        "document.cookie = 'nope'",
    );

    assert!(
        jar.cookies().is_empty(),
        "an assignment with no `=` stores nothing: {:?}",
        jar.cookies()
    );
}

#[test]
fn a_cookie_for_another_path_is_not_returned() {
    // Path matching is the jar's (RFC 6265 §5.1.4). A document at `/page.html`
    // is not under `/other`, so a cookie scoped there is absent from the cookie
    // string — the non-vacuous half being that the sibling cookie *is* present.
    let jar = CookieJar::new();
    let page = url("https://example.com/page.html");
    let now = SystemTime::now();
    jar.set_cookie(b"mine=1", &page, true, now);
    jar.set_cookie(b"other=2; Path=/other", &page, true, now);

    assert_eq!(
        eval(
            document(Some("https://example.com/page.html")),
            &jar,
            "document.cookie",
        ),
        "mine=1",
    );
}

#[test]
fn a_cookie_averse_document_reports_nothing_and_stores_nothing() {
    // The standard's cookie-averse `Document`: no URL, or a URL whose scheme is
    // neither `http` nor `https`. Both observable consequences are asserted for
    // both cases — the getter is the empty string, and the setter is a silent
    // no-op that leaves the jar empty.
    let jar = CookieJar::new();

    assert_eq!(eval(document(None), &jar, "document.cookie"), "");
    eval(document(None), &jar, "document.cookie = 'nope=1'");
    assert!(
        jar.cookies().is_empty(),
        "a document with no URL stores nothing: {:?}",
        jar.cookies()
    );

    // A `file:` URL has a scheme the rule refuses even though the tree can build
    // it. This is the case a getter wired to `canonical_host`'s permissiveness
    // would get wrong.
    assert_eq!(
        eval(document(Some("file:///tmp/x")), &jar, "document.cookie"),
        ""
    );
    eval(
        document(Some("file:///tmp/x")),
        &jar,
        "document.cookie = 'nope=1'",
    );
    assert!(
        jar.cookies().is_empty(),
        "a file: document stores nothing: {:?}",
        jar.cookies()
    );
}

#[test]
fn assigning_a_cookie_leaves_no_own_property_behind() {
    // The regression this package exists to fix: a plain assignment must reach
    // the accessor on the prototype, not define an own data property on the
    // wrapper. `Object.getOwnPropertyDescriptor(document, 'cookie')` staying
    // `undefined` proves the assignment went through the setter; the jar read in
    // `the_setter_stores_a_cookie_the_jar_can_see` proves the setter ran.
    let jar = CookieJar::new();

    assert_eq!(
        eval(
            document(Some("https://example.com/page.html")),
            &jar,
            r#"
            document.cookie = 'x=1';
            [
                document.hasOwnProperty('cookie'),
                Object.getOwnPropertyDescriptor(document, 'cookie') === undefined,
                Object.getOwnPropertyNames(document).includes('cookie'),
            ].join('|');
        "#,
        ),
        "false|true|false",
    );
}
