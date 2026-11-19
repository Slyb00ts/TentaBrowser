//! `DOMMatrix`, `DOMPoint` and `DOMQuad` on the V8 host (`tenta_js::matrix`).
//!
//! The expected numbers are worked out by hand from the CSS Transforms
//! definitions of the functions, not read back from the implementation.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// `six(m)` is a matrix's 2D part, `all(m)` its sixteen elements, both
/// rounded to six places so that a rotation's `6.1e-17` reads as `0`;
/// `thrown(f)` is the name of what `f` throws.
const HELPERS: &str = "function round(n) { n = Math.round(n * 1e6) / 1e6; return n === 0 ? 0 : n; } \
     function six(m) { return [m.a, m.b, m.c, m.d, m.e, m.f].map(round).join(); } \
     function all(m) { return Array.prototype.map.call(m.toFloat64Array(), round).join(); } \
     function point(p) { return [p.x, p.y, p.z, p.w].map(round).join(); } \
     function thrown(f) { try { f(); return 'none'; } catch (e) { return e.name; } } \
     'ready'";

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    assert_eq!(eval(&mut engine, HELPERS), "ready");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn the_interfaces_and_their_aliases_are_there() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var m = new DOMMatrix(), r = new DOMMatrixReadOnly(), p = new DOMPoint(); \
             [typeof DOMMatrix, typeof DOMMatrixReadOnly, typeof DOMPoint, typeof DOMPointReadOnly, \
              typeof DOMQuad, WebKitCSSMatrix === DOMMatrix, SVGMatrix === DOMMatrix, \
              SVGPoint === DOMPoint, m instanceof DOMMatrixReadOnly, r instanceof DOMMatrix, \
              p instanceof DOMPointReadOnly, Object.prototype.toString.call(m), \
              Object.prototype.toString.call(r), Object.prototype.toString.call(p), \
              Object.prototype.toString.call(new DOMQuad()), \
              Object.keys(globalThis).indexOf('DOMMatrix'), DOMMatrix.length, \
              DOMMatrix.prototype.translate === undefined, \
              Object.keys(DOMMatrixReadOnly.prototype).indexOf('translate') >= 0, \
              Object.keys(DOMMatrix.prototype).indexOf('m44') >= 0, \
              m.translate.length, m.setMatrixValue.length].join('|')",
        ),
        "function|function|function|function|function|true|true|true|true|false|true|\
         [object DOMMatrix]|[object DOMMatrixReadOnly]|[object DOMPoint]|[object DOMQuad]|\
         -1|0|false|true|true|0|1"
    );
}

#[test]
fn a_matrix_is_made_from_nothing_six_numbers_or_sixteen() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var none = new DOMMatrix(), two = new DOMMatrix([1, 2, 3, 4, 5, 6]), \
                 three = new DOMMatrix([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]), \
                 flat = new DOMMatrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]); \
             [all(none), none.is2D, none.isIdentity, all(two), two.is2D, two.isIdentity, \
              [three.m11, three.m12, three.m13, three.m14, three.m21, three.m34, three.m41, \
               three.m44, three.a, three.b, three.c, three.d, three.e, three.f].join(), \
              three.is2D, flat.is2D, flat.isIdentity, \
              thrown(function () { new DOMMatrix([1, 2, 3]); }), \
              thrown(function () { new DOMMatrix([]); }), \
              six(new DOMMatrix(new Set([1, 2, 3, 4, 5, 6]))), \
              six(new DOMMatrix(['1', '2', '3', '4', '5', '6']))].join('|')",
        ),
        "1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1|true|true|\
         1,2,0,0,3,4,0,0,0,0,1,0,5,6,0,1|true|false|\
         1,2,3,4,5,12,13,16,1,2,5,6,13,14|false|false|true|TypeError|TypeError|\
         1,2,3,4,5,6|1,2,3,4,5,6"
    );
}

