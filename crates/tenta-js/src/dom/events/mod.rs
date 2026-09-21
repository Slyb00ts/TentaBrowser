//! DOM events: the listener table, event records and the dispatch algorithm.
//!
//! This module is engine-independent. It knows nothing about V8 or QuickJS: a
//! stored JS callback is an opaque [`CallbackRef`] (the `u64` a wrapper world's
//! `store_value` handed back), and the only way to run one is through the
//! [`ScriptHost`] the engine installs on the [`HostContext`].
//!
//! A listener is registered on a [`TargetKey`] — a node, or the window. The
//! window is not a node, so it cannot be keyed by [`NodeRef`] the way an
//! element is; it is still the outermost entry of every event path, and it is
//! what makes `window.addEventListener('click', …)` fire for a click on a
//! descendant.
//!
//! # Borrow discipline (invariant A5)
//!
//! No `RefCell` borrow of the document, the listener table or the event slab is
//! ever held across a call into script. [`dispatch`] snapshots the propagation
//! path and each node's listener list, drops every borrow, and only then
//! invokes a listener. Every operation a listener can re-enter (`stopPropagation`,
//! `addEventListener`, a nested `dispatchEvent`, a DOM mutation) takes its own
//! short borrow, so re-entrancy can never alias a live borrow.

use std::collections::HashMap;

use tenta_core::NodeRef;

use crate::bindings::JsError;
use crate::bindings::host::{
    HostContext, HostKey, TAG_HOST, TAG_NODE, TAG_WINDOW, decode, encode_event, encode_host,
    encode_node, encode_window,
};
use crate::fetch::FetchStartError;
use crate::xhr::XhrRequest;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Script host boundary
// ---------------------------------------------------------------------------

/// An opaque reference to a JS value held by the wrapper world: the `u64` bits
/// that `WrapperWorld::store_value` returned. One stored reference is owned per
/// [`CallbackRef`]; dropping it means calling [`ScriptHost::release_value`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CallbackRef(pub u64);

/// An opaque reference to a pending JS promise, in the same bits-only style as
/// [`CallbackRef`]: the promise the host hands back to script, and the resolver
/// that settles it. Both `u64`s are stored references of the wrapper world, one
/// owned reference each, and [`ScriptHost::settle_promise`] gives both back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PromiseRef {
    /// The bits of the promise object, which is what the glue turns back into
    /// the value the member returns.
    pub promise: u64,
    /// The bits of the resolver, which is what settles the promise.
    pub resolver: u64,
}

