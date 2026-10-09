//! The QuickJS half of the inspection surface (`tenta_js::inspect`): installing
//! the realm script, calling its dispatcher, and reporting the exceptions only
//! the engine sees.
//!
//! Uncaught exceptions are reported where an entry ends in one
//! (`Engine::entry`) and where a listener throws. Unhandled promise rejections
//! are collected by the runtime's rejection tracker and reported once the
//! microtask checkpoint that produced them has finished, so a handler attached
//! during the checkpoint revokes the entry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rquickjs::function::{Rest, This};
use rquickjs::{Coerced, Ctx, FromJs, Function, Persistent, Value};
use tenta_js::bindings::quickjs_callback_boundary;
use tenta_js::engine::{ScriptError, ScriptErrorKind};
use tenta_js::inspect::{self, INSPECTOR_SOURCE, InspectorSink};

use crate::Engine;
use crate::host::State;

/// How many unreported rejections are remembered. A page that rejects faster
/// than the engine reports loses the oldest.
const MAX_PENDING_REJECTIONS: usize = 1000;

type Saved = Persistent<Value<'static>>;

struct Rejection {
    promise: Saved,
    reason: Saved,
}

/// The inspection state of one realm.
pub(crate) struct InspectorState {
    realm: Cell<u64>,
    handle_limit: Cell<u32>,
    sink: RefCell<Option<Rc<dyn InspectorSink>>>,
    dispatcher: RefCell<Option<Persistent<Function<'static>>>>,
    rejections: Rc<RefCell<Vec<Rejection>>>,
}

impl InspectorState {
    pub(crate) fn new() -> Self {
        Self {
            realm: Cell::new(inspect::next_realm_id()),
            handle_limit: Cell::new(inspect::HANDLE_LIMIT),
            sink: RefCell::new(None),
            dispatcher: RefCell::new(None),
            rejections: Rc::default(),
        }
    }

    fn sink(&self) -> Option<Rc<dyn InspectorSink>> {
        self.sink.borrow().clone()
    }

    /// Drops every saved value this state holds. The engine calls it while the
    /// runtime is alive.
    pub(crate) fn clear(&self) {
        self.dispatcher.borrow_mut().take();
        self.rejections.borrow_mut().clear();
        self.sink.borrow_mut().take();
    }

    /// Describes `exception` through the realm script and hands the result to
    /// the sink. Does nothing when nothing listens.
    pub(crate) fn report<'js>(
        &self,
        ctx: &Ctx<'js>,
        text: &str,
        exception: &Value<'js>,
        unhandled_rejection: bool,
    ) {
        let Some(sink) = self.sink() else {
            return;
        };
        if !sink.listening() {
            return;
        }
        let Some(dispatcher) = self.dispatcher.borrow().clone() else {
            return;
        };
        let reply = dispatcher.restore(ctx).and_then(|dispatcher| {
            dispatcher.call::<_, String>((
                "exception",
                inspect::exception_argument(text, None, None),
                exception.clone(),
            ))
        });
        match reply {
            Ok(reply) => {
                if let Some(event) = inspect::exception_event(
                    &reply,
                    self.realm.get(),
                    unhandled_rejection,
                    now_ms(),
                ) {
                    sink.exception(event);
                }
            }
            // Describing a hostile value threw; the report is dropped, and the
            // exception must not stay pending in the entry that follows.
            Err(_) => {
                ctx.catch();
            }
        }
    }
}

/// Has the realm script describe how page code ended.
fn finish_with<'js>(
    dispatcher: &Function<'js>,
    argument: &str,
    extra: Value<'js>,
    mode: &str,
) -> rquickjs::Result<String> {
    dispatcher.call(("finish", argument, extra, mode))
}

fn not_installed() -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Uncaught,
        message: "inspection is not installed in this realm".to_owned(),
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

