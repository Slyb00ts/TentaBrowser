//! Generated collection results define own data properties without page setters.
#![cfg(any(feature = "v8", feature = "quickjs"))]

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

const SETUP: &str = r#"
    globalThis.hits = 0;
    Object.defineProperty(Array.prototype, '0', {
        set(_) { ++globalThis.hits }, configurable: true
    });
    Object.defineProperty(Object.prototype, 'blocked', {
        value: 'inherited', writable: false, configurable: true
    });
    Object.defineProperty(Object.prototype, 'watched', {
        set(_) { ++globalThis.hits }, configurable: true
    });
"#;

const CHECK: &str = r#"
    hits === 0 &&
    Object.getPrototypeOf(sequence) === Array.prototype &&
    sequence.length === 2 && sequence[0] === 17 && sequence[1] === 23 &&
    Object.getPrototypeOf(record) === Object.prototype &&
    Object.keys(record).join(',') === '__proto__,blocked,watched' &&
    record.__proto__ === 41 && record.blocked === 43 && record.watched === 47 &&
    ['0', '1'].every(key => {
        const d = Object.getOwnPropertyDescriptor(sequence, key);
        return d && d.writable && d.enumerable && d.configurable;
    }) &&
    ['__proto__', 'blocked', 'watched'].every(key => {
        const d = Object.getOwnPropertyDescriptor(record, key);
        return d && d.writable && d.enumerable && d.configurable;
    })
"#;

#[cfg(feature = "v8")]
#[test]
fn v8_collection_output_bypasses_inherited_setters_and_readonly_properties() {
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
    eval(scope, SETUP);
    let sequence = bindings::sequence_to_v8(scope, vec![17, 23], |scope, value| {
        Ok(v8::Integer::new(scope, value).into())
    })
    .expect("convert sequence");
    let name = v8::String::new(scope, "sequence").unwrap();
    assert_eq!(
        context
            .global(scope)
            .create_data_property(scope, name.into(), sequence),
        Some(true)
    );
    let record = bindings::record_to_v8(
        scope,
        vec![("__proto__", 41), ("blocked", 43), ("watched", 47)],
        |scope, key| Ok(v8::String::new(scope, key).unwrap().into()),
        |scope, value| Ok(v8::Integer::new(scope, value).into()),
    )
    .expect("convert record");
    let name = v8::String::new(scope, "record").unwrap();
    assert_eq!(
        context
            .global(scope)
            .create_data_property(scope, name.into(), record),
        Some(true)
    );
    assert!(eval(scope, CHECK).boolean_value(scope));
}

#[cfg(feature = "quickjs")]
#[test]
fn quickjs_collection_output_bypasses_inherited_setters_and_readonly_properties() {
    use rquickjs::IntoJs;

    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    context.with(|ctx| {
        ctx.eval::<(), _>(SETUP)
            .expect("install inherited properties");
        let sequence =
            bindings::quickjs_sequence_value(&ctx, vec![17, 23], |value| value.into_js(&ctx))
                .expect("convert sequence");
        ctx.globals().set("sequence", sequence).unwrap();
        let record = bindings::quickjs_record_value(
            &ctx,
            vec![("__proto__", 41), ("blocked", 43), ("watched", 47)],
            |key| key.into_js(&ctx),
            |value| value.into_js(&ctx),
        )
        .expect("convert record");
        ctx.globals().set("record", record).unwrap();
        assert!(ctx.eval::<bool, _>(CHECK).expect("inspect own properties"));
    });
}
