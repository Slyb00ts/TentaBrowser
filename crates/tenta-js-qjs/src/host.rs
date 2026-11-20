//! Native globals and the engine-independent ScriptHost bridge.
use crate::world::World;
use rquickjs::function::{Opt, Rest, This};
use rquickjs::{Coerced, Ctx, FromJs, Function, IntoJs, Object, Persistent, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use tenta_core::Origin;
use tenta_js::bindings::host::EventHandlerNonNull;
use tenta_js::bindings::host::*;
use tenta_js::bindings::*;
use tenta_js::dom::events::{
    CallbackRef, EventHandlerName, HostValue, PromiseOutcome, PromiseRef, ScriptHost,
};
use tenta_js::event_loop::{AnimationFrameId, Scheduler, TimerId, TimerScope};
use tenta_js::fetch::{FetchDriver, FetchStartError};
use tenta_js::geometry::{GeometryHost, ViewportMetrics, WindowMetric};
use tenta_js::location::LocationPart;
use tenta_js::xhr::{XhrDriver, XhrHandle, XhrRequest};

type SavedFunction = Persistent<Function<'static>>;
type SavedValue = Persistent<Value<'static>>;
struct Pending {
    promise: u64,
    resolve: SavedFunction,
    reject: SavedFunction,
}
#[derive(Clone)]
pub(crate) struct Handler {
    callback: SavedValue,
    args: Vec<SavedValue>,
}
pub(crate) struct Ready {
    handler: Handler,
    pub timer: TimerId,
    pub scope: TimerScope,
    pub origin: Origin,
}
impl Ready {
    pub fn call(self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        self.handler.call(ctx, None)
    }
}
impl Handler {
    fn call(&self, ctx: &Ctx<'_>, timestamp: Option<f64>) -> rquickjs::Result<()> {
        let callback = self.callback.clone().restore(ctx)?;
        if let Some(function) = callback.as_function() {
            let args = if let Some(timestamp) = timestamp {
                vec![timestamp.into_js(ctx)?]
            } else {
                self.args
                    .iter()
                    .map(|v| v.clone().restore(ctx))
                    .collect::<rquickjs::Result<Vec<_>>>()?
            };
            function.call::<_, Value>((This(ctx.globals()), Rest(args)))?;
        } else {
            let source = Coerced::<String>::from_js(ctx, callback)?.0;
            crate::eval_classic::<Value>(ctx, source)?;
        }
        Ok(())
    }
}
pub(crate) struct State {
    pub world: Rc<World>,
    pub scheduler: Scheduler,
    pub ready: Rc<RefCell<VecDeque<Ready>>>,
    pub frames: Rc<RefCell<VecDeque<(Handler, f64)>>>,
    pub fetch_driver: RefCell<Option<Rc<dyn FetchDriver>>>,
    pub xhr_driver: RefCell<Option<Rc<dyn XhrDriver>>>,
    /// The layout-backed answers for script geometry, or `None` for a page
    /// with no layout.
    pub geometry_host: RefCell<Option<Rc<dyn GeometryHost>>>,
    /// What the window metrics answer while there is no geometry host: the
    /// page's configured viewport.
    pub viewport: Cell<ViewportMetrics>,
    /// The realm's inspection surface.
    pub(crate) inspector: crate::inspector::InspectorState,
    pending: RefCell<HashMap<u64, Pending>>,
    next_promise: Cell<u64>,
    #[cfg(test)]
    pub(crate) interrupt_after_crypto_write:
        RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}
impl State {
    pub fn new(world: Rc<World>, scheduler: Scheduler) -> Rc<Self> {
        Rc::new(Self {
            world,
            scheduler,
            ready: Rc::default(),
            frames: Rc::default(),
            fetch_driver: RefCell::new(None),
            xhr_driver: RefCell::new(None),
            geometry_host: RefCell::new(None),
            viewport: Cell::new(ViewportMetrics::default()),
            inspector: crate::inspector::InspectorState::new(),
            pending: RefCell::default(),
            next_promise: Cell::new(0),
            #[cfg(test)]
            interrupt_after_crypto_write: RefCell::new(None),
        })
    }
    fn context<R>(&self, f: impl for<'js> FnOnce(&Ctx<'js>) -> R) -> Option<R> {
        let raw = std::ptr::NonNull::new(self.world.active.get())?;
        // SAFETY: ActiveContext sets this pointer only inside Context::with on
        // the owning page thread. No re-lock of the runtime occurs during a
        // Rust -> JS -> Rust reentrant event dispatch. R cannot contain 'js.
        let ctx = unsafe { Ctx::from_raw(raw) };
        Some(f(&ctx))
    }
    pub fn clear(&self) {
        self.ready.borrow_mut().clear();
        self.frames.borrow_mut().clear();
        self.pending.borrow_mut().clear();
    }
    pub fn settle<'js>(
        &self,
        ctx: &Ctx<'js>,
        resolver: u64,
        value: Value<'js>,
        rejected: bool,
    ) -> bool {
        let pending = self.pending.borrow_mut().remove(&resolver);
        let Some(pending) = pending else { return false };
        let result = (|| {
            let callback = if rejected {
                &pending.reject
            } else {
                &pending.resolve
            }
            .clone()
            .restore(ctx)?;
            let world = self.world.clone();
            let promise = pending.promise;
            // Body methods can settle before their generated return conversion.
            // Keep their owned promise until the settlement microtask executes.
            // The queued JS arguments are traced by QuickJS, not hidden inside
            // Rust-owned persistent roots on the native function.
            let job = Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, callback: Function<'js>, value: Value<'js>| {
                    quickjs_callback_boundary(&ctx, "promise settlement", || {
                        let result = callback.call::<_, Value>((value,)).map(|_| ());
                        world.release(promise);
                        result
                    })
                },
            )?;
            job.defer((callback, value))
        })();
        if result.is_err() {
            self.pending.borrow_mut().insert(resolver, pending);
        }
        result.is_ok()
    }
    fn value<'js>(&self, ctx: &Ctx<'js>, value: HostValue<'_>) -> rquickjs::Result<Value<'js>> {
        match value {
            HostValue::Undefined => Ok(Value::new_undefined(ctx.clone())),
            HostValue::String(text) => text.into_js(ctx),
            HostValue::DomException { name, message } => dom_exception(ctx, name, message),
            HostValue::TypeError { message } => {
                let constructor: rquickjs::function::Constructor =
                    FromJs::from_js(ctx, quickjs_intrinsic(ctx, "TypeError")?)?;
                constructor.construct((message,))
            }
            HostValue::ArrayBuffer(bytes) => quickjs_buffer_value(ctx, bytes, "ArrayBuffer"),
            HostValue::Uint8Array(bytes) => quickjs_buffer_value(ctx, bytes, "Uint8Array"),
            HostValue::Blob(bits) => handle_to_quickjs::<Blob>(ctx, Handle::from_bits(bits)),
            HostValue::Json(bytes) => ctx.json_parse(bytes).inspect_err(|_| {
                // A malformed body is an answer, not an exception to leave
                // pending in the context.
                let _ = ctx.catch();
            }),
        }
    }
    pub fn call_frame(handler: Handler, timestamp: f64, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        handler.call(ctx, Some(timestamp))
    }
}
/// Proves that a ScriptHost callback is executing on this runtime's thread.
pub(crate) struct ActiveContext<'a> {
    world: &'a World,
    previous: *mut rquickjs::qjs::JSContext,
}
impl<'a> ActiveContext<'a> {
    pub fn new(world: &'a World, ctx: &Ctx<'_>) -> Self {
        Self {
            world,
            previous: world.active.replace(ctx.as_raw().as_ptr()),
        }
    }
}
impl Drop for ActiveContext<'_> {
    fn drop(&mut self) {
        self.world.active.set(self.previous)
    }
}
fn quickjs_buffer_host_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> JsError {
    match error {
        rquickjs::Error::Exception => pending_js_exception_error(),
        rquickjs::Error::Allocation => {
            // A Rust allocation error has not installed a QuickJS exception.
            // Materialize its native OOM value before propagating the marker.
            unsafe { rquickjs::qjs::JS_ThrowOutOfMemory(ctx.as_raw().as_ptr()) };
            pending_js_exception_error()
        }
        other => JsError::type_error(other.to_string()),
    }
}