/// The name of the engine class of `value`, which no page code can change.
fn class_name(ctx: &Ctx<'_>, value: &Value<'_>) -> String {
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `raw` is the live context of this call and `value` is an object of
    // it. The atom `JS_GetClassName` returns is a new reference, freed below
    // together with the C string made from it, and neither escapes this block.
    unsafe {
        let class_id = rquickjs::qjs::JS_GetClassID(value.as_raw());
        let atom = rquickjs::qjs::JS_GetClassName(rquickjs::qjs::JS_GetRuntime(raw), class_id);
        if atom == 0 {
            return String::new();
        }
        let mut length = 0;
        let text = rquickjs::qjs::JS_AtomToCStringLen(raw, &mut length, atom);
        let name = if text.is_null() {
            String::new()
        } else {
            let bytes = std::slice::from_raw_parts(text.cast::<u8>(), length as usize);
            let name = String::from_utf8_lossy(bytes).into_owned();
            rquickjs::qjs::JS_FreeCString(raw, text);
            name
        };
        rquickjs::qjs::JS_FreeAtom(raw, atom);
        name
    }
}

/// The realm script's `brand(value)`: the engine's own answer to what kind of
/// object `value` is, which a page cannot spoof or intercept.
fn brand_of(ctx: &Ctx<'_>, value: &Value<'_>) -> &'static str {
    if value.as_object().is_none() {
        return "";
    }
    if value.is_proxy() {
        return "proxy";
    }
    if value.is_promise() {
        return "promise";
    }
    if value.is_array() {
        return "array";
    }
    // SAFETY: both only inspect the class of a live value.
    let (date, regexp) = unsafe {
        (
            rquickjs::qjs::JS_IsDate(value.as_raw()),
            rquickjs::qjs::JS_IsRegExp(value.as_raw()),
        )
    };
    if date {
        return "date";
    }
    if regexp {
        return "regexp";
    }
    if value.is_error() {
        return "error";
    }
    match class_name(ctx, value).as_str() {
        "Map" => "map",
        "Set" => "set",
        "WeakMap" => "weakmap",
        "WeakSet" => "weakset",
        "ArrayBuffer" | "SharedArrayBuffer" => "arraybuffer",
        "DataView" => "dataview",
        "Generator" => "generator",
        name if name.ends_with("Array") => "typedarray",
        _ => "",
    }
}

/// Runs the realm script and returns its dispatcher.
fn build_dispatcher<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<State>,
) -> rquickjs::Result<Persistent<Function<'static>>> {
    let factory: Function = ctx.eval(INSPECTOR_SOURCE)?;
    let realm = state.inspector.realm.get();
    let post = {
        let state = Rc::clone(state);
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, kind: String, payload: String| {
                quickjs_callback_boundary(&ctx, "inspector_post", || {
                    if let Some(sink) = state.inspector.sink() {
                        inspect::deliver(&*sink, realm, &kind, &payload);
                    }
                    Ok(())
                })
            },
        )?
    };
    let listening = {
        let state = Rc::clone(state);
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            quickjs_callback_boundary(&ctx, "inspector_listening", || {
                Ok(state.inspector.sink().is_some_and(|sink| sink.listening()))
            })
        })?
    };
    let brand = Function::new(ctx.clone(), move |ctx: Ctx<'js>, value: Value<'js>| {
        quickjs_callback_boundary(&ctx, "inspector_brand", || {
            Ok(brand_of(&ctx, &value).to_owned())
        })
    })?;
    let dispatcher: Function = factory.call((
        post,
        listening,
        brand,
        realm as f64,
        state.inspector.handle_limit.get(),
    ))?;
    Ok(Persistent::save(ctx, dispatcher))
}

