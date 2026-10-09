//! Executes generated dictionary output conversions in both JavaScript engines.
//! A fresh object must gain own data properties without calling inherited setters.

// Generated integer conversions share casts across narrow and native-width types.
#[allow(
    dead_code,
    clippy::wrong_self_convention,
    clippy::clone_on_copy,
    clippy::unnecessary_cast
)]
mod bindings {
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
}

#[cfg(feature = "v8")]
#[test]
fn v8_dictionary_output_omits_absent_members_and_bypasses_inherited_setters() {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
    let mut isolate = v8::Isolate::new(Default::default());
    v8::scope!(let handle_scope, &mut isolate);
    let context = v8::Context::new(handle_scope, Default::default());
    let mut context_scope = v8::ContextScope::new(handle_scope, context);
    v8::scope!(let scope, &mut context_scope);

    fn eval<'s>(scope: &mut v8::PinScope<'s, '_>, source: &str) -> v8::Local<'s, v8::Value> {
        let source = v8::String::new(scope, source).expect("source");
        v8::Script::compile(scope, source, None)
            .expect("compile")
            .run(scope)
            .expect("script")
    }
    eval(
        scope,
        "globalThis.hits = 0; for (const key of ['initialValue', 'autoStart', 'label', 'nullableLabel']) Object.defineProperty(Object.prototype, key, { set(_) { ++globalThis.hits }, configurable: true });",
    );
    let value = bindings::CounterOptions {
        initial_value: 7,
        label: None,
        nullable_label: None,
        auto_start: false,
    }
    .to_v8(scope)
    .expect("convert dictionary");
    let key = v8::String::new(scope, "result").expect("key");
    assert_eq!(
        context.global(scope).set(scope, key.into(), value),
        Some(true)
    );
    let checked = eval(
        scope,
        "hits === 0 && Object.keys(result).join(',') === 'autoStart,initialValue' && !Object.hasOwn(result, 'label') && !Object.hasOwn(result, 'nullableLabel') && Object.getOwnPropertyDescriptor(result, 'initialValue').writable && Object.getOwnPropertyDescriptor(result, 'initialValue').enumerable && Object.getOwnPropertyDescriptor(result, 'initialValue').configurable && result.initialValue === 7 && result.autoStart === false",
    );
    assert!(checked.boolean_value(scope));

    let value = bindings::CounterOptions {
        initial_value: 8,
        label: Some("present".to_owned()),
        nullable_label: Some(None),
        auto_start: true,
    }
    .to_v8(scope)
    .expect("convert present optional member");
    let key = v8::String::new(scope, "present").expect("key");
    assert_eq!(
        context.global(scope).set(scope, key.into(), value),
        Some(true)
    );
    let checked = eval(
        scope,
        "hits === 0 && Object.keys(present).join(',') === 'autoStart,initialValue,label,nullableLabel' && Object.hasOwn(present, 'label') && Object.hasOwn(present, 'nullableLabel') && present.label === 'present' && present.nullableLabel === null",
    );
    assert!(checked.boolean_value(scope));

    let value = bindings::CounterOptions {
        initial_value: 9,
        label: None,
        nullable_label: Some(Some("text".to_owned())),
        auto_start: false,
    }
    .to_v8(scope)
    .expect("convert present non-null member");
    let key = v8::String::new(scope, "nonnull").expect("key");
    assert_eq!(
        context.global(scope).set(scope, key.into(), value),
        Some(true)
    );
    let checked = eval(
        scope,
        "hits === 0 && Object.keys(nonnull).join(',') === 'autoStart,initialValue,nullableLabel' && nonnull.nullableLabel === 'text'",
    );
    assert!(checked.boolean_value(scope));
}

#[cfg(feature = "quickjs")]
#[test]
fn quickjs_dictionary_output_omits_absent_members_and_bypasses_inherited_setters() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    context.with(|ctx| {
        ctx.eval::<(), _>(
            "globalThis.hits = 0; for (const key of ['initialValue', 'autoStart', 'label', 'nullableLabel']) Object.defineProperty(Object.prototype, key, { set(_) { ++globalThis.hits }, configurable: true });",
        )
        .expect("install inherited setters");
        let value = rquickjs::IntoJs::into_js(
            bindings::CounterOptions {
                initial_value: 7,
                label: None,
                nullable_label: None,
                auto_start: false,
            },
            &ctx,
        )
        .expect("convert dictionary");
        ctx.globals().set("result", value).expect("publish result");
        assert!(ctx
            .eval::<bool, _>(
                "hits === 0 && Object.keys(result).join(',') === 'autoStart,initialValue' && !Object.hasOwn(result, 'label') && !Object.hasOwn(result, 'nullableLabel') && Object.getOwnPropertyDescriptor(result, 'initialValue').writable && Object.getOwnPropertyDescriptor(result, 'initialValue').enumerable && Object.getOwnPropertyDescriptor(result, 'initialValue').configurable && result.initialValue === 7 && result.autoStart === false",
            )
            .expect("inspect result"));
        let value = rquickjs::IntoJs::into_js(
            bindings::CounterOptions {
                initial_value: 8,
                label: Some("present".to_owned()),
                nullable_label: Some(None),
                auto_start: true,
            },
            &ctx,
        )
        .expect("convert present optional member");
        ctx.globals().set("present", value).expect("publish present");
        assert!(ctx
            .eval::<bool, _>(
                "hits === 0 && Object.keys(present).join(',') === 'autoStart,initialValue,label,nullableLabel' && Object.hasOwn(present, 'label') && Object.hasOwn(present, 'nullableLabel') && present.label === 'present' && present.nullableLabel === null",
            )
            .expect("inspect present"));
        let value = rquickjs::IntoJs::into_js(
            bindings::CounterOptions {
                initial_value: 9,
                label: None,
                nullable_label: Some(Some("text".to_owned())),
                auto_start: false,
            },
            &ctx,
        )
        .expect("convert present non-null member");
        ctx.globals().set("nonnull", value).expect("publish nonnull");
        assert!(ctx
            .eval::<bool, _>(
                "hits === 0 && Object.keys(nonnull).join(',') === 'autoStart,initialValue,nullableLabel' && nonnull.nullableLabel === 'text'",
            )
            .expect("inspect nonnull"));
    });
}
