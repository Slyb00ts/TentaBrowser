//! The V8 wrapper world: the per-context map between DOM host objects and their
//! JavaScript wrappers (`docs/spec/05-javascript.md` §3).
//!
//! This is the first real [`WrapperWorld`] behind the generated glue. Its
//! lifecycle mirrors the critic-cleared spike in `tests/wrapper_gc.rs`, whose
//! rules are load-bearing:
//!
//!  - The host `Box`es are owned by [`WorldState::hosts`], keyed by handle
//!    bits; the wrapper's internal field only borrows the pointer. A finalizer
//!    reclaims a host by removing it from that map — it never reads the
//!    collected object and never drops a `v8::Weak`.
//!  - Wrapper identity lives in [`WorldState::wrappers`]. A live wrapper is
//!    revived with `Weak::to_local`; a dead `Weak` is a tombstone swept only
//!    outside any finalizer (overwritten on reuse, or dropped at teardown).
//!  - Teardown drains both maps while the isolate is still alive, which
//!    reclaims every host exactly once and disarms every finalizer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use tenta_core::NodeRef;
use tenta_core::atom::atoms;
use tenta_dom::NodeKind;
use tenta_js::bindings::host::{
    DocumentSet, HostObject, HostRecord, SharedContext, TAG_EVENT, TAG_HOST, TAG_HTML_COLLECTION,
    TAG_NAMED_NODE_MAP, TAG_NODE, TAG_NODE_LIST, TAG_STATIC_HTML_COLLECTION, TAG_STATIC_NODE_LIST,
    TAG_STYLE, TAG_TOKEN_LIST, TAG_WINDOW, decode, encode_window,
};
use tenta_js::bindings::{
    InterfaceDescriptor, InterfaceRegistry, WRAPPER_HOST_FIELD, WRAPPER_TAG, WrapperWorld,
    cached_template, unwrap_this,
};
use tenta_js::bindings::{JsError, Realm};
use tenta_js::storage::StorageKind;

pub(crate) mod cycles;

/// Initial pressure thresholds for newly published native wrappers. They
/// bound work between collector checkpoints, not the total native arena size.
pub(crate) const SOFT_WRAPPER_MATERIALIZATIONS: usize = 1024;
pub(crate) const HARD_WRAPPER_MATERIALIZATIONS: usize = 4096;

/// One entry in the value slab: a stored JS value with a reference count and,
/// for objects, the identity hash it was interned under (so it can leave the
/// identity index when its last reference is released).
struct ValueSlot {
    global: Option<Rc<v8::Global<v8::Value>>>,
    refs: u32,
    hash: Option<i32>,
}

/// The weak identity and provisional ownership of one materialized wrapper.
/// Node-anchoring and constructed-CSSOM wrappers have a strong `root`; the
/// collector transfers those roots into one ephemeron graph at a safe point.
struct WrapperEntry {
    weak: Rc<v8::Weak<v8::Object>>,
    root: Option<v8::Global<v8::Object>>,
    generation: u64,
}

#[derive(Clone, Copy)]
enum DeadWork {
    Node(u16, NodeRef),
    Snapshot(u64),
    Event(u64),
    Host(u64),
}

/// V8 handles extracted from a world before its `RefCell` borrow is released.
/// Dropping one can trigger GC, so the engine drops this only after `clear`
/// returns and the world's maps/finalizers have been disarmed.
pub(crate) struct ClearedWorld {
    _storage: [Option<v8::Global<v8::Object>>; 2],
    _crypto: Option<v8::Global<v8::Object>>,
    _wrappers: Vec<v8::Global<v8::Object>>,
    _values: Vec<Option<ValueSlot>>,
    _cycle: Option<cycles::Collection>,
    _cycle_intrinsics: Option<cycles::Intrinsics>,
    _buffer_resizable_getter: Option<v8::Global<v8::Function>>,
    _buffer_immutable_getter: Option<v8::Global<v8::Function>>,
    _buffer_intrinsic_global: Option<v8::Global<v8::Object>>,
}

/// The split state of one wrapper world.
///
/// `hosts` is the sole owner of the host allocations and thus the single
/// reclaim point; `wrappers` holds weak handles for identity; `values` is a
/// reference-counted, object-interned slab that backs `any`/`object`/`symbol`/
/// `Promise`/callback bits. Interning gives every distinct object (a listener
/// callback, a `CustomEvent` detail) a single stable bit pattern, so callback
/// identity — which `removeEventListener` needs — is `u64` equality, and a
/// value stored many times is held once.
#[derive(Default)]
pub struct WorldState {
    hosts: HashMap<u64, Box<HostObject>>,
    wrappers: HashMap<u64, WrapperEntry>,
    next_wrapper_generation: u64,
    /// Every newly published native wrapper since the last successful
    /// checkpoint, including Event, Host and snapshot wrappers with no
    /// provisional graph root. Cached live-wrapper lookups do not count.
    new_wrapper_materializations: usize,
    /// Each Window owns its two Storage objects, including their ordinary JS
    /// properties. Replacing an area's wrapper releases its previous root;
    /// script references can independently keep the old wrapper alive.
    storage_wrappers: [Option<v8::Global<v8::Object>>; 2],
    crypto_wrapper: Option<v8::Global<v8::Object>>,
    values: Vec<Option<ValueSlot>>,
    free_values: Vec<u64>,
    by_identity: HashMap<i32, Vec<u64>>,
    /// Nodes whose wrapper a finalizer has just collected, awaiting release
    /// of their wrapper claim. A finalizer must not borrow the document (it
    /// runs during GC, where an op may hold a borrow), so it records the
    /// generation-stamped reference here. Claims are drained at a safe
    /// point outside any finalizer (see `V8World::wrapper` and the engine's
    /// teardown).
    ///
    /// The entry is a `(slot, NodeRef)` pair because documents of one world
    /// have independent arenas: node 5 of the page and node 5 of a document
    /// DOMImplementation minted are different nodes. Its generation also
    /// prevents a late finalizer from releasing a reused slot's claim.
    dead: Vec<(u16, NodeRef)>,
    /// Snapshot wrapper handles (tagged with TAG_STATIC_NODE_LIST or TAG_STATIC_HTML_COLLECTION)
    /// that a finalizer has collected, awaiting cleanup of their slab entries.
    dead_snapshots: Vec<u64>,
    /// Event wrapper handles (tagged with TAG_EVENT) that a finalizer has
    /// collected, awaiting removal of their event-slab records.
    dead_events: Vec<u64>,
    /// Host-object wrapper handles (tagged with TAG_HOST) that a finalizer has
    /// collected, awaiting removal of their host-record slab entries.
    dead_hosts: Vec<u64>,
    /// Current and remaining finalizer work stays owned here until each item
    /// completes. A panic or native poison cannot silently discard a batch.
    pending_dead: Vec<DeadWork>,
    cycle_intrinsics: Option<cycles::Intrinsics>,
    cycle: Option<cycles::Collection>,
    buffer_resizable_getter: Option<v8::Global<v8::Function>>,
    buffer_immutable_getter: Option<v8::Global<v8::Function>>,
    buffer_intrinsic_global: Option<v8::Global<v8::Object>>,
}