#[test]
fn a_matrix_is_made_from_a_transform_list() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function s(text) { var m = new DOMMatrix(text); return six(m) + ':' + m.is2D; } \
             [s(''), s('none'), s(' NONE '), s('matrix(1, 2, 3, 4, 5, 6)'), \
              s('translate(10px, 20px)'), s('translate(10px)'), s('translateX(3px) translateY(4px)'), \
              s('scale(2) translate(10px)'), s('translate(10px) scale(2)'), s('scale(2, 3)'), \
              s('rotate(90deg)'), s('rotate(0.25turn)'), s('rotate(100grad)'), \
              s('rotate(1.5707963267948966rad)'), s('skew(45deg)'), s('skewY(45deg)'), \
              s('translate(1in, 72pt)'), s('translate(2.54cm, 1pc)'), s('translate(0, 0)'), \
              s('Matrix(1,0,0,1,-5.5,.5)'), s('scaleX(2)scaleY(4)'), \
              s('matrix(1e1, 0, 0, 1E1, 0, 0)')].join('|')",
        ),
        "1,0,0,1,0,0:true|1,0,0,1,0,0:true|1,0,0,1,0,0:true|1,2,3,4,5,6:true|\
         1,0,0,1,10,20:true|1,0,0,1,10,0:true|1,0,0,1,3,4:true|\
         2,0,0,2,20,0:true|2,0,0,2,10,0:true|2,0,0,3,0,0:true|\
         0,1,-1,0,0,0:true|0,1,-1,0,0,0:true|0,1,-1,0,0,0:true|0,1,-1,0,0,0:true|\
         1,0,1,1,0,0:true|1,1,0,1,0,0:true|1,0,0,1,96,96:true|1,0,0,1,96,16:true|\
         1,0,0,1,0,0:true|1,0,0,1,-5.5,0.5:true|2,0,0,4,0,0:true|10,0,0,10,0,0:true"
    );
}

#[test]
fn the_3d_functions_of_a_transform_list() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function s(text) { var m = new DOMMatrix(text); return all(m) + ':' + m.is2D; } \
             [s('translateZ(5px)'), s('translate3d(1px, 2px, 3px)'), s('scale3d(2, 3, 4)'), \
              s('scaleZ(2)'), s('perspective(100px)'), s('perspective(none)'), \
              s('rotateX(90deg)'), s('rotateY(90deg)'), s('rotateZ(90deg)'), \
              s('rotate3d(0, 0, 2, 90deg)'), \
              s('matrix3d(1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16)')].join('|')",
        ),
        "1,0,0,0,0,1,0,0,0,0,1,0,0,0,5,1:false|1,0,0,0,0,1,0,0,0,0,1,0,1,2,3,1:false|\
         2,0,0,0,0,3,0,0,0,0,4,0,0,0,0,1:false|1,0,0,0,0,1,0,0,0,0,2,0,0,0,0,1:false|\
         1,0,0,0,0,1,0,0,0,0,1,-0.01,0,0,0,1:false|1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:false|\
         1,0,0,0,0,0,1,0,0,-1,0,0,0,0,0,1:false|0,0,-1,0,0,1,0,0,1,0,0,0,0,0,0,1:false|\
         0,1,0,0,-1,0,0,0,0,0,1,0,0,0,0,1:false|0,1,0,0,-1,0,0,0,0,0,1,0,0,0,0,1:false|\
         1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16:false"
    );
}

#[test]
fn what_is_not_a_transform_list_is_a_syntax_error() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "['translate(1em)', 'translate(10%)', 'translate(10)', 'rotate(90)', 'spin(1deg)', \
              'matrix(1, 2, 3)', 'translate(10px', 'translate(10px) junk', 'scale()', \
              'translate(calc(1px + 2px))', 'perspective(-1px)', 'null'].map(function (text) { \
               try { new DOMMatrix(text); return 'none'; } \
               catch (e) { return (e instanceof DOMException) + ':' + e.name; } \
             }).concat([thrown(function () { new DOMMatrix(null); }), \
                        thrown(function () { new DOMMatrix({}); }), \
                        thrown(function () { new DOMMatrix(7); })]).join('|')",
        ),
        "true:SyntaxError|true:SyntaxError|true:SyntaxError|true:SyntaxError|true:SyntaxError|\
         true:SyntaxError|true:SyntaxError|true:SyntaxError|true:SyntaxError|true:SyntaxError|\
         true:SyntaxError|true:SyntaxError|SyntaxError|SyntaxError|SyntaxError"
    );
}

