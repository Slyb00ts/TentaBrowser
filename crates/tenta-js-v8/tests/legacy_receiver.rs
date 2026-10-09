//! Checks the V8 interceptor behavior WebIDL legacy setters rely on.

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

static SETS: AtomicUsize = AtomicUsize::new(0);

fn init_v8() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

fn get<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    key: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Value>,
) -> v8::Intercepted {
    if key.is_string() {
        rv.set_int32(4);
        v8::Intercepted::kYes
    } else {
        v8::Intercepted::kNo
    }
}

fn set<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    _key: v8::Local<'s, v8::Name>,
    _value: v8::Local<'s, v8::Value>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
    SETS.fetch_add(1, Ordering::SeqCst);
    rv.set_bool(true);
    v8::Intercepted::kYes
}

fn query<'s>(
    _scope: &mut v8::PinScope<'s, '_>,
    key: v8::Local<'s, v8::Name>,
    _args: v8::PropertyCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Integer>,
) -> v8::Intercepted {
    if key.is_string() {
        rv.set_int32(v8::PropertyAttribute::NONE.as_u32() as i32);
        v8::Intercepted::kYes
    } else {
        v8::Intercepted::kNo
    }
}

#[test]
fn derived_receiver_does_not_call_native_named_setter() {
    init_v8();
    SETS.store(0, Ordering::SeqCst);
    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let mut context_scope = v8::ContextScope::new(scope, context);
    v8::scope!(let scope, &mut context_scope);
    let template = v8::ObjectTemplate::new(scope);
    template.set_named_property_handler(
        v8::NamedPropertyHandlerConfiguration::new()
            .getter(get)
            .setter(set)
            .query(query)
            .flags(v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS),
    );
    let target = template.new_instance(scope).expect("interceptor instance");
    let name = v8::String::new(scope, "target").expect("global name");
    assert_eq!(
        context.global(scope).set(scope, name.into(), target.into()),
        Some(true)
    );

    let source = v8::String::new(
        scope,
        "const receiver = {}; Reflect.set(target, 'tracked', 7, receiver) && receiver.tracked === 7",
    )
    .expect("script source");
    let script = v8::Script::compile(scope, source, None).expect("compiled script");
    let result = script.run(scope).expect("script result");
    assert!(result.boolean_value(scope));
    assert_eq!(SETS.load(Ordering::SeqCst), 0);

    let source =
        v8::String::new(scope, "Reflect.set(target, 'tracked', 9)").expect("script source");
    let script = v8::Script::compile(scope, source, None).expect("compiled script");
    assert!(
        script
            .run(scope)
            .expect("script result")
            .boolean_value(scope)
    );
    assert_eq!(SETS.load(Ordering::SeqCst), 1);
}