impl WorldState {
    /// Captures the native ArrayBuffer resizable accessor before page script.
    /// Both roots are owned by this world and cleared during teardown.
    pub(crate) fn capture_buffer_intrinsic(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), JsError> {
        let array_buffer_key = v8::String::new(scope, "ArrayBuffer")
            .ok_or_else(|| JsError::type_error("buffer intrinsic key allocation"))?;
        let prototype_key = v8::String::new(scope, "prototype")
            .ok_or_else(|| JsError::type_error("buffer intrinsic key allocation"))?;
        let resizable_key = v8::String::new(scope, "resizable")
            .ok_or_else(|| JsError::type_error("buffer intrinsic key allocation"))?;
        let get_key = v8::String::new(scope, "get")
            .ok_or_else(|| JsError::type_error("buffer intrinsic key allocation"))?;
        let global = scope.get_current_context().global(scope);
        let constructor = global
            .get(scope, array_buffer_key.into())
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        let prototype = constructor
            .get(scope, prototype_key.into())
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        let descriptor = prototype
            .get_own_property_descriptor(scope, resizable_key.into())
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        let getter = descriptor
            .get(scope, get_key.into())
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        self.buffer_resizable_getter = Some(v8::Global::new(scope, getter));
        let immutable_key = v8::String::new(scope, "immutable")
            .ok_or_else(|| JsError::type_error("buffer intrinsic key allocation"))?;
        let immutable_descriptor = prototype
            .get_own_property_descriptor(scope, immutable_key.into())
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        self.buffer_immutable_getter = if immutable_descriptor.is_undefined() {
            None
        } else {
            let descriptor = v8::Local::<v8::Object>::try_from(immutable_descriptor)
                .map_err(|_| JsError::type_error("invalid immutable descriptor"))?;
            let getter = descriptor
                .get(scope, get_key.into())
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
            Some(v8::Global::new(scope, getter))
        };
        self.buffer_intrinsic_global = Some(v8::Global::new(scope, global));
        Ok(())
    }

    fn next_wrapper_generation(&mut self) -> u64 {
        self.next_wrapper_generation = self.next_wrapper_generation.wrapping_add(1);
        self.next_wrapper_generation
    }

    /// Reclaims every host and disarms every finalizer, and releases every
    /// stored value. Call this while the isolate is still alive: dropping the
    /// still-armed `Weak`s here — outside any finalizer — is the only safe
    /// place, and it makes the drain the sole reclaimer at teardown.
    /// The caller must drop the returned handles after releasing its mutable
    /// borrow of the world state.
    pub(crate) fn clear(&mut self) -> ClearedWorld {
        // Drop the host boxes first (the reclaim point), then the weak handles
        // (which disarms their finalizers), then the value slab.
        let storage = std::mem::take(&mut self.storage_wrappers);
        let crypto = self.crypto_wrapper.take();
        let wrappers = self
            .wrappers
            .values_mut()
            .filter_map(|entry| entry.root.take())
            .collect();
        let values = std::mem::take(&mut self.values);
        let cycle = self.cycle.take();
        let cycle_intrinsics = self.cycle_intrinsics.take();
        let buffer_resizable_getter = self.buffer_resizable_getter.take();
        let buffer_immutable_getter = self.buffer_immutable_getter.take();
        let buffer_intrinsic_global = self.buffer_intrinsic_global.take();
        self.hosts.clear();
        self.wrappers.clear();
        self.free_values.clear();
        self.by_identity.clear();
        self.dead.clear();
        self.dead_snapshots.clear();
        self.dead_events.clear();
        self.dead_hosts.clear();
        self.pending_dead.clear();
        ClearedWorld {
            _storage: storage,
            _crypto: crypto,
            _wrappers: wrappers,
            _values: values,
            _cycle: cycle,
            _cycle_intrinsics: cycle_intrinsics,
            _buffer_resizable_getter: buffer_resizable_getter,
            _buffer_immutable_getter: buffer_immutable_getter,
            _buffer_intrinsic_global: buffer_intrinsic_global,
        }
    }

    /// Releases one reference to the stored value at `bits`, freeing the slot and
    /// its identity-index entry when the last reference goes. Called by the
    /// script host when a listener is removed or fires once.
    pub fn release_value(&mut self, bits: u64) {
        drop(self.take_released_value(bits));
    }

    /// Collector-only release path: return the last V8 root so its finalizing
    /// drop can occur after WorldState's mutable borrow has ended.
    fn take_released_value(&mut self, bits: u64) -> Option<Rc<v8::Global<v8::Value>>> {
        let Some(Some(slot)) = self.values.get_mut(bits as usize) else {
            return None;
        };
        slot.refs = slot.refs.saturating_sub(1);
        if slot.refs > 0 {
            return None;
        }
        let hash = slot.hash;
        let released = self.values[bits as usize]
            .take()
            .and_then(|slot| slot.global);
        self.free_values.push(bits);
        if let Some(hash) = hash
            && let Some(bucket) = self.by_identity.get_mut(&hash)
        {
            bucket.retain(|&idx| idx != bits);
            if bucket.is_empty() {
                self.by_identity.remove(&hash);
            }
        }
        released
    }