/// The idiom the package is for: a transform read from style and taken apart
/// element by element, with the constructor found under an old name.
#[test]
fn a_transform_string_is_taken_apart_by_element() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var Matrix = window.DOMMatrix || window.WebKitCSSMatrix || window.MSCSSMatrix; \
             var t = new Matrix(String('matrix(1, 0, 0, 1, 40, -12.5)').trim()); \
             var rows = []; \
             for (var i = 1; i < 5; i++) { \
               var row = new Float32Array(4); \
               for (var j = 1; j < 5; j++) row[j - 1] = t['m' + i + j]; \
               rows.push(Array.prototype.join.call(row)); \
             } \
             [t instanceof DOMMatrix, rows.join(' ')].join('|')",
        ),
        "true|1,0,0,0 0,1,0,0 0,0,1,0 40,-12.5,0,1"
    );
}

#[test]
fn the_methods_of_a_read_only_matrix_return_a_new_matrix() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var r = new DOMMatrixReadOnly([1, 2, 3, 4, 5, 6]), t = r.translate(10, 20); \
             [t instanceof DOMMatrix, t !== r, six(r), six(t), \
              six(new DOMMatrix().translate(3)), six(new DOMMatrix().scale(2)), \
              six(new DOMMatrix().scale(2, 3)), six(new DOMMatrix().scale(2, 3, 1, 10, 10)), \
              six(new DOMMatrix().scaleNonUniform(2, 5)), six(new DOMMatrix().rotate(90)), \
              six(new DOMMatrix().rotate(0, 0, 90)), six(new DOMMatrix().rotateFromVector(0, 1)), \
              six(new DOMMatrix().rotateFromVector(0, 0)), \
              six(new DOMMatrix().rotateAxisAngle(0, 0, 1, 90)), \
              six(new DOMMatrix().skewX(45)), six(new DOMMatrix().skewY(45)), \
              six(r.flipX()), six(r.flipY()), six(new DOMMatrix().translate()), \
              six(new DOMMatrix().scale()), six(new DOMMatrix().rotate())].join('|')",
        ),
        "true|true|1,2,3,4,5,6|1,2,3,4,75,106|1,0,0,1,3,0|2,0,0,2,0,0|2,0,0,3,0,0|\
         2,0,0,3,-10,-20|2,0,0,5,0,0|0,1,-1,0,0,0|0,1,-1,0,0,0|0,1,-1,0,0,0|1,0,0,1,0,0|\
         0,1,-1,0,0,0|1,0,1,1,0,0|1,1,0,1,0,0|-1,-2,3,4,5,6|1,2,-3,-4,5,6|1,0,0,1,0,0|\
         1,0,0,1,0,0|1,0,0,1,0,0"
    );
}

