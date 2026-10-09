//! CSSStyleDeclaration per-property attributes through generated QuickJS bindings.

use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/css_style_properties.rs");