    /// Takes the event handle bits of wrappers collected since the last call, so
    /// the caller can remove their records from the event slab.
    pub fn take_dead_events(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_events)
    }

    /// Takes one generation-stamped claim for each finalized node-anchored
    /// wrapper. Multiplicity matters: a Node and its NodeList each own a claim,
    /// as do wrappers in other worlds sharing the same native Document.
    pub fn take_dead(&mut self) -> Vec<(u16, NodeRef)> {
        std::mem::take(&mut self.dead)
    }

    /// Every claim still owned by this world, including pending finalizers.
    /// Teardown releases each exactly once before disarming live finalizers.
    pub fn wrapped_nodes(&self) -> Vec<(u16, NodeRef)> {
        self.dead
            .iter()
            .copied()
            .chain(self.pending_dead.iter().filter_map(|work| match work {
                DeadWork::Node(slot, node) => Some((*slot, *node)),
                _ => None,
            }))
            .chain(self.hosts.keys().filter_map(|bits| {
                let decoded = decode(*bits);
                decoded.node_anchor().map(|id| {
                    (
                        decoded.slot,
                        NodeRef {
                            id,
                            generation: u32::from(decoded.generation),
                        },
                    )
                })
            }))
            .collect()
    }

    /// Takes the snapshot handle bits of wrappers collected since the last call,
    /// so the caller can reclaim snapshot slab entries. The slab is part of the
    /// context and outlives the engine; this drains dead snapshot tombstones and
    /// the stale `Weak` references.
    pub fn take_dead_snapshots(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_snapshots)
    }

    /// Takes the host-object wrapper handle bits of wrappers collected since the
    /// last call, so the caller can reclaim their host-record slab entries.
    pub fn take_dead_hosts(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_hosts)
    }

    /// Returns only an already materialized wrapper for these exact handle bits.
    /// Event-owned relatedTarget retention must never mint a replacement.
    pub(crate) fn existing_wrapper<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Object>> {
        self.wrappers.get(&bits)?.weak.to_local(scope)
    }

    /// The number of live stored values (test-only): a leaked `CustomEvent`
    /// detail would keep its slot occupied here after the event is reclaimed.
    pub fn value_count(&self) -> usize {
        self.values.iter().filter(|slot| slot.is_some()).count()
    }

    /// Stores `value` and returns its bits, incrementing the reference count when
    /// the value is an object already interned under the same identity.
    ///
    /// Interning objects by identity means the same listener callback or the same
    /// `AbortSignal.reason` object gets a single stable bit pattern and a single
    /// stored `Global`; primitives are not interned, so a caller that needs a
    /// stable bits for a primitive (`undefined`) caches the result itself.
    pub fn store_value(
        &mut self,
        scope: &mut v8::PinScope<'_, '_>,
        value: v8::Local<v8::Value>,
    ) -> u64 {
        let object_hash: Option<i32> = if value.is_object() {
            v8::Local::<v8::Object>::try_from(value)
                .ok()
                .map(|object| object.get_identity_hash().get())
        } else {
            None
        };

        if let Some(hash) = object_hash {
            // Look for an existing slot holding a strictly-equal object.
            let matched = self.by_identity.get(&hash).and_then(|bucket| {
                bucket.iter().copied().find(|&idx| {
                    self.values
                        .get(idx as usize)
                        .and_then(|slot| slot.as_ref())
                        .is_some_and(|slot| {
                            slot.global.as_deref().is_some_and(|global| {
                                let candidate = v8::Local::new(scope, global);
                                value.strict_equals(candidate)
                            })
                        })
                })
            });
            if let Some(idx) = matched {
                if let Some(Some(slot)) = self.values.get_mut(idx as usize) {
                    slot.refs += 1;
                }
                return idx;
            }
        }

        let global = Rc::new(v8::Global::new(scope.as_ref(), value));
        let bits = match self.free_values.pop() {
            Some(free) => {
                self.values[free as usize] = Some(ValueSlot {
                    global: Some(global),
                    refs: 1,
                    hash: object_hash,
                });
                free
            }
            None => {
                let bits = self.values.len() as u64;
                self.values.push(Some(ValueSlot {
                    global: Some(global),
                    refs: 1,
                    hash: object_hash,
                }));
                bits
            }
        };
        if let Some(hash) = object_hash {
            self.by_identity.entry(hash).or_default().push(bits);
        }
        bits
    }

    /// The value stored at `bits`, or `None` when the slot is free or stale.
    pub fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let slot = self.values.get(bits as usize)?.as_ref()?;
        Some(v8::Local::new(scope, slot.global.as_deref()?))
    }
}

/// What a wrapper's host is anchored to, which decides both whether the wrapper
/// marks a node as wrapped and what its finalizer has to record for a later
/// sweep. It is one value rather than a pair of flags because the three slab
/// kinds are mutually exclusive and a fourth is easy to add wrong.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapperKind {
    /// A live node: a `Node`, `NodeList`, `HTMLCollection`, `DOMTokenList` or
    /// `CSSStyleProperties`, anchored to a node id.
    Node,
    /// A static snapshot: its snapshot-slab entry is freed on finalize.
    Snapshot,
    /// An `Event`: its event-slab record is freed on finalize.
    Event,
    /// A JS-owned `URL` or `URLSearchParams`: its host-record is freed on
    /// finalize.
    Host,
}

/// The V8 wrapper world. Holds shared handles to the world state and the context
/// (document + snapshots); the owning engine keeps its own clones so it can drain
/// the state at teardown after this world has been moved into the isolate slot.
pub struct V8World {
    state: Rc<RefCell<WorldState>>,
    ctx: SharedContext,
}

/// A finalizer entry postponed while its still-live native record is wrapped
/// again. If wrapper creation fails, Drop restores the entry so the next safe
/// sweep can reclaim it; only a fully installed replacement disarms the guard.
struct PendingRewrap {
    state: Rc<RefCell<WorldState>>,
    bits: u64,
    snapshot: bool,
    committed: bool,
}

