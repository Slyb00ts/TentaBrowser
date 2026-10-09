//! The V8 half of the inspection surface (`tenta_js::inspect`): installing the
//! realm script, calling its dispatcher, and reporting the exceptions only the
//! engine sees.
//!
//! The realm script does the describing; this module owns what needs an
//! isolate. Uncaught exceptions are caught where script is entered (a classic
//! script, a timer or frame callback, an event listener), because a `TryCatch`
//! that swallows an exception is also what hides it from V8's own reporting.
//! Unhandled promise rejections are collected by the isolate's reject callback
//! and reported when the microtask checkpoint that produced them has finished,
//! which is the point where "unhandled" is decided: a handler attached during
//! the checkpoint revokes the entry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tenta_js::bindings::v8_registered_slow_callback_boundary;
use tenta_js::engine::ScriptErrorKind;
use tenta_js::inspect::{self, INSPECTOR_SOURCE, InspectorSink};

use crate::host::{HostState, host_state};
use crate::{Engine, ScriptFailure};

/// How many unreported rejections are remembered. A page that rejects faster
/// than the engine reports loses the oldest.
const MAX_PENDING_REJECTIONS: usize = 1000;

struct Rejection {
    promise: v8::Global<v8::Promise>,
    reason: v8::Global<v8::Value>,
}

/// The inspection state of one realm, reached from native callbacks through the
/// host state in the context slot.
pub(crate) struct InspectorState {
    realm: Cell<u64>,
    handle_limit: Cell<u32>,
    sink: RefCell<Option<Rc<dyn InspectorSink>>>,
    dispatcher: RefCell<Option<v8::Global<v8::Function>>>,
    rejections: RefCell<Vec<Rejection>>,
}

impl InspectorState {
    pub(crate) fn new() -> Self {
        Self {
            realm: Cell::new(inspect::next_realm_id()),
            handle_limit: Cell::new(inspect::HANDLE_LIMIT),
            sink: RefCell::new(None),
            dispatcher: RefCell::new(None),
            rejections: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn sink(&self) -> Option<Rc<dyn InspectorSink>> {
        self.sink.borrow().clone()
    }

    /// Drops every V8 handle this state holds. The engine calls it while the
    /// isolate is alive; a context slot is not dropped until the isolate is
    /// disposed.
    pub(crate) fn clear(&self) {
        self.dispatcher.borrow_mut().take();
        self.rejections.borrow_mut().clear();
        self.sink.borrow_mut().take();
    }
}

/// The realm script's `post(kind, payload)`.
fn post(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _retval: v8::ReturnValue) {
    let Some(state) = host_state(scope) else {
        return;
    };
    let Some(sink) = state.inspector.sink() else {
        return;
    };
    let kind = args.get(0).to_rust_string_lossy(scope);
    let payload = args.get(1).to_rust_string_lossy(scope);
    inspect::deliver(&*sink, state.inspector.realm.get(), &kind, &payload);
}

/// The realm script's `listening()`.
fn listening(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let listening = host_state(scope)
        .and_then(|state| state.inspector.sink())
        .is_some_and(|sink| sink.listening());
    retval.set_bool(listening);
}

/// The realm script's `brand(value)`: the engine's own answer to what kind of
/// object `value` is, which a page cannot spoof or intercept.
fn brand(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let value = args.get(0);
    let kind = if value.is_proxy() {
        "proxy"
    } else if value.is_promise() {
        "promise"
    } else if value.is_array() {
        "array"
    } else if value.is_date() {
        "date"
    } else if value.is_reg_exp() {
        "regexp"
    } else if value.is_native_error() {
        "error"
    } else if value.is_map() {
        "map"
    } else if value.is_set() {
        "set"
    } else if value.is_weak_map() {
        "weakmap"
    } else if value.is_weak_set() {
        "weakset"
    } else if value.is_array_buffer() || value.is_shared_array_buffer() {
        "arraybuffer"
    } else if value.is_data_view() {
        "dataview"
    } else if value.is_typed_array() {
        "typedarray"
    } else if value.is_generator_object() {
        "generator"
    } else {
        ""
    };
    if let Some(kind) = v8::String::new(scope, kind) {
        retval.set(kind.into());
    }
}

// Only these wrappers cross V8's callback ABI; a panic in the body poisons the
// isolate instead of unwinding into V8, as for every other native function.
fn guarded_post<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    retval: v8::ReturnValue<'s, v8::Value>,
) {
    // SAFETY: installed as a V8 slow function, never called from a Fast API or
    // GC hook.
    unsafe {
        v8_registered_slow_callback_boundary(scope, "inspector_post", (), |scope| {
            post(scope, args, retval)
        })
    };
}

fn guarded_brand<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    retval: v8::ReturnValue<'s, v8::Value>,
) {
    // SAFETY: as for `guarded_post`.
    unsafe {
        v8_registered_slow_callback_boundary(scope, "inspector_brand", (), |scope| {
            brand(scope, args, retval)
        })
    };
}

