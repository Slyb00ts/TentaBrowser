//! Host functions and globals that bridge JavaScript to the engine.
//!
//! Most of this file wires the engine-independent event loop
//! (`docs/spec/05-javascript.md` §5): `setTimeout`, `clearTimeout`,
//! `queueMicrotask`, `requestAnimationFrame` and `cancelAnimationFrame`. It also
//! installs the browsing-context globals a page expects to find already present:
//! the `window`/`self`/`frames`/`top`/`parent` self-aliases and the `navigator`
//! and `performance` objects.
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

use tenta_core::Origin;
use tenta_js::bindings::host::{HostObject, realm_context};
use tenta_js::bindings::{
    AddEventListenerOptionsOrBoolean, Event, EventListener, EventListenerOptionsOrBoolean,
    EventTargetOps, JsError, string_from_v8, throw_js_error, throw_type_error, value_from_v8,
    value_to_v8, world_realm,
};
use tenta_js::dom::events::{HostValue, PromiseOutcome, PromiseRef, ScriptHost};
use tenta_js::event_loop::{AnimationFrameId, Scheduler, TimerId};
use tenta_js::fetch::{FetchDriver, FetchRequest};
use tenta_js::xhr::XhrDriver;
use tenta_net::CookieJar;

/// Timer functions whose deadline has passed, in fire order, waiting for the
/// engine to invoke them with the isolate in hand.
/// A fired timer handler waiting to be invoked, carrying the loop state to
/// install around the call.
///
/// The JS call happens *outside* the task that dequeued the timer - the task
/// only moves the handle onto this queue - so the nesting depth and the
/// provenance have to travel with the handler. Without them a `setTimeout`
/// made from inside a handler would be scheduled at depth 1 from a top-level
/// task, which silently defeats both the nesting clamp and the settle rule.
pub struct ReadyHandler {
    pub handler: v8::Global<v8::Function>,
    pub nesting: u32,
    pub origin: Origin,
    /// The timer this handler came from. The push is what *defers* the call,
    /// so it is recorded as such on the loop; the id is handed back before the
    /// call so a `clearTimeout` landing in between can abort it. `None` only if
    /// a handler were ever queued from outside a timer firing, which nothing
    /// does today - and which is then simply never abortable.
    pub timer: Option<TimerId>,
}

pub type ReadyQueue = Rc<RefCell<VecDeque<ReadyHandler>>>;

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
    /// Set whenever script is entered, cleared by a microtask checkpoint.
    ///
    /// V8's microtask queue is not introspectable, so this is how the driver
    /// knows a promise reaction may still be waiting. A real-clock page parks
    /// on `next_wakeup`, and the timer heap knows nothing about microtasks: a
    /// `.then` after a script entry would sit unrun for as long as the page
    /// happened to have no timer scheduled.
    pub microtasks_pending: std::cell::Cell<bool>,
    /// The driver `fetch()` hands its requests to, or `None` before one is
    /// installed.
    ///
    /// It lives here rather than on the engine because the `fetch` callback's
    /// only handle on the world is this state, reached through the context slot
    /// ([`host_state`]) — and because [`install`], which defines `fetch`, runs
    /// before the page has a driver to give. A `RefCell` and not a `Cell`: the
    /// driver is an `Rc<dyn FetchDriver>`, which is not `Copy`.
    pub fetch_driver: RefCell<Option<Rc<dyn FetchDriver>>>,
    /// The browsing context's cookie jar, handed to the DOM context so
    /// `document.cookie` reads and writes the same store `tenta-net` sends
    /// from. It is a clone of one `Rc`-shared handle and its identity never
    /// changes, so it lives in the constructor rather than behind a setter:
    /// the DOM context is built during `Engine::build`, before any setter could
    /// run.
    pub cookies: CookieJar,
    /// The driver the page's `XMLHttpRequest` objects hand their requests to, or
    /// `None` before one is installed. It lives here for the same reason
    /// [`Self::fetch_driver`] does: the XHR surface reaches the world only
    /// through the context slot ([`host_state`]), and never through the engine.
    pub xhr_driver: RefCell<Option<Rc<dyn XhrDriver>>>,
    /// Set while an engine entry is dispatching a DOM event with **no V8
    /// callback anywhere on the stack**.
    ///
    /// Every other dispatch in this tree is reached from a live binding call, so
    /// [`V8ScriptHost::with_context`](crate::script_host::V8ScriptHost) can
    /// bootstrap a `v8::CallbackScope` — the one sanctioned use of that type. A
    /// native completion drained by the engine's turn loop has no callback to
    /// bootstrap from, so the entry that drains one sets this and
    /// `with_context` opens a plain `HandleScope` instead. It is set for the
    /// *whole* entry, not per dispatch, because `with_context` is also how the
    /// entry builds and throws values; a half-set flag would make the two
    /// disagree.
    pub native_dispatch: std::cell::Cell<bool>,
}