impl PendingRewrap {
    fn take(state: &Rc<RefCell<WorldState>>, bits: u64) -> Option<Self> {
        let snapshot = match decode(bits).tag {
            TAG_HOST => false,
            TAG_STATIC_NODE_LIST | TAG_STATIC_HTML_COLLECTION => true,
            _ => return None,
        };
        let mut world = state.borrow_mut();
        let pending = if snapshot {
            &mut world.dead_snapshots
        } else {
            &mut world.dead_hosts
        };
        let index = pending.iter().position(|queued| *queued == bits)?;
        pending.swap_remove(index);
        Some(Self {
            state: Rc::clone(state),
            bits,
            snapshot,
            committed: false,
        })
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingRewrap {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut world = self.state.borrow_mut();
        if self.snapshot {
            world.dead_snapshots.push(self.bits);
        } else {
            world.dead_hosts.push(self.bits);
        }
    }
}

impl V8World {
    /// Builds a world over `state` and `ctx`.
    pub fn new(state: Rc<RefCell<WorldState>>, ctx: SharedContext) -> Self {
        Self { state, ctx }
    }
}

/// Applies every piece of dead bookkeeping a finalizer has recorded: releases
/// wrapper claims for the node wrappers it collected, and reclaims the slab
/// entries of the collected snapshot, event and host-object wrappers.
///
/// This is *the* implementation of "dead bookkeeping is applied", shared by
/// [`V8World::wrapper`]'s lazy sweep, the engine's `trim_memory` and its
/// teardown, so the three cannot drift apart.
///
/// Call it where no document borrow is held and outside any finalizer: a
/// finalizer records ids only precisely because this is the safe point to
/// borrow the document ([`WorldState::take_dead`]).
///
/// It takes the world's [`DocumentSet`] rather than one context because a dead
/// wrapper's slot says which document it belonged to, and the slab a snapshot,
/// event or host record has to leave is the one that document's context owns.
pub fn reclaim_dead_bookkeeping(
    isolate: &v8::Isolate,
    state: &RefCell<WorldState>,
    documents: &DocumentSet,
) -> Result<bool, ()> {
    let mut released_native_pins = false;
    loop {
        if tenta_js::bindings::native_callback_failure(isolate).is_some() {
            return Err(());
        }
        let work = {
            let mut world = state.borrow_mut();
            if world.pending_dead.is_empty() {
                let nodes = world.take_dead();
                let snapshots = world.take_dead_snapshots();
                let events = world.take_dead_events();
                let hosts = world.take_dead_hosts();
                world
                    .pending_dead
                    .extend(nodes.into_iter().map(|(slot, id)| DeadWork::Node(slot, id)));
                world
                    .pending_dead
                    .extend(snapshots.into_iter().map(DeadWork::Snapshot));
                world
                    .pending_dead
                    .extend(events.into_iter().map(DeadWork::Event));
                world
                    .pending_dead
                    .extend(hosts.into_iter().map(DeadWork::Host));
            }
            world.pending_dead.last().copied()
        };
        let Some(work) = work else {
            return Ok(released_native_pins);
        };
        // The current item remains at the back of pending_dead until every
        // native release succeeds. A panic or sticky failure can neither lose
        // the rest of the batch nor run later finalizers after the failure.
        match work {
            DeadWork::Node(slot, node) => {
                if let Some(ctx) = documents.get(slot) {
                    ctx.doc.borrow_mut().release_wrapper(node);
                }
            }
            DeadWork::Snapshot(bits) => {
                let decoded = decode(bits);
                if let Some(ctx) = documents.get(decoded.slot) {
                    let key = decoded.snapshot_key();
                    released_native_pins |= ctx.snapshots.borrow().get(key).is_some();
                    ctx.snapshots.borrow_mut().remove(key);
                }
                if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                    return Err(());
                }
                let wrapper = state.borrow_mut().wrappers.remove(&bits);
                drop(wrapper);
            }
            DeadWork::Event(bits) => {
                let decoded = decode(bits);
                if let Some(ctx) = documents.get(decoded.slot) {
                    let record = ctx.events.borrow_mut().remove(decoded.event_key());
                    let detail = record.and_then(|record| record.kind.owned_stored_value());
                    if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                        return Err(());
                    }
                    if let Some(detail) = detail {
                        let root = state.borrow_mut().take_released_value(detail);
                        drop(root);
                    }
                }
                if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                    return Err(());
                }
                let wrapper = state.borrow_mut().wrappers.remove(&bits);
                drop(wrapper);
            }
            DeadWork::Host(bits) => {
                let decoded = decode(bits);
                if let Some(ctx) = documents.get(decoded.slot) {
                    let key = decoded.host_key();
                    let owns_pins = matches!(
                        ctx.host_records.borrow().get(key),
                        Some(
                            HostRecord::Attr(_)
                                | HostRecord::Range(_)
                                | HostRecord::StaticRange(_)
                                | HostRecord::MutationObserver(_)
                                | HostRecord::MutationRecord(_)
                        )
                    );
                    let reclaimed = ctx.reclaim_host(key);
                    if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                        return Err(());
                    }
                    released_native_pins |= owns_pins && reclaimed.keys.contains(&key);
                    let callbacks = ctx.drain_host_listeners(&reclaimed.keys);
                    if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                        return Err(());
                    }
                    for callback in callbacks {
                        let root = state.borrow_mut().take_released_value(callback.0);
                        drop(root);
                        if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                            return Err(());
                        }
                    }
                    for value in reclaimed.values {
                        let root = state.borrow_mut().take_released_value(value);
                        drop(root);
                        if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                            return Err(());
                        }
                    }
                }
                let wrapper = state.borrow_mut().wrappers.remove(&bits);
                drop(wrapper);
            }
        }
        if tenta_js::bindings::native_callback_failure(isolate).is_some() {
            return Err(());
        }
        state.borrow_mut().pending_dead.pop();
    }
}

/// Rolls back only a newly constructed Event/Host record whose V8 `this`
/// object was not published. `adopt` is called solely by generated constructors;
/// a cached/reminted wrapper uses `wrapper` and never enters this path. Exact
/// generational keys make a record already reclaimed by a finalizer a no-op.
/// Node/Document constructors have arena/context ownership, not a slab record,
/// and deliberately require a separate ownership transaction.
fn rollback_failed_constructor(
    isolate: &v8::Isolate,
    state: &RefCell<WorldState>,
    host: &HostObject,
) -> Result<(), ()> {
    // A prior native failure forbids further slab mutation or finalizing V8
    // root releases. The context and WorldState retain these until teardown.
    if tenta_js::bindings::native_callback_failure(isolate).is_some() {
        return Err(());
    }
    match host {
        HostObject::Event { ctx, key } => {
            let record = ctx.events.borrow_mut().remove(*key);
            if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                return Err(());
            }
            if let Some(detail) = record.and_then(|record| record.kind.owned_stored_value()) {
                let root = state.borrow_mut().take_released_value(detail);
                drop(root);
            }
        }
        HostObject::Host { ctx, key } => {
            let reclaimed = ctx.reclaim_host(*key);
            if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                return Err(());
            }
            let callbacks = ctx.drain_host_listeners(&reclaimed.keys);
            if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                return Err(());
            }
            for callback in callbacks {
                let root = state.borrow_mut().take_released_value(callback.0);
                drop(root);
                if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                    return Err(());
                }
            }
            for value in reclaimed.values {
                let root = state.borrow_mut().take_released_value(value);
                drop(root);
                if tenta_js::bindings::native_callback_failure(isolate).is_some() {
                    return Err(());
                }
            }
        }
        _ => {}
    }
    if tenta_js::bindings::native_callback_failure(isolate).is_some() {
        Err(())
    } else {
        Ok(())
    }
}