fn guarded_listening<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    retval: v8::ReturnValue<'s, v8::Value>,
) {
    // SAFETY: as for `guarded_post`.
    unsafe {
        v8_registered_slow_callback_boundary(scope, "inspector_listening", (), |scope| {
            listening(scope, args, retval)
        })
    };
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

/// Runs the realm script and returns its dispatcher.
fn build_dispatcher(
    scope: &mut v8::PinScope,
    realm: u64,
    handle_limit: u32,
) -> Option<v8::Global<v8::Function>> {
    let source = v8::String::new(scope, INSPECTOR_SOURCE)?;
    let script = v8::Script::compile(scope, source, None)?;
    let factory = v8::Local::<v8::Function>::try_from(script.run(scope)?).ok()?;
    let post = v8::Function::new(scope, guarded_post)?;
    let listening = v8::Function::new(scope, guarded_listening)?;
    let brand = v8::Function::new(scope, guarded_brand)?;
    let realm = v8::Number::new(scope, realm as f64);
    let limit = v8::Number::new(scope, f64::from(handle_limit));
    let receiver = v8::undefined(scope);
    let dispatcher = factory.call(
        scope,
        receiver.into(),
        &[
            post.into(),
            listening.into(),
            brand.into(),
            realm.into(),
            limit.into(),
        ],
    )?;
    let dispatcher = v8::Local::<v8::Function>::try_from(dispatcher).ok()?;
    Some(v8::Global::new(scope, dispatcher))
}

/// Records a promise that was rejected with nobody listening, and forgets one a
/// handler was attached to after all.
///
/// # Safety
///
/// Installed with `set_promise_reject_callback`, so V8 calls it only from the
/// owning isolate's thread while a script or microtask runs. The scope below is
/// the one such a callback may open, and no handle outlives the call.
unsafe extern "C" fn on_promise_reject(message: v8::PromiseRejectMessage) {
    v8::callback_scope!(unsafe let scope, &message);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(state) = host_state(scope) else {
            return;
        };
        match message.get_event() {
            v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
                let Some(reason) = message.get_value() else {
                    return;
                };
                let mut pending = state.inspector.rejections.borrow_mut();
                if pending.len() >= MAX_PENDING_REJECTIONS {
                    pending.remove(0);
                }
                pending.push(Rejection {
                    promise: v8::Global::new(scope, message.get_promise()),
                    reason: v8::Global::new(scope, reason),
                });
            }
            v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
                let promise = message.get_promise();
                state
                    .inspector
                    .rejections
                    .borrow_mut()
                    .retain(|pending| v8::Local::new(scope, &pending.promise) != promise);
            }
            _ => {}
        }
    }));
    if let Err(payload) = outcome {
        tenta_js::bindings::poison_native_callback(scope, "promise rejection tracking");
        if let Err(nested) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
        {
            std::mem::forget(nested);
        }
    }
}

