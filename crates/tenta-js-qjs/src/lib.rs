//! QuickJS-ng page runtime with native WebIDL bindings and the shared HTML loop.
//!
//! Each engine owns one runtime on its page thread. No module loader, filesystem
//! bindings or native network access are installed; requests use `tenta-js` drivers.

#[cfg(test)]
mod constructor_order_tests;
#[cfg(test)]
mod crypto_tests;
#[cfg(test)]
mod geometry_tests;
mod host;
mod inspector;
mod page_engine;
#[cfg(test)]
mod related_target_tests;
mod structured_clone;
mod world;

use rquickjs::{Coerced, Context, Ctx, FromJs, Runtime, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tenta_js::bindings::QuickJsWorldSlot;
use tenta_js::bindings::host::{HostContext, SharedDocument};
use tenta_js::engine::{ScriptError, ScriptErrorKind};
use tenta_js::event_loop::{Clock, EventLoop, FiringRelease, MonoTime};
use tenta_js::storage::PageStorage;
use tenta_net::CookieJar;

pub use page_engine::QuickJsEngineFactory;

/// An error raised by page script or runtime initialization.
pub type JsError = ScriptError;

/// One thread-affine QuickJS runtime and document world.
pub struct Engine {
    context: Context,
    runtime: Runtime,
    event_loop: EventLoop,
    state: Rc<host::State>,
    interrupt: Arc<AtomicBool>,
    deadline: Rc<Cell<Option<Instant>>>,
    script_timeout: Option<Duration>,
}

impl Engine {
    /// Creates a fresh runtime over an empty document using deterministic time.
    pub fn new() -> Result<Self, JsError> {
        Self::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())))
    }
    /// Creates a runtime over the document the parser and page share.
    pub fn with_document(document: SharedDocument) -> Result<Self, JsError> {
        Self::with_document_on_clock(
            document,
            Clock::virtual_at(MonoTime::ZERO),
            None,
            CookieJar::new(),
        )
    }
    /// Creates a bounded runtime with the page's clock and cookie jar.
    pub fn with_document_on_clock(
        document: SharedDocument,
        clock: Clock,
        heap: Option<usize>,
        cookies: CookieJar,
    ) -> Result<Self, JsError> {
        Self::with_document_on_clock_and_storage(
            document,
            clock,
            heap,
            cookies,
            PageStorage::isolated(),
        )
    }
    /// Creates a bounded runtime with the page's storage namespace.
    pub fn with_document_on_clock_and_storage(
        document: SharedDocument,
        clock: Clock,
        heap: Option<usize>,
        cookies: CookieJar,
        storage: PageStorage,
    ) -> Result<Self, JsError> {
        Self::with_document_on_clock_storage_and_memory(
            document,
            clock,
            heap,
            cookies,
            storage,
            tenta_core::NativeMemoryBudget::for_page(
                tenta_core::Budgets::platform_default().page_memory_mb,
            ),
        )
    }

    /// Creates a page runtime with the page's shared native memory ledger.
    pub fn with_document_on_clock_storage_and_memory(
        document: SharedDocument,
        clock: Clock,
        heap: Option<usize>,
        cookies: CookieJar,
        storage: PageStorage,
        native_memory: tenta_core::NativeMemoryBudget,
    ) -> Result<Self, JsError> {
        let runtime = Runtime::new().map_err(initialization_error)?;
        runtime.set_max_stack_size(512 * 1024);
        if let Some(limit) = heap {
            runtime.set_memory_limit(limit);
        }
        let interrupt = Arc::new(AtomicBool::new(false));
        let deadline = Rc::new(Cell::new(None::<Instant>));
        let requested = interrupt.clone();
        let expires = deadline.clone();
        let event_loop = EventLoop::new(clock);
        let host_context = HostContext::new_with_native_memory(
            document,
            event_loop.clock(),
            cookies,
            storage,
            native_memory,
        );
        host_context
            .documents
            .install_observer_world()
            .map_err(|error| ScriptError {
                kind: ScriptErrorKind::Terminated,
                message: format!("native observer installation failed: {error}"),
            })?;
        let world = world::World::new(host_context, interrupt.clone(), deadline.clone());
        let native_failure = world.native_failure.clone();
        let collecting = world.collecting.clone();
        let native_memory_for_interrupt = world.host.native_memory.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            // Recovery uses only captured native intrinsics and must finish
            // before a script deadline/watchdog request can be reported. The
            // request remains armed; native failure always interrupts first.
            native_failure.get().is_some()
                || native_memory_for_interrupt.first_failure().is_some()
                || (!collecting.get()
                    && (requested.load(Ordering::Acquire)
                        || expires.get().is_some_and(|end| Instant::now() >= end)))
        })));
        let context = Context::full(&runtime).map_err(initialization_error)?;
        let state = host::State::new(world.clone(), event_loop.scheduler());
        let initialized = context
            .with(|ctx| {
                let _entry = host::ActiveContext::new(&world, &ctx);
                ctx.store_userdata(QuickJsWorldSlot(Box::new(world::Bridge(world.clone()))))
                    .map_err(|_| rquickjs::Error::Unknown)?;
                world.initialize(&ctx)?;
                // SAFETY: this context belongs to this live runtime; a page agent
                // must never block its thread in Atomics.wait.
                unsafe {
                    rquickjs::qjs::JS_SetCanBlock(
                        rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr()),
                        false,
                    );
                }
                ctx.globals().remove("SharedArrayBuffer")?;
                host::install(&ctx, &state)
            })
            .map_err(|error| context.with(|ctx| script_error(&ctx, error)));
        if let Err(error) = initialized {
            state.clear();
            context.with(|ctx| {
                world.clear();
                let _ = ctx.remove_userdata::<QuickJsWorldSlot>();
            });
            return Err(error);
        }

        *world.host.script.borrow_mut() = Some(state.clone());
        Ok(Self {
            context,
            runtime,
            event_loop,
            state,
            interrupt,
            deadline,
            script_timeout: None,
        })
    }
    /// Evaluates source and returns the JavaScript string completion.
    pub fn evaluate(&mut self, source: &str) -> Result<String, JsError> {
        self.evaluate_with_timeout(source, self.script_timeout)
    }
    /// Evaluates under a wall-clock deadline, including completion coercion.
    pub fn evaluate_with_timeout(
        &mut self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<String, JsError> {
        self.entry_with(timeout, false, |ctx| {
            let value: Value = eval_classic(ctx, source)?;
            Coerced::<String>::from_js(ctx, value).map(|v| v.0)
        })
    }
    /// Applies a default deadline to scripts and callbacks.
    pub fn set_script_timeout(&mut self, timeout: Option<Duration>) {
        self.script_timeout = timeout;
    }
    /// Advances deterministic time. Real clocks ignore manual advancement.
    pub fn advance_time(&mut self, delta: Duration) {
        self.event_loop.clock().advance(delta);
    }
    /// Returns the first native callback that poisoned this engine.
    pub fn native_failure(&self) -> Option<&'static str> {
        self.state.world.native_failure.get()
    }
    /// An entry that runs page script: an exception it ends in is also reported
    /// to the realm's inspector as uncaught.
    fn entry<R>(
        &self,
        timeout: Option<Duration>,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, ScriptError> {
        self.entry_with(timeout, true, f)
    }
    fn entry_with<R>(
        &self,
        timeout: Option<Duration>,
        report: bool,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, ScriptError> {
        if self.native_failure().is_some() {
            return Err(ScriptError {
                kind: ScriptErrorKind::Terminated,
                message: "native callback failure poisoned the runtime".into(),
            });
        }
        if self
            .state
            .world
            .host
            .native_memory
            .first_failure()
            .is_some()
        {
            return Err(ScriptError {
                kind: ScriptErrorKind::Terminated,
                message: "page memory budget exceeded".into(),
            });
        }
        self.interrupt.store(false, Ordering::Release);
        self.deadline
            .set(timeout.and_then(|d| Instant::now().checked_add(d)));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.context.with(|ctx| {
                let _entry = host::ActiveContext::new(&self.state.world, &ctx);
                let result = f(&ctx);
                if let Some(error) = native_failure_error(&ctx) {
                    // Formatting an exception can invoke page-owned getters
                    // and toString. A poisoned runtime must execute none of
                    // them, including while reporting its native failure.
                    return Err(error);
                }
                if self
                    .state
                    .world
                    .host
                    .native_memory
                    .first_failure()
                    .is_some()
                {
                    ctx.catch();
                    return Err(ScriptError {
                        kind: ScriptErrorKind::Terminated,
                        message: "page memory budget exceeded".into(),
                    });
                }
                let result = if self.state.world.script_interrupted() {
                    // Formatting an interrupted exception could invoke page
                    // accessors. Closed native cleanup requires none of them.
                    ctx.catch();
                    Err(ScriptError {
                        kind: ScriptErrorKind::Terminated,
                        message: "script execution interrupted".into(),
                    })
                } else {
                    result.map_err(|e| {
                        script_error_with(&ctx, e, |exception| {
                            if report {
                                self.state
                                    .inspector
                                    .report(&ctx, "Uncaught", exception, false);
                            }
                        })
                    })
                };
                if let Some(error) = native_failure_error(&ctx) {
                    return Err(error);
                }
                // `R` has no borrowed context lifetime: outgoing conversion is
                // complete, and any returned JS value has an independent root.
                // Callback frames have closed before this task-end checkpoint.
                self.state
                    .world
                    .collect_checkpoint(&ctx, false)
                    .map_err(|e| script_error(&ctx, e))?;
                result
            })
        }));
        let expired = self.interrupt.load(Ordering::Acquire)
            || self.deadline.get().is_some_and(|t| Instant::now() >= t);
        self.deadline.set(None);
        match result {
            Err(payload) => {
                if self.state.world.native_failure.get().is_none() {
                    self.state.world.native_failure.set(Some("runtime entry"));
                }
                if let Err(nested) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(nested)
                }
                Err(ScriptError {
                    kind: ScriptErrorKind::Terminated,
                    message: "native runtime entry failed".into(),
                })
            }
            Ok(_) if self.native_failure().is_some() => Err(ScriptError {
                kind: ScriptErrorKind::Terminated,
                message: "native callback failed".into(),
            }),
            Ok(_)
                if self
                    .state
                    .world
                    .host
                    .native_memory
                    .first_failure()
                    .is_some() =>
            {
                Err(ScriptError {
                    kind: ScriptErrorKind::Terminated,
                    message: "page memory budget exceeded".into(),
                })
            }
            Ok(_) if expired => Err(ScriptError {
                kind: ScriptErrorKind::Terminated,
                message: "script deadline exceeded".into(),
            }),
            Ok(result) => result,
        }
    }
    /// Runs queued promise reactions under the engine's script deadline.
    pub fn run_microtasks(&mut self) {
        let _ = self.checkpoint();
    }
    fn checkpoint(&mut self) -> Result<(), ScriptError> {
        let limit = self.script_timeout.or(Some(Duration::from_secs(10)));
        let result = self.entry(limit, |ctx| {
            loop {
                let mut job_context = std::ptr::null_mut();
                // SAFETY: this active context holds its runtime lock. Jobs
                // belong to this one page runtime and never leave its thread.
                let status = unsafe {
                    rquickjs::qjs::JS_ExecutePendingJob(
                        rquickjs::qjs::JS_GetRuntime(ctx.as_raw().as_ptr()),
                        &mut job_context,
                    )
                };
                if status < 0 {
                    return Err(rquickjs::Error::Exception);
                }
                if status == 0 {
                    break;
                }
            }
            Ok(())
        });
        self.state.world.reclaim();
        self.flush_rejections();
        result
    }
    /// The duration until the page should next drive this engine.
    pub fn next_wakeup(&self) -> Option<Duration> {
        if self.runtime.is_job_pending() || !self.state.ready.borrow().is_empty() {
            Some(Duration::ZERO)
        } else {
            self.event_loop.next_wakeup()
        }
    }
    /// Runs at most `max_tasks` due timer callbacks and their checkpoints.
    pub fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        // Network and lifecycle delivery queue reactions even when no timer
        // is due. The page loop relies on this leading checkpoint before it
        // evaluates settle, just as it does with the V8 backend.
        if self.checkpoint().is_err() {
            return 0;
        }
        let mut count = 0;
        while count < max_tasks && self.native_failure().is_none() {
            if self
                .entry(self.script_timeout, |_| {
                    self.event_loop.fire_due_timers();
                    Ok(())
                })
                .is_err()
            {
                break;
            }
            let ready = self.state.ready.borrow_mut().pop_front();
            let Some(ready) = ready else { break };
            let scheduler = self.event_loop.scheduler();
            let Some(firing) = scheduler.claim_fired(ready.timer) else {
                continue;
            };
            let _release = FiringRelease::new(&scheduler, firing);
            let terminated = scheduler.with_origin(ready.origin, || {
                scheduler.with_timer_scope(ready.scope, || {
                    let result = self.entry(self.script_timeout, |ctx| ready.call(ctx));
                    if matches!(
                        result,
                        Err(ScriptError {
                            kind: ScriptErrorKind::Terminated,
                            ..
                        })
                    ) {
                        true
                    } else {
                        self.checkpoint().is_err()
                    }
                })
            });
            count += 1;
            if terminated {
                break;
            }
        }
        count
    }
    /// Runs deterministic timers until idle or the given task cap.
    pub fn run_event_loop(&mut self, max_tasks: usize) -> usize {
        let mut count = 0;
        self.run_microtasks();
        while count < max_tasks {
            let ran = self.pump_due_timers(max_tasks - count);
            count += ran;
            if count >= max_tasks || !self.event_loop.advance_to_next_timer() {
                break;
            }
        }
        count
    }
    /// Forces a collection and applies detached-wrapper bookkeeping.
    pub fn trim_memory(&mut self) {
        if self.native_failure().is_some() {
            return;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.context
                .with(|ctx| self.state.world.collect_checkpoint(&ctx, false))
        }));
        if !matches!(result, Ok(Ok(()))) {
            if self.state.world.native_failure.get().is_none() {
                self.state
                    .world
                    .native_failure
                    .set(Some("listener cycle collection"));
            }
            if let Err(payload) = result
                && let Err(nested) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
            {
                std::mem::forget(nested);
            }
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.event_loop.clear_pending_callbacks();
        self.state.clear();
        self.state.inspector.clear();
        self.context.with(|ctx| {
            self.state.world.clear();
            let _ = ctx.remove_userdata::<QuickJsWorldSlot>();
        });
        self.runtime.run_gc();
        self.state.world.reclaim();
    }
}
/// Evaluates page source as a classic script: global code that is sloppy
/// unless it says `"use strict"`. The binding's own default forces strict
/// mode, under which an ordinary page script that assigns an undeclared name
/// or reads `this` in a plain function call throws.
pub(crate) fn eval_classic<'js, V: FromJs<'js>>(
    ctx: &Ctx<'js>,
    source: impl Into<Vec<u8>>,
) -> rquickjs::Result<V> {
    let mut options = rquickjs::context::EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
}