impl HostState {
    /// Builds host state over an event loop's `scheduler` and a browsing
    /// context's `cookies`.
    pub fn new(scheduler: Scheduler, cookies: CookieJar) -> Rc<Self> {
        Rc::new(HostState {
            scheduler,
            ready: Rc::new(RefCell::new(VecDeque::new())),
            ready_frames: Rc::new(RefCell::new(VecDeque::new())),
            script_depth: std::cell::Cell::new(0),
            microtasks_pending: std::cell::Cell::new(false),
            fetch_driver: RefCell::new(None),
            cookies,
            xhr_driver: RefCell::new(None),
            native_dispatch: std::cell::Cell::new(false),
        })
    }

    /// Marks the start of a script activation; pair with [`Self::exit_script`].
    /// Entering script can queue a microtask, so this is where the driver
    /// learns a checkpoint is owed. It is not the only such place: a dispatched
    /// listener records the debt itself, since it re-enters the isolate without
    /// going through here.
    pub fn enter_script(&self) {
        self.script_depth.set(self.script_depth.get() + 1);
        self.microtasks_pending.set(true);
    }

    /// Marks the end of a script activation.
    pub fn exit_script(&self) {
        self.script_depth
            .set(self.script_depth.get().saturating_sub(1));
    }

    /// Installs the driver `fetch()` hands its requests to, replacing any driver
    /// installed before.
    ///
    /// One driver per navigation, because the completions it leaves behind
    /// belong to the document that is current when they are drained
    /// (`tenta_js::engine::PageEngine::set_fetch_driver`).
    pub fn set_fetch_driver(&self, driver: Rc<dyn FetchDriver>) {
        *self.fetch_driver.borrow_mut() = Some(driver);
    }

    /// The installed `fetch` driver, if there is one. See [`Self::set_fetch_driver`]
    /// for why a page may not have installed one yet.
    pub fn fetch_driver(&self) -> Option<Rc<dyn FetchDriver>> {
        self.fetch_driver.borrow().clone()
    }

    /// Installs the driver the page's `XMLHttpRequest` objects hand their
    /// requests to, replacing any driver installed before.
    ///
    /// One driver per navigation, for the same reason
    /// [`Self::set_fetch_driver`] takes one: the completions it leaves behind
    /// belong to the document that is current when they are drained
    /// (`tenta_js::engine::PageEngine::set_xhr_driver`).
    pub fn set_xhr_driver(&self, driver: Rc<dyn XhrDriver>) {
        *self.xhr_driver.borrow_mut() = Some(driver);
    }