/// Describes `value` through the realm script and hands the result to `sink`.
#[allow(clippy::too_many_arguments)]
fn deliver_exception(
    scope: &mut v8::PinScope,
    host: &HostState,
    sink: &Rc<dyn InspectorSink>,
    text: &str,
    value: v8::Local<v8::Value>,
    line: Option<u32>,
    column: Option<u32>,
    unhandled_rejection: bool,
) {
    let Some(dispatcher) = host.inspector.dispatcher.borrow().clone() else {
        return;
    };
    let function = v8::Local::new(scope, &dispatcher);
    let (Some(op), Some(argument)) = (
        v8::String::new(scope, "exception"),
        v8::String::new(scope, &inspect::exception_argument(text, line, column)),
    ) else {
        return;
    };
    let receiver = v8::undefined(scope);
    let Some(reply) = function.call(scope, receiver.into(), &[op.into(), argument.into(), value])
    else {
        return;
    };
    let Some(reply) = reply.to_string(scope) else {
        return;
    };
    let reply = reply.to_rust_string_lossy(scope);
    if let Some(event) = inspect::exception_event(
        &reply,
        host.inspector.realm.get(),
        unhandled_rejection,
        now_ms(),
    ) {
        sink.exception(event);
    }
}

/// Reports the exception a `TryCatch` holds as uncaught, then clears it. Does
/// nothing when nothing listens, when the script was terminated, or when no
/// exception is held.
pub(crate) fn report_caught(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
) {
    if tc.has_terminated() {
        return;
    }
    let Some(sink) = host.inspector.sink() else {
        return;
    };
    if !sink.listening() {
        return;
    }
    let Some(exception) = tc.exception() else {
        return;
    };
    let (line, column) = match tc.message() {
        Some(message) => (
            message
                .get_line_number(tc)
                .and_then(|line| u32::try_from(line).ok())
                .map(|line| line.saturating_sub(1)),
            u32::try_from(message.get_start_column()).ok(),
        ),
        None => (None, None),
    };
    tc.reset();
    deliver_exception(tc, host, &sink, "Uncaught", exception, line, column, false);
}

impl Engine {
    /// Installs the realm script and points its events at `sink`. A second call
    /// only replaces the sink.
    pub(crate) fn install_inspector(&mut self, sink: Rc<dyn InspectorSink>) {
        if self.native_failure().is_some() {
            return;
        }
        *self.host.inspector.sink.borrow_mut() = Some(sink);
        if self.host.inspector.dispatcher.borrow().is_some() {
            return;
        }
        let _entry = self.entered();
        let realm = self.host.inspector.realm.get();
        let handle_limit = self.host.inspector.handle_limit.get();
        let dispatcher = {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            build_dispatcher(scope, realm, handle_limit)
        };
        debug_assert!(
            dispatcher.is_some(),
            "the inspector script is static and must run"
        );
        *self.host.inspector.dispatcher.borrow_mut() = dispatcher;
        self.isolate.set_promise_reject_callback(on_promise_reject);
    }