impl WrapperWorld for V8World {
    fn wrapper<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        interface: &'static str,
        handle: u64,
    ) -> Result<Option<v8::Local<'s, v8::Object>>, JsError> {
        let refused = std::cell::Cell::new(false);
        let native_failed = std::cell::Cell::new(false);
        let result = (|| -> Option<v8::Local<'s, v8::Object>> {
            // A host operation can return a cached record key after its old JS
            // wrapper was finalized but before queued native reclamation ran.
            // Defer that one death until this wrapper is fully installed; an early
            // return or panic restores it for the next safe sweep.
            let mut pending_rewrap = PendingRewrap::take(&self.state, handle);
            // A `MutationRecord` wrapper is exempt from the per-checkpoint
            // wrapper bound: the standard delivers every record of an observer
            // in one call, and the native queue that holds the records is
            // already charged to the page budget, so the wrapper adds no
            // resource the bound was protecting.
            let is_record_wrapper = interface == "MutationRecord";

            // Reflect other wrappers collected since the last call into the
            // document, where no document borrow is held, and reclaim their slab
            // records. The finalizer only records ids during GC.
            //
            // INVARIANT: nothing on this method's path may call back into
            // `handle_to_v8` (which removes the wrapper-world slot around this
            // call). Wrapper creation only instantiates a template and attaches a
            // host — it never runs script or a getter — so re-entry cannot happen.
            if reclaim_dead_bookkeeping(scope, &self.state, &self.ctx.documents).is_err() {
                native_failed.set(true);
                return None;
            }

            // Fast path: a live wrapper for this handle is revived and reused, so a
            // host is created at most once per live id. The borrow is released with
            // the block; only the revived `Local` escapes.
            if let Some(existing) = {
                let state = self.state.borrow();
                state
                    .wrappers
                    .get(&handle)
                    .map(|entry| Rc::clone(&entry.weak))
            }
            .and_then(|weak| weak.to_local(scope))
            {
                if let Some(guard) = pending_rewrap.as_mut() {
                    guard.commit();
                }
                return Some(existing);
            }

            // A reentrant host call at the hard bound must not publish another
            // anchored node wrapper. The callback-entry guard normally refuses before
            // host mutation; this second check bounds a single outgoing result
            // that materializes several wrappers.
            if decode(handle).tag != TAG_WINDOW
                && !is_record_wrapper
                && self.state.borrow().new_wrapper_materializations >= HARD_WRAPPER_MATERIALIZATIONS
            {
                refused.set(true);
                return None;
            }

            // Decode the tagged, generational handle.
            let decoded = decode(handle);

            // The context the handle's document slot names. Slot 0 is the page's,
            // which the world was built over; every other slot is a document a
            // `DOMImplementation` minted, and its context owns the slabs a
            // snapshot, event or host record of that document lives in. A slot that
            // names no context is a handle to a document that is gone, which is the
            // same `None` a reclaimed slab entry produces.
            let ctx = self.ctx.documents.get(decoded.slot)?;

            // Pick the interface name and host. For snapshots, validate the slab entry
            // still exists. For nodes, validate the handle isn't stale.
            let interface_name: &str;
            let host: Box<HostObject>;
            // What the finalizer must record when this wrapper is collected: a
            // node id, or the handle of a slab entry that has to be reclaimed.
            let kind: WrapperKind;
            let mut storage_kind = None;
            let mut crypto = false;

            if decoded.tag == TAG_WINDOW {
                if !matches!(interface, "EventTarget" | "Window" | "WindowProxy") {
                    return None;
                }
                // The window *is* the global object: `window`, `self`, `top` and
                // `globalThis` must be one object, so no instance is created for it
                // and none is memoised in `wrappers`. Its members are own properties
                // of the global, installed by `host::install`.
                //
                // The current context is the page's only context, so the object
                // `window` resolves to is the one script is running in.
                return scope.get_current_context().global(scope).into();
            } else if decoded.tag == TAG_STATIC_NODE_LIST {
                interface_name = "NodeList";
                let key = decoded.snapshot_key();
                let _ = ctx.snapshots.borrow().get(key)?; // Snapshot was reclaimed or never existed
                host = Box::new(HostObject::static_node_list(ctx.clone(), key));
                kind = WrapperKind::Snapshot;
            } else if decoded.tag == TAG_STATIC_HTML_COLLECTION {
                interface_name = "HTMLCollection";
                let key = decoded.snapshot_key();
                let _ = ctx.snapshots.borrow().get(key)?;
                host = Box::new(HostObject::static_html_collection(ctx.clone(), key));
                kind = WrapperKind::Snapshot;
            } else if decoded.tag == TAG_EVENT {
                let key = decoded.event_key();
                // Validate the record still exists. A missing
                // record (reclaimed) yields `None`, which the prelude turns into a
                // thrown TypeError — an event wrapper is created right after
                // `createEvent`, while the record is alive, so this holds.
                interface_name = {
                    let events = ctx.events.borrow();
                    let record = events.get(key)?;
                    record.kind.interface_name()
                };
                host = Box::new(HostObject::event(ctx.clone(), key));
                kind = WrapperKind::Event;
            } else if decoded.tag == TAG_HOST {
                // A JS-owned host object. The tag does not name the interface — every
                // one of them shares it — so the record does, and a missing record
                // (reclaimed with its previous wrapper) is the same `None` the event
                // arm returns.
                let key = decoded.host_key();
                let records = ctx.host_records.borrow();
                let record = records.get(key)?;
                if let HostRecord::Storage(storage) = record {
                    storage_kind = Some(storage.kind());
                }
                crypto = matches!(record, HostRecord::Crypto);
                interface_name = record.interface_name();
                host = Box::new(HostObject::host_record(ctx.clone(), key));
                kind = WrapperKind::Host;
            } else {
                // Live node handle, anchored to the node's id.
                kind = WrapperKind::Node;
                let node_id = decoded.node_id;
                let node = {
                    let doc = ctx.doc.borrow();
                    if !doc.is_live(node_id)
                        || doc.node_ref(node_id).generation != u32::from(decoded.generation)
                    {
                        return None;
                    }
                    doc.node_ref(node_id)
                };

                interface_name = if decoded.tag == TAG_NODE_LIST {
                    "NodeList"
                } else if decoded.tag == TAG_HTML_COLLECTION {
                    "HTMLCollection"
                } else if decoded.tag == TAG_TOKEN_LIST {
                    "DOMTokenList"
                } else if decoded.tag == TAG_NAMED_NODE_MAP {
                    "NamedNodeMap"
                } else if decoded.tag == TAG_STYLE {
                    "CSSStyleProperties"
                } else {
                    let doc = ctx.doc.borrow();
                    match doc.kind(node_id) {
                        // HTML-namespace elements get the `HTMLElement` prototype
                        // (which carries `click()`), with `<iframe>` refined to
                        // `HTMLIFrameElement`; SVG-namespace elements get
                        // `SVGElement`; everything else stays `Element`.
                        NodeKind::Element => match doc.namespace(node_id) {
                            Some(ns) if ns == atoms::NS_HTML => match doc.tag(node_id) {
                                Some(tag) if tag == atoms::IFRAME => "HTMLIFrameElement",
                                Some(tag) if tag == atoms::TEMPLATE => "HTMLTemplateElement",
                                Some(tag) if tag == atoms::DIV => "HTMLDivElement",
                                Some(tag) if tag == atoms::SPAN => "HTMLSpanElement",
                                Some(tag) if tag == atoms::P => "HTMLParagraphElement",
                                Some(tag) if tag == atoms::A => "HTMLAnchorElement",
                                Some(tag) if tag == atoms::IMG => "HTMLImageElement",
                                Some(tag) if tag == atoms::LABEL => "HTMLLabelElement",
                                Some(tag) if tag == atoms::FORM => "HTMLFormElement",
                                Some(tag) if tag == atoms::INPUT => "HTMLInputElement",
                                Some(tag) if tag == atoms::BUTTON => "HTMLButtonElement",
                                Some(tag) if tag == atoms::SELECT => "HTMLSelectElement",
                                Some(tag) if tag == atoms::TEXTAREA => "HTMLTextAreaElement",
                                Some(tag) if tag == atoms::OPTION => "HTMLOptionElement",
                                Some(tag) if tag == atoms::SCRIPT => "HTMLScriptElement",
                                Some(tag) if tag == atoms::LINK => "HTMLLinkElement",
                                Some(tag) if tag == atoms::STYLE => "HTMLStyleElement",
                                _ => "HTMLElement",
                            },
                            Some(ns) if ns == atoms::NS_SVG => "SVGElement",
                            _ => "Element",
                        },
                        NodeKind::Document => "Document",
                        NodeKind::Text => "Text",
                        // A CDATA section is a `Text` subclass with an interface of
                        // its own, so it gets its own prototype rather than the
                        // `Text` one; `Document.createCDATASection` is the only
                        // factory, and it only exists on a non-HTML document.
                        NodeKind::CData => "CDATASection",
                        NodeKind::Comment => "Comment",
                        NodeKind::Fragment => "DocumentFragment",
                        // The two kinds `dom/nodes` reads most: a doctype and a
                        // processing instruction each have an interface of their
                        // own, and neither is a `Node`-only shell.
                        NodeKind::DocType => "DocumentType",
                        NodeKind::ProcessingInstruction => "ProcessingInstruction",
                        _ => "Node",
                    }
                };

                host = Box::new(if decoded.tag == TAG_NODE_LIST {
                    HostObject::node_list(ctx.clone(), node)
                } else if decoded.tag == TAG_HTML_COLLECTION {
                    HostObject::html_collection(ctx.clone(), node)
                } else if decoded.tag == TAG_TOKEN_LIST {
                    HostObject::token_list(ctx.clone(), node)
                } else if decoded.tag == TAG_NAMED_NODE_MAP {
                    HostObject::named_node_map(ctx.clone(), node)
                } else if decoded.tag == TAG_STYLE {
                    HostObject::style(ctx.clone(), node)
                } else {
                    HostObject::node(ctx.clone(), node)
                });
            }

            // CSS records retain related wrappers; standalone EventTargets
            // retain listeners. Classify their native ownership before
            // publishing the provisional wrapper root.
            let host_graph_root = tenta_js::bindings::host::cssom::graph::is_css_host(&host)
                || host.is_standalone_event_target();
            // The template must already be installed for this interface.
            let template = cached_template(scope, interface_name)?;
            let instance = template.instance_template(scope).new_instance(scope)?;
            if interface_name == "CSSStyleProperties" {
                tenta_js::bindings::style_properties::ensure_v8::<HostObject>(scope, instance)
                    .ok()?;
            }

            // Box the host so its address is stable across map growth: only the
            // pointer moves, never the pointee, so the internal field stays valid.
            let raw = host.as_ref() as *const HostObject as *const c_void;
            instance.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, raw, WRAPPER_TAG);

            // Allocate the persistent handle before borrowing WorldState. V8 may
            // collect while creating a handle; a finalizer must never re-borrow a
            // WorldState that this path already holds mutably.
            let crypto_wrapper = crypto.then(|| v8::Global::new(scope, instance));
            let storage_wrapper = storage_kind.map(|kind| {
                let index = match kind {
                    StorageKind::Local => 0,
                    StorageKind::Session => 1,
                };
                (index, v8::Global::new(scope, instance))
            });

            let generation = self.state.borrow_mut().next_wrapper_generation();
            let state = self.state.clone();
            let weak = v8::Weak::with_finalizer(
                scope,
                instance,
                Box::new(move |isolate| {
                    tenta_js::bindings::v8_isolate_callback_boundary(
                        isolate,
                        "wrapper_finalizer",
                        (),
                        || {
                            // Eager reclamation: drop the host box (the sole owner)
                            // and record what a later sweep must reclaim. Touch
                            // ONLY `state` here: no document or `Weak` access is
                            // safe from a GC finalizer.
                            let mut state = state.borrow_mut();
                            if state
                                .wrappers
                                .get(&handle)
                                .is_some_and(|entry| entry.generation != generation)
                            {
                                return;
                            }
                            if state.hosts.remove(&handle).is_some() {
                                match kind {
                                    WrapperKind::Node => {
                                        state.dead.push((
                                            decoded.slot,
                                            NodeRef {
                                                id: decoded.node_id,
                                                generation: u32::from(decoded.generation),
                                            },
                                        ));
                                    }
                                    WrapperKind::Snapshot => state.dead_snapshots.push(handle),
                                    WrapperKind::Event => state.dead_events.push(handle),
                                    WrapperKind::Host => state.dead_hosts.push(handle),
                                }
                            }
                        },
                    );
                }),
            );

            let graph_root = decoded.node_anchor().is_some() || host_graph_root;
            let root = graph_root.then(|| v8::Global::new(scope, instance));
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                native_failed.set(true);
                return None;
            }
            let mut state = self.state.borrow_mut();
            // All fallible V8 allocation and template lookup is complete.
            // Acquire native wrapper claims with the host/weak entries, never
            // for an instance that failed before it acquired a finalizer.
            match kind {
                WrapperKind::Event => {
                    ctx.events
                        .borrow_mut()
                        .get_mut(decoded.event_key())?
                        .has_wrapper = true;
                }
                WrapperKind::Host => {
                    let mut records = ctx.host_records.borrow_mut();
                    let record = records.get_mut(decoded.host_key())?;
                    match record {
                        HostRecord::Url(record) => record.has_wrapper = true,
                        HostRecord::Signal(record) => record.has_wrapper = true,
                        HostRecord::Headers(record) => record.has_wrapper = true,
                        HostRecord::File(record) => record.has_wrapper = true,
                        _ => {}
                    }
                }
                WrapperKind::Node => {
                    let node = NodeRef {
                        id: decoded.node_id,
                        generation: u32::from(decoded.generation),
                    };
                    if ctx.doc.borrow_mut().retain_wrapper(node).is_err() {
                        tenta_js::bindings::poison_native_callback(scope, "node wrapper claim");
                        native_failed.set(true);
                        return None;
                    }
                }
                WrapperKind::Snapshot => {}
            }
            state.hosts.insert(handle, host);
            // Overwrites any dead tombstone `Weak` for a reused id. Safe: this runs
            // in `wrapper`, not in a finalizer, so dropping the old `Weak` is fine.
            state.wrappers.insert(
                handle,
                WrapperEntry {
                    weak: Rc::new(weak),
                    root,
                    generation,
                },
            );
            if !is_record_wrapper {
                state.new_wrapper_materializations =
                    state.new_wrapper_materializations.saturating_add(1);
            }
            let previous_storage_wrapper = storage_wrapper
                .and_then(|(index, wrapper)| state.storage_wrappers[index].replace(wrapper));
            let previous_crypto_wrapper =
                crypto_wrapper.and_then(|wrapper| state.crypto_wrapper.replace(wrapper));
            if let Some(guard) = pending_rewrap.as_mut() {
                guard.commit();
            }
            drop(state);
            drop(previous_storage_wrapper);
            drop(previous_crypto_wrapper);
            Some(instance)
        })();
        if native_failed.get() || tenta_js::bindings::native_callback_failure(scope).is_some() {
            Err(tenta_js::bindings::v8_pending_exception_error())
        } else if refused.get() {
            Err(JsError::range_error(
                "native wrapper allocation limit exceeded",
            ))
        } else {
            Ok(result)
        }
    }

    fn handle(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        interface: &'static str,
        wrapper: v8::Local<v8::Object>,
    ) -> Option<u64> {
        // SAFETY: only wrappers created by `wrapper` carry a host pointer in
        // internal field 0; `unwrap_this` rejects any object without one.
        if let Ok(host) = unsafe { unwrap_this::<HostObject>(&wrapper) } {
            return Some(host.handle_bits());
        }
        // The reverse of the `TAG_WINDOW` arm in `wrapper`: an interface
        // instance's host carries the bits, but the window has no host at all —
        // it is the global object — so it is recognised by identity against the
        // realm's global. Without this, a value holding the window is rejected
        // as "not an interface instance" wherever an interface-typed argument is
        // converted (`new MouseEvent('mouseout', { relatedTarget: window })`).
        //
        // Only the page Window/WindowProxy and its EventTarget inheritance may
        // use the global as an interface-typed value.
        let global = scope.get_current_context().global(scope);
        if matches!(interface, "EventTarget" | "Window" | "WindowProxy")
            && wrapper.strict_equals(global.into())
        {
            return Some(encode_window());
        }
        None
    }

    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64 {
        self.state.borrow_mut().store_value(scope, value)
    }

    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let global = self
            .state
            .borrow()
            .values
            .get(bits as usize)?
            .as_ref()?
            .global
            .as_ref()
            .map(Rc::clone)?;
        Some(v8::Local::new(scope, global.as_ref()))
    }

    fn array_buffer_resizable<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        let state = self.state.borrow();
        let getter = state
            .buffer_resizable_getter
            .as_ref()
            .ok_or_else(|| JsError::type_error("buffer reference intrinsic was not captured"))?;
        let owner = state
            .buffer_intrinsic_global
            .as_ref()
            .ok_or_else(|| JsError::type_error("buffer reference intrinsic has no world"))?;
        let getter = v8::Local::new(scope, getter);
        let owner = v8::Local::new(scope, owner);
        drop(state);
        let current_global = scope.get_current_context().global(scope);
        if !owner.strict_equals(current_global.into()) {
            return Err(JsError::type_error(
                "buffer reference used in a different context",
            ));
        }
        let value = getter
            .call(scope, buffer.into(), &[])
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        Ok(value.boolean_value(scope))
    }

    fn array_buffer_immutable<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        let state = self.state.borrow();
        let owner = state
            .buffer_intrinsic_global
            .as_ref()
            .ok_or_else(|| JsError::type_error("buffer reference intrinsic has no world"))?;
        let owner = v8::Local::new(scope, owner);
        let getter = state
            .buffer_immutable_getter
            .as_ref()
            .map(|getter| v8::Local::new(scope, getter));
        drop(state);
        let current_global = scope.get_current_context().global(scope);
        if !owner.strict_equals(current_global.into()) {
            return Err(JsError::type_error(
                "buffer reference used in a different context",
            ));
        }
        let Some(getter) = getter else {
            return Ok(false);
        };
        let value = getter
            .call(scope, buffer.into(), &[])
            .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
        Ok(value.boolean_value(scope))
    }

    fn realm(&self) -> Realm {
        // The wrapper world's context, type-erased. `realm_context` in the host
        // downcasts it straight back to the same `Rc<HostContext>`.
        self.ctx.clone()
    }

    fn adopt(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        interface: &'static str,
        wrapper: v8::Local<v8::Object>,
        host: Box<dyn std::any::Any>,
    ) -> Result<(), JsError> {
        // Only generated constructors call adopt_host. Its HostObject owns a
        // newly inserted Event or Host record; wrapper() handles every remint
        // and must never roll back a shared record on publication failure.
        let _ = interface;
        let mut host = Some(
            host.downcast::<HostObject>()
                .map_err(|_| JsError::type_error("adopt: the host is not a HostObject"))?,
        );
        let handle = host.as_ref().expect("constructed host").handle_bits();
        let decoded = decode(handle);
        // Preserve the constructed host's own context for rollback even if its
        // document slot was invalidated between construction and publication.
        let owner = host.as_ref().expect("constructed host").context().clone();
        let host_graph_root = tenta_js::bindings::host::cssom::graph::is_css_host(
            host.as_ref().expect("constructed host"),
        ) || host
            .as_ref()
            .expect("constructed host")
            .is_standalone_event_target();
        let graph_root = decoded.node_anchor().is_some() || host_graph_root;

        let result = (|| -> Result<(), JsError> {
            if self.state.borrow().new_wrapper_materializations >= HARD_WRAPPER_MATERIALIZATIONS {
                return Err(JsError::range_error(
                    "native wrapper allocation limit exceeded",
                ));
            }
            let ctx = self
                .ctx
                .documents
                .get(decoded.slot)
                .filter(|ctx| Rc::ptr_eq(ctx, &owner))
                .ok_or_else(|| JsError::type_error("adopt: the document is gone"))?;
            let kind = if decoded.tag == TAG_EVENT {
                if ctx.events.borrow().get(decoded.event_key()).is_none() {
                    return Err(JsError::type_error("adopt: the event record is gone"));
                }
                WrapperKind::Event
            } else if decoded.tag == TAG_HOST {
                if ctx.host_records.borrow().get(decoded.host_key()).is_none() {
                    return Err(JsError::type_error("adopt: the host record is gone"));
                }
                WrapperKind::Host
            } else if decoded.tag == TAG_NODE {
                let live = {
                    let doc = ctx.doc.borrow();
                    doc.is_live(decoded.node_id)
                        && doc.node_ref(decoded.node_id).generation == u32::from(decoded.generation)
                };
                if !live {
                    return Err(JsError::type_error("adopt: the node is stale"));
                }
                WrapperKind::Node
            } else {
                return Err(JsError::type_error(
                    "adopt: the interface is not constructible",
                ));
            };

            // Allocate all V8 handles while the fresh host is still owned by
            // this local transaction. A finalizer can run during these calls,
            // so neither the record's has_wrapper flag nor the raw internal
            // pointer is published yet. `this` remains rooted by the callback.
            let generation = self.state.borrow_mut().next_wrapper_generation();
            let state = self.state.clone();
            let weak = v8::Weak::with_finalizer(
                scope,
                wrapper,
                Box::new(move |isolate| {
                    tenta_js::bindings::v8_isolate_callback_boundary(
                        isolate,
                        "adopted_wrapper_finalizer",
                        (),
                        || {
                            let mut state = state.borrow_mut();
                            if state
                                .wrappers
                                .get(&handle)
                                .is_some_and(|entry| entry.generation != generation)
                            {
                                return;
                            }
                            if state.hosts.remove(&handle).is_some() {
                                match kind {
                                    WrapperKind::Event => state.dead_events.push(handle),
                                    WrapperKind::Host => state.dead_hosts.push(handle),
                                    WrapperKind::Node => {
                                        state.dead.push((
                                            decoded.slot,
                                            NodeRef {
                                                id: decoded.node_id,
                                                generation: u32::from(decoded.generation),
                                            },
                                        ));
                                    }
                                    WrapperKind::Snapshot => {}
                                }
                            }
                        },
                    );
                }),
            );
            let root = graph_root.then(|| v8::Global::new(scope, wrapper));
            if tenta_js::bindings::native_callback_failure(scope).is_some() {
                return Err(tenta_js::bindings::v8_pending_exception_error());
            }

            // Acquire the native claim before publishing the internal pointer.
            // Allocation failure is fatal without changing previous claims.
            // The finalizer, root and generational entry are ready to commit.
            match kind {
                WrapperKind::Event => {
                    if let Some(record) = ctx.events.borrow_mut().get_mut(decoded.event_key()) {
                        record.has_wrapper = true;
                    }
                }
                WrapperKind::Host => {
                    if let Some(record) = ctx.host_records.borrow_mut().get_mut(decoded.host_key())
                    {
                        match record {
                            HostRecord::Url(record) => record.has_wrapper = true,
                            HostRecord::Signal(record) => record.has_wrapper = true,
                            HostRecord::Headers(record) => record.has_wrapper = true,
                            HostRecord::File(record) => record.has_wrapper = true,
                            _ => {}
                        }
                    }
                }
                WrapperKind::Node => {
                    let node = NodeRef {
                        id: decoded.node_id,
                        generation: u32::from(decoded.generation),
                    };
                    if ctx.doc.borrow_mut().retain_wrapper(node).is_err() {
                        tenta_js::bindings::poison_native_callback(scope, "node wrapper claim");
                        return Err(tenta_js::bindings::v8_pending_exception_error());
                    }
                }
                WrapperKind::Snapshot => unreachable!("constructors do not adopt snapshots"),
            }
            let raw = host.as_ref().expect("unpublished host").as_ref() as *const HostObject
                as *const c_void;
            wrapper.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, raw, WRAPPER_TAG);
            let mut state = self.state.borrow_mut();
            state
                .hosts
                .insert(handle, host.take().expect("one constructed host"));
            state.wrappers.insert(
                handle,
                WrapperEntry {
                    weak: Rc::new(weak),
                    root,
                    generation,
                },
            );
            state.new_wrapper_materializations =
                state.new_wrapper_materializations.saturating_add(1);
            Ok(())
        })();
        if result.is_err()
            && let Some(host) = host.as_ref()
            && rollback_failed_constructor(scope, &self.state, host).is_err()
        {
            return Err(tenta_js::bindings::v8_pending_exception_error());
        }
        result
    }
}