impl Engine {
    /// Installs the realm script and points its events at `sink`. A second call
    /// only replaces the sink.
    pub(crate) fn install_inspector(&mut self, sink: Rc<dyn InspectorSink>) {
        if self.native_failure().is_some() {
            return;
        }
        *self.state.inspector.sink.borrow_mut() = Some(sink);
        if self.state.inspector.dispatcher.borrow().is_some() {
            return;
        }
        let state = Rc::clone(&self.state);
        let installed = self.entry_with(None, false, |ctx| build_dispatcher(ctx, &state));
        debug_assert!(
            installed.is_ok(),
            "the inspector script is static and must run"
        );
        if let Ok(dispatcher) = installed {
            *self.state.inspector.dispatcher.borrow_mut() = Some(dispatcher);
        }
        let rejections = Rc::clone(&self.state.inspector.rejections);
        self.runtime
            .set_host_promise_rejection_tracker(Some(Box::new(
                move |ctx, promise, reason, handled| {
                    let mut pending = rejections.borrow_mut();
                    if handled {
                        pending.retain(|rejection| {
                            rejection
                                .promise
                                .clone()
                                .restore(&ctx)
                                .map_or(true, |saved| saved != promise)
                        });
                        return;
                    }
                    if pending.len() >= MAX_PENDING_REJECTIONS {
                        pending.remove(0);
                    }
                    pending.push(Rejection {
                        promise: Persistent::save(&ctx, promise),
                        reason: Persistent::save(&ctx, reason),
                    });
                },
            )));
    }

    /// One operation of the realm script, run like any other script entry:
    /// under a deadline, but without reporting its own exceptions.
    pub(crate) fn call_inspector(
        &mut self,
        op: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        let Some(dispatcher) = self.state.inspector.dispatcher.borrow().clone() else {
            return Err(not_installed());
        };
        self.entry_with(Some(timeout), false, |ctx| {
            dispatcher.restore(ctx)?.call::<_, String>((op, argument))
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
    ) -> Result<String, ScriptError> {
        let Some(dispatcher) = self.state.inspector.dispatcher.borrow().clone() else {
            return Err(not_installed());
        };
        self.entry_with(Some(timeout), false, |ctx| {
            let dispatcher = dispatcher.restore(ctx)?;
            let finish = |extra, mode| finish_with(&dispatcher, argument, extra, mode);
            if kind != "call" {
                return match ctx.eval::<Value, _>(source) {
                    Ok(value) => finish(value, "value"),
                    Err(error) if error.is_exception() => finish(ctx.catch(), "thrown"),
                    Err(error) => Err(error),
                };
            }
            let prepared: rquickjs::Array = dispatcher.call(("prepare", argument))?;
            let refusal: Value = prepared.get(0)?;
            if refusal.is_string() {
                return Coerced::<String>::from_js(ctx, refusal).map(|text| text.0);
            }
            let receiver: Value = prepared.get(1)?;
            let mut arguments = Vec::new();
            for index in 2..prepared.len() {
                arguments.push(prepared.get::<Value>(index)?);
            }
            let function = match ctx.eval::<Value, _>(format!("({source}\n)")) {
                Ok(value) => value,
                Err(error) if error.is_exception() => return finish(ctx.catch(), "thrown"),
                Err(error) => return Err(error),
            };
            let Some(function) = function.as_function() else {
                return finish(Value::new_undefined(ctx.clone()), "notfn");
            };
            match function.call::<_, Value>((This(receiver), Rest(arguments))) {
                Ok(value) => finish(value, "value"),
                Err(error) if error.is_exception() => finish(ctx.catch(), "thrown"),
                Err(error) => Err(error),
            }
        })
    }

    /// Reports the rejections nobody handled during the checkpoint that just
    /// ran.
    pub(crate) fn flush_rejections(&mut self) {
        let pending = std::mem::take(&mut *self.state.inspector.rejections.borrow_mut());
        if pending.is_empty() || self.native_failure().is_some() {
            return;
        }
        let state = Rc::clone(&self.state);
        let _ = self.entry_with(None, false, |ctx| {
            for rejection in pending {
                let reason = rejection.reason.restore(ctx)?;
                state
                    .inspector
                    .report(ctx, "Uncaught (in promise)", &reason, true);
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
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
        settled: RefCell<Vec<(u64, Result<inspect::Completion, InspectError>)>>,
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
        let mut engine = Engine::new().expect("engine");
        engine.state.inspector.handle_limit.set(handle_limit);
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
