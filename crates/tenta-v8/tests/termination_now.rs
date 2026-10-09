//! A hard host failure must stop the active API callback before the next JS statement.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

static CALLBACK_PANICKED: AtomicBool = AtomicBool::new(false);

fn initialize() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    v8::V8::initialize_platform(
      v8::new_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

fn stop_now(
  scope: &mut v8::PinScope,
  _args: v8::FunctionCallbackArguments,
  _return_value: v8::ReturnValue<v8::Value>,
) {
  if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
    // SAFETY: V8 called this slow function callback on the isolate thread.
    // No page exception is pending; a hard host failure owns this path.
    unsafe { scope.terminate_execution_now_from_callback() };
    // Repeated termination must leave the original uncatchable exception.
    unsafe { scope.terminate_execution_now_from_callback() };
  })) {
    CALLBACK_PANICKED.store(true, Ordering::SeqCst);
    std::mem::forget(payload);
    scope.terminate_execution();
  }
}

fn stop_after_ordinary_throw(
  scope: &mut v8::PinScope,
  _args: v8::FunctionCallbackArguments,
  _return_value: v8::ReturnValue<v8::Value>,
) {
  if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
    let message =
      v8::String::new(scope, "page exception").expect("fixed string");
    let ordinary = v8::Exception::type_error(scope, message);
    scope.throw_exception(ordinary);
    // SAFETY: this is the same slow callback and isolate thread. The hard
    // failure supersedes its ordinary pending page exception.
    unsafe { scope.terminate_execution_now_from_callback() };
  })) {
    CALLBACK_PANICKED.store(true, Ordering::SeqCst);
    std::mem::forget(payload);
    scope.terminate_execution();
  }
}

fn evaluate(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
  let source = v8::String::new(scope, source).expect("source");
  let script =
    v8::Script::compile(scope, source, None).expect("compiled script");
  script
    .run(scope)
    .expect("run script")
    .to_rust_string_lossy(scope)
}

fn run_case(callback: impl v8::MapFnTo<v8::FunctionCallback>) {
  initialize();
  CALLBACK_PANICKED.store(false, Ordering::SeqCst);
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let handle_scope, isolate);
  let context = v8::Context::new(handle_scope, Default::default());
  let mut context_scope = v8::ContextScope::new(handle_scope, context);
  v8::scope!(let scope, &mut context_scope);
  let key = v8::String::new(scope, "stopNow").expect("fixed key");
  let function = v8::Function::builder(callback)
    .build(scope)
    .expect("native callback");
  assert_eq!(
    context
      .global(scope)
      .set(scope, key.into(), function.into()),
    Some(true),
  );
  assert_eq!(
    evaluate(
      scope,
      "globalThis.after = 0; globalThis.caught = 0; globalThis.finalized = 0; 'ready'",
    ),
    "ready",
  );
  {
    v8::tc_scope!(let tc, scope);
    let source = v8::String::new(
      tc,
      "try { stopNow(); globalThis.after = 1; } catch (_) { globalThis.caught = 1; } finally { globalThis.finalized = 1; }",
    )
    .expect("source");
    let script =
      v8::Script::compile(tc, source, None).expect("compiled script");
    assert!(script.run(tc).is_none());
    assert!(tc.has_terminated());
    assert!(!tc.can_continue());
  }
  assert!(!CALLBACK_PANICKED.load(Ordering::SeqCst));
  scope.cancel_terminate_execution();
  assert_eq!(
    evaluate(scope, "[after, caught, finalized].join('|')"),
    "0|0|0",
  );
}

#[test]
fn synchronous_termination_skips_after_catch_and_finally() {
  run_case(stop_now);
}

#[test]
fn synchronous_termination_supersedes_a_pending_page_exception() {
  run_case(stop_after_ordinary_throw);
}
