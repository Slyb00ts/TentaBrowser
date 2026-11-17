//! `document.fonts`, `FontFaceSet` and `FontFace`.
//!
//! The API has no engine state behind it (`crates/tenta-js/src/fonts.rs`): it
//! answers as a document whose fonts have all loaded. These tests pin that
//! answer and the shape a page relies on to wait for its fonts.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// An engine with the preludes a page gets.
fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, drains the microtasks it queued, then reads `after`.
fn settled(source: &str, after: &str) -> String {
    let mut engine = engine();
    eval(&mut engine, source);
    engine.run_microtasks();
    eval(&mut engine, after)
}

#[test]
fn document_fonts_is_one_loaded_font_face_set() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const attempt = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            [
                document.fonts instanceof FontFaceSet, document.fonts instanceof EventTarget,
                document.fonts === document.fonts, String(document.fonts), document.fonts.status,
                document.fonts.size, document.fonts.ready instanceof Promise,
                document.fonts.ready === document.fonts.ready,
                document.fonts.check('16px serif'),
                attempt(() => new FontFaceSet()),
                attempt(() => document.fonts.check()),
                attempt(() => Object.getOwnPropertyDescriptor(Document.prototype, 'fonts').get.call({})),
                'fonts' in document.implementation.createHTMLDocument('other'),
                document.implementation.createHTMLDocument('other').fonts !== document.fonts,
            ].join('|');
            "#
        ),
        "true|true|true|[object FontFaceSet]|loaded|0|true|true|true|TypeError|TypeError|\
         TypeError|true|true"
    );
}

#[test]
fn a_page_that_waits_for_its_fonts_goes_on() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            document.fonts.ready.then(set => out.push('ready:' + (set === document.fonts)));
            document.fonts.load('bold 16px "Inter", sans-serif', 'abc')
                .then(found => out.push('load:' + Array.isArray(found) + ':' + found.length));
            document.fonts.load('not a font').then(found => out.push('odd:' + found.length));
            document.fonts.load().catch(error => out.push(error.constructor.name));
            "#,
            "out.join('|')"
        ),
        "ready:true|load:true:0|odd:0|TypeError"
    );
}

#[test]
fn a_font_face_keeps_its_descriptors_and_loads_without_a_request() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            const attempt = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            const face = new FontFace('Inter', 'url(/inter.woff2)', { weight: '700', style: 'italic' });
            out.push(String(face), face.family, face.weight, face.style, face.stretch, face.display,
                     face.unicodeRange, face.status, face.loaded instanceof Promise,
                     face.loaded === face.loaded);
            face.family = 'Other';
            face.weight = 400;
            out.push(face.family, face.weight);
            const loading = face.load();
            out.push(loading === face.loaded, face.status);
            loading.then(loaded => out.push('loaded:' + (loaded === face)));
            const binary = new FontFace('Bin', new Uint8Array([0, 1]).buffer);
            out.push(binary.status);
            out.push(attempt(() => new FontFace('x')), attempt(() => FontFace('x', 'y')));
            "#,
            "out.join('|')"
        ),
        "[object FontFace]|Inter|700|italic|normal|auto|U+0-10FFFF|unloaded|true|true|Other|400|\
         true|loaded|loaded|TypeError|TypeError|loaded:true"
    );
}

#[test]
fn the_set_holds_the_faces_a_page_adds_and_load_finds_them_by_family() {
    assert_eq!(
        settled(
            r#"
            globalThis.out = [];
            const attempt = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            const inter = new FontFace('Inter', 'url(/a.woff2)');
            const mono = new FontFace('"Fira Code"', 'url(/b.woff2)');
            const set = document.fonts;
            out.push(set.add(inter) === set, set.add(mono).size, set.add(inter).size,
                     set.has(inter), attempt(() => set.add({})));
            out.push([...set].length, [...set.values()][1] === mono, [...set.entries()][0][0] === inter);
            const seen = [];
            set.forEach(function (face, same, owner) {
                seen.push(face === same && owner === set && this === out);
            }, out);
            out.push(seen.join(','));
            set.load("italic 12px/1.5 'fira code', serif").then(found => {
                out.push(found.length, found[0] === mono, mono.status, inter.status);
                out.push(set.delete(inter), set.delete(inter), set.size);
                set.clear();
                out.push(set.size);
            });
            "#,
            "out.join('|')"
        ),
        "true|2|2|true|TypeError|2|true|true|true,true|1|true|loaded|unloaded|true|false|1|0"
    );
}

#[test]
fn the_set_is_an_event_target_with_handler_attributes() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const set = document.fonts, heard = [];
            const handler = () => {};
            set.onloadingdone = handler;
            set.onloading = 5;
            set.addEventListener('loadingdone', event => heard.push(event.target === set));
            set.dispatchEvent(new Event('loadingdone'));
            [set.onloadingdone === handler, String(set.onloading), String(set.onloadingerror),
             heard.join(',')].join('|');
            "#
        ),
        "true|null|null|true"
    );
}