#[test]
fn multiplying_and_inverting() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var a = new DOMMatrix([1, 2, 3, 4, 5, 6]), b = new DOMMatrix([7, 8, 9, 10, 11, 12]); \
             var deep = new DOMMatrix().translate(1, 2, 3).scale(2, 4, 8); \
             var turned = new DOMMatrix('rotate3d(1, 2, 3, 40deg) translate3d(4px, 5px, 6px)'); \
             var none = new DOMMatrix([1, 2, 2, 4, 0, 0]).inverse(); \
             [String(a.multiply(b)), six(a), String(new DOMMatrix(a).preMultiplySelf(b)), \
              String(a.multiply({ e: 10 })), String(a.multiply()), \
              String(new DOMMatrix([2, 0, 0, 4, 10, 20]).inverse()), \
              all(deep), all(deep.inverse()), deep.multiply(deep.inverse()).isIdentity, \
              all(turned.multiply(turned.inverse())), \
              all(none), none.is2D, thrown(function () { String(none); }), \
              a.multiply(deep).is2D, a.multiply(b).is2D].join('|')",
        ),
        "matrix(31, 46, 39, 58, 52, 76)|1,2,3,4,5,6|matrix(25, 28, 57, 64, 100, 112)|\
         matrix(1, 2, 3, 4, 15, 26)|matrix(1, 2, 3, 4, 5, 6)|matrix(0.5, 0, 0, 0.25, -5, -5)|\
         2,0,0,0,0,4,0,0,0,0,8,0,1,2,3,1|\
         0.5,0,0,0,0,0.25,0,0,0,0,0.125,0,-0.5,-0.5,-0.375,1|true|\
         1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1|\
         NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN,NaN|false|\
         InvalidStateError|false|true"
    );
}

#[test]
fn the_self_methods_change_the_matrix_and_return_it() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var m = new DOMMatrix(); \
             var same = m.translateSelf(10, 20) === m && m.scaleSelf(2) === m && \
               m.rotateSelf(90) === m && m.skewXSelf(0) === m && m.skewYSelf(0) === m && \
               m.multiplySelf(new DOMMatrix()) === m && m.rotateFromVectorSelf(1, 0) === m && \
               m.rotateAxisAngleSelf(0, 0, 1, 0) === m && m.scale3dSelf(1) === m; \
             var first = six(m); \
             var inverted = m.invertSelf() === m, second = six(m); \
             var set = m.setMatrixValue('translate(7px, 8px)') === m, third = six(m); \
             m.a = 3; m.m22 = '4'; m.e = 9; m.m42 = 1; \
             [same, first, inverted, second, set, third, String(m), \
              thrown(function () { m.setMatrixValue('nope'); }), six(m), \
              thrown(function () { m.setMatrixValue(); }), \
              'translateSelf' in new DOMMatrixReadOnly(), \
              Object.getOwnPropertyDescriptor(DOMMatrixReadOnly.prototype, 'a').set, \
              typeof Object.getOwnPropertyDescriptor(DOMMatrix.prototype, 'a').set, \
              thrown(function () { 'use strict'; new DOMMatrixReadOnly().a = 2; })].join('|')",
        ),
        "true|0,2,-2,0,10,20|true|0,-0.5,0.5,0,-10,5|true|1,0,0,1,7,8|\
         matrix(3, 0, 0, 4, 9, 1)|SyntaxError|3,0,0,4,9,1|TypeError|false||function|TypeError"
    );
}

/// What makes a matrix stop being 2D, and what does not.
#[test]
fn is_2d_follows_the_standard() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function after(change) { var m = new DOMMatrix(); change(m); return m.is2D; } \
             [after(function (m) { m.m13 = 0; m.m33 = 1; m.m44 = 1; m.m34 = -0; }), \
              after(function (m) { m.m13 = 1; }), after(function (m) { m.m44 = 2; }), \
              after(function (m) { m.m33 = 0; }), after(function (m) { m.m43 = NaN; }), \
              after(function (m) { m.m13 = 1; m.m13 = 0; }), \
              after(function (m) { m.translateSelf(1, 2, 0); }), \
              after(function (m) { m.translateSelf(1, 2, -0); }), \
              after(function (m) { m.translateSelf(1, 2, 3); }), \
              after(function (m) { m.scaleSelf(2, 3, 1); }), \
              after(function (m) { m.scaleSelf(2, 3, 4); }), \
              after(function (m) { m.scaleSelf(2, 3, 1, 0, 0, 5); }), \
              after(function (m) { m.scale3dSelf(1); }), after(function (m) { m.scale3dSelf(2); }), \
              after(function (m) { m.rotateSelf(30); }), after(function (m) { m.rotateSelf(30, 0, 0); }), \
              after(function (m) { m.rotateSelf(0, 0, 30); }), \
              after(function (m) { m.rotateSelf(0, 30); }), \
              after(function (m) { m.rotateAxisAngleSelf(0, 0, 1, 30); }), \
              after(function (m) { m.rotateAxisAngleSelf(1, 0, 0, 30); }), \
              after(function (m) { m.multiplySelf(new DOMMatrix('translateZ(0px)')); }), \
              after(function (m) { m.invertSelf(); })].join()",
        ),
        "true,false,false,false,false,false,true,true,false,true,false,false,true,false,true,\
         false,true,false,true,false,false,true"
    );
}