    /// Marks the start of an engine entry that runs with no V8 callback on the
    /// stack, so [`V8ScriptHost::with_context`](crate::script_host::V8ScriptHost)
    /// bootstraps a plain `HandleScope`. Pair by dropping the guard.
    ///
    /// The guard restores the flag it found rather than clearing it, so an
    /// entry nested inside another cannot turn the outer one's scope back into
    /// the callback path on the way out.
    pub fn enter_native_dispatch(&self) -> NativeDispatchGuard<'_> {
        NativeDispatchGuard {
            flag: &self.native_dispatch,
            previous: self.native_dispatch.replace(true),
        }
    }
}

/// The RAII half of [`HostState::enter_native_dispatch`].
///
/// A guard rather than a bare `set`, because a body that returns early — or
/// unwinds — with the flag still set would put every later `with_context` on the
/// plain path, including one reached from a live callback. The flag has to
/// describe the stack it is on, so it is restored on the way out of the entry
/// that set it.
pub struct NativeDispatchGuard<'a> {
    flag: &'a std::cell::Cell<bool>,
    previous: bool,
}

impl Drop for NativeDispatchGuard<'_> {
    fn drop(&mut self) {
        self.flag.set(self.previous);
    }
}

/// Installs the timer/microtask/animation-frame host functions, the
/// window/global self-aliases, the window's event-target members, and the
/// `navigator` and `performance` globals as own properties of `context`'s global
/// object. Call once per context, inside a context scope. `time_origin_ms` is
/// the loop clock's wall-clock origin, which `performance.timeOrigin` reports;
/// it is passed in because the host state slot this context reads is only set
/// after `install` returns.
///
/// The window's prototype chain is *not* set here — [`install_window_prototype`]
/// does that, and needs the interface template this function has no reason to
/// know about.
pub fn install(scope: &mut v8::PinScope, context: v8::Local<v8::Context>, time_origin_ms: f64) {
    let global = context.global(scope);
    define(scope, global, "setTimeout", set_timeout);
    define(scope, global, "clearTimeout", clear_timeout);
    define(scope, global, "setInterval", set_interval);
    define(scope, global, "clearInterval", clear_interval);
    define(scope, global, "queueMicrotask", queue_microtask);
    // `fetch` is defined here, before any driver exists: the global has to be
    // present from the moment script can run, and a call made before a driver is
    // installed rejects rather than throwing a `ReferenceError`.
    define(scope, global, "fetch", fetch);
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

    install_performance(scope, global, time_origin_ms);
    install_navigator(scope, global);
    install_window_event_target(scope, global);
}

/// Installs the window's `EventTarget` members on the global object.
///
/// The generated `EventTarget` glue cannot serve the window: it resolves its
/// receiver through the wrapper's internal field and a `Signature` that only
/// instances of the interface's template satisfy, and the window is the global
/// object, which has neither. So the three operations are installed here as own
/// properties of the global — which is also what makes them reachable as
/// `self.addEventListener` and `top.addEventListener`, since every alias names
/// this one object.
///
/// Only the argument decoding is repeated from the generated glue; the
/// semantics — the listener table, the duplicate rule, the reference discipline
/// on the stored callback — stay in [`HostObject`]'s `EventTargetOps`
/// implementation, reached through a window host.
fn install_window_event_target(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    define(scope, global, "addEventListener", window_add_event_listener);
    define(
        scope,
        global,
        "removeEventListener",
        window_remove_event_listener,
    );
    define(scope, global, "dispatchEvent", window_dispatch_event);
}

