//! `window.location` (read side), `atob` and `btoa` on the QuickJS host.
use tenta_js_qjs::Engine;

fn eval(source: &str) -> String {
    let mut engine = Engine::new().expect("QuickJS runtime");
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("{source}: {error:?}"))
}

fn thrown(source: &str) -> String {
    eval(&format!(
        "(function () {{ try {{ {source}; return 'no exception'; }} \
         catch (e) {{ return e.name + ': ' + e.message; }} }})()"
    ))
}

#[test]
fn a_document_without_a_url_is_at_about_blank() {
    assert_eq!(
        eval(
            "[location.href, location.origin, location.protocol, location.host, \
              location.pathname, location.search, location.hash].join('|')"
        ),
        "about:blank|null|about:||blank||"
    );
}

#[test]
fn location_is_one_unforgeable_object_that_stringifies_to_href() {
    assert_eq!(
        eval(
            "(function () {
                var own = Object.getOwnPropertyDescriptor(window, 'location');
                var href = Object.getOwnPropertyDescriptor(location, 'href');
                return [
                    window.location === location, self.location === location,
                    String(location) === location.href, location.toString() === location.href,
                    Object.prototype.toString.call(location),
                    own.writable, own.configurable, own.enumerable,
                    typeof href.get, href.set, href.configurable,
                    delete window.location, delete location.href,
                    Object.keys(location).join('/'),
                ].join(',');
            })()"
        ),
        "true,true,true,true,[object Location],false,false,true,function,,false,false,false,\
         href/origin/protocol/host/hostname/port/pathname/search/hash/toString"
    );
}

#[test]
fn navigation_through_location_is_absent_not_inert() {
    assert_eq!(
        eval(
            "location.href = 'https://other.example/'; location = 'https://other.example/'; \
             [location.href, typeof location.assign, typeof location.replace, \
              typeof location.reload].join(' ')"
        ),
        "about:blank undefined undefined undefined"
    );
    assert!(
        thrown("(function () { 'use strict'; location.href = 'https://other.example/'; })()")
            .starts_with("TypeError"),
    );
}

#[test]
fn btoa_and_atob_are_the_binary_string_codecs() {
    assert_eq!(
        eval(
            "[btoa('foobar'), atob('Zm9vYmFy'), atob(' Zm9v\\tYg '), btoa(''), atob(''), \
              btoa(null), btoa(String.fromCharCode(0, 255, 254)), \
              atob('AP/+').charCodeAt(1), atob('AP/+').length, \
              atob(btoa('caf\\u00e9')) === 'caf\\u00e9'].join(',')"
        ),
        "Zm9vYmFy,foobar,foob,,,bnVsbA==,AP/+,255,3,true"
    );
}

#[test]
fn btoa_and_atob_refuse_what_the_standard_refuses() {
    assert_eq!(
        thrown("btoa('\\u0100')"),
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
            "try { atob('Z') } catch (e) { throw new Error(String(e instanceof DOMException)) }"
        ),
        "Error: true"
    );
    assert_eq!(
        thrown("btoa()"),
        "TypeError: Failed to execute 'btoa' on 'Window': 1 argument required, but only 0 present."
    );
    assert_eq!(
        thrown("btoa({ toString() { throw new RangeError('mine') } })"),
        "RangeError: mine"
    );
}