/// The installed Web IDL interface descriptors for a V8 page world.
///
/// `install_*` records each interface's descriptor here. Once the real page
/// context exists, the descriptors link derived constructor functions to their
/// parent interface constructors in that context.
#[derive(Default)]
pub struct SkeletonRegistry {
    descriptors: Vec<InterfaceDescriptor>,
}

impl SkeletonRegistry {
    /// FunctionTemplate inheritance establishes instance chains, but Web IDL
    /// also links the constructor functions in each realm. Templates were
    /// installed under a bootstrap context; resolve them again only after the
    /// real page context exists, before any page script can change globals.
    pub fn link_constructor_prototypes(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), JsError> {
        for descriptor in &self.descriptors {
            let Some(parent_name) = descriptor.parent else {
                continue;
            };
            let child_template = cached_template(scope, descriptor.name).ok_or_else(|| {
                JsError::type_error(format!("{} template is missing", descriptor.name))
            })?;
            let parent_template = cached_template(scope, parent_name)
                .ok_or_else(|| JsError::type_error(format!("{parent_name} template is missing")))?;
            let child = child_template
                .get_function(scope)
                .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
            let parent = parent_template
                .get_function(scope)
                .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
            let child: v8::Local<v8::Value> = child.into();
            let child = child
                .to_object(scope)
                .ok_or_else(tenta_js::bindings::v8_pending_exception_error)?;
            let parent: v8::Local<v8::Value> = parent.into();
            match child.set_prototype(scope, parent) {
                Some(true) => {}
                Some(false) => {
                    return Err(JsError::type_error(format!(
                        "{} constructor prototype could not be linked",
                        descriptor.name
                    )));
                }
                None => return Err(tenta_js::bindings::v8_pending_exception_error()),
            }
        }
        Ok(())
    }
}