/// Chains the global object's prototype to `EventTarget.prototype`.
///
/// The window's members are own properties of the global (see
/// [`install_window_event_target`]), but own properties say nothing to
/// `instanceof`, which walks the prototype chain: without this,
/// `window instanceof EventTarget` is false, and code that feature-detects
/// event support that way takes the wrong branch.
///
/// The interface's own `FunctionTemplate` is the authority on what
/// `EventTarget.prototype` is — [`v8::FunctionTemplate::get_function`] is the
/// documented route to the one function instance a template has in a context,
/// and its `prototype` property is the object the generated `instanceof` checks
/// against. The template arrives from the caller rather than being looked up in
/// the template cache here, because a template cache read in this position comes
/// back empty: caching happens while the templates are built for the bootstrap
/// context, and the slot is not visible from here. `tests/dom_window.rs` pins
/// the result against the script-visible `EventTarget.prototype`, so a template
/// that did not match the global's constructor would fail there rather than
/// silently chain the window to a prototype nothing else uses.
///
/// Chaining to that object is what a browser's `window → Window.prototype →
/// WindowProperties.prototype → EventTarget.prototype` chain reaches; the two
/// intermediate links stand for interfaces this engine does not implement yet,
/// and their absence is observable in `window.constructor` reporting
/// `EventTarget`.
///
/// Replacing the global proxy's prototype does not hide the global object's own
/// properties — a script's global reference resolves through the proxy to the
/// global object, which is not what `v8::Object::Get` on the proxy does.
/// `tests/dom_window.rs` pins both halves: the `instanceof` this exists for and
/// the globals that must survive it.
pub fn install_window_prototype(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
    event_target: &v8::Global<v8::FunctionTemplate>,
) {
    let template = v8::Local::new(scope, event_target);
    let constructor = template
        .get_function(scope)
        .expect("a template has a function instance in the context it is installed in");
    // `to_object` because a function is not a `v8::Object` in V8's type
    // hierarchy — only a JS-level object. `ToObject` of a callable is the
    // callable, which is where its properties live.
    let constructor: v8::Local<v8::Value> = constructor.into();
    let constructor = constructor
        .to_object(scope)
        .expect("ToObject of a callable is the callable");
    let key = v8::String::new(scope, "prototype").expect("the member name fits");
    let prototype = constructor
        .get(scope, key.into())
        .expect("the EventTarget constructor has a prototype object");
    let global = context.global(scope);
    let installed = global
        .set_prototype(scope, prototype)
        .expect("the global's prototype is settable before script has run");
    assert!(
        installed,
        "the global object must accept EventTarget.prototype, or `window instanceof EventTarget` \
         silently stays false"
    );
}

/// The window host for the current realm, or the error to report.
///
/// Resolving it per call rather than caching it keeps the window's state on the
/// `HostContext` the bindings already share, so a listener registered through
/// `window.addEventListener` and one registered through
/// `document.addEventListener` live in the same table.
fn window_host(scope: &mut v8::PinScope) -> Result<HostObject, JsError> {
    let realm = world_realm(scope)?;
    let ctx = realm_context(&realm)?;
    Ok(HostObject::window(ctx))
}

/// Decodes the `(type, callback, options)` tail shared by `addEventListener` and
/// `removeEventListener`, mirroring the generated glue's coercion exactly.
fn event_listener_args(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
) -> Result<(String, Option<EventListener>), JsError> {
    let type_ = string_from_v8(scope, args.get(0));
    let raw = args.get(1);
    let callback = if raw.is_null_or_undefined() {
        None
    } else {
        Some(EventListener::from_bits(value_from_v8(scope, raw)?))
    };
    Ok((type_, callback))
}