#[test]
fn a_matrix_from_a_dictionary_is_validated() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function from(init) { var m = DOMMatrix.fromMatrix(init); return all(m) + ':' + m.is2D; } \
             [from(), from({}), from({ a: 2, f: 7 }), from({ m11: 2, m42: 7 }), \
              from({ a: 2, m11: 2 }), from({ a: NaN, m11: NaN }).split(',')[0], \
              from({ m13: 1 }), from({ m33: 1, m34: -0 }), from({ is2D: false }), \
              thrown(function () { from({ a: 1, m11: 2 }); }), \
              thrown(function () { from({ f: 1, m42: 2 }); }), \
              thrown(function () { from({ m13: 1, is2D: true }); }), \
              thrown(function () { from(7); }), \
              DOMMatrix.fromMatrix(new DOMMatrixReadOnly([1, 2, 3, 4, 5, 6])) instanceof DOMMatrix, \
              DOMMatrixReadOnly.fromMatrix({}) instanceof DOMMatrix, \
              six(DOMMatrix.fromFloat32Array(new Float32Array([1, 2, 3, 4, 5, 6]))), \
              DOMMatrix.fromFloat64Array(new Float64Array(16)).is2D, \
              thrown(function () { DOMMatrix.fromFloat32Array(new Float64Array(6)); }), \
              thrown(function () { DOMMatrix.fromFloat64Array([1, 2, 3, 4, 5, 6]); }), \
              thrown(function () { DOMMatrix.fromFloat32Array(new Float32Array(5)); })].join('|')",
        ),
        "1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:true|1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:true|\
         2,0,0,0,0,1,0,0,0,0,1,0,0,7,0,1:true|2,0,0,0,0,1,0,0,0,0,1,0,0,7,0,1:true|\
         2,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:true|NaN|1,0,1,0,0,1,0,0,0,0,1,0,0,0,0,1:false|\
         1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:true|1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1:false|\
         TypeError|TypeError|TypeError|TypeError|true|false|1,2,3,4,5,6|false|TypeError|\
         TypeError|TypeError"
    );
}

#[test]
fn a_matrix_serializes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var two = new DOMMatrix([1, 2, 3, 4, 5, 6]), three = new DOMMatrix('translateZ(2px)'); \
             [String(two), two + '', String(three), JSON.stringify(two), \
              Object.keys(three.toJSON()).length, three.toJSON().m43, three.toJSON().is2D, \
              new DOMMatrix().toJSON().isIdentity, \
              two.toFloat32Array() instanceof Float32Array, two.toFloat64Array().length, \
              String(new DOMMatrix([0.1, -0, 1e21, 1e-7, 5, 6])), \
              thrown(function () { String(new DOMMatrix([1, 0, 0, 1, Infinity, 0])); }), \
              thrown(function () { DOMMatrix.prototype.toString.call({}); }), \
              thrown(function () { Object.getOwnPropertyDescriptor( \
                DOMMatrixReadOnly.prototype, 'a').get.call({}); })].join('|')",
        ),
        "matrix(1, 2, 3, 4, 5, 6)|matrix(1, 2, 3, 4, 5, 6)|\
         matrix3d(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 2, 1)|\
         {\"a\":1,\"b\":2,\"c\":3,\"d\":4,\"e\":5,\"f\":6,\"m11\":1,\"m12\":2,\"m13\":0,\"m14\":0,\
         \"m21\":3,\"m22\":4,\"m23\":0,\"m24\":0,\"m31\":0,\"m32\":0,\"m33\":1,\"m34\":0,\"m41\":5,\
         \"m42\":6,\"m43\":0,\"m44\":1,\"is2D\":true,\"isIdentity\":false}|24|2|false|true|true|16|\
         matrix(0.1, 0, 1e+21, 1e-7, 5, 6)|InvalidStateError|TypeError|TypeError"
    );
}

