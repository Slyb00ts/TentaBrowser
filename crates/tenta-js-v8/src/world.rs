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

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::NodeKind;
use tenta_js::bindings::host::{
    HostObject, SharedContext, TAG_EVENT, TAG_HTML_COLLECTION, TAG_NODE_LIST, TAG_STYLE,
    TAG_TOKEN_LIST, TAG_WINDOW, decode, encode_window,
};
use tenta_js::bindings::{
    InterfaceDescriptor, InterfaceRegistry, WRAPPER_HOST_FIELD, WRAPPER_TAG, WrapperWorld,
    cached_template, unwrap_this,
};
use tenta_js::bindings::{JsError, Realm};
use tenta_js::dom::events::{EventKind, EventSlab};

/// One entry in the value slab: a stored JS value with a reference count and,
/// for objects, the identity hash it was interned under (so it can leave the
/// identity index when its last reference is released).
struct ValueSlot {
    global: v8::Global<v8::Value>,
    refs: u32,
    hash: Option<i32>,
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
    wrappers: HashMap<u64, v8::Weak<v8::Object>>,
    values: Vec<Option<ValueSlot>>,
    free_values: Vec<u64>,
    by_identity: HashMap<i32, Vec<u64>>,
    /// Nodes whose wrapper a finalizer has just collected, awaiting a
    /// `set_has_wrapper(false)` on the document. A finalizer must not borrow
    /// the document (it runs during GC, where a running op may hold a document
    /// borrow), so it only records the id here; the ids are drained at a safe
    /// point outside any finalizer (see `V8World::wrapper` and the engine's
    /// teardown).
    dead: Vec<NodeId>,
    /// Snapshot wrapper handles (tagged with TAG_STATIC_NODE_LIST or TAG_STATIC_HTML_COLLECTION)
    /// that a finalizer has collected, awaiting cleanup of their slab entries.
    dead_snapshots: Vec<u64>,
    /// Event wrapper handles (tagged with TAG_EVENT) that a finalizer has
    /// collected, awaiting removal of their event-slab records.
    dead_events: Vec<u64>,
}

impl WorldState {
    /// Reclaims every host and disarms every finalizer, and releases every
    /// stored value. Call this while the isolate is still alive: dropping the
    /// still-armed `Weak`s here — outside any finalizer — is the only safe
    /// place, and it makes the drain the sole reclaimer at teardown.
    pub fn clear(&mut self) {
        // Drop the host boxes first (the reclaim point), then the weak handles
        // (which disarms their finalizers), then the value slab.
        self.hosts.clear();
        self.wrappers.clear();
        self.values.clear();
        self.free_values.clear();
        self.by_identity.clear();
        self.dead.clear();
        self.dead_snapshots.clear();
        self.dead_events.clear();
    }

    /// Releases one reference to the stored value at `bits`, freeing the slot and
    /// its identity-index entry when the last reference goes. Called by the
    /// script host when a listener is removed or fires once.
    pub fn release_value(&mut self, bits: u64) {
        let Some(Some(slot)) = self.values.get_mut(bits as usize) else {
            return;
        };
        slot.refs = slot.refs.saturating_sub(1);
        if slot.refs > 0 {
            return;
        }
        let hash = slot.hash;
        self.values[bits as usize] = None;
        self.free_values.push(bits);
        if let Some(hash) = hash
            && let Some(bucket) = self.by_identity.get_mut(&hash)
        {
            bucket.retain(|&idx| idx != bits);
            if bucket.is_empty() {
                self.by_identity.remove(&hash);
            }
        }
    }