    /// One operation of the realm script, run like any other script entry:
    /// under a deadline, with a stray termination discarded first.
    pub(crate) fn call_inspector(
        &mut self,
        op: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptFailure> {
        let Some(dispatcher) = self.host.inspector.dispatcher.borrow().clone() else {
            return Err(ScriptFailure {
                kind: ScriptErrorKind::Uncaught,
                message: "inspection is not installed in this realm".to_owned(),
            });
        };
        self.run_with(Some(timeout), |tc| {
            let function = v8::Local::new(tc, &dispatcher);
            let (Some(op), Some(argument)) =
                (v8::String::new(tc, op), v8::String::new(tc, argument))
            else {
                return Err(ScriptFailure {
                    kind: ScriptErrorKind::Uncaught,
                    message: "the request exceeds the V8 string length limit".to_owned(),
                });
            };
            let receiver = v8::undefined(tc);
            let undefined = v8::undefined(tc);
            let reply = function.call(
                tc,
                receiver.into(),
                &[op.into(), argument.into(), undefined.into()],
            );
            let Some(reply) = reply else {
                return Err(ScriptFailure {
                    kind: if tc.has_terminated() {
                        ScriptErrorKind::Terminated
                    } else {
                        ScriptErrorKind::Uncaught
                    },
                    message: "script execution was terminated".to_owned(),
                });
            };
            reply
                .to_string(tc)
                .map(|reply| reply.to_rust_string_lossy(tc))
                .ok_or_else(|| ScriptFailure {
                    kind: ScriptErrorKind::Uncaught,
                    message: "the inspector reply could not be read".to_owned(),
                })
        })
    }

    /// Compiles and runs page code for the host, then has the realm script
    /// describe how it ended (`PageEngine::inspector_run`).
    ///
    /// The code runs here rather than inside the realm script so the page's
    /// stack ends at its own frame; the script only ever sees the result.
    pub(crate) fn run_inspector(
        &mut self,
        kind: &str,
        source: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptFailure> {
        let Some(dispatcher) = self.host.inspector.dispatcher.borrow().clone() else {
            return Err(not_installed());
        };
        self.run_with(Some(timeout), |tc| {
            let function = v8::Local::new(tc, &dispatcher);
            let reply = if kind == "call" {
                run_call(tc, function, source, argument)?
            } else {
                match run_source(tc, source)? {
                    Ran::Value(value) => {
                        dispatch(tc, function, "finish", argument, value, "value")?
                    }
                    Ran::Thrown(exception) => {
                        dispatch(tc, function, "finish", argument, exception, "thrown")?
                    }
                }
            };
            reply_text(tc, reply)
        })
    }

    /// Reports the rejections nobody handled during the checkpoint that just
    /// ran. The caller's entry carries the deadline; a termination ends the
    /// report.
    pub(crate) fn flush_rejections(&mut self) {
        let pending = std::mem::take(&mut *self.host.inspector.rejections.borrow_mut());
        if pending.is_empty() || self.native_failure().is_some() {
            return;
        }
        let host = Rc::clone(&self.host);
        let Some(sink) = host.inspector.sink() else {
            return;
        };
        if !sink.listening() {
            return;
        }
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        v8::tc_scope!(let tc, scope);
        for rejection in &pending {
            let reason = v8::Local::new(tc, &rejection.reason);
            deliver_exception(
                tc,
                &host,
                &sink,
                "Uncaught (in promise)",
                reason,
                None,
                None,
                true,
            );
            if tc.has_terminated() {
                break;
            }
            // Describing a hostile value may have thrown; nothing of it may
            // stay pending in the isolate.
            tc.reset();
        }
    }
}

fn not_installed() -> ScriptFailure {
    ScriptFailure {
        kind: ScriptErrorKind::Uncaught,
        message: "inspection is not installed in this realm".to_owned(),
    }
}

/// How page code ended.
enum Ran<'s> {
    Value(v8::Local<'s, v8::Value>),
    Thrown(v8::Local<'s, v8::Value>),
}

/// Compiles and runs `source` as a script, keeping the exception it throws.
fn run_source<'s>(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, 's, v8::HandleScope<'_>>>,
    source: &str,
) -> Result<Ran<'s>, ScriptFailure> {
    let Some(code) = v8::String::new(tc, source) else {
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message: "the request exceeds the V8 string length limit".to_owned(),
        });
    };
    let value = v8::Script::compile(tc, code, None).and_then(|script| script.run(tc));
    if let Some(value) = value {
        return Ok(Ran::Value(value));
    }
    if tc.has_terminated() {
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Terminated,
            message: "script execution was terminated".to_owned(),
        });
    }
    let exception = tc.exception().unwrap_or_else(|| v8::undefined(tc).into());
    tc.reset();
    Ok(Ran::Thrown(exception))
}