impl ScriptHost for State {
    fn geometry_host(&self) -> Option<Rc<dyn GeometryHost>> {
        self.geometry_host.borrow().clone()
    }

    fn computed_style_value(&self, node: tenta_core::NodeId, property: &str) -> Option<String> {
        self.geometry_host
            .borrow()
            .clone()?
            .computed_value(node, None, property)
    }

    fn array_buffer_view_info(&self, view: ArrayBufferViewRef) -> Result<BufferMetadata, JsError> {
        self.context(|ctx| {
            quickjs_array_buffer_view_info(ctx, view)
                .map_err(|error| quickjs_buffer_host_error(ctx, error))
        })
        .unwrap_or_else(|| Err(JsError::type_error("no active QuickJS callback")))
    }

    fn write_array_buffer_view(
        &self,
        view: ArrayBufferViewRef,
        bytes: &[u8],
    ) -> Result<BufferMetadata, JsError> {
        self.context(|ctx| {
            let info = quickjs_write_array_buffer_view(ctx, view, bytes)
                .map_err(|error| quickjs_buffer_host_error(ctx, error))?;
            #[cfg(test)]
            if let Some(interrupt) = self.interrupt_after_crypto_write.borrow_mut().take() {
                // Request the real page watchdog gate only after the checked
                // native writer completed. The shared host must poll again.
                interrupt.store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(info)
        })
        .unwrap_or_else(|| Err(JsError::type_error("no active QuickJS callback")))
    }

    fn check_native_buffer_work(&self) -> Result<(), JsError> {
        self.context(|ctx| {
            quickjs_check_buffer_interrupt(ctx)
                .map_err(|error| quickjs_buffer_host_error(ctx, error))
        })
        .unwrap_or_else(|| Err(JsError::type_error("no active QuickJS callback")))
    }

    fn native_buffer_memory_failure(&self) -> JsError {
        self.context(|ctx| {
            if self.world.native_failure.get().is_some() {
                let _ = quickjs_check_native_failure(ctx);
            } else {
                let _ = World::native_memory_budget_error(ctx);
            }
            pending_js_exception_error()
        })
        .unwrap_or_else(|| JsError::type_error("no active QuickJS callback"))
    }

    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) -> bool {
        if self.world.native_failure.get().is_some() {
            return false;
        }
        self.context(|ctx| {
            let result = (|| {
                let this =
                    handle_to_quickjs::<EventTarget>(ctx, Handle::from_bits(current_target))?;
                let event = handle_to_quickjs::<Event>(ctx, Handle::from_bits(event))?;
                let callback = value_to_quickjs(ctx, callback.0)?;
                let result: Value = if let Some(function) = callback.as_function() {
                    function.call((This(this), event))?
                } else if let Some(object) = callback.as_object() {
                    let method: Function = object.get("handleEvent")?;
                    method.call((This(object.clone()), event))?
                } else {
                    return Ok(false);
                };
                Ok::<_, rquickjs::Error>(result.as_bool() == Some(false))
            })();
            if result.is_err() && self.world.native_failure.get().is_none() {
                let exception = ctx.catch();
                self.inspector.report(ctx, "Uncaught", &exception, false);
            }
            result.unwrap_or(false)
        })
        .unwrap_or(false)
    }
    fn release_value(&self, bits: u64) {
        self.world.release(bits)
    }