    /// Takes the event handle bits of wrappers collected since the last call, so
    /// the caller can remove their records from the event slab.
    pub fn take_dead_events(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_events)
    }

    /// Takes the ids of wrappers collected since the last call, so the caller
    /// can apply `set_has_wrapper(false)` where no document borrow is held.
    ///
    /// A node and its child `NodeList` are distinct wrapper handles that anchor
    /// to the same node id, so `HAS_WRAPPER` is a single bit shared by both. An
    /// id is surfaced only once no live host still anchors to it, so collecting
    /// one of a node's wrappers never clears the bit while the other is alive.
    pub fn take_dead(&mut self) -> Vec<NodeId> {
        let dead = std::mem::take(&mut self.dead);
        dead.into_iter()
            .filter(|id| {
                !self.hosts.keys().any(|bits| {
                    decode(*bits)
                        .node_anchor()
                        .map(|nid| nid == *id)
                        .unwrap_or(false)
                })
            })
            .collect()
    }

    /// Every node id that still carries a `HAS_WRAPPER` bit: the ids of
    /// wrappers a finalizer has already collected (`dead`) plus the anchor of
    /// every live host. Teardown clears the bit for all of these before
    /// dropping the hosts, so a document that outlives the engine does not keep
    /// nodes pinned. A wrapper still live at teardown is dropped without ever
    /// running its finalizer, so its bit would otherwise never be cleared. Ids
    /// may repeat (a node and its `NodeList`, or a dead id still anchored);
    /// `set_has_wrapper(false)` is idempotent, so the caller need not dedup.
    pub fn wrapped_node_ids(&self) -> Vec<NodeId> {
        self.dead
            .iter()
            .copied()
            .chain(
                self.hosts
                    .keys()
                    .filter_map(|bits| decode(*bits).node_anchor()),
            )
            .collect()
    }

    /// Takes the snapshot handle bits of wrappers collected since the last call,
    /// so the caller can reclaim snapshot slab entries. The slab is part of the
    /// context and outlives the engine; this drains dead snapshot tombstones and
    /// the stale `Weak` references.
    pub fn take_dead_snapshots(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dead_snapshots)
    }

    /// The number of live stored values (test-only): a leaked `CustomEvent`
    /// detail would keep its slot occupied here after the event is reclaimed.
    pub fn value_count(&self) -> usize {
        self.values.iter().filter(|slot| slot.is_some()).count()
    }
}

/// The V8 wrapper world. Holds shared handles to the world state and the context
/// (document + snapshots); the owning engine keeps its own clones so it can drain
/// the state at teardown after this world has been moved into the isolate slot.
pub struct V8World {
    state: Rc<RefCell<WorldState>>,
    ctx: SharedContext,
}

impl V8World {
    /// Builds a world over `state` and `ctx`.
    pub fn new(state: Rc<RefCell<WorldState>>, ctx: SharedContext) -> Self {
        Self { state, ctx }
    }
}

/// Reclaims the event-slab records of `dead_event_bits` — event wrappers a
/// finalizer collected — releasing any interned `CustomEvent` detail each owned
/// and disarming their tombstone `Weak`s. Shared by [`V8World::wrapper`]'s
/// sweep and the engine's teardown so both keep the same "record removed
/// implies detail released" invariant.
pub fn reclaim_dead_events(
    state: &mut WorldState,
    events: &RefCell<EventSlab>,
    dead_event_bits: Vec<u64>,
) {
    for bits in dead_event_bits {
        let event_key = decode(bits).event_key();
        let removed = events.borrow_mut().remove(event_key);
        if let Some(EventKind::Custom {
            detail: Some(detail_bits),
        }) = removed.map(|record| record.kind)
        {
            state.release_value(detail_bits);
        }
        state.wrappers.remove(&bits);
    }
}

/// Applies every piece of dead bookkeeping a finalizer has recorded: clears
/// `HAS_WRAPPER` for the node wrappers it collected, and reclaims the slab
/// entries of the collected snapshot and event wrappers.
///
/// This is *the* implementation of "dead bookkeeping is applied", shared by
/// [`V8World::wrapper`]'s lazy sweep, the engine's `trim_memory` and its
/// teardown, so the three cannot drift apart.
///
/// Call it where no document borrow is held and outside any finalizer: a
/// finalizer records ids only precisely because this is the safe point to
/// borrow the document ([`WorldState::take_dead`]).
pub fn reclaim_dead_bookkeeping(state: &RefCell<WorldState>, ctx: &SharedContext) {
    {
        let mut state_mut = state.borrow_mut();
        let dead = state_mut.take_dead();
        if !dead.is_empty() {
            let mut doc = ctx.doc.borrow_mut();
            for id in dead {
                doc.set_has_wrapper(id, false);
            }
        }

        // Remove dead snapshot wrappers' slab entries and disarm their
        // tombstone `Weak`s.
        let dead_snapshot_bits = state_mut.take_dead_snapshots();
        for bits in dead_snapshot_bits {
            let snapshot_key = decode(bits).snapshot_key();
            ctx.snapshots.borrow_mut().remove(snapshot_key);
            state_mut.wrappers.remove(&bits);
        }

        // Remove dead event wrappers' event-slab records, release any interned
        // `CustomEvent` detail they owned, and disarm tombstone `Weak`s.
        let dead_event_bits = state_mut.take_dead_events();
        reclaim_dead_events(&mut state_mut, &ctx.events, dead_event_bits);
    }
}