fn initialization_error(error: rquickjs::Error) -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Terminated,
        message: format!("QuickJS initialization failed: {error}"),
    }
}
fn script_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> ScriptError {
    script_error_with(ctx, error, |_| {})
}

/// [`script_error`] that also shows `observe` the exception value before it is
/// turned into text.
fn script_error_with<'js>(
    ctx: &Ctx<'js>,
    error: rquickjs::Error,
    observe: impl FnOnce(&Value<'js>),
) -> ScriptError {
    if let Some(error) = native_failure_error(ctx) {
        return error;
    }
    if error.is_exception() {
        let exception = ctx.catch();
        observe(&exception);
        let name = exception
            .as_object()
            .and_then(|o| match o.get::<_, String>("name") {
                Ok(name) => Some(name),
                Err(_) => {
                    ctx.catch();
                    None
                }
            })
            .unwrap_or_default();
        if let Some(error) = native_failure_error(ctx) {
            return error;
        }
        let message = Coerced::<String>::from_js(ctx, exception)
            .map(|v| v.0)
            .unwrap_or_else(|_| {
                ctx.catch();
                "JavaScript exception".into()
            });
        if let Some(error) = native_failure_error(ctx) {
            return error;
        }
        ScriptError {
            kind: if name == "SyntaxError" {
                ScriptErrorKind::Syntax
            } else {
                ScriptErrorKind::Uncaught
            },
            message,
        }
    } else {
        ScriptError {
            kind: ScriptErrorKind::Uncaught,
            message: error.to_string(),
        }
    }
}

fn native_failure_error(ctx: &Ctx<'_>) -> Option<ScriptError> {
    if !ctx
        .userdata::<QuickJsWorldSlot>()
        .is_some_and(|world| world.0.native_failure().is_some())
    {
        return None;
    }
    ctx.catch();
    Some(ScriptError {
        kind: ScriptErrorKind::Terminated,
        message: "native callback failed".into(),
    })
}