    fn retain_existing_target(&self, handle: u64) -> Result<Option<u64>, JsError> {
        self.context(|ctx| self.world.retain_existing_target(ctx, handle))
            .unwrap_or_else(|| {
                Err(JsError::type_error(
                    "relatedTarget has no active script context",
                ))
            })
    }
    fn build_value(&self, value: HostValue<'_>) -> Option<u64> {
        self.context(|ctx| {
            self.value(ctx, value)
                .ok()
                .map(|v| self.world.store_value(ctx, v))
        })
        .flatten()
    }
    fn throw_value(&self, bits: u64) {
        self.context(|ctx| {
            if let Some(value) = self.world.load_value(ctx, bits) {
                ctx.throw(value);
            }
        });
    }
    fn new_promise(&self) -> Option<PromiseRef> {
        self.context(|ctx| {
            let (promise, resolve, reject) = rquickjs::Promise::new(ctx).ok()?;
            let promise = self.world.store_value(ctx, promise.into_value());
            let resolver = self.next_promise.get();
            self.next_promise.set(resolver + 1);
            self.pending.borrow_mut().insert(
                resolver,
                Pending {
                    promise,
                    resolve: Persistent::save(ctx, resolve),
                    reject: Persistent::save(ctx, reject),
                },
            );
            Some(PromiseRef { promise, resolver })
        })
        .flatten()
    }
    fn settle_promise(&self, resolver: u64, outcome: PromiseOutcome<'_>) -> bool {
        self.context(|ctx| {
            let (value, rejected) = match outcome {
                PromiseOutcome::Resolve(value) => (self.value(ctx, value), false),
                PromiseOutcome::Reject(value) => (self.value(ctx, value), true),
                PromiseOutcome::ParseJson(bytes) => match ctx.json_parse(bytes) {
                    Ok(v) => (Ok(v), false),
                    Err(_) => (Ok(ctx.catch()), true),
                },
            };
            value.is_ok_and(|value| self.settle(ctx, resolver, value, rejected))
        })
        .unwrap_or(false)
    }
    fn start_xhr(&self, request: XhrRequest, handle: XhrHandle) -> Result<(), FetchStartError> {
        let driver = self
            .xhr_driver
            .borrow()
            .clone()
            .ok_or_else(|| FetchStartError::new("XMLHttpRequest is not available"))?;
        driver.start(
            request.into_fetch_request(self.scheduler.current_origin()),
            handle,
        )
    }
    fn abort_xhr(&self, handle: XhrHandle) -> bool {
        self.xhr_driver
            .borrow()
            .clone()
            .is_some_and(|d| d.abort(handle))
    }
    fn schedule_background_task(&self, delay_ms: u64, task: Box<dyn FnOnce()>) {
        self.scheduler.set_background_timeout(delay_ms, task);
    }
}

fn require_window_receiver<'js>(ctx: &Ctx<'js>, this: &Value<'js>) -> rquickjs::Result<()> {
    if this.is_null()
        || this.is_undefined()
        || this
            .as_object()
            .is_some_and(|object| object == &ctx.globals())
    {
        Ok(())
    } else {
        Err(rquickjs::Exception::throw_type(ctx, "Illegal invocation"))
    }
}

