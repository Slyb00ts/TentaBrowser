//! `HTMLHyperlinkElementUtils` on `<a>` and `<area>` on the V8 host: the
//! element's `href` taken apart and put back together, which is how script
//! parsed URLs before `URL` existed and how a good deal of it still does.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html?here=1#top"
            .parse()
            .expect("test URL"),
    ));
    Engine::with_document(Rc::new(RefCell::new(document)))
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn an_anchor_takes_its_href_apart() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'); \
             a.href = 'https://user:secret@shop.example:8443/a/b.html?q=1&r=2#frag'; \
             [a.href, a.origin, a.protocol, a.username, a.password, a.host, a.hostname, \
              a.port, a.pathname, a.search, a.hash].join(' ')",
        ),
        "https://user:secret@shop.example:8443/a/b.html?q=1&r=2#frag https://shop.example:8443 \
         https: user secret shop.example:8443 shop.example 8443 /a/b.html ?q=1&r=2 #frag"
    );
}

#[test]
fn a_relative_href_is_resolved_against_the_document() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'); a.setAttribute('href', '../other?x=y'); \
             [a.href, a.protocol, a.host, a.port, a.pathname, a.search, a.hash, \
              a.getAttribute('href')].join(' | ')",
        ),
        "https://example.com/other?x=y | https: | example.com |  | /other | ?x=y |  | ../other?x=y"
    );
}

/// The idiom the package exists for, as axios 0.x writes it: resolve a URL
/// through an anchor and compare its parts with the page's.
#[test]
fn an_anchor_works_as_a_url_parser() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var r = document.createElement('a'); \
             function parse(e) { \
               r.setAttribute('href', e); \
               return { href: r.href, protocol: r.protocol ? r.protocol.replace(/:$/, '') : '', \
                 host: r.host, search: r.search ? r.search.replace(/^\\?/, '') : '', \
                 hash: r.hash ? r.hash.replace(/^#/, '') : '', hostname: r.hostname, \
                 port: r.port, \
                 pathname: '/' === r.pathname.charAt(0) ? r.pathname : '/' + r.pathname }; \
             } \
             var here = parse(location.href), same = parse('/api/items?page=2'), \
                 other = parse('https://cdn.example.net/x.js'); \
             [here.pathname, same.pathname, same.search, \
              same.protocol === here.protocol && same.host === here.host, \
              other.protocol === here.protocol && other.host === here.host].join(' ')",
        ),
        "/dir/page.html /api/items page=2 true false"
    );
}

#[test]
fn an_anchor_without_a_url_answers_empty_strings() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'), parts = ['href', 'origin', 'protocol', \
               'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']; \
             var none = parts.map(function (p) { return '[' + a[p] + ']'; }).join(''); \
             a.pathname = '/ignored'; a.hash = 'ignored'; \
             var still = a.hasAttribute('href'); \
             a.setAttribute('href', 'https://exa mple/'); \
             [none, still, a.href, a.protocol, a.pathname, String(a)].join(' ')",
        ),
        "[][][:][][][][][][][][] false https://exa mple/ :  https://exa mple/"
    );
}

#[test]
fn a_part_that_is_set_rewrites_the_href_attribute() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'); a.href = '/start?old=1#old'; \
             a.pathname = '/a b'; a.search = 'q=1'; a.hash = '#h'; \
             var first = a.getAttribute('href'); \
             a.protocol = 'http'; a.host = 'other.example:8080'; \
             var second = a.href; \
             a.hostname = 'third.example'; a.port = '80'; a.username = 'u'; a.password = 'p'; \
             var third = a.href; \
             a.search = ''; a.hash = ''; a.port = ''; \
             [first, second, third, a.href].join(' ')",
        ),
        "https://example.com/a%20b?q=1#h http://other.example:8080/a%20b?q=1#h \
         http://u:p@third.example/a%20b?q=1#h http://u:p@third.example/a%20b"
    );
}

#[test]
fn an_anchor_stringifies_to_its_url() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'); a.href = 'next'; \
             var empty = document.createElement('a'); \
             [String(a), a + '', a.toString(), '[' + String(empty) + ']', \
              Object.prototype.toString.call(a)].join(' ')",
        ),
        "https://example.com/dir/next https://example.com/dir/next https://example.com/dir/next \
         [] [object HTMLAnchorElement]"
    );
}

#[test]
fn an_area_has_the_same_members() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var area = document.createElement('area'); area.href = '//maps.example/p?z=3#x'; \
             area.hash = 'y'; \
             [area.href, area.origin, area.pathname, area.search, String(area), \
              area instanceof HTMLAreaElement].join(' ')",
        ),
        "https://maps.example/p?z=3#y https://maps.example /p ?z=3 https://maps.example/p?z=3#y \
         true"
    );
}

#[test]
fn the_reflected_strings_of_an_anchor_follow_its_attributes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = document.createElement('a'); \
             a.target = '_blank'; a.rel = 'noopener nofollow'; a.download = 'f.txt'; \
             a.hreflang = 'pl'; a.type = 'text/plain'; a.text = 'read <me>'; \
             [a.getAttribute('target'), a.getAttribute('rel'), a.getAttribute('download'), \
              a.getAttribute('hreflang'), a.getAttribute('type'), a.textContent, a.text, \
              a.childNodes.length, document.createElement('a').target === ''].join('|')",
        ),
        "_blank|noopener nofollow|f.txt|pl|text/plain|read <me>|read <me>|1|true"
    );
}

#[test]
fn the_members_live_on_the_prototypes_as_accessors() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var names = ['href', 'origin', 'protocol', 'username', 'password', 'host', \
               'hostname', 'port', 'pathname', 'search', 'hash']; \
             function shape(proto) { \
               return names.map(function (name) { \
                 var d = Object.getOwnPropertyDescriptor(proto, name); \
                 return (d && typeof d.get === 'function' ? 'g' : '-') + \
                        (d && typeof d.set === 'function' ? 's' : '-'); \
               }).join(','); \
             } \
             [shape(HTMLAnchorElement.prototype), shape(HTMLAreaElement.prototype), \
              'pathname' in document.createElement('div')].join(' ')",
        ),
        "gs,g-,gs,gs,gs,gs,gs,gs,gs,gs,gs gs,g-,gs,gs,gs,gs,gs,gs,gs,gs,gs false"
    );
}
