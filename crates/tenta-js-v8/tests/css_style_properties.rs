//! CSSStyleDeclaration per-property attributes through generated V8 bindings.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/css_style_properties.rs");