pub(crate) fn install<'js>(ctx: &Ctx<'js>, state: &Rc<State>) -> rquickjs::Result<()> {
    let global = ctx.globals();
    install_bindings(ctx, &global)?;
    crate::structured_clone::install(ctx, state)?;
    for (name, local) in [("localStorage", true), ("sessionStorage", false)] {
        let state = state.clone();
        global.prop(
            name,
            rquickjs::object::Accessor::new_get(move |ctx: Ctx<'js>, this: This<Value<'js>>| {
                quickjs_callback_boundary(&ctx, name, || {
                    if !this.0.is_null()
                        && !this.0.is_undefined()
                        && !this
                            .0
                            .as_object()
                            .is_some_and(|object| object == &ctx.globals())
                    {
                        return Err(rquickjs::Exception::throw_type(&ctx, "Illegal invocation"));
                    }
                    let window = HostObject::window(state.world.host.clone());
                    let handle = if local {
                        window.local_storage()
                    } else {
                        window.session_storage()
                    }
                    .map_err(|error| throw_quickjs_error(&ctx, &error))?;
                    state.world.storage_wrapper(&ctx, local, handle)
                })
            })
            .enumerable()
            .configurable(),
        )?;
    }

    {
        let state = state.clone();
        global.prop(
            "crypto",
            rquickjs::object::Accessor::new_get(move |ctx: Ctx<'js>, this: This<Value<'js>>| {
                quickjs_callback_boundary(&ctx, "crypto", || {
                    if !this.0.is_null()
                        && !this.0.is_undefined()
                        && !this
                            .0
                            .as_object()
                            .is_some_and(|object| object == &ctx.globals())
                    {
                        return Err(rquickjs::Exception::throw_type(&ctx, "Illegal invocation"));
                    }
                    let window = HostObject::window(state.world.host.clone());
                    let handle = window
                        .crypto()
                        .map_err(|error| throw_quickjs_error(&ctx, &error))?;
                    state.world.crypto_wrapper(&ctx, handle)
                })
            })
            .enumerable()
            .configurable(),
        )?;
    }

    global.set_prototype(Some(&quickjs_prototype(ctx, "EventTarget")?))?;
    for name in ["window", "self", "top", "parent", "frames"] {
        global.set(name, global.clone())?;
    }
    let document = {
        let doc = state.world.host.doc.borrow();
        doc.node_ref(doc.document_node())
    };
    global.set(
        "document",
        handle_to_quickjs::<Document>(
            ctx,
            Handle::from_bits(encode_node(state.world.host.slot, document)),
        )?,
    )?;
    for (name, interval) in [("setTimeout", false), ("setInterval", true)] {
        let state = state.clone();
        global.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                quickjs_callback_boundary(&ctx, name, || {
                    let callback = args.0.first().cloned().ok_or_else(|| {
                        rquickjs::Exception::throw_type(&ctx, "missing timer handler")
                    })?;
                    let delay = Coerced::<f64>::from_js(
                        &ctx,
                        args.0
                            .get(1)
                            .cloned()
                            .unwrap_or_else(|| Value::new_int(ctx.clone(), 0)),
                    )?
                    .0;
                    let delay = if !delay.is_finite() || delay < 0.0 || delay > i32::MAX as f64 {
                        0
                    } else {
                        delay as u64
                    };
                    let handler = Handler {
                        callback: Persistent::save(&ctx, callback),
                        args: args
                            .0
                            .into_iter()
                            .skip(2)
                            .map(|v| Persistent::save(&ctx, v))
                            .collect(),
                    };
                    let ready = state.ready.clone();
                    let scheduler = state.scheduler.clone();
                    let push = move || {
                        if let Some(timer) = scheduler.defer_firing() {
                            ready.borrow_mut().push_back(Ready {
                                handler: handler.clone(),
                                timer,
                                scope: scheduler.timer_scope(),
                                origin: scheduler.current_origin(),
                            });
                        }
                    };
                    let id = if interval {
                        state.scheduler.set_interval(delay, Box::new(push))
                    } else {
                        state.scheduler.set_timeout(delay, Box::new(push))
                    };
                    Ok(id.as_u64())
                })
            })?,
        )?;
    }
    for name in ["clearTimeout", "clearInterval"] {
        let state = state.clone();
        global.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: Opt<Coerced<f64>>| {
                quickjs_callback_boundary(&ctx, name, || {
                    if let Some(id) = id.0 {
                        state
                            .scheduler
                            .clear_timeout(TimerId::from_u64(id.0.max(0.0) as u64));
                    }
                    Ok(())
                })
            })?,
        )?;
    }
    let queue = rquickjs::Function::new(ctx.clone(), |ctx: Ctx<'js>, callback: Function<'js>| {
        quickjs_callback_boundary(&ctx, "queueMicrotask", || {
            let (promise, resolve, _) = rquickjs::Promise::new(&ctx)?;
            resolve.call::<_, Value>(())?;
            let then: Function = promise.then()?;
            then.call::<_, Value>((This(promise), callback))?;
            drop(resolve);
            Ok(())
        })
    })?;
    global.set("queueMicrotask", queue)?;
    let frames = state.clone();
    global.set(
        "requestAnimationFrame",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, callback: Function<'js>| {
                quickjs_callback_boundary(&ctx, "requestAnimationFrame", || {
                    let handler = Handler {
                        callback: Persistent::save(&ctx, callback.into_value()),
                        args: Vec::new(),
                    };
                    let ready = frames.frames.clone();
                    let id = frames
                        .scheduler
                        .request_animation_frame(Box::new(move |timestamp| {
                            ready.borrow_mut().push_back((handler, timestamp));
                        }));
                    Ok(id.as_u64())
                })
            },
        )?,
    )?;
    let frames = state.clone();
    global.set(
        "cancelAnimationFrame",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: Coerced<f64>| {
            quickjs_callback_boundary(&ctx, "cancelAnimationFrame", || {
                frames
                    .scheduler
                    .cancel_animation_frame(AnimationFrameId::from_u64(id.0.max(0.0) as u64));
                Ok(())
            })
        })?,
    )?;
    let performance = Object::new(ctx.clone())?;
    performance.set("timeOrigin", state.scheduler.clock().time_origin_ms())?;
    let clock = state.scheduler.clock();
    performance.set(
        "now",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            quickjs_callback_boundary(&ctx, "performance.now", || Ok(clock.now().as_millis_f64()))
        })?,
    )?;
    global.set("performance", performance)?;
    let date: Object = global.get("Date")?;
    let clock = state.scheduler.clock();
    date.set(
        "now",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            quickjs_callback_boundary(&ctx, "Date.now", || {
                Ok(clock.time_origin_ms() + clock.now().as_millis_f64())
            })
        })?,
    )?;
    for metric in WindowMetric::ALL {
        let getter_state = state.clone();
        global.prop(
            metric.name(),
            rquickjs::object::Accessor::new(
                move |ctx: Ctx<'js>, this: This<Value<'js>>| {
                    quickjs_callback_boundary(&ctx, metric.name(), || {
                        require_window_receiver(&ctx, &this.0)?;
                        // Cloned out so no borrow is held while the host runs.
                        let host = getter_state.geometry_host.borrow().clone();
                        Ok(metric.read(host.as_deref(), &getter_state.viewport.get()))
                    })
                },
                // `[Replaceable]`: assignment defines an own data property that
                // shadows the accessor.
                move |ctx: Ctx<'js>, this: This<Value<'js>>, value: Value<'js>| {
                    quickjs_callback_boundary(&ctx, metric.name(), || {
                        require_window_receiver(&ctx, &this.0)?;
                        ctx.globals().prop(
                            metric.name(),
                            rquickjs::object::Property::from(value)
                                .writable()
                                .enumerable()
                                .configurable(),
                        )
                    })
                },
            )
            .enumerable()
            .configurable(),
        )?;
    }
    install_exception(ctx)?;
    install_base64(ctx, &global)?;
    install_location(ctx, &global, state)?;
    let state_fetch = state.clone();
    global.set(
        "fetch",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, input: Value<'js>, init: Opt<Value<'js>>| {
                quickjs_callback_boundary(&ctx, "fetch", || {
                    let promise = state_fetch.new_promise().ok_or_else(|| {
                        rquickjs::Exception::throw_type(&ctx, "cannot create fetch promise")
                    })?;
                    let result = (|| {
                        // `RequestInfo`: a `Request` by brand, anything else
                        // as a string.
                        let input = RequestInfo::from_js(&ctx, input)?;
                        let init = RequestInit::from_js(
                            &ctx,
                            init.0.unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                        )?;
                        // This backend has no abort algorithms, so a signal
                        // that could abort is refused rather than ignored.
                        let member = refused_request_init_member(&init).or(matches!(
                            init.signal,
                            Some(Some(_))
                        )
                        .then_some("signal"));
                        if let Some(member) = member {
                            return Err(rquickjs::Exception::throw_type(
                                &ctx,
                                &format!("RequestInit member {member} is not implemented"),
                            ));
                        }
                        // The `Request` constructor's steps, which `fetch()`
                        // runs first and rejects with.
                        let parts =
                            request_parts(&state_fetch.world.host, input, init, FETCH_CONTEXT)
                                .map_err(|e| throw_quickjs_error(&ctx, &e))?;
                        // A request made with a signal carries it here.
                        if parts.signal.is_some() {
                            return Err(rquickjs::Exception::throw_type(
                                &ctx,
                                "RequestInit member signal is not implemented",
                            ));
                        }
                        let initiator = state_fetch
                            .world
                            .host
                            .doc
                            .borrow()
                            .url()
                            .cloned()
                            .unwrap_or_else(|| parts.url.clone());
                        let request = parts
                            .into_fetch_request(initiator, state_fetch.scheduler.current_origin());
                        let driver =
                            state_fetch.fetch_driver.borrow().clone().ok_or_else(|| {
                                rquickjs::Exception::throw_type(&ctx, "fetch is not available")
                            })?;
                        driver
                            .start(request, promise.resolver)
                            .map_err(|e| rquickjs::Exception::throw_type(&ctx, e.message()))?;
                        Ok::<_, rquickjs::Error>(())
                    })();
                    quickjs_check_native_failure(&ctx)?;
                    let value = value_to_quickjs(&ctx, promise.promise)?;
                    if let Err(error) = result {
                        let error = if error.is_exception() {
                            ctx.catch()
                        } else {
                            state_fetch.value(
                                &ctx,
                                HostValue::TypeError {
                                    message: &error.to_string(),
                                },
                            )?
                        };
                        state_fetch.settle(&ctx, promise.resolver, error, true);
                    }
                    Ok(value)
                })
            },
        )?,
    )?;
    let getter = state.clone();
    let setter = state.clone();
    global.prop(
        "onload",
        rquickjs::object::Accessor::new(
            move |ctx: Ctx<'js>| {
                quickjs_callback_boundary(&ctx, "window.onload get", || {
                    HostObject::window(getter.world.host.clone())
                        .event_handler(EventHandlerName::new("onload"))
                        .map(|v| v.map(|v| v.bits()))
                        .map_err(|e| throw_quickjs_error(&ctx, &e))
                        .and_then(|v| match v {
                            Some(bits) => value_to_quickjs(&ctx, bits),
                            None => Ok(Value::new_null(ctx.clone())),
                        })
                })
            },
            move |ctx: Ctx<'js>, value: Value<'js>| {
                quickjs_callback_boundary(&ctx, "window.onload set", || {
                    let callback = if value.is_object() {
                        Some(EventHandlerNonNull::from_bits(
                            setter.world.store_value(&ctx, value),
                        ))
                    } else {
                        None
                    };
                    HostObject::window(setter.world.host.clone())
                        .set_event_handler(EventHandlerName::new("onload"), callback)
                        .map_err(|e| throw_quickjs_error(&ctx, &e))
                })
            },
        )
        .enumerable()
        .configurable(),
    )?;
    Ok(())
}

