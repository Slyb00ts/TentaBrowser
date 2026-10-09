//! Controls for the opt-in Web IDL legacy interceptor path.
//!
//! This test belongs to the pinned rusty_v8 fork. It must run against a
//! source-built V8 archive containing the corresponding native patch.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

fn initialize_v8() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
  });
}

fn decline_indexed_set<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _index: u32,
  _value: v8::Local<'s, v8::Value>,
  _args: v8::PropertyCallbackArguments<'s>,
  _rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
  match catch_unwind(AssertUnwindSafe(|| v8::Intercepted::kNo)) {
    Ok(result) => result,
    Err(payload) => {
      std::mem::forget(payload);
      scope.terminate_execution();
      v8::Intercepted::kYes
    }
  }
}

fn reject_indexed_define<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _index: u32,
  _descriptor: &v8::PropertyDescriptor,
  _args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
  match catch_unwind(AssertUnwindSafe(|| {
    rv.set_bool(false);
    v8::Intercepted::kYes
  })) {
    Ok(result) => result,
    Err(payload) => {
      std::mem::forget(payload);
      scope.terminate_execution();
      v8::Intercepted::kYes
    }
  }
}

fn decline_named_set<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _name: v8::Local<'s, v8::Name>,
  _value: v8::Local<'s, v8::Value>,
  _args: v8::PropertyCallbackArguments<'s>,
  _rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
  match catch_unwind(AssertUnwindSafe(|| v8::Intercepted::kNo)) {
    Ok(result) => result,
    Err(payload) => {
      std::mem::forget(payload);
      scope.terminate_execution();
      v8::Intercepted::kYes
    }
  }
}

fn reject_named_define<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _name: v8::Local<'s, v8::Name>,
  _descriptor: &v8::PropertyDescriptor,
  _args: v8::PropertyCallbackArguments<'s>,
  mut rv: v8::ReturnValue<v8::Boolean>,
) -> v8::Intercepted {
  match catch_unwind(AssertUnwindSafe(|| {
    rv.set_bool(false);
    v8::Intercepted::kYes
  })) {
    Ok(result) => result,
    Err(payload) => {
      std::mem::forget(payload);
      scope.terminate_execution();
      v8::Intercepted::kYes
    }
  }
}

#[test]
fn only_marked_indexed_handlers_consult_the_definer_during_set() {
  initialize_v8();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut context_scope = v8::ContextScope::new(scope, context);
  v8::scope!(let scope, &mut context_scope);

  for (name, flags) in [
    ("ordinary", v8::PropertyHandlerFlags::NONE),
    ("legacy", v8::PropertyHandlerFlags::WEB_IDL_LEGACY),
  ] {
    let template = v8::ObjectTemplate::new(scope);
    template.set_indexed_property_handler(
      v8::IndexedPropertyHandlerConfiguration::new()
        .setter(decline_indexed_set)
        .definer(reject_indexed_define)
        .flags(flags),
    );
    let object = template.new_instance(scope).expect("interceptor instance");
    let property = v8::String::new(scope, name).expect("global name");
    assert_eq!(
      context
        .global(scope)
        .set(scope, property.into(), object.into()),
      Some(true)
    );
  }

  for (name, flags) in [
    (
      "ordinary_named",
      v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS,
    ),
    (
      "legacy_named",
      v8::PropertyHandlerFlags::ONLY_INTERCEPT_STRINGS
        | v8::PropertyHandlerFlags::WEB_IDL_LEGACY,
    ),
  ] {
    let template = v8::ObjectTemplate::new(scope);
    template.set_named_property_handler(
      v8::NamedPropertyHandlerConfiguration::new()
        .setter(decline_named_set)
        .definer(reject_named_define)
        .flags(flags),
    );
    let object = template
      .new_instance(scope)
      .expect("named interceptor instance");
    let property = v8::String::new(scope, name).expect("global name");
    assert_eq!(
      context
        .global(scope)
        .set(scope, property.into(), object.into()),
      Some(true)
    );
  }

  let source = v8::String::new(
        scope,
        "const symbol = Symbol('ordinary'); \
         const derived = {}; \
         [Reflect.set(ordinary, '2', 8), Object.hasOwn(ordinary, '2'), \
          Reflect.set(legacy, '2', 8), Object.hasOwn(legacy, '2'), \
          Reflect.set(legacy, '3', 9, derived), \
          Object.hasOwn(derived, '3') && !Object.hasOwn(legacy, '3'), \
          Reflect.set(legacy, symbol, 10), legacy[symbol] === 10, \
          Reflect.set(ordinary_named, 'plain', 1), Object.hasOwn(ordinary_named, 'plain'), \
          Reflect.set(legacy_named, 'plain', 1), Object.hasOwn(legacy_named, 'plain'), \
          Reflect.set(legacy_named, '4294967295', 2), \
          Object.hasOwn(legacy_named, '4294967295'), \
          Reflect.set(legacy_named, symbol, 3), legacy_named[symbol] === 3].join('|')",
    )
    .expect("script source");
  let script =
    v8::Script::compile(scope, source, None).expect("compiled script");
  let result = script.run(scope).expect("script result");
  assert_eq!(
    result.to_rust_string_lossy(scope),
    "true|true|false|false|true|true|true|true|true|true|false|false|false|false|true|true"
  );
}
