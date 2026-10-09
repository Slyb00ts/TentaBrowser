//! Real V8 values exercise the generated callback union converter.

#![cfg(feature = "v8")]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[allow(
    dead_code,
    unused_variables,
    clippy::wrong_self_convention,
    clippy::unnecessary_cast,
    clippy::single_match,
    clippy::unnecessary_map_on_constructor,
    clippy::clone_on_copy
)]
mod bindings {
    include!("golden/callback_union/prelude.rs");
    include!("golden/callback_union/types.rs");
}
use bindings::*;

#[derive(Default)]
struct Roots(RefCell<Vec<v8::Global<v8::Value>>>);
struct World(Rc<Roots>);
impl WrapperWorld for World {
    fn wrapper<'s>(
        &self,
        _: &mut v8::PinScope<'s, '_>,
        _: &'static str,
        _: u64,
    ) -> Result<Option<v8::Local<'s, v8::Object>>, JsError> {
        Ok(None)
    }
    fn handle(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
    ) -> Option<u64> {
        None
    }
    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64 {
        let mut values = self.0.0.borrow_mut();
        let bits = values.len() as u64;
        values.push(v8::Global::new(scope, value));
        bits
    }
    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        self.0
            .0
            .borrow()
            .get(bits as usize)
            .map(|value| v8::Local::new(scope, value))
    }
    fn realm(&self) -> Realm {
        Rc::new(())
    }
    fn adopt(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
        _: Box<dyn Any>,
    ) -> Result<(), JsError> {
        Ok(())
    }
}

fn eval<'s>(scope: &mut v8::PinScope<'s, '_>, source: &str) -> v8::Local<'s, v8::Value> {
    let source = v8::String::new(scope, source).expect("source");
    v8::Script::compile(scope, source, None)
        .expect("compile")
        .run(scope)
        .expect("evaluate")
}

#[test]
fn callback_union_selects_plain_listener_objects_and_callable_functions() {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
    let roots = Rc::new(Roots::default());
    let mut isolate = v8::Isolate::new(Default::default());
    {
        v8::scope!(let handle_scope, &mut isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        set_wrapper_world(scope, Box::new(World(roots.clone()))).expect("install world");
        let host_calls = Cell::new(0);

        for (source, expected) in [
            ("({handleEvent() {}})", "listener"),
            (
                "({get handleEvent() {throw Error('must not read at conversion');}})",
                "listener",
            ),
            ("new Proxy({}, {})", "listener"),
            ("(function() {})", "handler"),
            ("new Proxy(function() {}, {})", "handler"),
        ] {
            let value = eval(scope, source);
            let converted = ListenerOrHandler::from_v8(scope, value).expect("union conversion");
            let (kind, bits) = match converted {
                ListenerOrHandler::Listener(callback) => ("listener", callback.bits()),
                ListenerOrHandler::Handler(callback) => ("handler", callback.bits()),
            };
            assert_eq!(kind, expected);
            let stored = value_to_v8(scope, bits).expect("retained exact callback");
            assert!(stored.strict_equals(value));
            host_calls.set(host_calls.get() + 1);
        }
        assert_eq!(host_calls.get(), 5);
        let roots_before_errors = roots.0.borrow().len();
        for source in ["17", "null", "undefined"] {
            let value = eval(scope, source);
            assert_eq!(
                ListenerOrHandler::from_v8(scope, value),
                Err(JsError::type_error("the value matches no arm of the union")),
                "wrong primitive must fail before the host effect"
            );
            assert_eq!(host_calls.get(), 5);
            assert_eq!(roots.0.borrow().len(), roots_before_errors);
        }

        let iterable = eval(
            scope,
            "({handleEvent() {}, *[Symbol.iterator]() { yield 'a'; }})",
        );
        let selected = ListenerOrSequenceOfDomString::from_v8(scope, iterable)
            .expect("iterable union conversion");
        assert!(
            matches!(selected, ListenerOrSequenceOfDomString::SequenceOfDomString(ref values) if values.len() == 1 && values[0] == "a")
        );
        roots.0.borrow_mut().clear();
    }
    clear_legacy_reflect_set(&mut isolate);
}