/// `btoa` and `atob`, the binary-string codecs.
fn install_base64<'js>(ctx: &Ctx<'js>, global: &Object<'js>) -> rquickjs::Result<()> {
    type Codec = fn(&str) -> Option<String>;
    let codecs: [(&'static str, Codec, &'static str); 2] = [
        (
            "btoa",
            tenta_js::base64::btoa,
            "The string to be encoded contains characters outside of the Latin1 range.",
        ),
        (
            "atob",
            tenta_js::base64::atob,
            "The string to be decoded is not correctly encoded.",
        ),
    ];
    for (name, codec, refusal) in codecs {
        global.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                quickjs_callback_boundary(&ctx, name, || {
                    let Some(data) = args.0.first() else {
                        return Err(rquickjs::Exception::throw_type(
                            &ctx,
                            &format!(
                                "Failed to execute '{name}' on 'Window': 1 argument required, \
                                 but only 0 present."
                            ),
                        ));
                    };
                    let data = Coerced::<String>::from_js(&ctx, data.clone())?.0;
                    codec(&data).ok_or_else(|| {
                        throw_quickjs_error(
                            &ctx,
                            &JsError::dom_exception("InvalidCharacterError", refusal),
                        )
                    })
                })
            })?,
        )?;
    }
    Ok(())
}