/// One call of the realm script's dispatcher.
fn dispatch<'s>(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, 's, v8::HandleScope<'_>>>,
    function: v8::Local<'s, v8::Function>,
    op: &str,
    argument: &str,
    extra: v8::Local<'s, v8::Value>,
    mode: &str,
) -> Result<v8::Local<'s, v8::Value>, ScriptFailure> {
    let (Some(op), Some(argument), Some(mode)) = (
        v8::String::new(tc, op),
        v8::String::new(tc, argument),
        v8::String::new(tc, mode),
    ) else {
        return Err(ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message: "the request exceeds the V8 string length limit".to_owned(),
        });
    };
    let receiver = v8::undefined(tc);
    function
        .call(
            tc,
            receiver.into(),
            &[op.into(), argument.into(), extra, mode.into()],
        )
        .ok_or_else(|| ScriptFailure {
            kind: if tc.has_terminated() {
                ScriptErrorKind::Terminated
            } else {
                ScriptErrorKind::Uncaught
            },
            message: "script execution was terminated".to_owned(),
        })
}

fn reply_text<'s>(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, 's, v8::HandleScope<'_>>>,
    reply: v8::Local<'s, v8::Value>,
) -> Result<String, ScriptFailure> {
    reply
        .to_string(tc)
        .map(|reply| reply.to_rust_string_lossy(tc))
        .ok_or_else(|| ScriptFailure {
            kind: ScriptErrorKind::Uncaught,
            message: "the inspector reply could not be read".to_owned(),
        })
}

