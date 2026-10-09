use tenta_js_qjs::Engine;

fn new_engine() -> Engine {
    Engine::new().expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/listener_conversion_lease.rs");