/// `window.location`, read side only: the nine URL attributes and the
/// stringifier as own properties of one object, answering from the document's
/// current URL. Navigation through `location` is not built, so its setters and
/// methods are absent. Unlike the V8 host this one has no `Location` interface
/// object and does not check the receiver of the getters.
fn install_location<'js>(
    ctx: &Ctx<'js>,
    global: &Object<'js>,
    state: &Rc<State>,
) -> rquickjs::Result<()> {
    let location = Object::new(ctx.clone())?;
    for part in LocationPart::ALL {
        let state = state.clone();
        location.prop(
            part.name(),
            rquickjs::object::Accessor::new_get(move |ctx: Ctx<'js>| {
                quickjs_callback_boundary(&ctx, part.name(), || {
                    Ok(HostObject::window(state.world.host.clone()).location_part(part))
                })
            })
            .enumerable(),
        )?;
    }
    let state = state.clone();
    location.prop(
        "toString",
        rquickjs::object::Property::from(Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            quickjs_callback_boundary(&ctx, "Location.toString", || {
                Ok(HostObject::window(state.world.host.clone()).location_part(LocationPart::Href))
            })
        })?)
        .enumerable(),
    )?;
    location.prop(
        rquickjs::atom::PredefinedAtom::SymbolToStringTag,
        rquickjs::object::Property::from("Location").configurable(),
    )?;
    // Not writable and not configurable: the property is unforgeable.
    global.prop(
        "location",
        rquickjs::object::Property::from(location).enumerable(),
    )
}

fn dom_exception<'js>(ctx: &Ctx<'js>, name: &str, message: &str) -> rquickjs::Result<Value<'js>> {
    quickjs_dom_exception(ctx, name, message)
}
fn install_exception<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<()> {
    let prototype = Object::new(ctx.clone())?;
    let error: Object = ctx.globals().get("Error")?;
    prototype.set_prototype(Some(&error.get("prototype")?))?;
    prototype.set("name", "Error")?;
    prototype.set("message", "")?;
    prototype.prop(
        rquickjs::atom::PredefinedAtom::SymbolToStringTag,
        rquickjs::object::Property::from("DOMException").configurable(),
    )?;
    quickjs_cache_prototype(ctx, "DOMException", prototype.clone(), &["DOMException"])?;
    let ctor = rquickjs::function::Constructor::new_prototype_construct_only(
        ctx,
        prototype,
        |ctx: Ctx<'js>, target: Function<'js>, args: Rest<Value<'js>>| {
            quickjs_callback_boundary(&ctx, "DOMException", || {
                let message = match args.0.first().filter(|value| !value.is_undefined()) {
                    Some(value) => Coerced::<String>::from_js(&ctx, value.clone())?.0,
                    None => String::new(),
                };
                let name = match args.0.get(1).filter(|value| !value.is_undefined()) {
                    Some(value) => Coerced::<String>::from_js(&ctx, value.clone())?.0,
                    None => "Error".into(),
                };
                let resolved_prototype =
                    quickjs_constructor_prototype(&ctx, &target, "DOMException")?;
                let value = quickjs_dom_exception(&ctx, &name, &message)?;
                Ok((value, resolved_prototype))
            })
        },
    )?;
    ctx.globals().set("DOMException", ctor)
}