/// How a promise settles: the Fetch Standard's *consume body* resolves its
/// promise with a converted value and rejects it with an error, and `json()`
/// converts a byte sequence rather than a value.
///
/// The two are distinct steps in the standard and stay distinct here because
/// the host can do neither: it holds no isolate, so it cannot build a
/// JavaScript value, and it cannot run `JSON.parse`, whose result is an
/// arbitrary object graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromiseOutcome<'a> {
    /// Resolve the promise with this value.
    Resolve(HostValue<'a>),
    /// Reject the promise with this value.
    Reject(HostValue<'a>),
    /// Resolve with the JavaScript value `JSON.parse` of these bytes, or reject
    /// with a `SyntaxError` if they do not parse — the standard's "parse JSON
    /// from bytes", one step.
    ParseJson(&'a [u8]),
}

/// The engine's callback boundary. The event dispatcher calls into script only
/// through this trait, so the dispatch algorithm stays engine-independent and
/// unit-testable with a recording mock.
pub trait ScriptHost {
    /// Invokes the stored callback with `this` set to the wrapper of
    /// `current_target` and one argument, the wrapper of `event` (both are
    /// handle bits). A callable value is called directly; a non-callable object
    /// has its `handleEvent` method invoked with `this` set to the object; any
    /// other value is ignored. Exceptions are reported and swallowed. A
    /// microtask checkpoint runs afterwards only when no script is already on
    /// the stack.
    ///
    /// Returns whether the callback returned exactly `false`. The comparison is
    /// strict because an `EventHandler` returns `any`: `0`, `""`, `null` and
    /// `undefined` are not `false` and do not cancel. Only an event handler's
    /// result is meaningful (HTML §8.1.8.1's processing algorithm); the
    /// dispatcher discards it for every other listener.
    ///
    /// # Preconditions
    ///
    /// The caller holds no `RefCell` borrow of the document, listener table or
    /// event slab (invariant A5).
    fn invoke_listener(&self, callback: CallbackRef, current_target: u64, event: u64) -> bool;

    /// Drops one owned reference to a stored value (a callback, or a
    /// `CustomEvent` detail).
    fn release_value(&self, bits: u64);

    /// Interns a value the engine has to build and returns the bits that stand
    /// for it, or `None` when this world cannot build one.
    ///
    /// The host owns records whose members hand JavaScript values back, and two
    /// of those values have no representation to hand back from the host side:
    /// `undefined` is not one of the prelude's sentinels (only `null` is), and a
    /// `DOMException` exists only once a constructor call on the context's
    /// global has built it. Both are built here, behind the same boundary the
    /// dispatcher already uses to run a listener.
    ///
    /// # Ownership
    ///
    /// The returned bits carry one stored reference, exactly as
    /// `WrapperWorld::store_value` returns them: the caller owns it and must
    /// give it back through [`Self::release_value`] when the record that holds
    /// it is freed — except for the world-wide `undefined`, which the
    /// implementation is allowed to cache and owns itself, so that every unset
    /// `any` reads back as the same value.
    fn build_value(&self, value: HostValue<'_>) -> Option<u64>;

    /// Throws the value at `bits` in the script context that is on the stack, so
    /// the exception propagates out of the binding call that asked for it.
    ///
    /// A no-op when the value is gone or no context is entered. This is the only
    /// way a binding can raise a value that is *not* a fresh `JsError`: WebIDL
    /// says `AbortSignal.throwIfAborted()` throws the signal's abort reason, and
    /// the reason is an arbitrary value the page stored — including one the page
    /// holds an identity for, which `assert_throws_exactly` compares by
    /// identity.
    fn throw_value(&self, bits: u64);

    /// Creates a pending JavaScript promise, for a member that has to return one
    /// before its work is done — `Body.text()` and `Body.json()` are the first
    /// two.
    ///
    /// # Ownership
    ///
    /// Both `u64`s of the returned [`PromiseRef`] carry one stored reference,
    /// exactly as `WrapperWorld::store_value` returns them. They are given back
    /// by the settlement, not by the caller: [`Self::settle_promise`] releases
    /// the resolver's reference, and the promise's too, because by the time a
    /// settlement runs the glue has already handed the promise object to script
    /// and the world's own hold on it is over. A promise that is created and
    /// never settled therefore leaks both slots. Every producer settles the
    /// promise it makes, but `fetch()` is the one whose settlement arrives
    /// *outside* the call that made it: the engine queues it when the driver's
    /// completion is drained (`crate::engine::PageEngine::settle_fetches`), so a
    /// request no driver ever completes leaves its pair behind until the engine
    /// is dropped.
    ///
    /// `None` when this world cannot build one.
    fn new_promise(&self) -> Option<PromiseRef>;

    /// Starts an `XMLHttpRequest` the page asked for by calling `send()`.
    ///
    /// This is a boundary rather than a convenience for the same reason every
    /// other member is: the host layer holds no isolate and no scheduler, so it
    /// can neither name the task's [`Origin`](tenta_core::Origin) nor reach the
    /// driver slot, and both live in the backend. The host builds an
    /// [`XhrRequest`] out of the object's own record — the page's half of the
    /// request — and hands it here; the implementation adds the provenance and
    /// calls the installed [`XhrDriver`](crate::xhr::XhrDriver).
    ///
    /// `target` is the opaque handle bits of the `XMLHttpRequest` object, the
    /// same encoding [`DispatchTarget::host`] decodes, so the completion a
    /// driver leaves behind lands at the object that made the request.
    ///
    /// # Failure
    ///
    /// `Err` means **no request was started**, for any reason: no driver is
    /// installed, or the driver refused it before sending anything. XHR has no
    /// promise to reject, so the caller turns the message into the standard's
    /// *request error steps* — a synchronous report the page observes as a
    /// failed request, never as a request still in flight.
    fn start_xhr(&self, request: XhrRequest, target: u64) -> Result<(), FetchStartError>;

    /// Settles the promise whose resolver holds the bits `resolver`, reporting
    /// whether a settlement was queued.
    ///
    /// The settlement is queued as a microtask rather than run in place, which
    /// is what keeps the standard's ordering: a settlement requested from inside
    /// a microtask — `Promise.resolve().then(() => r.text().then(f))` — has to
    /// call `f` in the same checkpoint pass, and a settlement applied before the
    /// pass would push it a whole checkpoint later.
    ///
    /// Returns `false` when no settlement was queued: a resolver whose promise
    /// has already been settled, or one this world does not know. A caller
    /// inside script always has a scope to build the value in. A completion that
    /// arrives with no script on the stack does not come through here at all —
    /// it is left in a table for the engine to drain (`d-4a-1`'s
    /// `PageEngine::settle_fetches`), which builds the value in a scope of its
    /// own and queues the settlement directly.
    fn settle_promise(&self, resolver: u64, outcome: PromiseOutcome<'_>) -> bool;
}

/// A JavaScript value the host needs the engine to build, because the host
/// holds no isolate of its own (`docs/spec/05-javascript.md` §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostValue<'a> {
    /// The JavaScript `undefined`, interned once per world so every read of an
    /// unset `any` is the same value.
    Undefined,
    /// A fresh `DOMException` built as `new DOMException(message, name)` from
    /// the context's global object.
    DomException {
        /// The `DOMException.name` of the new object, e.g. `"AbortError"`.
        name: &'a str,
        /// The `DOMException.message` of the new object.
        message: &'a str,
    },
    /// A JavaScript string. This is how a body read resolves: `text()` settles
    /// its promise with the decoded bytes.
    ///
    /// The text is owned rather than borrowed because it is *converted* data:
    /// decoding a body produces a fresh `String`, and a promise outcome that
    /// borrowed it would have to name a temporary that lives no longer than the
    /// conversion step that made it.
    String(String),
    /// A fresh `TypeError` built as `new TypeError(message)` from the context's
    /// global object.
    ///
    /// Fetch rejects with a `TypeError` in several places — a body read twice,
    /// a body on a null-body status — and `JsError::type_error` can only
    /// *throw*, which would break the promise contract those steps have.
    TypeError {
        /// The `TypeError.message` of the new object.
        message: &'a str,
    },
}

// ---------------------------------------------------------------------------
// Generational slab
// ---------------------------------------------------------------------------

/// A key into a [`GenSlab`]: a slot index plus the generation the slot had when
/// the key was minted. A key whose generation no longer matches its slot is
/// stale and resolves to `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GenKey {
    /// Index of the slot.
    pub index: u32,
    /// Generation the slot held when this key was created.
    pub generation: u16,
}

struct GenSlot<T> {
    generation: u16,
    value: Option<T>,
}

/// A slab with free-list slot reuse and per-slot generation counters, so a key
/// to a freed slot cannot alias the next occupant (until the 16-bit generation
/// wraps, which matches the node-handle TODO and is accepted for this phase).
pub struct GenSlab<T> {
    slots: Vec<GenSlot<T>>,
    free: Vec<u32>,
}

impl<T> GenSlab<T> {
    /// A new empty slab.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Inserts a value, reusing a free slot when available, and returns its key.
    pub fn insert(&mut self, value: T) -> GenKey {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            GenKey {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(GenSlot {
                generation: 0,
                value: Some(value),
            });
            GenKey {
                index,
                generation: 0,
            }
        }
    }

    /// Borrows the value for `key`, or `None` when the key is stale/out of range.
    pub fn get(&self, key: GenKey) -> Option<&T> {
        let slot = self.slots.get(key.index as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.value.as_ref()
    }

    /// Mutably borrows the value for `key`, or `None` when stale/out of range.
    pub fn get_mut(&mut self, key: GenKey) -> Option<&mut T> {
        let slot = self.slots.get_mut(key.index as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.value.as_mut()
    }

    /// Frees the slot for `key` (bumping its generation) and returns the value,
    /// or `None` when the key is already stale.
    pub fn remove(&mut self, key: GenKey) -> Option<T> {
        let slot = self.slots.get_mut(key.index as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.generation = slot.generation.wrapping_add(1);
        let value = slot.value.take();
        self.free.push(key.index);
        value
    }

    /// The number of live values (test/measurement hook).
    pub fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }

    /// Whether the slab holds no live values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Default for GenSlab<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Event records
// ---------------------------------------------------------------------------

/// `Event.eventPhase` value: not being dispatched.
pub const PHASE_NONE: u16 = 0;
/// `Event.eventPhase` value: capturing phase.
pub const PHASE_CAPTURING: u16 = 1;
/// `Event.eventPhase` value: at the target.
pub const PHASE_AT_TARGET: u16 = 2;
/// `Event.eventPhase` value: bubbling phase.
pub const PHASE_BUBBLING: u16 = 3;

/// A key into the per-world [`EventSlab`].
pub type EventKey = GenKey;

/// The generational slab of live event records for one world.
pub type EventSlab = GenSlab<EventRecord>;

/// The mutable dispatch flags of an [`EventRecord`], per the DOM standard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventFlags {
    /// The "initialized" flag: set by a constructor or `initEvent`.
    pub initialized: bool,
    /// The "dispatch" flag: set while the event is being dispatched.
    pub dispatch: bool,
    /// The "stop propagation" flag.
    pub stop_propagation: bool,
    /// The "stop immediate propagation" flag.
    pub stop_immediate: bool,
    /// The "canceled" flag (set by `preventDefault`).
    pub canceled: bool,
    /// The "in passive listener" flag (suppresses `preventDefault`).
    pub in_passive_listener: bool,
}

/// The interface-specific payload of an event.
#[derive(Clone, Debug)]
pub enum EventKind {
    /// A plain `Event`.
    Plain,
    /// A `CustomEvent`; `detail` is interned value bits (released on reclaim).
    Custom {
        /// The `detail` value's handle bits, if any.
        detail: Option<u64>,
    },
    /// A `UIEvent`.
    Ui {
        /// The `detail` field.
        detail: i32,
    },
    /// A `MouseEvent`.
    Mouse {
        /// `UIEvent.detail`.
        detail: i32,
        /// Screen coordinates.
        screen_x: i32,
        /// Screen coordinates.
        screen_y: i32,
        /// Client coordinates.
        client_x: i32,
        /// Client coordinates.
        client_y: i32,
        /// Modifier: control.
        ctrl: bool,
        /// Modifier: shift.
        shift: bool,
        /// Modifier: alt.
        alt: bool,
        /// Modifier: meta.
        meta: bool,
        /// The button that changed state.
        button: i16,
        /// The buttons held down.
        buttons: u16,
        /// The related target handle bits, if any.
        related_target: Option<u64>,
    },
    /// A `ProgressEvent`.
    ///
    /// The three fields are independent, as the XHR standard's getter steps say
    /// ("return the value they were initialized to"): `length_computable` is
    /// *not* derived from `total`, so a non-zero `total` with the init default
    /// answers `false`. The platform's rule for *firing* one — `lengthComputable`
    /// true and `total` set only when the length is not zero — lives with the
    /// firing code, not here.
    ///
    /// Unlike `Custom`'s `detail`, nothing here is a stored JS value, so no
    /// record-reclamation path has anything to release.
    Progress {
        /// `ProgressEvent.lengthComputable`.
        length_computable: bool,
        /// `ProgressEvent.loaded`.
        loaded: f64,
        /// `ProgressEvent.total`.
        total: f64,
    },
}

impl EventKind {
    /// The most-derived interface name for this event, used to pick the wrapper
    /// prototype.
    pub fn interface_name(&self) -> &'static str {
        match self {
            EventKind::Plain => "Event",
            EventKind::Custom { .. } => "CustomEvent",
            EventKind::Ui { .. } => "UIEvent",
            EventKind::Mouse { .. } => "MouseEvent",
            EventKind::Progress { .. } => "ProgressEvent",
        }
    }
}

/// One live event object's state. Fields the dispatcher mutates (target,
/// currentTarget, phase, path, flags) are updated only under a short borrow of
/// the event slab, never while script runs.
#[derive(Clone, Debug)]
pub struct EventRecord {
    /// The event type (`Event.type`).
    pub type_: String,
    /// Whether the event bubbles.
    pub bubbles: bool,
    /// Whether the event is cancelable.
    pub cancelable: bool,
    /// Whether the event is composed.
    pub composed: bool,
    /// Whether the event is trusted (dispatched by the engine, not script).
    pub is_trusted: bool,
    /// `Event.timeStamp` (ms since the world's origin).
    pub time_stamp: f64,
    /// The target's handle bits, once dispatch begins.
    pub target: Option<u64>,
    /// The current target's handle bits during dispatch.
    pub current_target: Option<u64>,
    /// The current `eventPhase`.
    pub phase: u16,
    /// The propagation path (handle bits), for `composedPath()`.
    pub path: Vec<u64>,
    /// The mutable dispatch flags.
    pub flags: EventFlags,
    /// Whether a JS wrapper has been created for this record.
    pub has_wrapper: bool,
    /// The interface-specific payload.
    pub kind: EventKind,
}

impl EventRecord {
    /// A freshly-created, uninitialized record of the given kind and type.
    pub fn new(type_: String, kind: EventKind, time_stamp: f64) -> Self {
        Self {
            type_,
            bubbles: false,
            cancelable: false,
            composed: false,
            is_trusted: false,
            time_stamp,
            target: None,
            current_target: None,
            phase: PHASE_NONE,
            path: Vec::new(),
            flags: EventFlags::default(),
            has_wrapper: false,
            kind,
        }
    }
}

// ---------------------------------------------------------------------------
// Listener table
// ---------------------------------------------------------------------------

/// A stable identity for one registered listener, so a listener removed while a
/// snapshot of it is mid-flight is skipped rather than invoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerId(pub u64);

/// The name of an event handler IDL attribute: `onclick`, `onload`, `onabort`.
///
/// HTML §8.1.8.1 gives every event handler a *name*, "a string that always
/// starts with `on` and is followed by the name of the event for which the
/// handler is intended", and the listener a handler registers for listens for
/// that event type. So the type is derived by stripping the prefix, which is
/// exactly what the standard's name/type pairs do (`onload` → `load`).
///
/// It is *not* what the legacy-prefixed handlers do: `onwebkitanimationend`'s
/// event type is `webkitAnimationEnd`, whose name does not start with the
/// stripped remainder at all, so those names cannot be built by prefix
/// stripping and are not supported here. Every handler this tree needs so far
/// (`onabort`, and b-5's XHR `onload`/`onerror`/`onprogress`/`ontimeout`/
/// `onreadystatechange`) is a plain one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EventHandlerName(&'static str);

impl EventHandlerName {
    /// The handler named by its IDL attribute name, e.g. `onload`.
    pub const fn new(attribute: &'static str) -> Self {
        Self(attribute)
    }

    /// The IDL attribute name, e.g. `onload`.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The event type the handler's listener listens for: `onload` → `load`.
    pub fn event_type(self) -> &'static str {
        self.0.strip_prefix("on").unwrap_or(self.0)
    }
}

/// One registered event listener.
#[derive(Clone, Debug)]
pub struct Listener {
    /// This listener's stable id.
    pub id: ListenerId,
    /// The event type it listens for.
    pub type_: String,
    /// The stored JS callback.
    pub callback: CallbackRef,
    /// Whether it fires in the capture phase.
    pub capture: bool,
    /// Whether it is removed after firing once.
    pub once: bool,
    /// Whether it is passive (its `preventDefault` is ignored).
    pub passive: bool,
    /// Whether it is the listener of an event handler rather than one the page
    /// registered with `addEventListener` (HTML §8.1.8.1).
    ///
    /// The flag exists for dispatch: a handler's callback is the page's own
    /// handler function, so the return value the dispatcher receives is the
    /// handler's, and exactly `false` sets the event's canceled flag. A plain
    /// listener's return value means nothing.
    ///
    /// That same sharing is why identity matching has to know the flag. HTML
    /// registers a handler's listener with an internal callback the page cannot
    /// name, so this entry is never the one a page's `addEventListener` made or
    /// one its `removeEventListener` may take: [`ListenerTable::add_handler_at`]
    /// skips the duplicate rule and [`ListenerTable::remove_at`] skips these
    /// entries, which is how the page's own function passed to an `onX` setter
    /// stays two independent listeners from the same function passed to
    /// `addEventListener`.
    pub handler: bool,
}

/// One listener as [`ListenerTable::snapshot_at`] reports it: the fields the
/// dispatcher needs, copied out of the table so no borrow of it is held while
/// script runs.
#[derive(Clone, Copy, Debug)]
pub struct ListenerSnapshot {
    /// The listener's stable id, for the removal-during-dispatch check.
    pub id: ListenerId,
    /// The stored JS callback.
    pub callback: CallbackRef,
    /// Whether the entry is removed after firing once.
    pub once: bool,
    /// Whether the entry is passive.
    pub passive: bool,
    /// Whether the entry is an event handler's listener (HTML §8.1.8.1), the
    /// only kind whose return value the dispatcher acts on.
    pub handler: bool,
}

#[derive(Default)]
struct TargetListeners {
    listeners: Vec<Listener>,
}

/// What a listener can be registered on: a node, or the window.
///
/// The window is not a node — it has no arena slot, no id and no place in the
/// tree — so it cannot be keyed by [`NodeRef`] the way an element is. It is
/// still a target, and it is the outermost entry of every event path, which is
/// what makes `window.addEventListener('click', …)` fire for a click on a
/// descendant.
///
/// The node key is a full [`NodeRef`], generation included. The arena reuses a
/// freed slot for a different node under a bumped generation, so keying on the
/// bare [`tenta_core::NodeId`] would let a dispatch whose path contains the
/// recycled index match a listener left behind by the node that used to live
/// there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TargetKey {
    Node(NodeRef),
    Window,
    /// A host object that is an `EventTarget` and is neither a node nor the
    /// window: an `AbortSignal` or an `XMLHttpRequest`.
    ///
    /// It is keyed by its [`HostKey`], generation included, because that is the
    /// only identity such an object has — it is not in the tree, so it has no
    /// [`NodeRef`]. Keying it by anything coarser would be wrong rather than
    /// merely imprecise: a `URL`, a `Headers`, an `AbortSignal` and an
    /// `XMLHttpRequest` all share the `TAG_HOST` handle tag, so a table keyed by
    /// the tag — or by the interface name its record reports — would let a
    /// listener registered on one signal fire for another. The generation
    /// matters for the same reason it does on a node: the slab reuses a freed
    /// slot, and an object that took a recycled slot must not inherit the
    /// listeners of the object that used to live there.
    Host(HostKey),
}

impl From<NodeRef> for TargetKey {
    fn from(node: NodeRef) -> TargetKey {
        TargetKey::Node(node)
    }
}

/// The target of a dispatch: the table key it is keyed under, plus the handle
/// bits script sees as `event.target`.
///
/// The two are carried together on purpose. The key is a [`TargetKey`], because
/// that is all `addEventListener` has; the bits are a full handle — a node
/// handle with its generation, or the window's payload-free handle — because
/// that is what script resolves back to an object, so a handle taken before a
/// node was freed does not silently become the node that reused its slot.
///
/// A dispatch walks a path of these, the target first; the target is also the
/// only entry whose key is used for script-visible identity, so this type is
/// introduced at the call site and reused as the head of the path.
#[derive(Clone, Copy)]
pub struct DispatchTarget {
    target: TargetKey,
    bits: u64,
}

impl DispatchTarget {
    /// Dispatches at `node`, whose handle bits carry the generation script sees.
    pub fn node(node: NodeRef) -> DispatchTarget {
        DispatchTarget {
            target: TargetKey::Node(node),
            bits: encode_node(node),
        }
    }

    /// Dispatches at the window, which has no arena slot and no id.
    pub fn window() -> DispatchTarget {
        DispatchTarget {
            target: TargetKey::Window,
            bits: encode_window(),
        }
    }

    /// Dispatches at the host object whose record key is `key` — an
    /// `AbortSignal` or an `XMLHttpRequest`.
    ///
    /// The bits are the record's own tagged handle, which is exactly what
    /// `controller.signal` handed script: the world memoises one wrapper per
    /// handle, so `event.target` resolves back to the *same* object the page
    /// holds, which is what the WPT `abort` event test compares.
    pub fn host(key: HostKey) -> DispatchTarget {
        DispatchTarget {
            target: TargetKey::Host(key),
            bits: encode_host(key),
        }
    }

    /// The host record this target dispatches at, or `None` for a target that
    /// is a node or the window.
    ///
    /// The XHR completion route needs it: a completion names its target as
    /// opaque bits, and the object the bits name is the object whose state
    /// machine has to run — so the key has to come back out of the target the
    /// bits were decoded into. It is the record, not the tag, that says which
    /// interface an `XMLHttpRequest` is (see [`TargetKey::Host`]).
    pub fn host_key(&self) -> Option<HostKey> {
        match self.target {
            TargetKey::Host(key) => Some(key),
            TargetKey::Node(_) | TargetKey::Window => None,
        }
    }

    /// The dispatch target a handle's bits name, or `None` for a handle that is
    /// not an event target (a `NodeList`, an `Event`, a `URL`, …).
    ///
    /// The inverse of the three constructors above, for a caller that holds bits
    /// and no handle: an XHR completion names its target as opaque bits
    /// ([`crate::xhr::XhrCompletion::target`]), because the driver that produced
    /// it cannot name a handle type. Re-encoding what this decodes gives the
    /// input back, which is what keeps `event.target` the *same* object the page
    /// holds — the world memoises one wrapper per handle.
    ///
    /// A node's generation survives only as the 16 bits `encode_node` packs, so
    /// a reference decoded from bits is the one that was encoded; nothing here
    /// widens the handle format (see the `pack` TODO in `bindings::host`).
    pub fn from_bits(bits: u64) -> Option<DispatchTarget> {
        let decoded = decode(bits);
        match decoded.tag {
            TAG_NODE => Some(DispatchTarget::node(NodeRef::new(
                decoded.node_id,
                u32::from(decoded.generation),
            ))),
            TAG_WINDOW => Some(DispatchTarget::window()),
            TAG_HOST => Some(DispatchTarget::host(decoded.host_key())),
            _ => None,
        }
    }
}

/// The per-target listener lists for one world.
#[derive(Default)]
pub struct ListenerTable {
    targets: HashMap<TargetKey, TargetListeners>,
    next_id: u64,
}

impl ListenerTable {
    /// A new empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a listener for the node `target`. See [`Self::add_at`].
    pub fn add(
        &mut self,
        target: NodeRef,
        type_: String,
        callback: CallbackRef,
        capture: bool,
        once: bool,
        passive: bool,
    ) -> Option<ListenerId> {
        self.add_at(target.into(), type_, callback, capture, once, passive)
    }

    /// Adds a listener for `target`. Returns the new listener's id, or `None`
    /// when an identical `(type, callback, capture)` entry already exists (the
    /// DOM "add an event listener" duplicate rule) — in that case the caller
    /// must release `callback`.
    ///
    /// Only a listener the page registered itself can be that duplicate. An
    /// event handler's listener is skipped in both directions: HTML registers it
    /// with an internal callback the page cannot name, so the same page function
    /// passed to an `onX` setter and to `addEventListener` are two independent
    /// listeners whichever order they were registered in. See
    /// [`Self::add_handler_at`] for the other half.
    pub fn add_at(
        &mut self,
        target: TargetKey,
        type_: String,
        callback: CallbackRef,
        capture: bool,
        once: bool,
        passive: bool,
    ) -> Option<ListenerId> {
        let duplicate = self.targets.get(&target).is_some_and(|entry| {
            entry.listeners.iter().any(|l| {
                !l.handler && l.type_ == type_ && l.callback == callback && l.capture == capture
            })
        });
        if duplicate {
            return None;
        }
        let id = self.next_id();
        let entry = self.targets.entry(target).or_default();
        entry.listeners.push(Listener {
            id,
            type_,
            callback,
            capture,
            once,
            passive,
            handler: false,
        });
        Some(id)
    }

    /// Registers the listener of an event handler (HTML §8.1.8.1) for `target`
    /// and returns its id.
    ///
    /// This is not `addEventListener`. The standard registers a handler's
    /// listener with an internal callback the page cannot name, so the DOM
    /// duplicate rule — which is what [`Self::add_at`] applies — does not apply
    /// here: the same page function registered as a handler and as a plain
    /// listener are two independent listeners, and setting a handler twice is
    /// caught by the handler map, not by the table. The two registrations spell
    /// their flags out separately rather than sharing a helper so that the
    /// difference between them, and every flag a new one would add, is visible
    /// at the point of registration.
    pub fn add_handler_at(
        &mut self,
        target: TargetKey,
        type_: String,
        callback: CallbackRef,
    ) -> ListenerId {
        let id = self.next_id();
        let entry = self.targets.entry(target).or_default();
        entry.listeners.push(Listener {
            id,
            type_,
            callback,
            capture: false,
            once: false,
            passive: false,
            handler: true,
        });
        id
    }

    /// Allocates the next listener id.
    fn next_id(&mut self) -> ListenerId {
        let id = ListenerId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Removes the listener matching `(type, callback, capture)` for node
    /// `target` and returns its callback so the caller can release it.
    pub fn remove(
        &mut self,
        target: NodeRef,
        type_: &str,
        callback: CallbackRef,
        capture: bool,
    ) -> Option<CallbackRef> {
        self.remove_at(target.into(), type_, callback, capture)
    }

    /// Removes the listener matching `(type, callback, capture)` for `target`
    /// and returns its callback so the caller can release it.
    ///
    /// A handler's listener is never a match: HTML registers it with an internal
    /// callback the page cannot name, so `removeEventListener` can only remove
    /// what `addEventListener` registered.
    pub fn remove_at(
        &mut self,
        target: TargetKey,
        type_: &str,
        callback: CallbackRef,
        capture: bool,
    ) -> Option<CallbackRef> {
        let entry = self.targets.get_mut(&target)?;
        let pos = entry.listeners.iter().position(|l| {
            !l.handler && l.type_ == type_ && l.callback == callback && l.capture == capture
        })?;
        let removed = entry.listeners.remove(pos);
        Some(removed.callback)
    }

    /// Removes the listener with `id` for node `target`. See
    /// [`Self::remove_by_id_at`]. Removing a freed node's entries is not this
    /// call's job: a live target's key no longer names them, because they were
    /// keyed under the generation the node lived under. The sweep drainer
    /// ([`crate::bindings::host::HostContext::drain_swept_listeners`]) reclaims
    /// them by the reference the document recorded.
    pub fn remove_by_id(&mut self, target: NodeRef, id: ListenerId) -> Option<CallbackRef> {
        self.remove_by_id_at(target.into(), id)
    }

    /// Removes the listener with `id` for `target`, returning its callback.
    pub fn remove_by_id_at(&mut self, target: TargetKey, id: ListenerId) -> Option<CallbackRef> {
        let entry = self.targets.get_mut(&target)?;
        let pos = entry.listeners.iter().position(|l| l.id == id)?;
        Some(entry.listeners.remove(pos).callback)
    }

    /// Whether a listener with `id` is still registered for node `target`.
    pub fn contains(&self, target: NodeRef, id: ListenerId) -> bool {
        self.contains_at(target.into(), id)
    }

    /// Whether a listener with `id` is still registered for `target`.
    pub fn contains_at(&self, target: TargetKey, id: ListenerId) -> bool {
        self.targets
            .get(&target)
            .is_some_and(|e| e.listeners.iter().any(|l| l.id == id))
    }

    /// Replaces the stored callback of the listener with `id` on `target`, in
    /// place, and returns the callback it displaced so the caller can release it.
    ///
    /// In-place replacement is the rule for reassigning an event handler: HTML
    /// §8.1.8.1's "activate an event handler" returns early when the handler
    /// already has a listener, so the new value is stored in that same listener
    /// and its position among the target's listeners does not move.
    ///
    /// `None` when no listener with `id` is registered for `target`, including a
    /// listener whose target's entries a sweep or a reclamation has drained; the
    /// caller keeps ownership of `callback` in that case.
    pub fn set_callback_at(
        &mut self,
        target: TargetKey,
        id: ListenerId,
        callback: CallbackRef,
    ) -> Option<CallbackRef> {
        let entry = self.targets.get_mut(&target)?;
        let listener = entry.listeners.iter_mut().find(|l| l.id == id)?;
        Some(std::mem::replace(&mut listener.callback, callback))
    }

    /// The stored callback of the listener with `id` on `target`, if it is still
    /// registered.
    ///
    /// The bits are handed back as they are — non-owning. The table holds the
    /// one owned reference for the entry, so two reads give the same bits and
    /// `el.onx === el.onx` holds by identity.
    pub fn callback_by_id_at(&self, target: TargetKey, id: ListenerId) -> Option<CallbackRef> {
        self.targets
            .get(&target)?
            .listeners
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.callback)
    }

    /// A snapshot of every listener on node `target`. See [`Self::snapshot_at`].
    pub fn snapshot(&self, target: NodeRef, type_: &str, capture: bool) -> Vec<ListenerSnapshot> {
        self.snapshot_at(target.into(), type_, capture)
    }

    /// A snapshot of every listener for `target` matching `type_` and `capture`,
    /// in registration order.
    pub fn snapshot_at(
        &self,
        target: TargetKey,
        type_: &str,
        capture: bool,
    ) -> Vec<ListenerSnapshot> {
        self.targets
            .get(&target)
            .map(|e| {
                e.listeners
                    .iter()
                    .filter(|l| l.type_ == type_ && l.capture == capture)
                    .map(|l| ListenerSnapshot {
                        id: l.id,
                        callback: l.callback,
                        once: l.once,
                        passive: l.passive,
                        handler: l.handler,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The total number of registered listeners across all targets
    /// (test/measurement hook).
    pub fn len(&self) -> usize {
        self.targets.values().map(|e| e.listeners.len()).sum()
    }

    /// Whether no listeners are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes and returns every callback registered for `target`, matched by
    /// the key taken exactly as given — generation included.
    ///
    /// Exact matching is what makes this safe for the sweep drainer
    /// ([`crate::bindings::host::HostContext::drain_swept_listeners`]), its
    /// caller: it drains a node the document freed and holds the [`NodeRef`]
    /// recorded before the slot was recycled. That key matches the entries
    /// registered while the node lived — which the slot's current generation no
    /// longer reports — and cannot match a listener registered on the node that
    /// later took the slot. Matching on the bare id would remove that live
    /// node's listener too, which is why no generation-blind drain exists.
    pub fn drain_target_at(&mut self, target: TargetKey) -> Vec<CallbackRef> {
        self.targets
            .remove(&target)
            .map(|e| e.listeners.into_iter().map(|l| l.callback).collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatches the event stored under `key` at `target`, per the DOM standard's
/// dispatch algorithm (capture, at-target, bubble). `trusted` sets the event's
/// `isTrusted` flag. Returns `Ok(!canceled)` — i.e. `false` when a listener
/// called `preventDefault` on a cancelable event.
///
/// Throws `InvalidStateError` when the event is uninitialized or already being
/// dispatched (a nested dispatch of the same event).
pub fn dispatch(
    ctx: &HostContext,
    target: NodeRef,
    key: EventKey,
    trusted: bool,
) -> Result<bool, JsError> {
    dispatch_at(ctx, DispatchTarget::node(target), key, trusted)
}

/// Dispatches `key` at `target`, walking the path from the window down to the
/// target and back up. The target is a [`DispatchTarget`] rather than a node so
/// the window can be one: `window.dispatchEvent(new Event("x"))` has no node to
/// name, and its path is the window alone.
pub fn dispatch_at(
    ctx: &HostContext,
    target: DispatchTarget,
    key: EventKey,
    trusted: bool,
) -> Result<bool, JsError> {
    // Step 1: validate + set up, under a short borrow of the event slab.
    let type_ = {
        let mut events = ctx.events.borrow_mut();
        let record = events
            .get_mut(key)
            .ok_or_else(|| invalid_state("the event has been reclaimed"))?;
        if !record.flags.initialized || record.flags.dispatch {
            return Err(invalid_state(
                "the event is uninitialized or already being dispatched",
            ));
        }
        record.flags.dispatch = true;
        record.is_trusted = trusted;
        record.target = Some(target.bits);
        record.flags.stop_propagation = false;
        record.flags.stop_immediate = false;
        record.type_.clone()
    };

    // Step 2: build the propagation path under a short borrow of the document.
    // The window is the outermost entry: it is not an ancestor in the tree, but
    // it is the document's parent in the event path, which is what makes a
    // listener on `window` fire for an event on a descendant. A dispatch *at*
    // the window has no ancestors to walk and no window above it, so its path is
    // the target alone.
    let path: Vec<PathEntry> = {
        let doc = ctx.doc.borrow();
        let mut path = Vec::new();
        path.push(target);
        if let TargetKey::Node(node) = target.target {
            for ancestor in doc.ancestors(node.id) {
                path.push(PathEntry::node(doc.node_ref(ancestor)));
            }
            path.push(PathEntry::window());
        }
        path
    };
    let bubbles = {
        let mut events = ctx.events.borrow_mut();
        if let Some(record) = events.get_mut(key) {
            record.path = path.iter().map(|entry| entry.bits).collect();
            record.bubbles
        } else {
            false
        }
    };

    let event_bits = encode_event(key);
    let script = ctx.script.borrow().clone();

    if let Some(script) = script {
        // Step 4: capture pass, from the root down to and including the target.
        for entry in path.iter().rev() {
            let phase = if entry.target == target.target {
                PHASE_AT_TARGET
            } else {
                PHASE_CAPTURING
            };
            let stop =
                invoke_listeners(ctx, &*script, *entry, &type_, phase, true, key, event_bits);
            if stop {
                break;
            }
        }

        // Step 5: bubble pass, from the target up to the root (ancestors only
        // when the event bubbles).
        if !propagation_stopped(ctx, key) {
            for (index, entry) in path.iter().enumerate() {
                if index > 0 && !bubbles {
                    break;
                }
                let phase = if entry.target == target.target {
                    PHASE_AT_TARGET
                } else {
                    PHASE_BUBBLING
                };
                let stop =
                    invoke_listeners(ctx, &*script, *entry, &type_, phase, false, key, event_bits);
                if stop {
                    break;
                }
            }
        }
    }

    // Step 6: clear the transient dispatch state and report the result.
    let mut events = ctx.events.borrow_mut();
    if let Some(record) = events.get_mut(key) {
        record.phase = PHASE_NONE;
        record.current_target = None;
        record.flags.dispatch = false;
        record.flags.stop_propagation = false;
        record.flags.stop_immediate = false;
        record.path.clear();
        Ok(!record.flags.canceled)
    } else {
        Ok(true)
    }
}

/// One entry of a propagation path. The target is the first entry; the rest are
/// its ancestors, then the window.
type PathEntry = DispatchTarget;

/// Runs the listeners registered on `target` for `(type_, capture)`, in order,
/// honouring `once`/`passive`/removal-during-dispatch and
/// `stopImmediatePropagation`. Returns whether the "stop propagation" flag is
/// set afterwards.
#[allow(clippy::too_many_arguments)]
fn invoke_listeners(
    ctx: &HostContext,
    script: &dyn ScriptHost,
    entry: PathEntry,
    type_: &str,
    phase: u16,
    capture: bool,
    key: EventKey,
    event_bits: u64,
) -> bool {
    let target = entry.target;
    let snapshot = ctx.listeners.borrow().snapshot_at(target, type_, capture);
    if snapshot.is_empty() {
        return propagation_stopped(ctx, key);
    }

    {
        let mut events = ctx.events.borrow_mut();
        if let Some(record) = events.get_mut(key) {
            record.current_target = Some(entry.bits);
            record.phase = phase;
        }
    }

    for listener in snapshot {
        // Skip a listener removed by an earlier listener in this dispatch.
        if !ctx.listeners.borrow().contains_at(target, listener.id) {
            continue;
        }
        if listener.once {
            ctx.listeners
                .borrow_mut()
                .remove_by_id_at(target, listener.id);
        }
        if listener.passive {
            set_passive(ctx, key, true);
        }

        // No borrow is held across the call into script.
        let returned_false = script.invoke_listener(listener.callback, entry.bits, event_bits);

        if listener.passive {
            set_passive(ctx, key, false);
        }
        if listener.once {
            script.release_value(listener.callback.0);
        }
        // The event handler processing algorithm (HTML §8.1.8.1): a handler that
        // returns exactly `false` sets the event's canceled flag. Only a handler
        // entry's result is acted on — a plain `addEventListener` callback's
        // return value is meaningless — and the flag is set here, where no JS
        // call is on the stack (invariant A5), rather than inside the callback.
        //
        // The flag is set unconditionally rather than gated on `cancelable`,
        // which is what `Event.canceled` applies. The standard's two exceptions
        // are a window `onerror` handler, where `true` cancels, and
        // `onbeforeunload`, where any non-null value cancels; neither is
        // implemented.
        if listener.handler && returned_false {
            set_canceled(ctx, key);
        }
        if immediate_stopped(ctx, key) {
            break;
        }
    }

    propagation_stopped(ctx, key)
}

fn set_passive(ctx: &HostContext, key: EventKey, value: bool) {
    if let Some(record) = ctx.events.borrow_mut().get_mut(key) {
        record.flags.in_passive_listener = value;
    }
}

fn set_canceled(ctx: &HostContext, key: EventKey) {
    if let Some(record) = ctx.events.borrow_mut().get_mut(key) {
        record.flags.canceled = true;
    }
}

fn propagation_stopped(ctx: &HostContext, key: EventKey) -> bool {
    ctx.events
        .borrow()
        .get(key)
        .is_some_and(|r| r.flags.stop_propagation)
}

fn immediate_stopped(ctx: &HostContext, key: EventKey) -> bool {
    ctx.events
        .borrow()
        .get(key)
        .is_some_and(|r| r.flags.stop_immediate)
}

fn invalid_state(message: &str) -> JsError {
    JsError::dom_exception("InvalidStateError", message)
}
