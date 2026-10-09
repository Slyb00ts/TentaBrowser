//! The native ordinary-record classifier must not execute page-owned behavior.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

fn initialize() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    v8::V8::initialize_platform(
      v8::new_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

fn evaluate<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  source: &str,
) -> v8::Local<'s, v8::Value> {
  let text = v8::String::new(scope, source).unwrap();
  v8::Script::compile(scope, text, None)
    .unwrap()
    .run(scope)
    .unwrap()
}

#[test]
fn script_records_and_ordinary_intrinsic_prototypes_are_accepted() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  for source in [
    "({})",
    "Object.create(null)",
    "new (class UserClass {})()",
    "Object.create(Date.prototype)",
    "Object.create(Intl.DateTimeFormat.prototype)",
    "Object.freeze({answer: 42})",
    "(() => { const o={}; for(let i=0;i<100;i++)o['p'+i]=i; delete o.p1; return o })()",
    "Object.prototype",
    "Promise.prototype",
    "RegExp.prototype",
    "Set.prototype",
    "Object.getPrototypeOf([][Symbol.iterator]())",
    "Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()))",
    "Object.getPrototypeOf(new Map()[Symbol.iterator]())",
    "Object.getPrototypeOf(new Set()[Symbol.iterator]())",
    "Object.getPrototypeOf(''[Symbol.iterator]())",
    "Object.getPrototypeOf(Uint8Array.prototype)",
  ] {
    let value = evaluate(scope, source);
    assert!(value.is_structured_clone_ordinary_object(), "{source}");
  }
}

#[test]
fn specialized_values_and_proxy_traps_do_not_enter_the_ordinary_branch() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  evaluate(scope, "globalThis.traps=0");
  for source in [
    "undefined",
    "null",
    "42",
    "true",
    "'text'",
    "1n",
    "Symbol('x')",
    "[]",
    "(function f(){})",
    "(class C{})",
    "new Date()",
    "Object.setPrototypeOf(new Date(), Object.prototype)",
    "Object.setPrototypeOf(new Date(), null)",
    "/x/g",
    "new Map()",
    "new Set()",
    "new WeakMap()",
    "new WeakSet()",
    "new Error('x')",
    "Promise.resolve(1)",
    "new ArrayBuffer(8)",
    "new SharedArrayBuffer(8)",
    "new DataView(new ArrayBuffer(8))",
    "new Uint8Array(8)",
    "new Number(1)",
    "new Boolean(true)",
    "new String('x')",
    "Object(Symbol('x'))",
    "Object(1n)",
    "new WeakRef({})",
    "[][Symbol.iterator]()",
    "new Map()[Symbol.iterator]()",
    "globalThis",
    "new Intl.DateTimeFormat('en')",
    "Object.setPrototypeOf(new Intl.DateTimeFormat('en'), Object.prototype)",
    "Object.setPrototypeOf(new Intl.DateTimeFormat('en'), null)",
    "Temporal.PlainDate.from('2026-09-24')",
    "new Proxy({}, {get(){traps++;throw 1},getPrototypeOf(){traps++;throw 2},ownKeys(){traps++;throw 3}})",
    "(() => {const p=Proxy.revocable({},{});p.revoke();return p.proxy})()",
  ] {
    let value = evaluate(scope, source);
    assert!(!value.is_structured_clone_ordinary_object(), "{source}");
  }
  let plain = evaluate(
    scope,
    "Object.defineProperties({}, {constructor:{get(){traps++;throw 4}},[Symbol.toStringTag]:{get(){traps++;throw 5}}})",
  );
  assert!(plain.is_structured_clone_ordinary_object());
  assert_eq!(evaluate(scope, "traps").integer_value(scope), Some(0));
}

static INTERCEPTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUNCTION_CALLS: AtomicUsize = AtomicUsize::new(0);

fn callable_object(
  scope: &mut v8::PinScope,
  _args: v8::FunctionCallbackArguments,
  _return_value: v8::ReturnValue<v8::Value>,
) {
  if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
    FUNCTION_CALLS.fetch_add(1, Ordering::Relaxed);
  })) {
    std::mem::forget(payload);
    scope.terminate_execution();
  }
}

fn named_getter<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  _name: v8::Local<'s, v8::Name>,
  _args: v8::PropertyCallbackArguments<'s>,
  _return_value: v8::ReturnValue<v8::Value>,
) -> v8::Intercepted {
  match catch_unwind(AssertUnwindSafe(|| {
    INTERCEPTOR_CALLS.fetch_add(1, Ordering::Relaxed);
    v8::Intercepted::kNo
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
fn native_wrappers_and_interceptors_are_rejected_without_a_callback() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let template = v8::ObjectTemplate::new(scope);
  template.set_internal_field_count(1);
  let host = template.new_instance(scope).unwrap();
  assert!(!host.is_structured_clone_ordinary_object());
  let callable = v8::ObjectTemplate::new(scope);
  callable.set_call_as_function_handler(callable_object, None);
  let callable = callable.new_instance(scope).unwrap();
  assert!(callable.is_callable());
  assert!(!callable.is_structured_clone_ordinary_object());
  assert_eq!(FUNCTION_CALLS.load(Ordering::Relaxed), 0);
  let intercepted = v8::ObjectTemplate::new(scope);
  intercepted.set_named_property_handler(
    v8::NamedPropertyHandlerConfiguration::new().getter(named_getter),
  );
  let intercepted = intercepted.new_instance(scope).unwrap();
  assert!(!intercepted.is_structured_clone_ordinary_object());
  assert_eq!(INTERCEPTOR_CALLS.load(Ordering::Relaxed), 0);
}

#[test]
fn cross_realm_objects_remain_classifiable_after_real_gc() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let first = v8::Context::new(scope, Default::default());
  let value = {
    let first_scope = &mut v8::ContextScope::new(scope, first);
    evaluate(first_scope, "new (class FromFirstRealm {})()")
  };
  let second = v8::Context::new(scope, Default::default());
  let second_scope = &mut v8::ContextScope::new(scope, second);
  assert!(value.is_structured_clone_ordinary_object());
  second_scope.low_memory_notification();
  assert!(value.is_structured_clone_ordinary_object());
}