struct Registry;
impl InterfaceRegistry for Registry {
    fn register(&mut self, _: InterfaceDescriptor) {}
}
fn install_bindings<'js>(ctx: &Ctx<'js>, global: &Object<'js>) -> rquickjs::Result<()> {
    use tenta_js::bindings::quickjs::*;
    let registry = &mut Registry;
    install_event_target::<HostObject>(ctx, global, registry)?;
    install_dom_parser::<HostObject>(ctx, global, registry)?;
    install_xml_serializer::<HostObject>(ctx, global, registry)?;
    install_node::<HostObject>(ctx, global, registry)?;
    install_character_data::<HostObject>(ctx, global, registry)?;
    install_text::<HostObject>(ctx, global, registry)?;
    install_cdata_section::<HostObject>(ctx, global, registry)?;
    install_comment::<HostObject>(ctx, global, registry)?;
    install_element::<HostObject>(ctx, global, registry)?;
    install_svg_element::<HostObject>(ctx, global, registry)?;
    install_html_element::<HostObject>(ctx, global, registry)?;
    install_htmli_frame_element::<HostObject>(ctx, global, registry)?;
    install_html_template_element::<HostObject>(ctx, global, registry)?;
    install_html_div_element::<HostObject>(ctx, global, registry)?;
    install_html_span_element::<HostObject>(ctx, global, registry)?;
    install_html_paragraph_element::<HostObject>(ctx, global, registry)?;
    install_html_html_element::<HostObject>(ctx, global, registry)?;
    install_html_head_element::<HostObject>(ctx, global, registry)?;
    install_html_title_element::<HostObject>(ctx, global, registry)?;
    install_html_base_element::<HostObject>(ctx, global, registry)?;
    install_html_body_element::<HostObject>(ctx, global, registry)?;
    install_html_heading_element::<HostObject>(ctx, global, registry)?;
    install_htmlhr_element::<HostObject>(ctx, global, registry)?;
    install_html_pre_element::<HostObject>(ctx, global, registry)?;
    install_html_quote_element::<HostObject>(ctx, global, registry)?;
    install_htmlo_list_element::<HostObject>(ctx, global, registry)?;
    install_htmlu_list_element::<HostObject>(ctx, global, registry)?;
    install_html_menu_element::<HostObject>(ctx, global, registry)?;
    install_htmlli_element::<HostObject>(ctx, global, registry)?;
    install_htmld_list_element::<HostObject>(ctx, global, registry)?;
    install_html_data_element::<HostObject>(ctx, global, registry)?;
    install_html_time_element::<HostObject>(ctx, global, registry)?;
    install_htmlbr_element::<HostObject>(ctx, global, registry)?;
    install_html_mod_element::<HostObject>(ctx, global, registry)?;
    install_html_picture_element::<HostObject>(ctx, global, registry)?;
    install_html_embed_element::<HostObject>(ctx, global, registry)?;
    install_html_object_element::<HostObject>(ctx, global, registry)?;
    install_html_track_element::<HostObject>(ctx, global, registry)?;
    install_html_map_element::<HostObject>(ctx, global, registry)?;
    install_html_area_element::<HostObject>(ctx, global, registry)?;
    install_html_table_element::<HostObject>(ctx, global, registry)?;
    install_html_table_caption_element::<HostObject>(ctx, global, registry)?;
    install_html_table_col_element::<HostObject>(ctx, global, registry)?;
    install_html_table_section_element::<HostObject>(ctx, global, registry)?;
    install_html_table_row_element::<HostObject>(ctx, global, registry)?;
    install_html_table_cell_element::<HostObject>(ctx, global, registry)?;
    install_html_field_set_element::<HostObject>(ctx, global, registry)?;
    install_html_legend_element::<HostObject>(ctx, global, registry)?;
    install_html_opt_group_element::<HostObject>(ctx, global, registry)?;
    install_html_data_list_element::<HostObject>(ctx, global, registry)?;
    install_html_output_element::<HostObject>(ctx, global, registry)?;
    install_html_progress_element::<HostObject>(ctx, global, registry)?;
    install_html_meter_element::<HostObject>(ctx, global, registry)?;
    install_html_details_element::<HostObject>(ctx, global, registry)?;
    install_html_param_element::<HostObject>(ctx, global, registry)?;
    install_html_font_element::<HostObject>(ctx, global, registry)?;
    install_html_directory_element::<HostObject>(ctx, global, registry)?;
    install_html_frame_element::<HostObject>(ctx, global, registry)?;
    install_html_frame_set_element::<HostObject>(ctx, global, registry)?;
    install_html_marquee_element::<HostObject>(ctx, global, registry)?;
    install_html_unknown_element::<HostObject>(ctx, global, registry)?;
    install_html_anchor_element::<HostObject>(ctx, global, registry)?;
    install_html_image_element::<HostObject>(ctx, global, registry)?;
    install_html_label_element::<HostObject>(ctx, global, registry)?;
    install_html_form_element::<HostObject>(ctx, global, registry)?;
    install_html_input_element::<HostObject>(ctx, global, registry)?;
    install_html_button_element::<HostObject>(ctx, global, registry)?;
    install_html_select_element::<HostObject>(ctx, global, registry)?;
    install_html_text_area_element::<HostObject>(ctx, global, registry)?;
    install_html_option_element::<HostObject>(ctx, global, registry)?;
    install_html_script_element::<HostObject>(ctx, global, registry)?;
    install_html_link_element::<HostObject>(ctx, global, registry)?;
    install_html_style_element::<HostObject>(ctx, global, registry)?;
    install_html_media_element::<HostObject>(ctx, global, registry)?;
    install_html_video_element::<HostObject>(ctx, global, registry)?;
    install_html_audio_element::<HostObject>(ctx, global, registry)?;
    install_html_canvas_element::<HostObject>(ctx, global, registry)?;
    install_html_slot_element::<HostObject>(ctx, global, registry)?;
    install_html_meta_element::<HostObject>(ctx, global, registry)?;
    install_html_source_element::<HostObject>(ctx, global, registry)?;
    install_html_dialog_element::<HostObject>(ctx, global, registry)?;
    install_svg_graphics_element::<HostObject>(ctx, global, registry)?;
    install_svgsvg_element::<HostObject>(ctx, global, registry)?;
    install_svga_element::<HostObject>(ctx, global, registry)?;
    install_dom_rect_read_only::<HostObject>(ctx, global, registry)?;
    install_dom_rect::<HostObject>(ctx, global, registry)?;
    install_dom_rect_list::<HostObject>(ctx, global, registry)?;
    install_document_fragment::<HostObject>(ctx, global, registry)?;
    install_shadow_root::<HostObject>(ctx, global, registry)?;
    install_document::<HostObject>(ctx, global, registry)?;
    install_document_type::<HostObject>(ctx, global, registry)?;
    install_processing_instruction::<HostObject>(ctx, global, registry)?;
    install_dom_implementation::<HostObject>(ctx, global, registry)?;
    install_abstract_range::<HostObject>(ctx, global, registry)?;
    install_static_range::<HostObject>(ctx, global, registry)?;
    install_range::<HostObject>(ctx, global, registry)?;
    install_mutation_record::<HostObject>(ctx, global, registry)?;
    install_mutation_observer::<HostObject>(ctx, global, registry)?;
    install_node_list::<HostObject>(ctx, global, registry)?;
    install_html_collection::<HostObject>(ctx, global, registry)?;
    install_dom_token_list::<HostObject>(ctx, global, registry)?;
    install_dom_string_map::<HostObject>(ctx, global, registry)?;
    install_validity_state::<HostObject>(ctx, global, registry)?;
    install_attr::<HostObject>(ctx, global, registry)?;
    install_named_node_map::<HostObject>(ctx, global, registry)?;
    install_style_sheet::<HostObject>(ctx, global, registry)?;
    install_css_style_sheet::<HostObject>(ctx, global, registry)?;
    install_css_rule::<HostObject>(ctx, global, registry)?;
    install_css_grouping_rule::<HostObject>(ctx, global, registry)?;
    install_css_style_rule::<HostObject>(ctx, global, registry)?;
    install_css_condition_rule::<HostObject>(ctx, global, registry)?;
    install_css_media_rule::<HostObject>(ctx, global, registry)?;
    install_media_list::<HostObject>(ctx, global, registry)?;
    install_css_supports_rule::<HostObject>(ctx, global, registry)?;
    install_css_layer_block_rule::<HostObject>(ctx, global, registry)?;
    install_css_layer_statement_rule::<HostObject>(ctx, global, registry)?;
    install_css_scope_rule::<HostObject>(ctx, global, registry)?;
    install_css_namespace_rule::<HostObject>(ctx, global, registry)?;
    install_css_property_rule::<HostObject>(ctx, global, registry)?;
    install_css_keyframes_rule::<HostObject>(ctx, global, registry)?;
    install_css_keyframe_rule::<HostObject>(ctx, global, registry)?;
    install_css_font_face_rule::<HostObject>(ctx, global, registry)?;
    install_css_font_feature_values_rule::<HostObject>(ctx, global, registry)?;
    install_css_nested_declarations::<HostObject>(ctx, global, registry)?;
    install_css_import_rule::<HostObject>(ctx, global, registry)?;
    install_css_page_rule::<HostObject>(ctx, global, registry)?;
    install_css_rule_list::<HostObject>(ctx, global, registry)?;
    install_style_sheet_list::<HostObject>(ctx, global, registry)?;
    install_css_style_declaration::<HostObject>(ctx, global, registry)?;
    install_css_style_properties::<HostObject>(ctx, global, registry)?;
    install_css::<HostObject>(ctx, global, registry)?;
    install_storage::<HostObject>(ctx, global, registry)?;
    install_crypto::<HostObject>(ctx, global, registry)?;
    install_event::<HostObject>(ctx, global, registry)?;
    install_custom_event::<HostObject>(ctx, global, registry)?;
    install_ui_event::<HostObject>(ctx, global, registry)?;
    install_composition_event::<HostObject>(ctx, global, registry)?;
    install_focus_event::<HostObject>(ctx, global, registry)?;
    install_keyboard_event::<HostObject>(ctx, global, registry)?;
    install_mouse_event::<HostObject>(ctx, global, registry)?;
    install_wheel_event::<HostObject>(ctx, global, registry)?;
    install_progress_event::<HostObject>(ctx, global, registry)?;
    install_error_event::<HostObject>(ctx, global, registry)?;
    install_url_search_params::<HostObject>(ctx, global, registry)?;
    install_url::<HostObject>(ctx, global, registry)?;
    install_headers::<HostObject>(ctx, global, registry)?;
    install_abort_signal::<HostObject>(ctx, global, registry)?;
    install_abort_controller::<HostObject>(ctx, global, registry)?;
    install_text_encoder::<HostObject>(ctx, global, registry)?;
    install_text_decoder::<HostObject>(ctx, global, registry)?;
    install_response::<HostObject>(ctx, global, registry)?;
    install_request::<HostObject>(ctx, global, registry)?;
    install_xml_http_request_event_target::<HostObject>(ctx, global, registry)?;
    install_xml_http_request::<HostObject>(ctx, global, registry)?;
    install_blob::<HostObject>(ctx, global, registry)?;
    install_file::<HostObject>(ctx, global, registry)?;
    install_form_data::<HostObject>(ctx, global, registry)?;
    Ok(())
}