impl InterfaceRegistry for SkeletonRegistry {
    fn register(&mut self, descriptor: InterfaceDescriptor) {
        self.descriptors.push(descriptor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("world/materialization_tests.inc.rs");
    include!("world/listener_value_lease_tests.inc.rs");
    include!("world/standalone_event_target_tests.inc.rs");
    use tenta_js::bindings::host::{SnapshotKey, encode_host, encode_snapshot};
    use tenta_js::dom::events::GenKey;

    #[test]
    fn failed_rewrap_restores_pending_native_death() {
        let state = Rc::new(RefCell::new(WorldState::default()));
        let host = encode_host(
            0,
            GenKey {
                index: 7,
                generation: 2,
            },
        );
        let snapshot = encode_snapshot(
            0,
            TAG_STATIC_NODE_LIST,
            SnapshotKey {
                index: 4,
                generation: 3,
            },
        );
        state.borrow_mut().dead_hosts.push(host);
        state.borrow_mut().dead_snapshots.push(snapshot);

        {
            let _host = PendingRewrap::take(&state, host).expect("host death was queued");
            let _snapshot =
                PendingRewrap::take(&state, snapshot).expect("snapshot death was queued");
            assert!(state.borrow().dead_hosts.is_empty());
            assert!(state.borrow().dead_snapshots.is_empty());
        }
        assert_eq!(state.borrow().dead_hosts, vec![host]);
        assert_eq!(state.borrow().dead_snapshots, vec![snapshot]);

        PendingRewrap::take(&state, host)
            .expect("restored host death")
            .commit();
        PendingRewrap::take(&state, snapshot)
            .expect("restored snapshot death")
            .commit();
        assert!(state.borrow().dead_hosts.is_empty());
        assert!(state.borrow().dead_snapshots.is_empty());
    }
}