#[test]
fn points() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var p = new DOMPoint(), q = new DOMPoint(1, 2, 3, 4), r = new DOMPointReadOnly(5, 6); \
             q.x = 10; q.w = '2'; \
             var moved = new DOMMatrix('translate(10px, 20px)').transformPoint({ x: 1, y: 2 }); \
             var turned = new DOMPoint(1, 0).matrixTransform(new DOMMatrix('rotate(90deg)')); \
             var scaled = r.matrixTransform({ a: 2, d: 3 }); \
             var deep = new DOMMatrix([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]) \
               .transformPoint(new DOMPoint(1, 2, 3, 4)); \
             [point(p), point(q), point(r), point(moved), moved instanceof DOMPoint, point(turned), \
              point(scaled), scaled instanceof DOMPoint, point(deep), \
              point(new DOMMatrix().transformPoint()), JSON.stringify(q), \
              point(DOMPoint.fromPoint({ y: 7 })), point(DOMPoint.fromPoint()), \
              DOMPointReadOnly.fromPoint(q) instanceof DOMPoint, \
              DOMPoint.fromPoint(r) instanceof DOMPoint, \
              Object.getOwnPropertyDescriptor(DOMPointReadOnly.prototype, 'x').set, \
              thrown(function () { DOMPoint.fromPoint(3); }), \
              thrown(function () { DOMPointReadOnly.prototype.toJSON.call({}); }), \
              Object.keys(DOMPointReadOnly.prototype).join()].join('|')",
        ),
        "0,0,0,1|10,2,3,2|5,6,0,1|11,22,0,1|true|0,1,0,1|10,18,0,1|true|90,100,110,120|\
         0,0,0,1|{\"x\":10,\"y\":2,\"z\":3,\"w\":2}|0,7,0,1|0,0,0,1|false|true||TypeError|\
         TypeError|matrixTransform,toJSON,x,y,z,w"
    );
}

#[test]
fn quads() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var q = DOMQuad.fromRect({ x: 1, y: 2, width: 3, height: 4 }); \
             function rect(r) { return [r.x, r.y, r.width, r.height].join(); } \
             var corners = [q.p1, q.p2, q.p3, q.p4].map(point).join(' '); \
             var bounds = q.getBounds(), same = q.p1 === q.p1; \
             q.p1.x = -5; q.p3.y = 20; \
             var given = new DOMPoint(1, 1), made = new DOMQuad(given, { x: 2 }); \
             [corners, bounds instanceof DOMRect, rect(bounds), same, rect(q.getBounds()), \
              made.p1 !== given, point(made.p1), point(made.p2), point(made.p4), \
              point(DOMQuad.fromQuad({ p2: { x: 9, y: 9 } }).p2), \
              point(DOMQuad.fromQuad().p1), rect(DOMQuad.fromRect().getBounds()), \
              Object.keys(JSON.parse(JSON.stringify(q))).join(), \
              rect(new DOMQuad({ x: NaN }).getBounds()), \
              thrown(function () { DOMQuad.prototype.getBounds.call({}); })].join('|')",
        ),
        "1,2,0,1 4,2,0,1 4,6,0,1 1,6,0,1|true|1,2,3,4|true|-5,2,9,18|true|1,1,0,1|2,0,0,1|\
         0,0,0,1|9,9,0,1|0,0,0,1|0,0,0,0|p1,p2,p3,p4|NaN,0,NaN,0|TypeError"
    );
}