impl WrapperWorld for V8World {
    fn wrapper<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        interface: &'static str,
        handle: u64,
    ) -> Option<v8::Local<'s, v8::Object>> {
        // Reflect wrappers collected since the last call into the document now,
        // where no document borrow is held, and reclaim their slab records. The
        // finalizer only records ids (it must not borrow the document during
        // GC), so this is the safe point to apply the bookkeeping.
        //
        // INVARIANT: nothing on this method's path may call back into
        // `handle_to_v8` (which removes the wrapper-world slot around this
        // call). Wrapper creation only instantiates a template and attaches a
        // host — it never runs script or a getter — so re-entry cannot happen.
        reclaim_dead_bookkeeping(&self.state, &self.ctx);

        // Fast path: a live wrapper for this handle is revived and reused, so a
        // host is created at most once per live id. The borrow is released with
        // the block; only the revived `Local` escapes.
        if let Some(existing) = {
            let state = self.state.borrow();
            state
                .wrappers
                .get(&handle)
                .and_then(|weak| weak.to_local(scope))
        } {
            return Some(existing);
        }

        // Decode the tagged, generational handle.
        let decoded = decode(handle);

        // Pick the interface name and host. For snapshots, validate the slab entry
        // still exists. For nodes, validate the handle isn't stale.
        let _ = interface;
        let interface_name: &str;
        let host: Box<HostObject>;
        let is_snapshot: bool;
        // An event wrapper, like a snapshot wrapper, does not anchor to a node,
        // so it skips the node liveness/`set_has_wrapper` path; its slab record
        // is reclaimed on finalize via `dead_events`.
        let is_event: bool;

        if decoded.tag == TAG_WINDOW {
            // The window *is* the global object: `window`, `self`, `top` and
            // `globalThis` must be one object, so no instance is created for it
            // and none is memoised in `wrappers`. Its members are own properties
            // of the global, installed by `host::install`.
            //
            // The current context is the page's only context, so the object
            // `window` resolves to is the one script is running in.
            return scope.get_current_context().global(scope).into();
        } else if decoded.tag == 3 {
            // TAG_STATIC_NODE_LIST
            interface_name = "NodeList";
            let key = decoded.snapshot_key();
            let _ = self.ctx.snapshots.borrow().get(key)?; // Snapshot was reclaimed or never existed
            host = Box::new(HostObject::static_node_list(self.ctx.clone(), key));
            is_snapshot = true;
            is_event = false;
        } else if decoded.tag == 4 {
            // TAG_STATIC_HTML_COLLECTION
            interface_name = "HTMLCollection";
            let key = decoded.snapshot_key();
            let _ = self.ctx.snapshots.borrow().get(key)?;
            host = Box::new(HostObject::static_html_collection(self.ctx.clone(), key));
            is_snapshot = true;
            is_event = false;
        } else if decoded.tag == TAG_EVENT {
            let key = decoded.event_key();
            // Validate the record still exists and mark it wrapped. A missing
            // record (reclaimed) yields `None`, which the prelude turns into a
            // thrown TypeError — an event wrapper is created right after
            // `createEvent`, while the record is alive, so this holds.
            interface_name = {
                let mut events = self.ctx.events.borrow_mut();
                let record = events.get_mut(key)?;
                record.has_wrapper = true;
                record.kind.interface_name()
            };
            host = Box::new(HostObject::event(self.ctx.clone(), key));
            is_snapshot = false;
            is_event = true;
        } else {
            // Live node handle
            is_snapshot = false;
            is_event = false;
            let node_id = decoded.node_id;
            let node = {
                let doc = self.ctx.doc.borrow();
                if !doc.is_live(node_id)
                    || doc.node_ref(node_id).generation as u16 != decoded.generation
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
            } else if decoded.tag == TAG_STYLE {
                "CSSStyleDeclaration"
            } else {
                let doc = self.ctx.doc.borrow();
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
                            _ => "HTMLElement",
                        },
                        Some(ns) if ns == atoms::NS_SVG => "SVGElement",
                        _ => "Element",
                    },
                    NodeKind::Document => "Document",
                    NodeKind::Text => "Text",
                    NodeKind::Comment => "Comment",
                    NodeKind::Fragment => "DocumentFragment",
                    _ => "Node",
                }
            };

            host = Box::new(if decoded.tag == TAG_NODE_LIST {
                HostObject::node_list(self.ctx.clone(), node)
            } else if decoded.tag == TAG_HTML_COLLECTION {
                HostObject::html_collection(self.ctx.clone(), node)
            } else if decoded.tag == TAG_TOKEN_LIST {
                HostObject::token_list(self.ctx.clone(), node)
            } else if decoded.tag == TAG_STYLE {
                HostObject::style(self.ctx.clone(), node)
            } else {
                HostObject::node(self.ctx.clone(), node)
            });
        }

        // The template must already be installed for this interface.
        let template = cached_template(scope, interface_name)?;
        let instance = template.instance_template(scope).new_instance(scope)?;

        // Box the host so its address is stable across map growth: only the
        // pointer moves, never the pointee, so the internal field stays valid.
        let raw = host.as_ref() as *const HostObject as *const c_void;
        instance.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, raw, WRAPPER_TAG);

        // For live node hosts, mark the node as having a wrapper. Snapshot and
        // event hosts don't anchor to a node, so they don't touch the document.
        if !is_snapshot && !is_event {
            let node_id = decoded.node_id;
            self.ctx.doc.borrow_mut().set_has_wrapper(node_id, true);
        }

        let state = self.state.clone();
        let weak = v8::Weak::with_finalizer(
            scope,
            instance,
            Box::new(move |_isolate| {
                // Eager reclamation: drop the host box (the sole owner) and
                // record the id for a later `set_has_wrapper(false)` or snapshot cleanup.
                // Touch ONLY `state` here — never the document (a running op may hold
                // a document borrow while this GC callback fires) and never a
                // `Weak` (dropping this entry's `Weak` inside its own finalizer
                // would dangle rusty_v8's `second_pass` reference). The dead
                // `Weak` is swept elsewhere; if a teardown drain already removed
                // the host, this is a no-op.
                let mut state = state.borrow_mut();
                if state.hosts.remove(&handle).is_some() {
                    if is_event {
                        state.dead_events.push(handle);
                    } else if is_snapshot {
                        state.dead_snapshots.push(handle);
                    } else {
                        state.dead.push(decoded.node_id);
                    }
                }
            }),
        );

        let mut state = self.state.borrow_mut();
        state.hosts.insert(handle, host);
        // Overwrites any dead tombstone `Weak` for a reused id. Safe: this runs
        // in `wrapper`, not in a finalizer, so dropping the old `Weak` is fine.
        state.wrappers.insert(handle, weak);
        Some(instance)
    }

    fn handle(
        &self,
        scope: &mut v8::PinScope<'_, '_>,
        _interface: &'static str,
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
        // Like `wrapper`, this answers for every interface name rather than only
        // the ones the window can stand for: the tag decides what the bits mean,
        // and the consumer that decodes them rejects a window where one is not
        // allowed.
        let global = scope.get_current_context().global(scope);
        if wrapper.strict_equals(global.into()) {
            return Some(encode_window());
        }
        None
    }

    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64 {
        // Intern objects by identity so a value stored more than once (the same
        // listener callback, say) gets a single stable bit pattern and a single
        // stored `Global`. Primitives are not interned.
        let object_hash: Option<i32> = if value.is_object() {
            v8::Local::<v8::Object>::try_from(value)
                .ok()
                .map(|object| object.get_identity_hash().get())
        } else {
            None
        };

        if let Some(hash) = object_hash {
            // Look for an existing slot holding a strictly-equal object.
            let matched = {
                let state = self.state.borrow();
                state.by_identity.get(&hash).and_then(|bucket| {
                    bucket.iter().copied().find(|&idx| {
                        state
                            .values
                            .get(idx as usize)
                            .and_then(|slot| slot.as_ref())
                            .is_some_and(|slot| {
                                let candidate = v8::Local::new(scope, &slot.global);
                                value.strict_equals(candidate)
                            })
                    })
                })
            };
            if let Some(idx) = matched {
                if let Some(Some(slot)) = self.state.borrow_mut().values.get_mut(idx as usize) {
                    slot.refs += 1;
                }
                return idx;
            }
        }

        let global = v8::Global::new(scope.as_ref(), value);
        let mut state = self.state.borrow_mut();
        let bits = match state.free_values.pop() {
            Some(free) => {
                state.values[free as usize] = Some(ValueSlot {
                    global,
                    refs: 1,
                    hash: object_hash,
                });
                free
            }
            None => {
                let bits = state.values.len() as u64;
                state.values.push(Some(ValueSlot {
                    global,
                    refs: 1,
                    hash: object_hash,
                }));
                bits
            }
        };
        if let Some(hash) = object_hash {
            state.by_identity.entry(hash).or_default().push(bits);
        }
        bits
    }

    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let state = self.state.borrow();
        let slot = state.values.get(bits as usize)?.as_ref()?;
        Some(v8::Local::new(scope, &slot.global))
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
        // The constructor counterpart of `wrapper`: rather than instantiate a
        // fresh template object, take the `this` object V8 already allocated for
        // the `new` call (which carries the base template's internal field, even
        // for `class E extends Event`) and attach the freshly constructed host.
        //
        // INVARIANT: like `wrapper`, nothing here may call back into
        // `handle_to_v8`/`value_from_v8` — the wrapper-world slot is removed for
        // the duration of this call. Attaching a host and installing a finalizer
        // runs no script, so re-entry cannot happen.
        let _ = interface;
        let host: Box<HostObject> = host
            .downcast::<HostObject>()
            .map_err(|_| JsError::type_error("adopt: the host is not a HostObject"))?;
        let handle = host.handle_bits();
        let decoded = decode(handle);
        if decoded.tag != TAG_EVENT {
            return Err(JsError::type_error(
                "adopt: only Event objects are constructible",
            ));
        }
        let key = decoded.event_key();

        // Mark the record wrapped, mirroring the TAG_EVENT branch of `wrapper`,
        // so the dead-event sweep reclaims it exactly once.
        {
            let mut events = self.ctx.events.borrow_mut();
            let record = events
                .get_mut(key)
                .ok_or_else(|| JsError::type_error("adopt: the event record is gone"))?;
            record.has_wrapper = true;
        }

        // Box address is stable across map growth: only the pointer moves.
        let raw = host.as_ref() as *const HostObject as *const c_void;
        wrapper.set_aligned_pointer_in_internal_field(WRAPPER_HOST_FIELD, raw, WRAPPER_TAG);

        let state = self.state.clone();
        let weak = v8::Weak::with_finalizer(
            scope,
            wrapper,
            Box::new(move |_isolate| {
                // Same discipline as the event branch of `wrapper`: touch only
                // `state`, never the document or a `Weak`, and record the handle
                // for a later dead-event sweep.
                let mut state = state.borrow_mut();
                if state.hosts.remove(&handle).is_some() {
                    state.dead_events.push(handle);
                }
            }),
        );

        let mut state = self.state.borrow_mut();
        state.hosts.insert(handle, host);
        state.wrappers.insert(handle, weak);
        Ok(())
    }
}

/// A minimal [`InterfaceRegistry`] for the Phase 0c slice.
///
/// `install_*` records each interface's descriptor here; the slice does not yet
/// consume them (lazy per-context prototype installation lands with binding
/// phase 1), so this just retains them.
#[derive(Default)]
pub struct SkeletonRegistry {
    descriptors: Vec<InterfaceDescriptor>,
}

impl InterfaceRegistry for SkeletonRegistry {
    fn register(&mut self, descriptor: InterfaceDescriptor) {
        self.descriptors.push(descriptor);
    }
}
