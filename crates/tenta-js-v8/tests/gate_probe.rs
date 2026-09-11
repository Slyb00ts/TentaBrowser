//! Phase 4 exit-gate probe (P2): does React 18 / Vue 3 hydrate and respond to a
//! node-targeted click on the engine's DOM surface?
//!
//! These are `#[ignore]` by default — they load the vendored production bundles
//! (crates/tenta-js-v8/tests/fixtures/frameworks/) and drive a minimal app. Run
//! explicitly to see the outcome / the first missing-API gap:
//!
//!   cargo nextest run -p tenta-js-v8 --run-ignored all gate_probe
//!   # or: cargo test -p tenta-js-v8 --test gate_probe -- --ignored --nocapture

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

const REACT: &str = include_str!("fixtures/frameworks/react.production.min.js");
const REACT_DOM: &str = include_str!("fixtures/frameworks/react-dom.production.min.js");
const VUE: &str = include_str!("fixtures/frameworks/vue.global.prod.js");

/// `<html><body><div id="root"></div></body></html>`
fn document_with_root() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.create_html_element(atoms::DIV);
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
    document
        .mutate(Mutation::Append {
            parent: body,
            child: root,
        })
        .expect("append root <div>");
    document
        .mutate(Mutation::SetAttribute {
            element: root,
            name: tenta_html::QualName::new(atoms::EMPTY, atoms::ID),
            value: "root".to_owned(),
        })
        .expect("set id=root");
    Rc::new(RefCell::new(document))
}

/// Evaluates `source`, panicking with a stage-labelled message on a JS error so
/// the first gap is visible under `--nocapture`.
fn step(engine: &mut Engine, stage: &str, source: &str) -> String {
    match engine.evaluate(source) {
        Ok(value) => value,
        Err(error) => panic!("[{stage}] threw: {error}"),
    }
}

#[test]
#[ignore = "exit-gate probe: run explicitly to see the React gap list"]
fn react_18_renders_and_handles_a_click() {
    let mut engine = Engine::with_document(document_with_root());

    step(&mut engine, "load react", REACT);
    step(&mut engine, "load react-dom", REACT_DOM);
    step(
        &mut engine,
        "expose globals",
        "globalThis.React = globalThis.React || React; globalThis.ReactDOM = globalThis.ReactDOM || ReactDOM; 'ok';",
    );

    step(
        &mut engine,
        "render",
        r#"
        globalThis.__clicks = 0;
        const e = React.createElement;
        const App = () => e('button', { id: 'btn', onClick: () => { globalThis.__clicks++; } }, 'Hello');
        const root = ReactDOM.createRoot(document.getElementById('root'));
        root.render(e(App));
        'ok';
    "#,
    );

    // React 18 commits asynchronously through its scheduler.
    engine.run_event_loop(1000);

    let mounted = step(
        &mut engine,
        "inspect",
        "const b = document.getElementById('btn'); b ? b.textContent : 'NONE';",
    );
    assert_eq!(mounted, "Hello", "React should have mounted the button");

    step(
        &mut engine,
        "click",
        "document.getElementById('btn').click();",
    );
    engine.run_event_loop(1000);

    let clicks = step(&mut engine, "read clicks", "String(globalThis.__clicks);");
    assert_eq!(clicks, "1", "the click handler should have fired once");
}

#[test]
#[ignore = "exit-gate probe: run explicitly to see the Vue gap list"]
fn vue_3_mounts_and_handles_a_click() {
    let mut engine = Engine::with_document(document_with_root());

    step(&mut engine, "load vue", VUE);
    step(
        &mut engine,
        "expose globals",
        "globalThis.Vue = globalThis.Vue || Vue; 'ok';",
    );

    step(
        &mut engine,
        "mount",
        r#"
        globalThis.__clicks = 0;
        const { createApp, h } = Vue;
        const App = {
            render() {
                return h('button', { id: 'btn', onClick: () => { globalThis.__clicks++; } }, 'Hello');
            },
        };
        createApp(App).mount(document.getElementById('root'));
        'ok';
    "#,
    );

    engine.run_event_loop(1000);

    let mounted = step(
        &mut engine,
        "inspect",
        "const b = document.getElementById('btn'); b ? b.textContent : 'NONE';",
    );
    assert_eq!(mounted, "Hello", "Vue should have mounted the button");

    step(
        &mut engine,
        "click",
        "document.getElementById('btn').click();",
    );
    engine.run_event_loop(1000);

    let clicks = step(&mut engine, "read clicks", "String(globalThis.__clicks);");
    assert_eq!(clicks, "1", "the click handler should have fired once");
}
