//! V8 backend (rusty_v8): isolates, snapshots, fast API, native Wasm, worker isolates.
//!
//! P2 package (a) foundation (`docs/spec/05-javascript.md` §4). This first
//! slice initialises the V8 platform once per process and evaluates a script
//! on a freshly created isolate. The per-thread isolate pool, the dedicated
//! engine root thread that owns `V8::initialize`, the custom `PlatformImpl`
//! task routing and the startup snapshot land in the following WP1 steps; the
//! `evaluate` entry point and its error mapping are kept stable so they grow
//! into the pooled path rather than being replaced.

#[allow(unused_imports)]
use tenta_core as _;

mod host;
mod script_host;
mod world;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use tenta_js::bindings::WrapperWorld;
use tenta_js::bindings::host::{HostContext, HostObject, SharedContext, SharedDocument};
use tenta_js::dom::events::ScriptHost;
use tenta_js::event_loop::{Clock, EventLoop, MonoTime};

use host::{HostState, ReadyHandler};
use script_host::V8ScriptHost;

static PLATFORM_INIT: Once = Once::new();

/// Initialises the V8 platform exactly once for the process.
///
/// V8's platform and the global flags are process-global and may be
/// initialised only once; `Once` makes concurrent first calls from several
/// page threads safe. The dedicated engine-root-thread model of
/// `docs/spec/05-javascript.md` §4 (so descendant threads inherit the
/// thread-isolated JIT pkey) is layered on top of this in a later WP1 step.
pub fn ensure_platform() {
    PLATFORM_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// A script error surfaced from V8, carrying the message V8 produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub message: String,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JsError {}

/// One V8 isolate with a single long-lived context. This is the seed of the
/// per-thread pooled isolate of `docs/spec/05-javascript.md` §4: it owns the
/// isolate, is neither `Send` nor `Sync`, and is created and used on one
/// thread. Reusing one [`Engine`] across evaluations keeps global state, which
/// a page needs between script runs.
pub struct Engine {
    // The isolate's near-heap-limit callback holds a raw pointer into
    // `heap_guard`; `Engine`'s `Drop` removes that callback before either field
    // is dropped, so this is sound regardless of field order.
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    heap_guard: Option<Box<HeapGuard>>,
    // The engine-independent event loop this isolate drives, plus the state the
    // host callbacks reach through the context slot. Both share the same
    // `Scheduler` and ready queue, so a `setTimeout` from script schedules into
    // the very loop `run_event_loop` drives.
    event_loop: EventLoop,
    host: Rc<HostState>,
    // Present when the engine was built with a DOM document. The engine keeps
    // its own clones of the wrapper world's state (and the document) so its
    // `Drop` can drain every host and disarm every finalizer while the isolate
    // is still alive, after the `V8World` itself has been moved into a slot.
    dom: Option<DomWorld>,
}

/// The DOM state an [`Engine::with_document`] engine keeps alongside the isolate
/// so it can tear the wrapper world down before the isolate is disposed.
struct DomWorld {
    state: Rc<RefCell<world::WorldState>>,
    ctx: SharedContext,
}

/// Callback state for the near-heap-limit hook: a thread-safe handle to
/// terminate the offending script and a flag the owning [`Engine`] reads after
/// a run to turn that termination into a heap-limit [`JsError`].
struct HeapGuard {
    handle: v8::IsolateHandle,
    tripped: std::sync::atomic::AtomicBool,
}

/// Near-heap-limit callback: terminates the running script and raises the limit
/// enough for V8 to unwind instead of aborting the process
/// (`docs/spec/05-javascript.md` §11, `docs/spec/14-memory.md`).
unsafe extern "C" fn on_near_heap_limit(
    data: *mut std::os::raw::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the pointer to the `HeapGuard` boxed in the owning
    // `Engine`, which outlives the isolate that holds this callback.
    let guard = unsafe { &*(data as *const HeapGuard) };
    guard
        .tripped
        .store(true, std::sync::atomic::Ordering::Release);
    guard.handle.terminate_execution();
    // Headroom so the interrupted allocation can complete and unwind.
    current_heap_limit + current_heap_limit / 2
}

impl Engine {
    /// Builds an isolate with one fresh context and no heap cap.
    pub fn new() -> Self {
        Self::build(None, None, Clock::virtual_at(MonoTime::ZERO))
    }

    /// Builds an isolate whose heap is capped at `max_heap_bytes`; a script that
    /// exceeds it is terminated and reported as a [`JsError`] rather than
    /// crashing the process (`docs/spec/05-javascript.md` §11).
    pub fn with_heap_limit(max_heap_bytes: usize) -> Self {
        Self::build(
            Some(max_heap_bytes),
            None,
            Clock::virtual_at(MonoTime::ZERO),
        )
    }

    /// Builds an isolate whose global object exposes the read-only DOM of
    /// `doc`: the `Node`, `Element` and `Document` interfaces and a `document`
    /// global bound to the document node (`docs/spec/05-javascript.md` §3).
    /// This is the Phase 0c vertical slice that proves the generated V8 glue
    /// end to end; it grows into the full binding surface in phase 1.
    pub fn with_document(doc: SharedDocument) -> Self {
        Self::build(None, Some(doc), Clock::virtual_at(MonoTime::ZERO))
    }

    /// Builds a DOM engine over `doc` driven by `clock`. A live page passes
    /// [`Clock::real`] so timers fire in wall time and `performance.now()`
    /// advances without anyone stepping it; headless work keeps the virtual
    /// clock the other constructors install, which makes timer ordering
    /// reproducible (`docs/spec/05-javascript.md` §5).
    ///
    /// A real-clock engine is driven by [`Self::next_wakeup`] and
    /// [`Self::pump_due_timers`] rather than [`Self::run_event_loop`], which
    /// drives the clock itself and so only moves a virtual one.
    ///
    /// `max_heap_bytes` caps the isolate's heap as [`Self::with_heap_limit`]
    /// does. It is a parameter rather than an afterthought because this is the
    /// constructor that runs *untrusted* page script: without a cap, script
    /// that allocates without bound aborts the process instead of producing a
    /// contained [`JsError`].
    pub fn with_document_on_clock(
        doc: SharedDocument,
        clock: Clock,
        max_heap_bytes: Option<usize>,
    ) -> Self {
        Self::build(max_heap_bytes, Some(doc), clock)
    }

    fn build(max_heap_bytes: Option<usize>, doc: Option<SharedDocument>, clock: Clock) -> Self {
        ensure_platform();
        let mut params = v8::CreateParams::default();
        if let Some(max) = max_heap_bytes {
            params = params.heap_limits(0, max);
        }
        let mut isolate = v8::Isolate::new(params);

        // The event loop owns "when a task runs"; V8 must not drain microtasks on
        // its own, so the engine can run them at the HTML checkpoint instead.
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

        let heap_guard = max_heap_bytes.map(|_| {
            let mut guard = Box::new(HeapGuard {
                handle: isolate.thread_safe_handle(),
                tripped: std::sync::atomic::AtomicBool::new(false),
            });
            let data = (&mut *guard as *mut HeapGuard).cast::<std::os::raw::c_void>();
            isolate.add_near_heap_limit_callback(on_near_heap_limit, data);
            guard
        });

        let event_loop = EventLoop::new(clock);
        let host = HostState::new(event_loop.scheduler());

        let (context, dom) = {
            v8::scope!(let handle_scope, &mut isolate);
            match doc {
                None => {
                    let local = v8::Context::new(handle_scope, Default::default());
                    {
                        let mut context_scope = v8::ContextScope::new(handle_scope, local);
                        v8::scope!(let scope, &mut context_scope);
                        host::install(scope, local, host.scheduler.clock().time_origin_ms());
                    }
                    // The host callbacks reach this state through the context slot.
                    local.set_slot::<HostState>(host.clone());
                    (v8::Global::new(handle_scope, local), None)
                }
                Some(doc) => Self::install_dom(handle_scope, &host, doc),
            }
        };
        let mut engine = Engine {
            isolate,
            context,
            heap_guard,
            event_loop,
            host,
            dom,
        };
        engine.install_script_host();
        engine
    }

    /// Wires the DOM event dispatcher's callback boundary to this engine's
    /// isolate. The dispatcher ([`tenta_js::dom::events`]) invokes listeners only
    /// through [`ScriptHost`]; a [`V8ScriptHost`] over this isolate turns handle
    /// bits back into V8 objects and calls the stored functions. Stored in the
    /// `HostContext` the bindings already share, and dropped in `Drop` before the
    /// wrapper world is cleared. A no-op for a DOM-less engine.
    fn install_script_host(&mut self) {
        let Some(dom) = &self.dom else {
            return;
        };
        // The stable address of the underlying V8 isolate, captured by value.
        // A raw `*mut v8::Isolate` would point into `self.isolate`'s
        // `NonNull<RealIsolate>` field, which moves with the `Engine` when
        // `build` returns it by value, leaving `V8ScriptHost` with a dangling
        // pointer; the raw isolate address itself never moves.
        //
        // SAFETY: `as_raw_isolate_ptr` reads the live isolate's address, only
        // ever reconstructed by `invoke_listener`, which runs synchronously
        // inside a DOM binding call on this engine's thread while the engine
        // (and thus the isolate) is alive. `Drop` clears the stored host before
        // the isolate is disposed.
        let isolate = unsafe { self.isolate.as_raw_isolate_ptr() };
        let script: Rc<dyn ScriptHost> = Rc::new(V8ScriptHost::new(
            isolate,
            self.context.clone(),
            dom.state.clone(),
            self.host.clone(),
        ));
        *dom.ctx.script.borrow_mut() = Some(script);
    }

    /// Creates the DOM-enabled context: installs `Node`, `Element` and
    /// `Document` on a fresh global template (parent-first, so each finds its
    /// parent constructor in the isolate's template cache), builds the context
    /// from it, binds the `document` global and hands the wrapper world to the
    /// runtime. Returns the context and the state the engine drains at teardown.
    fn install_dom(
        handle_scope: &mut v8::PinScope<'_, '_, ()>,
        host: &Rc<HostState>,
        doc: SharedDocument,
    ) -> (v8::Global<v8::Context>, Option<DomWorld>) {
        let state = Rc::new(RefCell::new(world::WorldState::default()));
        // Events timestamp against the loop clock, so `evt.timeStamp` and
        // `performance.now()` are readings of the same clock.
        let ctx = Rc::new(HostContext::new(doc.clone(), host.scheduler.clock()));
        let document_ref = {
            let doc = doc.borrow();
            doc.node_ref(doc.document_node())
        };

        // The generated installers require a context-carrying scope, but the
        // global template must exist before the context built from it. A
        // throwaway bootstrap context supplies that scope; the templates it
        // builds are isolate-global, so the real context inherits every
        // interface.
        let bootstrap = v8::Context::new(handle_scope, Default::default());
        let mut bootstrap_scope = v8::ContextScope::new(handle_scope, bootstrap);
        v8::scope!(let scope, &mut bootstrap_scope);

        let global_template = v8::ObjectTemplate::new(scope);
        {
            let mut registry = world::SkeletonRegistry::default();
            // Install interfaces parent-first: EventTarget is the root (Node
            // inherits it), so it must precede Node.
            tenta_js::bindings::install_event_target::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the EventTarget interface");
            tenta_js::bindings::install_node::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Node interface");
            tenta_js::bindings::install_character_data::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CharacterData interface");
            tenta_js::bindings::install_text::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Text interface");
            tenta_js::bindings::install_comment::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Comment interface");
            tenta_js::bindings::install_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Element interface");
            // SVGElement : Element — installs after Element; SVG-namespace
            // elements get this prototype (see world.rs interface selection).
            tenta_js::bindings::install_svg_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the SVGElement interface");
            // HTMLElement's prototype chain roots at Element, so it installs
            // after Element and before Document (HTML-namespace elements get
            // the HTMLElement prototype; see world.rs interface selection).
            tenta_js::bindings::install_html_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLElement interface");
            // HTMLIFrameElement : HTMLElement — installs after HTMLElement;
            // `<iframe>` elements get this prototype. (The snake name fuses the
            // `HTMLI` capital run per the generator's word splitter.)
            tenta_js::bindings::install_htmli_frame_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLIFrameElement interface");
            // HTMLTemplateElement : HTMLElement — its `content` returns a
            // DocumentFragment, so DocumentFragment must be installed too.
            tenta_js::bindings::install_html_template_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLTemplateElement interface");
            // Phase 4d per-tag interfaces, all `: HTMLElement`, so they install
            // after it. Each gives its tag's wrappers a distinct prototype and a
            // global constructor for `instanceof` (see world.rs tag selection).
            tenta_js::bindings::install_html_div_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLDivElement interface");
            tenta_js::bindings::install_html_span_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLSpanElement interface");
            tenta_js::bindings::install_html_paragraph_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLParagraphElement interface");
            tenta_js::bindings::install_html_anchor_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLAnchorElement interface");
            tenta_js::bindings::install_html_image_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLImageElement interface");
            tenta_js::bindings::install_html_label_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLLabelElement interface");
            tenta_js::bindings::install_html_form_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLFormElement interface");
            tenta_js::bindings::install_html_input_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLInputElement interface");
            tenta_js::bindings::install_html_button_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLButtonElement interface");
            tenta_js::bindings::install_html_select_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLSelectElement interface");
            tenta_js::bindings::install_html_text_area_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLTextAreaElement interface");
            tenta_js::bindings::install_html_option_element::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLOptionElement interface");
            tenta_js::bindings::install_document_fragment::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the DocumentFragment interface");
            tenta_js::bindings::install_document::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the Document interface");
            tenta_js::bindings::install_node_list::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the NodeList interface");
            tenta_js::bindings::install_html_collection::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the HTMLCollection interface");
            tenta_js::bindings::install_dom_token_list::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the DOMTokenList interface");
            tenta_js::bindings::install_css_style_declaration::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CSSStyleDeclaration interface");
            // Event has no DOM parent (its prototype chain roots at Object), so
            // its install order relative to the node types is free.
            tenta_js::bindings::install_event::<HostObject>(scope, global_template, &mut registry)
                .expect("install the Event interface");
            // CustomEvent's prototype chain roots at Event, so it installs after.
            tenta_js::bindings::install_custom_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the CustomEvent interface");
            // UIEvent : Event, then MouseEvent : UIEvent (parent-first).
            tenta_js::bindings::install_ui_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the UIEvent interface");
            tenta_js::bindings::install_mouse_event::<HostObject>(
                scope,
                global_template,
                &mut registry,
            )
            .expect("install the MouseEvent interface");
        }

        let local = v8::Context::new(
            scope,
            v8::ContextOptions {
                global_template: Some(global_template),
                ..Default::default()
            },
        );
        {
            let mut context_scope = v8::ContextScope::new(scope, local);
            v8::scope!(let inner, &mut context_scope);
            host::install(inner, local, host.scheduler.clock().time_origin_ms());
            install_dom_exception(inner);

            // Materialise the `document` wrapper, expose it as a global, then
            // hand the world to the runtime so the generated getters can create
            // further wrappers (e.g. `documentElement`) on demand.
            let world = world::V8World::new(state.clone(), ctx.clone());
            let document_object = world
                .wrapper(
                    inner,
                    "Document",
                    tenta_js::bindings::host::encode_node(document_ref),
                )
                .expect("the document wrapper");
            let key = v8::String::new(inner, "document").expect("`document` fits the string limit");
            local
                .global(inner)
                .set(inner, key.into(), document_object.into());
            tenta_js::bindings::set_wrapper_world(inner, Box::new(world));
        }
        // The host callbacks reach this state through the context slot.
        local.set_slot::<HostState>(host.clone());
        (v8::Global::new(scope, local), Some(DomWorld { state, ctx }))
    }

    /// Compiles and runs `source` in this engine's context. Global state set by
    /// an earlier call is still visible. A compile-time or runtime exception is
    /// returned as [`JsError`] rather than unwinding across the V8 boundary
    /// (`docs/spec/02-architecture.md` §8).
    pub fn evaluate(&mut self, source: &str) -> Result<String, JsError> {
        self.run(source, None)
    }

    /// Forces a full garbage collection (test/diagnostic only). The low-memory
    /// notification runs a full GC synchronously and is safe for a browser
    /// build, unlike exposing `gc()` to web content.
    pub fn low_memory_gc(&mut self) {
        self.isolate.low_memory_notification();
    }

    /// Drains event wrappers a finalizer has collected, releasing their slab
    /// records and any interned `CustomEvent` detail (test-only; the same sweep
    /// runs lazily at the head of `wrapper`). Call after [`Engine::low_memory_gc`]
    /// to make reclamation observable without waiting for the next wrapper.
    pub fn reclaim_dead_events(&mut self) {
        if let Some(dom) = &self.dom {
            let dead = dom.state.borrow_mut().take_dead_events();
            let mut state = dom.state.borrow_mut();
            world::reclaim_dead_events(&mut state, &dom.ctx.events, dead);
        }
    }

    /// The number of live stored values in the wrapper world (test-only): a
    /// leaked `CustomEvent` detail keeps its slot occupied after the event is
    /// reclaimed.
    pub fn stored_value_count(&self) -> usize {
        self.dom
            .as_ref()
            .map(|dom| dom.state.borrow().value_count())
            .unwrap_or(0)
    }

    /// Like [`Engine::evaluate`], but a watchdog thread requests termination if
    /// the script runs longer than `timeout` (`docs/spec/05-javascript.md` §11).
    pub fn evaluate_with_timeout(
        &mut self,
        source: &str,
        timeout: std::time::Duration,
    ) -> Result<String, JsError> {
        self.run(source, Some(timeout))
    }

    /// The shared run path: an optional deadline watchdog, one context-scoped
    /// evaluation, then translation of a termination into the right [`JsError`]
    /// (heap-limit or deadline) and a reset so the isolate stays usable.
    fn run(
        &mut self,
        source: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<String, JsError> {
        let watchdog = timeout.map(|timeout| {
            let handle = self.isolate.thread_safe_handle();
            let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
            let join = std::thread::spawn(move || {
                // Terminate only on a genuine deadline; an early cancel (the run
                // finished) or a dropped sender (the caller panicked) is a no-op.
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    cancel_rx.recv_timeout(timeout)
                {
                    handle.terminate_execution();
                }
            });
            (cancel_tx, join)
        });

        // Mark a script activation so a DOM event dispatched from within this
        // run does not run a microtask checkpoint between listeners.
        self.host.enter_script();
        let (outcome, terminated) = {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            // Re-open as a context-carrying `PinScope` so `to_string` and
            // friends see `HandleScope<Context>`.
            v8::scope!(let scope, &mut context_scope);
            v8::tc_scope!(let tc, scope);
            let outcome = compile_and_run(tc, source);
            (outcome, tc.has_terminated())
        };
        self.host.exit_script();

        if let Some((cancel_tx, join)) = watchdog {
            let _ = cancel_tx.send(());
            let _ = join.join();
        }

        let heap_tripped = self
            .heap_guard
            .as_ref()
            .map(|guard| {
                guard
                    .tripped
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
            })
            .unwrap_or(false);

        // A watchdog can call `terminate_execution()` just after the script
        // already finished — a stray termination that `has_terminated()` did not
        // observe. Clear any pending termination unconditionally once the
        // watchdog has joined (and whenever a run was terminated), so the next
        // evaluation on this isolate never inherits it.
        if terminated || timeout.is_some() {
            self.isolate.cancel_terminate_execution();
        }

        if heap_tripped {
            Err(JsError {
                message: "script exceeded the isolate heap limit".to_owned(),
            })
        } else if terminated && timeout.is_some() {
            Err(JsError {
                message: format!(
                    "script execution exceeded the {:?} deadline",
                    timeout.expect("timeout is set when a deadline watchdog ran")
                ),
            })
        } else {
            outcome
        }
    }

    /// Drives the event loop to quiescence: drains any microtasks the last
    /// script left pending, then repeatedly advances the virtual clock to the
    /// next timer and runs the timers that come due, each followed by a
    /// microtask checkpoint. `max_tasks` bounds the number of timer callbacks
    /// run so a `setTimeout`-recursion cannot spin forever; it returns the
    /// number actually run (`docs/spec/05-javascript.md` §5, §11).
    pub fn run_event_loop(&mut self, max_tasks: usize) -> usize {
        // Microtasks queued by the preceding top-level script run first.
        self.checkpoint();

        let mut run = 0;
        while run < max_tasks {
            let progressed = self.event_loop.advance_to_next_timer();
            if self.host.ready.borrow().is_empty() {
                if progressed {
                    // The clock moved but every due timer was cancelled; keep
                    // draining later deadlines.
                    continue;
                }
                break;
            }
            // The ready queue stays authoritative: a handler is taken from it
            // one at a time rather than moved into a local batch. A panic in a
            // binding unwinds through here (the page thread survives it), and a
            // local batch would be dropped on the way out, silently losing every
            // handler that had not run yet.
            run += self.invoke_ready(max_tasks - run);
        }
        run
    }

    /// Whether a rendering opportunity is owed, so a woken driver knows to call
    /// [`Self::run_animation_frame`]. See [`Self::next_wakeup`]'s contract.
    pub fn frame_due(&self) -> bool {
        !self.host.ready_frames.borrow().is_empty() || self.event_loop.frame_due()
    }

    /// Runs a microtask checkpoint: every pending promise reaction and
    /// `queueMicrotask` callback, including those the checkpoint itself queues,
    /// until the queue is empty.
    ///
    /// The drive methods run one after each callback. A driver that entered
    /// script by some other route — [`Self::evaluate`], a dispatched DOM event
    /// — owes one too, and until it is paid [`Self::next_wakeup`] reports zero.
    pub fn run_microtasks(&mut self) {
        self.checkpoint();
    }

    /// Drains V8's microtask queue and records that the debt is paid.
    ///
    /// Every checkpoint goes through here so the "a checkpoint is owed" flag
    /// cannot drift from reality. It is cleared *after* the drain: V8 runs
    /// microtasks queued by microtasks in the same pass, so once this returns
    /// the queue is genuinely empty, and any script entered afterwards sets the
    /// flag again on its own.
    fn checkpoint(&mut self) {
        self.isolate.perform_microtask_checkpoint();
        self.host.microtasks_pending.set(false);
    }

    /// Invokes up to `budget` handlers from the ready queue, each followed by a
    /// microtask checkpoint, and returns how many ran. Handlers past the budget
    /// stay queued in order for the next drive.
    fn invoke_ready(&mut self, budget: usize) -> usize {
        let mut run = 0;
        let mut taken = 0;
        while taken < budget {
            let next = self.host.ready.borrow_mut().pop_front();
            let Some(ready) = next else { break };
            // A handler cancelled in the deferral window still costs budget: a
            // flood of cancelled timers must not let one drive run unbounded.
            // It runs no script, so it is owed no checkpoint and is not counted.
            taken += 1;
            if self.invoke_timer(&ready) {
                self.checkpoint();
                run += 1;
            }
        }
        run
    }

    /// How long until this engine next has work, or `None` when it has none —
    /// the interval a real-clock page thread parks for. `Duration::ZERO` means
    /// work is due already and the thread must not park at all.
    ///
    /// **Contract.** A woken driver must run what it was woken for, or it will
    /// be told "now" again and spin: [`Self::pump_due_timers`] for timers and
    /// owed microtasks, and [`Self::run_animation_frame`] when
    /// [`Self::frame_due`] says a rendering opportunity is owed — that call is
    /// the only thing that advances the frame clock.
    pub fn next_wakeup(&self) -> Option<std::time::Duration> {
        // Handlers a bounded pump held back are due work the timer heap no
        // longer knows about: it handed them over when they fired. A microtask
        // checkpoint owed since the last script entry is work no queue here
        // knows about at all — V8's microtask queue is not introspectable.
        // Parking on the timer heap alone would leave either unrun until some
        // unrelated timer happened to come due.
        if !self.host.ready.borrow().is_empty()
            || !self.host.ready_frames.borrow().is_empty()
            || self.host.microtasks_pending.get()
        {
            return Some(std::time::Duration::ZERO);
        }
        self.event_loop.next_wakeup()
    }

    /// Runs the timers that are due *now*, without moving the clock, and
    /// returns how many callbacks ran. `max_tasks` bounds one pump so a timer
    /// that reschedules itself at zero delay cannot starve the rest of the
    /// thread; anything still due is picked up by the next pump.
    ///
    /// This is the real-clock counterpart of [`Self::run_event_loop`]: wall
    /// time supplies the advance, so the engine only ever fires what has
    /// already come due (`docs/spec/05-javascript.md` §5).
    pub fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        self.checkpoint();

        // Exactly one sweep of the timer heap per pump. A zero-delay timer a
        // handler schedules is due the moment it is scheduled, so re-reading
        // the heap here would let a `setTimeout(f, 0)` chain spin inside one
        // pump and starve everything else on the thread. It waits for the next
        // pump instead, which the caller enters immediately because
        // `next_wakeup` reports zero.
        self.event_loop.fire_due_timers();
        // Handlers past `max_tasks` stay queued, in order, for the next pump —
        // which the caller enters immediately, because `next_wakeup` reports
        // zero while the ready queue is non-empty.
        self.invoke_ready(max_tasks)
    }

    /// Runs one rendering opportunity at the current virtual time: every
    /// `requestAnimationFrame` callback registered before now runs once, each
    /// with the frame timestamp in milliseconds, followed by a microtask
    /// checkpoint. Callbacks registered during the frame wait for the next one
    /// (`docs/spec/05-javascript.md` §5).
    pub fn run_animation_frame(&mut self) {
        // Moves each due callback's handle onto the ready-frame queue.
        self.event_loop.run_animation_frame();
        self.drain_ready_frames();
    }

    /// Steps the virtual clock forward one 60 Hz frame, runs the timers that
    /// come due within it, then the rendering opportunity — the building block
    /// of a deterministic headless render loop. New timers a callback schedules
    /// this frame do not fire until a later frame, so one `tick_frame` cannot
    /// spin.
    pub fn tick_frame(&mut self) {
        self.event_loop.tick_frame();
        // Timers that came due this frame run as tasks, each with a checkpoint.
        // Through `invoke_ready` like every other drive path, so a panicking
        // handler cannot take the rest of the queue down with it.
        self.invoke_ready(usize::MAX);
        self.drain_ready_frames();
    }

    /// Invokes every animation-frame callback whose frame has come, each with
    /// its timestamp, followed by a microtask checkpoint.
    fn drain_ready_frames(&mut self) {
        let frames: Vec<(v8::Global<v8::Function>, f64)> =
            self.host.ready_frames.borrow_mut().drain(..).collect();
        for (handler, timestamp) in frames {
            self.invoke_handler(&handler, Some(timestamp));
            self.checkpoint();
        }
    }

    /// Calls one fired timer handler with the loop state it was queued under.
    ///
    /// The nesting depth and provenance are reinstalled here, around the JS
    /// call itself, because the task that dequeued the timer only moved the
    /// handle onto the ready queue and has already restored them. Without this
    /// a `setTimeout(f, 0)` chain would be scheduled from depth 1 every time
    /// and never reach the clamp, and every timer-scheduled task would look
    /// like top-level work to the settle machine.
    fn invoke_timer(&mut self, ready: &ReadyHandler) -> bool {
        let scheduler = self.host.scheduler.clone();
        if let Some(id) = ready.timer
            && !scheduler.claim_fired(id)
        {
            // `clearTimeout` landed after this timer fired but before the call.
            // HTML aborts such a timer, and script relies on it: the debounce
            // idiom cancels a pending callback from another callback that came
            // due in the same sweep.
            return false;
        }
        let origin = ready.origin;
        let nesting = ready.nesting;
        let handler = &ready.handler;
        scheduler.with_nesting(nesting, || {
            scheduler.with_origin(origin, || self.invoke_handler(handler, None));
        });
        true
    }

    /// Calls one handler in this engine's context, passing `timestamp` as the
    /// sole argument when present (animation-frame callbacks) or no arguments
    /// (timers). The call runs under a `TryCatch` so an exception the handler
    /// throws is contained here — it leaves no pending exception on the isolate
    /// to disturb the following microtask checkpoint, and the next handler still
    /// runs. Surfacing the exception to the error console lands with the
    /// reporting work package.
    fn invoke_handler(&mut self, handler: &v8::Global<v8::Function>, timestamp: Option<f64>) {
        // A timer or animation-frame callback is a script activation too: an
        // event it dispatches must not checkpoint microtasks between listeners.
        self.host.enter_script();
        {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let mut context_scope = v8::ContextScope::new(handle_scope, context);
            v8::scope!(let scope, &mut context_scope);
            v8::tc_scope!(let tc, scope);
            let function = v8::Local::new(tc, handler);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            match timestamp {
                Some(ms) => {
                    let arg: v8::Local<v8::Value> = v8::Number::new(tc, ms).into();
                    let _ = function.call(tc, recv, &[arg]);
                }
                None => {
                    let _ = function.call(tc, recv, &[]);
                }
            }
        }
        self.host.exit_script();
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Release every script-handle a pending timer or animation-frame
        // callback still holds while the isolate is alive. These callbacks own
        // `v8::Global<Function>` handles (a live `setInterval` keeps one
        // indefinitely, so this is the common case, not an edge one); dropping
        // them here — rather than through field drop order, where `isolate` goes
        // first — keeps teardown sound regardless of that order, matching how the
        // heap guard and wrapper world are handled below. `ready`/`ready_frames`
        // hold handles for callbacks already fired but not yet invoked.
        self.event_loop.clear_pending_callbacks();
        self.host.ready.borrow_mut().clear();
        self.host.ready_frames.borrow_mut().clear();

        // Drain the wrapper world while the isolate is still alive: this
        // reclaims every host box and drops every still-armed `Weak`, which
        // disarms its finalizer so no later collection can run against a
        // reclaimed host. The world is the sole owner of the host boxes, so
        // this drain is the no-leak guarantee (`tests/wrapper_gc.rs`).
        if let Some(dom) = &self.dom {
            // Drop the script host first: it holds a raw isolate pointer and
            // clones of the world state, so it must go before the isolate is
            // disposed and before the world is drained below.
            dom.ctx.script.borrow_mut().take();

            // Clean up any live snapshot wrappers first: their slab entries are
            // held by the context and must be reclaimed.
            let dead_snapshot_bits = dom.state.borrow_mut().take_dead_snapshots();
            for bits in dead_snapshot_bits {
                let snapshot_key = tenta_js::bindings::host::decode(bits).snapshot_key();
                dom.ctx.snapshots.borrow_mut().remove(snapshot_key);
            }

            // Symmetrically reclaim event records for wrappers a finalizer
            // recorded but that no later `wrapper` call swept: their records are
            // held by the context's event slab. `clear` below drops the
            // `dead_events` list, so this final sweep must run before it.
            let dead_event_bits = dom.state.borrow_mut().take_dead_events();
            {
                let mut state = dom.state.borrow_mut();
                world::reclaim_dead_events(&mut state, &dom.ctx.events, dead_event_bits);
            }

            // Clear `HAS_WRAPPER` for every node still wrapped — the ids a
            // finalizer already recorded AND the anchor of every live host —
            // while the document is still borrowable and the isolate still
            // alive, before dropping the hosts. Wrappers still live at teardown
            // are dropped here without ever running a finalizer, so their bits
            // must be cleared explicitly; otherwise a document that outlives
            // the engine keeps those nodes pinned forever.
            let ids = dom.state.borrow().wrapped_node_ids();
            if !ids.is_empty() {
                let mut doc = dom.ctx.doc.borrow_mut();
                for id in ids {
                    doc.set_has_wrapper(id, false);
                }
            }
            dom.state.borrow_mut().clear();
        }
        // Remove the near-heap-limit callback while both fields are still alive,
        // so the isolate can never dereference the `HeapGuard` pointer during
        // teardown. This makes soundness independent of struct field drop order
        // rather than relying on `isolate` being declared before `heap_guard`.
        if self.heap_guard.is_some() {
            self.isolate
                .remove_near_heap_limit_callback(on_near_heap_limit, 0);
        }
    }
}

/// Compiles and runs `source` on a throwaway isolate. Convenience for one-shot
/// evaluation; a page uses a long-lived [`Engine`].
pub fn evaluate(source: &str) -> Result<String, JsError> {
    Engine::new().evaluate(source)
}

/// Compiles `source`, runs it, and coerces the result to a string, mapping any
/// caught exception to [`JsError`].
/// JavaScript that installs `globalThis.DOMException` with the legacy code
/// table (<https://webidl.spec.whatwg.org/#idl-DOMException>). The binding throw
/// path (`throw_js_error`) constructs instances with `new DOMException(message,
/// name)`, so `e.name`, `e.message`, `e.code` and `e instanceof DOMException`
/// all behave per the standard.
const DOM_EXCEPTION_BOOTSTRAP: &str = r#"
(function () {
  "use strict";
  const CODES = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
    InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8,
    NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11,
    SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18,
    NetworkError: 19, AbortError: 20, URLMismatchError: 21,
    QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };
  function DOMException(message = "", name = "Error") {
    const self = Object.create(DOMException.prototype);
    Object.defineProperty(self, "message", {
      value: String(message), enumerable: false, configurable: true,
    });
    Object.defineProperty(self, "name", {
      value: String(name), enumerable: false, configurable: true,
    });
    if (typeof Error.captureStackTrace === "function") {
      Error.captureStackTrace(self, DOMException);
    }
    return self;
  }
  DOMException.prototype = Object.create(Error.prototype);
  Object.defineProperty(DOMException.prototype, "constructor", {
    value: DOMException, writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "name", {
    value: "Error", writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "message", {
    value: "", writable: true, configurable: true,
  });
  Object.defineProperty(DOMException.prototype, "code", {
    get() { return CODES[this.name] || 0; },
    enumerable: false, configurable: true,
  });
  const CONSTANTS = {
    INDEX_SIZE_ERR: 1, DOMSTRING_SIZE_ERR: 2, HIERARCHY_REQUEST_ERR: 3,
    WRONG_DOCUMENT_ERR: 4, INVALID_CHARACTER_ERR: 5, NO_DATA_ALLOWED_ERR: 6,
    NO_MODIFICATION_ALLOWED_ERR: 7, NOT_FOUND_ERR: 8, NOT_SUPPORTED_ERR: 9,
    INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11, SYNTAX_ERR: 12,
    INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14, INVALID_ACCESS_ERR: 15,
    VALIDATION_ERR: 16, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18,
    NETWORK_ERR: 19, ABORT_ERR: 20, URL_MISMATCH_ERR: 21,
    QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24,
    DATA_CLONE_ERR: 25,
  };
  for (const key of Object.keys(CONSTANTS)) {
    const descriptor = { value: CONSTANTS[key], enumerable: true };
    Object.defineProperty(DOMException, key, descriptor);
    Object.defineProperty(DOMException.prototype, key, descriptor);
  }
  Object.defineProperty(globalThis, "DOMException", {
    value: DOMException, writable: true, configurable: true,
  });
})();
"#;

/// Installs the `DOMException` global into the entered context.
fn install_dom_exception(scope: &mut v8::PinScope) {
    let code = v8::String::new(scope, DOM_EXCEPTION_BOOTSTRAP)
        .expect("the DOMException bootstrap fits the V8 string limit");
    let script =
        v8::Script::compile(scope, code, None).expect("the DOMException bootstrap compiles");
    script
        .run(scope)
        .expect("the DOMException bootstrap runs without throwing");
}

fn compile_and_run(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    source: &str,
) -> Result<String, JsError> {
    let Some(code) = v8::String::new(tc, source) else {
        return Err(JsError {
            message: "source string exceeds the V8 string length limit".to_owned(),
        });
    };

    let Some(script) = v8::Script::compile(tc, code, None) else {
        return Err(exception(tc));
    };
    let Some(value) = script.run(tc) else {
        // A terminated run has no exception value; report it as termination so
        // the caller (`Engine::run`) can classify it as a deadline or heap-limit
        // stop instead of a misleading "uncaught exception with no value".
        if tc.has_terminated() {
            return Err(JsError {
                message: "script execution was terminated".to_owned(),
            });
        }
        return Err(exception(tc));
    };

    match value.to_string(tc) {
        Some(string) => Ok(string.to_rust_string_lossy(tc)),
        None => Err(JsError {
            message: "result value could not be coerced to a string".to_owned(),
        }),
    }
}

/// Extracts the pending exception message from a caught [`v8::TryCatch`].
fn exception(tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>) -> JsError {
    let Some(value) = tc.exception() else {
        return JsError {
            message: "uncaught exception with no value".to_owned(),
        };
    };
    let message = value
        .to_string(tc)
        .map(|string| string.to_rust_string_lossy(tc))
        .unwrap_or_else(|| "uncaught exception".to_owned());
    JsError { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_arithmetic_to_a_number() {
        assert_eq!(evaluate("2 + 3").unwrap(), "5");
    }

    #[test]
    fn evaluates_string_expressions() {
        assert_eq!(evaluate("'ten' + 'ta'").unwrap(), "tenta");
    }

    #[test]
    fn a_thrown_error_is_reported_not_panicked() {
        let error = evaluate("throw new Error('boom')").unwrap_err();
        assert!(
            error.message.contains("boom"),
            "expected the thrown message, got {error:?}"
        );
    }

    #[test]
    fn global_state_survives_across_evaluations_on_one_engine() {
        let mut engine = Engine::new();
        assert_eq!(engine.evaluate("globalThis.x = 41; x + 1").unwrap(), "42");
        // A second evaluation sees the global set by the first.
        assert_eq!(engine.evaluate("x + 1").unwrap(), "42");
    }

    #[test]
    fn an_infinite_loop_is_terminated_by_the_watchdog() {
        let mut engine = Engine::new();
        let error = engine
            .evaluate_with_timeout("while (true) {}", std::time::Duration::from_millis(200))
            .unwrap_err();
        assert!(
            error.message.contains("deadline"),
            "expected a timeout error, got {error:?}"
        );
        // The isolate is reusable after a terminated run.
        assert_eq!(engine.evaluate("1 + 1").unwrap(), "2");
    }

    #[test]
    fn a_watchdog_firing_near_completion_does_not_poison_later_runs() {
        let mut engine = Engine::new();
        // A zero deadline makes the watchdog terminate around the moment the
        // trivial script finishes, exercising the stray-termination window.
        // Every run must leave the isolate clean for the next one.
        for _ in 0..50 {
            let _ = engine.evaluate_with_timeout("1 + 1", std::time::Duration::from_millis(0));
        }
        // A subsequent normal evaluation must not inherit a stray termination.
        assert_eq!(engine.evaluate("3 + 4").unwrap(), "7");
    }

    #[test]
    fn deep_recursion_throws_range_error_and_the_engine_survives() {
        let mut engine = Engine::new();
        let error = engine
            .evaluate("function f() { return f(); } f();")
            .unwrap_err();
        assert!(
            error.message.contains("RangeError") || error.message.to_lowercase().contains("stack"),
            "expected a stack-overflow RangeError, got {error:?}"
        );
        // The stack overflow was an exception, not a crash.
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn exceeding_the_heap_limit_is_reported_and_the_engine_survives() {
        let mut engine = Engine::with_heap_limit(24 * 1024 * 1024);
        let error = engine
            .evaluate("const a = []; for (;;) { a.push(new Array(100_000).fill(0)); }")
            .unwrap_err();
        assert!(
            error.message.to_lowercase().contains("heap"),
            "expected a heap-limit error, got {error:?}"
        );
        // The near-heap-limit callback terminated the script instead of aborting
        // the process, so the isolate is still usable.
        assert_eq!(engine.evaluate("6 * 7").unwrap(), "42");
    }

    #[test]
    fn a_syntax_error_is_reported() {
        let error = evaluate("function (").unwrap_err();
        assert!(
            error.message.to_ascii_lowercase().contains("syntax"),
            "expected a syntax error, got {error:?}"
        );
    }

    #[test]
    fn a_set_timeout_handler_runs_when_the_loop_is_driven() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.hit = 0; setTimeout(() => { globalThis.hit = 42; }, 10);")
            .unwrap();
        // Nothing runs until the loop is driven.
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "0");
        let run = engine.run_event_loop(100);
        assert_eq!(run, 1);
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "42");
    }

    #[test]
    fn clear_timeout_prevents_a_handler_from_running() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.hit = 0; \
                 const id = setTimeout(() => { globalThis.hit = 1; }, 10); \
                 clearTimeout(id);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 0);
        assert_eq!(engine.evaluate("globalThis.hit").unwrap(), "0");
    }

    #[test]
    fn timers_run_in_deadline_order_across_the_boundary() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.log = ''; \
                 setTimeout(() => { globalThis.log += 'b'; }, 20); \
                 setTimeout(() => { globalThis.log += 'a'; }, 10);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 2);
        assert_eq!(engine.evaluate("globalThis.log").unwrap(), "ab");
    }

    #[test]
    fn a_timer_can_schedule_another_timer() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.n = 0; \
                 setTimeout(() => { globalThis.n++; \
                   setTimeout(() => { globalThis.n++; }, 5); }, 5);",
            )
            .unwrap();
        assert_eq!(engine.run_event_loop(100), 2);
        assert_eq!(engine.evaluate("globalThis.n").unwrap(), "2");
    }

    #[test]
    fn queue_microtask_runs_at_the_checkpoint() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.m = ''; queueMicrotask(() => { globalThis.m = 'ran'; });")
            .unwrap();
        // The microtask waits for the checkpoint the driven loop performs.
        assert_eq!(engine.evaluate("globalThis.m").unwrap(), "");
        engine.run_event_loop(100);
        assert_eq!(engine.evaluate("globalThis.m").unwrap(), "ran");
    }

    #[test]
    fn a_promise_reaction_resolves_when_the_loop_drains_microtasks() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.p = 0; Promise.resolve().then(() => { globalThis.p = 7; });")
            .unwrap();
        assert_eq!(engine.evaluate("globalThis.p").unwrap(), "0");
        engine.run_event_loop(100);
        assert_eq!(engine.evaluate("globalThis.p").unwrap(), "7");
    }

    #[test]
    fn the_task_budget_bounds_a_set_timeout_recursion() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.c = 0; \
                 function spin() { globalThis.c++; setTimeout(spin, 0); } \
                 setTimeout(spin, 0);",
            )
            .unwrap();
        // A self-rescheduling timer would run forever; the budget caps it.
        let run = engine.run_event_loop(10);
        assert_eq!(run, 10);
        assert_eq!(engine.evaluate("globalThis.c").unwrap(), "10");
    }

    #[test]
    fn a_throwing_timer_handler_is_contained_and_the_loop_continues() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.x = 0; \
                 setTimeout(() => { throw new Error('boom'); }, 10); \
                 setTimeout(() => { globalThis.x = 1; }, 20);",
            )
            .unwrap();
        // The first handler throws; it must not abort the loop or corrupt the
        // isolate, so the second handler still runs and both count as run.
        let run = engine.run_event_loop(100);
        assert_eq!(run, 2);
        assert_eq!(engine.evaluate("globalThis.x").unwrap(), "1");
        // The isolate is still healthy for ordinary evaluation.
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn dropping_an_engine_with_unrun_timers_is_safe() {
        // Both a fired-but-not-invoked handler (in the ready queue) and an
        // un-fired one (held in the scheduler's timer closure) leave a
        // `v8::Global<Function>` alive at drop; rusty_v8 no-ops those resets once
        // the isolate is disposed, so this must not crash under the leak/UAF
        // checkers.
        let mut engine = Engine::new();
        engine
            .evaluate(
                "setTimeout(() => { globalThis.x = 1; }, 1_000_000); \
                 setTimeout(() => { globalThis.y = 2; }, 5);",
            )
            .unwrap();
        // Advance the clock enough to move the 5 ms timer onto the ready queue,
        // but do not invoke it, then drop with both Globals still held.
        engine.event_loop.advance_to_next_timer();
        assert!(!engine.host.ready.borrow().is_empty());
        drop(engine);
    }

    #[test]
    fn an_animation_frame_callback_runs_when_a_frame_is_driven() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.f = -1; requestAnimationFrame((ts) => { globalThis.f = ts; });")
            .unwrap();
        // Nothing runs until a frame is driven.
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "-1");
        engine.run_animation_frame();
        // At the loop origin the frame timestamp is zero.
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "0");
    }

    #[test]
    fn a_frame_callback_receives_the_advancing_frame_timestamp() {
        let mut engine = Engine::new();
        engine
            .evaluate("globalThis.f = -1; requestAnimationFrame((ts) => { globalThis.f = ts; });")
            .unwrap();
        // tick_frame advances the virtual clock one 60 Hz frame (~16.667 ms)
        // before running the callback.
        engine.tick_frame();
        assert_eq!(
            engine
                .evaluate("globalThis.f > 16 && globalThis.f < 17")
                .unwrap(),
            "true"
        );
    }

    #[test]
    fn cancel_animation_frame_prevents_a_callback_from_running() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.f = 0; \
                 const id = requestAnimationFrame(() => { globalThis.f = 1; }); \
                 cancelAnimationFrame(id);",
            )
            .unwrap();
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "0");
    }

    #[test]
    fn a_frame_callback_that_reschedules_runs_on_the_following_frame() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.count = 0; \
                 function tick() { globalThis.count++; \
                   if (globalThis.count < 2) requestAnimationFrame(tick); } \
                 requestAnimationFrame(tick);",
            )
            .unwrap();
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.count").unwrap(), "1");
        // The reschedule lands on the next frame, not this one.
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.count").unwrap(), "2");
    }

    #[test]
    fn a_throwing_frame_callback_is_contained_and_the_next_frame_runs() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.f = 0; \
                 requestAnimationFrame(() => { throw new Error('boom'); }); \
                 requestAnimationFrame(() => { globalThis.f = 1; });",
            )
            .unwrap();
        // The first callback throws; it must not abort the frame or corrupt the
        // isolate, so the second callback in the same frame still runs.
        engine.run_animation_frame();
        assert_eq!(engine.evaluate("globalThis.f").unwrap(), "1");
        assert_eq!(engine.evaluate("2 + 2").unwrap(), "4");
    }

    #[test]
    fn tick_frame_runs_due_timers_then_the_animation_frame() {
        let mut engine = Engine::new();
        engine
            .evaluate(
                "globalThis.log = ''; \
                 setTimeout(() => { globalThis.log += 't'; }, 5); \
                 requestAnimationFrame(() => { globalThis.log += 'r'; });",
            )
            .unwrap();
        // One 60 Hz frame is ~16.7 ms, so the 5 ms timer is due; it runs before
        // the rendering opportunity's animation-frame callback.
        engine.tick_frame();
        assert_eq!(engine.evaluate("globalThis.log").unwrap(), "tr");
    }
}