/// Runs a function for the host: the realm script prepares the receiver and
/// arguments, the function is evaluated and called here, and the realm script
/// describes the outcome.
fn run_call<'s>(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, 's, v8::HandleScope<'_>>>,
    dispatcher: v8::Local<'s, v8::Function>,
    source: &str,
    argument: &str,
) -> Result<v8::Local<'s, v8::Value>, ScriptFailure> {
    let undefined: v8::Local<v8::Value> = v8::undefined(tc).into();
    let prepared = dispatch(tc, dispatcher, "prepare", argument, undefined, "")?;
    let prepared = v8::Local::<v8::Array>::try_from(prepared).map_err(|_| ScriptFailure {
        kind: ScriptErrorKind::Uncaught,
        message: "the inspector could not prepare the call".to_owned(),
    })?;
    let at = |tc: &mut v8::PinnedRef<'_, v8::TryCatch<'_, 's, v8::HandleScope<'_>>>, index| {
        prepared.get_index(tc, index)
    };
    let refused = at(tc, 0).filter(|refusal| refusal.is_string());
    if let Some(refusal) = refused {
        return Ok(refusal);
    }
    let receiver = at(tc, 1).unwrap_or(undefined);
    let mut arguments = Vec::new();
    for index in 2..prepared.length() {
        arguments.push(at(tc, index).unwrap_or(undefined));
    }
    let wrapped = format!("({source}\n)");
    let function = match run_source(tc, &wrapped)? {
        Ran::Value(value) => value,
        Ran::Thrown(exception) => {
            return dispatch(tc, dispatcher, "finish", argument, exception, "thrown");
        }
    };
    let Ok(function) = v8::Local::<v8::Function>::try_from(function) else {
        return dispatch(tc, dispatcher, "finish", argument, undefined, "notfn");
    };
    match function.call(tc, receiver, &arguments) {
        Some(value) => dispatch(tc, dispatcher, "finish", argument, value, "value"),
        None if tc.has_terminated() => Err(ScriptFailure {
            kind: ScriptErrorKind::Terminated,
            message: "script execution was terminated".to_owned(),
        }),
        None => {
            let exception = tc.exception().unwrap_or(undefined);
            tc.reset();
            dispatch(tc, dispatcher, "finish", argument, exception, "thrown")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tenta_dom::Document;
    use tenta_js::engine::PageEngine;
    use tenta_js::inspect::{
        BindingCall, Completion, ConsoleCall, Entered, ExceptionEvent, InspectError,
        InspectErrorKind, RemoteValue, Settings,
    };

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(10);

    #[derive(Default)]
    struct Recorder {
        exceptions: RefCell<Vec<ExceptionEvent>>,
        settled: RefCell<Vec<(u64, Result<Completion, InspectError>)>>,
    }

    impl InspectorSink for Recorder {
        fn listening(&self) -> bool {
            true
        }
        fn console(&self, _call: ConsoleCall) {}
        fn exception(&self, event: ExceptionEvent) {
            self.exceptions.borrow_mut().push(event);
        }
        fn binding_called(&self, _call: BindingCall) {}
        fn settled(&self, ticket: u64, result: Result<Completion, InspectError>) {
            self.settled.borrow_mut().push((ticket, result));
        }
    }

    fn engine(handle_limit: u32) -> (Engine, Rc<Recorder>) {
        let document = Rc::new(RefCell::new(Document::new()));
        let mut engine = Engine::with_document(document);
        engine.host.inspector.handle_limit.set(handle_limit);
        let recorder = Rc::new(Recorder::default());
        engine.install_inspector(Rc::clone(&recorder) as Rc<dyn InspectorSink>);
        (engine, recorder)
    }

    #[test]
    fn the_handle_table_refuses_past_its_limit_and_recovers_after_a_release() {
        let (mut engine, _) = engine(3);
        let engine: &mut dyn PageEngine = &mut engine;
        let settings = Settings::default();
        let mut ids = Vec::new();
        for _ in 0..3 {
            let Ok(Entered::Done(Completion::Value(RemoteValue::Handle(object)))) =
                inspect::evaluate(engine, "({})", &settings, DEADLINE)
            else {
                panic!("an object under the limit is a handle");
            };
            ids.push(object.id);
        }
        let refused = inspect::evaluate(engine, "({})", &settings, DEADLINE);
        assert_eq!(
            refused
                .expect_err("the fourth handle exceeds the limit")
                .kind,
            InspectErrorKind::HandleLimit
        );
        assert_eq!(inspect::live_handles(engine, DEADLINE), Ok(3));

        assert_eq!(inspect::release(engine, &ids[0], DEADLINE), Ok(true));
        assert!(matches!(
            inspect::evaluate(engine, "({})", &settings, DEADLINE),
            Ok(Entered::Done(Completion::Value(RemoteValue::Handle(_))))
        ));
    }

    #[test]
    fn a_promise_a_virtual_timer_settles_reports_when_the_clock_reaches_it() {
        let (mut engine, recorder) = engine(100);
        let settings = Settings {
            by_value: true,
            await_ticket: Some(7),
            ..Settings::default()
        };
        let entered = inspect::evaluate(
            &mut engine,
            "new Promise((resolve) => setTimeout(() => resolve('late'), 5000))",
            &settings,
            DEADLINE,
        );
        assert_eq!(entered, Ok(Entered::Pending));
        assert!(
            recorder.settled.borrow().is_empty(),
            "nothing settles before the timer's turn"
        );

        engine.run_event_loop(10);

        let settled = recorder.settled.borrow();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].0, 7);
        assert_eq!(
            settled[0].1,
            Ok(Completion::Value(RemoteValue::String("late".to_owned())))
        );
    }

    #[test]
    fn an_exception_is_reported_even_when_explicit_handles_fill_the_table() {
        let (mut engine, recorder) = engine(2);
        let settings = Settings::default();
        for _ in 0..2 {
            inspect::evaluate(&mut engine, "({})", &settings, DEADLINE).expect("under the limit");
        }
        assert!(inspect::evaluate(&mut engine, "({})", &settings, DEADLINE).is_err());

        let _ = PageEngine::run_script(
            &mut engine,
            "throw new Error('still reported')",
            tenta_js::engine::ScriptKind::Classic,
        );

        let exceptions = recorder.exceptions.borrow();
        assert_eq!(exceptions.len(), 1);
        assert_eq!(exceptions[0].details.message, "Error: still reported");
        assert!(matches!(
            exceptions[0].details.exception,
            RemoteValue::Handle(_)
        ));
    }
}