/// `window.addEventListener(type, callback, options)`.
///
/// The lifetimes are named because the `options` union's `from_v8` ties its
/// value to the scope it converts in: the generated converter builds a sequence
/// from the value, so it needs the value to belong to the scope's handle
/// partition, and the arguments object carries that same lifetime.
fn window_add_event_listener<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut retval: v8::ReturnValue,
) {
    if args.length() < 2 {
        throw_type_error(
            scope,
            "Failed to execute 'addEventListener' on 'EventTarget': 2 argument(s) required",
        );
        return;
    }
    let host = match window_host(scope) {
        Ok(host) => host,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let (type_, callback) = match event_listener_args(scope, &args) {
        Ok(parts) => parts,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let options = if args.length() > 2 && !args.get(2).is_undefined() {
        match AddEventListenerOptionsOrBoolean::from_v8(scope, args.get(2)) {
            Ok(options) => Some(options),
            Err(error) => {
                throw_js_error(scope, &error);
                return;
            }
        }
    } else {
        None
    };
    if let Err(error) = host.add_event_listener(&type_, callback, options) {
        throw_js_error(scope, &error);
        return;
    }
    retval.set_undefined();
}

/// `window.removeEventListener(type, callback, options)`.
fn window_remove_event_listener<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut retval: v8::ReturnValue,
) {
    if args.length() < 2 {
        throw_type_error(
            scope,
            "Failed to execute 'removeEventListener' on 'EventTarget': 2 argument(s) required",
        );
        return;
    }
    let host = match window_host(scope) {
        Ok(host) => host,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let (type_, callback) = match event_listener_args(scope, &args) {
        Ok(parts) => parts,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let options = if args.length() > 2 && !args.get(2).is_undefined() {
        match EventListenerOptionsOrBoolean::from_v8(scope, args.get(2)) {
            Ok(options) => Some(options),
            Err(error) => {
                throw_js_error(scope, &error);
                return;
            }
        }
    } else {
        None
    };
    if let Err(error) = host.remove_event_listener(&type_, callback, options) {
        throw_js_error(scope, &error);
        return;
    }
    retval.set_undefined();
}

/// `window.dispatchEvent(event)`, which dispatches *at* the window: its path is
/// the window alone, so a listener on a node does not see it.
fn window_dispatch_event(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    if args.length() < 1 {
        throw_type_error(
            scope,
            "Failed to execute 'dispatchEvent' on 'EventTarget': 1 argument(s) required",
        );
        return;
    }
    let host = match window_host(scope) {
        Ok(host) => host,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let event = match tenta_js::bindings::handle_from_v8::<Event>(scope, args.get(0)) {
        Ok(event) => event,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    match host.dispatch_event(event) {
        Ok(not_canceled) => retval.set(v8::Boolean::new(scope, not_canceled).into()),
        Err(error) => throw_js_error(scope, &error),
    }
}

/// Sets a string-valued own data property on `target`.
fn set_string(scope: &mut v8::PinScope, target: v8::Local<v8::Object>, name: &str, value: &str) {
    let key = v8::String::new(scope, name).expect("property name fits the V8 string limit");
    let value = v8::String::new(scope, value).expect("property value fits the V8 string limit");
    target.set(scope, key.into(), value.into());
}

/// Sets a number-valued own data property on `target`.
fn set_number(scope: &mut v8::PinScope, target: v8::Local<v8::Object>, name: &str, value: f64) {
    let key = v8::String::new(scope, name).expect("property name fits the V8 string limit");
    let value = v8::Number::new(scope, value);
    target.set(scope, key.into(), value.into());
}

/// Sets a boolean-valued own data property on `target`.
fn set_bool(scope: &mut v8::PinScope, target: v8::Local<v8::Object>, name: &str, value: bool) {
    let key = v8::String::new(scope, name).expect("property name fits the V8 string limit");
    let value = v8::Boolean::new(scope, value);
    target.set(scope, key.into(), value.into());
}

/// Installs `performance` with `now()` and `timeOrigin`, both taken from the
/// event loop's clock: `now()` advances with the virtual scheduler under tests
/// and with the monotonic clock in production, and `timeOrigin` is that clock's
/// wall-clock anchor.
fn install_performance(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    time_origin_ms: f64,
) {
    let performance = v8::Object::new(scope);
    define(scope, performance, "now", performance_now);
    // Fixed for the life of the loop, so a data property rather than a getter.
    // A virtual clock reports 0 (no wall-clock anchor, which is what keeps a
    // headless trace reproducible); a real one reports the Unix-epoch
    // millisecond the loop started, making `timeOrigin + performance.now()` a
    // wall-clock reading.
    set_number(scope, performance, "timeOrigin", time_origin_ms);

    let key = v8::String::new(scope, "performance").expect("`performance` fits the string limit");
    global.set(scope, key.into(), performance.into());
}

/// `performance.now()`: milliseconds since the loop origin, from the scheduler's
/// clock (`MonoTime::as_millis_f64`).
fn performance_now(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let state = host_state(scope);
    // A missing slot would mean `performance.now()` ran in a context `install`
    // never set up. It cannot happen in the current single-context engine; the
    // `unwrap_or` keeps release builds safe, but a return to 0 after a larger
    // reading would violate the monotonic-non-decreasing contract, so a future
    // multi-context change must not reach this path silently.
    debug_assert!(
        state.is_some(),
        "performance.now() requires the host state slot to be installed"
    );
    let now = state
        .map(|state| state.scheduler.now().as_millis_f64())
        .unwrap_or(0.0);
    retval.set_double(now);
}

/// A Chrome-compatible user-agent string minus its leading `Mozilla/` token,
/// which is what `navigator.appVersion` reports.
const APP_VERSION: &str = "5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// A deterministic reported core count. The engine reports a fixed value rather
/// than the real host's `available_parallelism` so the profile stays a stable,
/// reproducible fingerprint and script content cannot read the machine's core
/// count. Eight is a common mid-range value that sizes web worker pools sensibly.
const HARDWARE_CONCURRENCY: f64 = 8.0;

/// Installs a minimal `navigator`. The values are static, Chrome-compatible
/// placeholders for now; a profile-driven user agent and locale land with the
/// snapshot work (`docs/spec/05-javascript.md` §5). The strings mimic Chrome
/// because much of the web (and the eventual Teams target) branches on them, and
/// the shape mirrors Chrome's so unguarded probes like `navigator.plugins.length`
/// or `navigator.webdriver` read a present value instead of throwing.
fn install_navigator(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let navigator = v8::Object::new(scope);
    set_string(
        scope,
        navigator,
        "userAgent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    );
    set_string(scope, navigator, "appCodeName", "Mozilla");
    set_string(scope, navigator, "appName", "Netscape");
    set_string(scope, navigator, "appVersion", APP_VERSION);
    set_string(scope, navigator, "product", "Gecko");
    set_string(scope, navigator, "productSub", "20030107");
    set_string(scope, navigator, "vendor", "Google Inc.");
    set_string(scope, navigator, "vendorSub", "");
    set_string(scope, navigator, "platform", "MacIntel");
    set_string(scope, navigator, "language", "en-US");

    let languages = v8::Array::new(scope, 2);
    for (index, tag) in ["en-US", "en"].into_iter().enumerate() {
        let value = v8::String::new(scope, tag).expect("a language tag fits the string limit");
        languages.set_index(scope, index as u32, value.into());
    }
    let key = v8::String::new(scope, "languages").expect("`languages` fits the string limit");
    navigator.set(scope, key.into(), languages.into());

    set_number(
        scope,
        navigator,
        "hardwareConcurrency",
        HARDWARE_CONCURRENCY,
    );
    set_number(scope, navigator, "maxTouchPoints", 0.0);
    set_bool(scope, navigator, "onLine", true);
    set_bool(scope, navigator, "cookieEnabled", true);
    // Real Chrome reports `false` here; an undefined `webdriver` is itself an
    // automation-detection signal, so the Chrome-compat profile reports `false`.
    set_bool(scope, navigator, "webdriver", false);

    // `plugins` and `mimeTypes` are empty-but-present on modern Chrome. Empty
    // arrays satisfy the common `navigator.plugins.length` / iteration probes
    // without pretending specific plugins exist.
    let plugins = v8::Array::new(scope, 0);
    let key = v8::String::new(scope, "plugins").expect("`plugins` fits the string limit");
    navigator.set(scope, key.into(), plugins.into());
    let mime_types = v8::Array::new(scope, 0);
    let key = v8::String::new(scope, "mimeTypes").expect("`mimeTypes` fits the string limit");
    navigator.set(scope, key.into(), mime_types.into());

    let key = v8::String::new(scope, "navigator").expect("`navigator` fits the string limit");
    global.set(scope, key.into(), navigator.into());
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
    let scheduler = state.scheduler.clone();
    let id = state.scheduler.set_timeout(
        delay,
        Box::new(move || {
            ready.borrow_mut().push_back(ReadyHandler {
                handler,
                nesting: scheduler.current_nesting(),
                origin: scheduler.current_origin(),
                timer: scheduler.defer_firing(),
            });
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

/// `setInterval(handler, delay)`: schedules `handler` to run every `delay` ms
/// and returns the interval id. The callback is re-armed by the loop under a
/// stable id; each firing pushes the handler onto the ready queue for the engine
/// to invoke, exactly like `setTimeout`.
fn set_interval(
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
    let delay = args.get(1).integer_value(scope).unwrap_or(0).max(0) as u64;

    let handler = v8::Global::new(scope, handler);
    let ready = state.ready.clone();
    let scheduler = state.scheduler.clone();
    let id = state.scheduler.set_interval(
        delay,
        Box::new(move || {
            ready.borrow_mut().push_back(ReadyHandler {
                handler: handler.clone(),
                nesting: scheduler.current_nesting(),
                origin: scheduler.current_origin(),
                timer: scheduler.defer_firing(),
            });
        }),
    );
    retval.set_double(id.as_u64() as f64);
}

/// `clearInterval(id)`: cancels a repeating timer so it is not re-armed and its
/// handler never reaches the ready queue again.
fn clear_interval(
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
        state.scheduler.clear_interval(TimerId::from_u64(id as u64));
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

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------

/// `fetch(input, init)`: creates the promise, resolves `input` against the
/// document, and hands the request to the installed driver.
///
/// Everything after the argument conversion is a *rejection*, not a throw: the
/// Fetch Standard's own `fetch()` wraps its steps in a promise so that a failed
/// call is a settled promise rather than an abrupt completion. The two things
/// that do throw are the dictionary conversion this engine refuses (§ the
/// initializer below) and the absence of a world to create the promise in.
fn fetch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue,
) {
    let Some(state) = host_state(scope) else {
        return;
    };
    let input = string_from_v8(scope, args.get(0));

    // `RequestInfo` is `(Request or USVString)`; only the second arm is
    // reachable, because `Request` is not a global this engine installs.
    if let Some(reason) = unsupported_init(scope, args.get(1)) {
        throw_type_error(scope, &format!("Failed to execute 'fetch': {reason}"));
        return;
    }

    let realm = match world_realm(scope) {
        Ok(realm) => realm,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let ctx = match realm_context(&realm) {
        Ok(ctx) => ctx,
        Err(error) => {
            throw_js_error(scope, &error);
            return;
        }
    };
    let script = ctx.script.borrow().as_ref().map(Rc::clone);
    let Some(script) = script else {
        throw_type_error(
            scope,
            "Failed to execute 'fetch': this world has no script host",
        );
        return;
    };
    let Some(promise) = script.new_promise() else {
        throw_type_error(
            scope,
            "Failed to execute 'fetch': the promise could not be created",
        );
        return;
    };

    // The promise exists before the URL is resolved so that every step from here
    // on can reject. The document borrow ends with this block: nothing below may
    // hold it, because the engine re-enters Rust on the way out of a settlement
    // (`tenta_js::engine`, boundary rule 1).
    let resolved = {
        let doc = ctx.doc.borrow();
        doc.resolve_url(&input).map(|url| (url, doc.url().cloned()))
    };
    let Some((url, document_url)) = resolved else {
        let message = format!("Failed to parse URL from {input}");
        reject_promise(scope, &mut retval, &script, promise, &message);
        return;
    };

    // The document's URL, or the request's when the document has none: a
    // document built without one reports the empty string, and an empty
    // initiator is not a URL a driver could send.
    let initiator = document_url.unwrap_or_else(|| url.clone());
    let request = FetchRequest::get(url, initiator, state.scheduler.current_origin());

    // No driver is a state a page can be in — `fetch` is installed before the
    // page has one — so it is a rejection rather than a panic. Once 4a-1b lands,
    // a page always has one.
    let Some(driver) = state.fetch_driver() else {
        let message = "fetch is not available";
        reject_promise(scope, &mut retval, &script, promise, message);
        return;
    };
    match driver.start(request, promise.resolver) {
        // A refusal rejects the promise rather than throwing: the promise exists
        // precisely so a failure the driver reports reaches script as one.
        Err(error) => reject_promise(scope, &mut retval, &script, promise, error.message()),
        Ok(()) => return_promise(scope, &mut retval, promise),
    }
}

/// Hands `promise` to script as the call's return value.
fn return_promise(scope: &mut v8::PinScope, retval: &mut v8::ReturnValue, promise: PromiseRef) {
    // The promise's reference is *not* released here: the settlement releases
    // it, because by then script has the object and the world's own hold on it
    // is over (`tenta_js::dom::events::ScriptHost::new_promise`).
    if let Ok(value) = value_to_v8(scope, promise.promise) {
        retval.set(value);
    }
}

/// Rejects `promise` with a `TypeError` carrying `message`, and returns the
/// promise to script. A settlement that could not be queued leaves the promise
/// pending, which is the one outcome worse than a rejection and the one this
/// cannot report.
fn reject_promise(
    scope: &mut v8::PinScope,
    retval: &mut v8::ReturnValue,
    script: &Rc<dyn ScriptHost>,
    promise: PromiseRef,
    message: &str,
) {
    script.settle_promise(
        promise.resolver,
        PromiseOutcome::Reject(HostValue::TypeError { message }),
    );
    return_promise(scope, retval, promise);
}

/// Why the second argument cannot be honoured, or `None` when it can.
///
/// `RequestInit` is declared as nothing and refused when present: ignoring
/// `{method: "POST"}` would send a *different request than the page asked for*,
/// silently, and a declared-but-unimplemented member is the bug class this
/// engine does not ship. The slice that implements the member removes the
/// refusal.
///
/// A WebIDL dictionary conversion is what decides which second arguments are
/// even a `RequestInit`: `null` and `undefined` are the empty dictionary, an
/// object's own **enumerable** keys are its members — inherited ones are not —
/// and anything that is not an object at all throws. All three cases are handled
/// here, so `fetch(url)`, `fetch(url, undefined)`, `fetch(url, null)` and
/// `fetch(url, {})` are all a `GET`.
fn unsupported_init(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Option<String> {
    if value.is_null_or_undefined() {
        return None;
    }
    let object = match v8::Local::<v8::Object>::try_from(value) {
        Ok(object) => object,
        Err(_) => return Some("the provided value is not of type 'RequestInit'".to_owned()),
    };
    let args = v8::GetPropertyNamesArgs {
        // Symbol keys are never dictionary members, and a non-enumerable own
        // property is not a member either — which is exactly this filter.
        property_filter: v8::PropertyFilter::ONLY_ENUMERABLE | v8::PropertyFilter::SKIP_SYMBOLS,
        ..Default::default()
    };
    let names = object.get_own_property_names(scope, args)?;
    // An empty dictionary is the empty `RequestInit`, and there is no member to
    // name. The length is checked rather than the first element: V8's
    // `get_index` out of range hands back `undefined` instead of failing, so
    // reading index 0 off `{}` would name a member called "undefined".
    if names.length() == 0 {
        return None;
    }
    let first = names.get_index(scope, 0)?;
    Some(format!(
        "the RequestInit member '{}' is not implemented",
        string_from_v8(scope, first)
    ))
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
