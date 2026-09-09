//! Host functions that bridge JavaScript to the engine-independent event loop
//! (`docs/spec/05-javascript.md` §5): `setTimeout`, `clearTimeout`,
//! `queueMicrotask`, `requestAnimationFrame` and `cancelAnimationFrame`.
//!
//! The event loop's timer and animation-frame callbacks are `FnOnce` closures
//! with no access to the isolate, but invoking a JS function needs one. The
//! bridge: when a timer or frame callback is scheduled its
//! `v8::Global<Function>` is moved into the closure; when it comes due the
//! closure moves that handle (with the frame timestamp, for `requestAnimationFrame`)
//! onto the matching ready queue. The `Engine`, which owns the isolate, drains
//! those queues and calls each function in its own scope, performing a V8
//! microtask checkpoint after each — so the HTML "run a task, then a microtask
//! checkpoint" discipline holds where it is observable. `queueMicrotask` needs
//! no bridge: the callback reaches the isolate through the scope and enqueues
//! straight into V8's microtask queue, which the same checkpoints drain.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use tenta_js::event_loop::{AnimationFrameId, Scheduler, TimerId};

/// Timer functions whose deadline has passed, in fire order, waiting for the
/// engine to invoke them with the isolate in hand.
pub type ReadyQueue = Rc<RefCell<VecDeque<v8::Global<v8::Function>>>>;

/// Animation-frame callbacks for the current rendering opportunity, each paired
/// with the frame timestamp to pass it, waiting for the engine to invoke them.
pub type ReadyFrameQueue = Rc<RefCell<VecDeque<(v8::Global<v8::Function>, f64)>>>;

/// State shared between the host callbacks (reached through the context slot)
/// and the owning engine. Every field is interior-mutable, so it is stored as a
/// plain `Rc` rather than wrapped again.
pub struct HostState {
    /// The event loop's scheduling surface.
    pub scheduler: Scheduler,
    /// Timers that have fired and await invocation.
    pub ready: ReadyQueue,
    /// Animation-frame callbacks whose frame has come and await invocation.
    pub ready_frames: ReadyFrameQueue,
    /// How many script activations are on the stack. A DOM event dispatched from
    /// within running script (`el.dispatchEvent(...)`) must not run a microtask
    /// checkpoint between listeners; one dispatched from native code (a future
    /// `element.click()` outside script) may. The event dispatcher checks this
    /// to decide (`docs/spec/05-javascript.md` §5).
    pub script_depth: std::cell::Cell<u32>,
}

impl HostState {
    /// Builds host state over an event loop's `scheduler`.
    pub fn new(scheduler: Scheduler) -> Rc<Self> {
        Rc::new(HostState {
            scheduler,
            ready: Rc::new(RefCell::new(VecDeque::new())),
            ready_frames: Rc::new(RefCell::new(VecDeque::new())),
            script_depth: std::cell::Cell::new(0),
        })
    }

    /// Marks the start of a script activation; pair with [`Self::exit_script`].
    pub fn enter_script(&self) {
        self.script_depth.set(self.script_depth.get() + 1);
    }

    /// Marks the end of a script activation.
    pub fn exit_script(&self) {
        self.script_depth
            .set(self.script_depth.get().saturating_sub(1));
    }
}

/// Installs the timer, microtask and animation-frame host functions as own
/// properties of `context`'s global object. Call once per context, inside a
/// context scope.
pub fn install(scope: &mut v8::PinScope, context: v8::Local<v8::Context>) {
    let global = context.global(scope);
    define(scope, global, "setTimeout", set_timeout);
    define(scope, global, "clearTimeout", clear_timeout);
    define(scope, global, "queueMicrotask", queue_microtask);
    define(
        scope,
        global,
        "requestAnimationFrame",
        request_animation_frame,
    );
    define(
        scope,
        global,
        "cancelAnimationFrame",
        cancel_animation_frame,
    );

    // The window/global self-aliases: `window`, `self`, `frames`, `top` and
    // `parent` all name the global object itself — this is a single, same-origin
    // top-level browsing context. Frameworks gate `canUseDOM`/`inBrowser` on
    // `typeof window`, and read `window.event`/`window.HTMLIFrameElement`.
    for name in ["window", "self", "frames", "top", "parent"] {
        let key = v8::String::new(scope, name).expect("global alias name fits the V8 string limit");
        global.set(scope, key.into(), global.into());
    }
}

/// Defines one own property on `target` whose value is a native function.
fn define(
    scope: &mut v8::PinScope,
    target: v8::Local<v8::Object>,
    name: &str,
    callback: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let key = v8::String::new(scope, name).expect("binding name fits the V8 string limit");
    let function =
        v8::Function::new(scope, callback).expect("function creation while a context is entered");
    target.set(scope, key.into(), function.into());
}

/// Fetches the host state from the current context's slot.
fn host_state(scope: &mut v8::PinScope) -> Option<Rc<HostState>> {
    scope.get_current_context().get_slot::<HostState>()
}

/// `setTimeout(handler, delay)`: schedules `handler` after `delay` ms and
/// returns the timer id. A missing or non-function handler, or a missing delay,
/// is treated as a no-op / zero the way the web platform coerces them here.
fn set_timeout(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    let Ok(handler) = v8::Local::<v8::Function>::try_from(args.get(0)) else {
        return;
    };
    // A negative or non-numeric delay clamps to zero, per the timer algorithm.
    let delay = args.get(1).integer_value(scope).unwrap_or(0).max(0) as u64;

    let handler = v8::Global::new(scope, handler);
    let ready = state.ready.clone();
    let id = state.scheduler.set_timeout(
        delay,
        Box::new(move || {
            ready.borrow_mut().push_back(handler);
        }),
    );
    retval.set_double(id.as_u64() as f64);
}

/// `clearTimeout(id)`: cancels the timer, so its handler never reaches the
/// ready queue and its handle is dropped.
fn clear_timeout(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    if let Some(id) = args.get(0).integer_value(scope)
        && id >= 0
    {
        state.scheduler.clear_timeout(TimerId::from_u64(id as u64));
    }
}

/// `queueMicrotask(callback)`: enqueues a microtask straight into V8's queue,
/// which the engine's checkpoints drain in FIFO order alongside promise jobs.
fn queue_microtask(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    if let Ok(callback) = v8::Local::<v8::Function>::try_from(args.get(0)) {
        scope.enqueue_microtask(callback);
    }
}

/// `requestAnimationFrame(callback)`: registers `callback` for the next
/// rendering opportunity and returns its id. The callback receives the frame's
/// timestamp in milliseconds when the engine runs the frame.
fn request_animation_frame(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    let Ok(callback) = v8::Local::<v8::Function>::try_from(args.get(0)) else {
        return;
    };
    let callback = v8::Global::new(scope, callback);
    let ready_frames = state.ready_frames.clone();
    let id = state
        .scheduler
        .request_animation_frame(Box::new(move |timestamp| {
            ready_frames.borrow_mut().push_back((callback, timestamp));
        }));
    retval.set_double(id.as_u64() as f64);
}

/// `cancelAnimationFrame(id)`: unregisters a pending animation-frame callback so
/// it never reaches the ready queue and its handle is dropped.
fn cancel_animation_frame(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    if let Some(id) = args.get(0).integer_value(scope)
        && id >= 0
    {
        state
            .scheduler
            .cancel_animation_frame(AnimationFrameId::from_u64(id as u64));
    }
}
