//! `SVGAnimatedString`, `SVGElement.className` and `SVGAElement.href` on the
//! V8 host (`tenta_js::svg_animated`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// An `<svg>` with a `<circle>` and two `<a>`, one linked the old way, and an
/// HTML `<a>` beside it.
const TREE: &str = "var SVG = 'http://www.w3.org/2000/svg', XLINK = 'http://www.w3.org/1999/xlink'; \
     var html = document.createElement('html'), body = document.createElement('body'); \
     document.appendChild(html); html.appendChild(body); \
     var svg = document.createElementNS(SVG, 'svg'), circle = document.createElementNS(SVG, 'circle'), \
         link = document.createElementNS(SVG, 'a'), old = document.createElementNS(SVG, 'a'), \
         plain = document.createElement('a'); \
     body.id = 'app'; body.appendChild(svg); body.appendChild(plain); \
     svg.appendChild(circle); svg.appendChild(link); svg.appendChild(old); \
     circle.setAttribute('class', 'dot big'); link.setAttribute('href', '/in/svg'); \
     old.setAttributeNS(XLINK, 'xlink:href', '/old/way'); plain.href = '/in/html'; \
     function thrown(f) { try { f(); return 'none'; } catch (e) { return e.name; } } \
     'ready'";

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/dir/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    assert_eq!(eval(&mut engine, TREE), "ready");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn the_class_name_of_an_svg_element_is_an_object_over_the_attribute() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var c = circle.className; \
             var before = [typeof c, c instanceof SVGAnimatedString, Object.prototype.toString.call(c), \
                           c.baseVal, c.animVal, c === circle.className, typeof svg.className, \
                           svg.className.baseVal === ''].join(); \
             c.baseVal = 'ring'; \
             var written = [circle.getAttribute('class'), c.animVal, circle.classList.contains('ring')].join(); \
             circle.setAttribute('class', 'again'); \
             circle.className = 'ignored'; \
             [before, written, c.baseVal, circle.getAttribute('class'), \
              thrown(function () { 'use strict'; circle.className = 'x'; }), \
              typeof body.className, typeof plain.className].join('|')",
        ),
        "object,true,[object SVGAnimatedString],dot big,dot big,true,object,true|\
         ring,ring,true|again|again|TypeError|string|string"
    );
}

#[test]
fn the_href_of_an_svg_link_is_an_object_and_of_an_html_link_a_string() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var before = [link.href instanceof SVGAnimatedString, link.href.baseVal, \
                           link.href.animVal, link.href === link.href, old.href.baseVal, \
                           typeof plain.href, plain.href, 'href' in circle].join(); \
             link.href.baseVal = '/moved'; old.href.baseVal = '/old/moved'; \
             var empty = document.createElementNS(SVG, 'a'); \
             var none = empty.href.baseVal; \
             empty.href.baseVal = '/new'; \
             [before, link.getAttribute('href'), old.getAttributeNS(XLINK, 'href'), \
              old.hasAttribute('href'), '[' + none + ']', empty.getAttribute('href'), \
              link instanceof SVGAElement].join('|')",
        ),
        "true,/in/svg,/in/svg,true,/old/way,string,https://example.com/in/html,false|\
         /moved|/old/moved|false|[]|/new|true"
    );
}

/// The idiom the package is for, as VitePress writes it: every link of the
/// page, whichever kind its `href` is.
#[test]
fn script_can_ask_which_kind_of_href_a_link_has() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "Array.prototype.map.call(document.querySelectorAll('#app a'), function (e) { \
               var url = new URL(e.href instanceof SVGAnimatedString ? e.href.animVal : e.href, \
                                 e.baseURI); \
               return url.pathname; \
             }).join(' ')",
        ),
        "/in/svg /old/way /in/html"
    );
}

#[test]
fn the_interface_cannot_be_constructed_or_borrowed() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var proto = SVGAnimatedString.prototype; \
             var base = Object.getOwnPropertyDescriptor(proto, 'baseVal'), \
                 anim = Object.getOwnPropertyDescriptor(proto, 'animVal'); \
             var name = Object.getOwnPropertyDescriptor(SVGElement.prototype, 'className'), \
                 href = Object.getOwnPropertyDescriptor(SVGAElement.prototype, 'href'); \
             [typeof SVGAnimatedString, thrown(function () { new SVGAnimatedString(); }), \
              thrown(function () { base.get.call({}); }), \
              thrown(function () { base.set.call(Object.create(proto), 'x'); }), \
              typeof base.set, anim.set, base.enumerable, Object.keys(proto).join(), \
              typeof name.get, name.set, name.enumerable, typeof href.get, href.set, \
              thrown(function () { name.get.call(body); }), \
              thrown(function () { name.get.call({}); }), \
              thrown(function () { href.get.call(circle); }), \
              Object.keys(globalThis).indexOf('SVGAnimatedString')].join('|')",
        ),
        "function|TypeError|TypeError|TypeError|function||true|baseVal,animVal|function||true|\
         function||TypeError|TypeError|TypeError|-1"
    );
}
