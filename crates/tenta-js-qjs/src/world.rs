//! Per-runtime native wrappers and reference-counted script values.

use rquickjs::{Class, Ctx, Function, Object, Persistent, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;
use tenta_core::atom::atoms;
use tenta_dom::NodeKind;
use tenta_js::bindings::host::*;
use tenta_js::bindings::*;

mod cycles;
mod legacy;

struct ValueSlot {
    value: Option<Persistent<Value<'static>>>,
    refs: usize,
}
struct Wrapper {
    weak: Persistent<Object<'static>>,
    generation: u64,
    // Native ownership of materialized node and constructed-CSSOM wrappers
    // is classified at safe points. This is a JS root, not a native pin.
    root: Option<Persistent<Object<'static>>>,
}
struct Prototype {
    value: Persistent<Object<'static>>,
    ancestors: &'static [&'static str],
}

// Bound newly admitted wrapper materializations since the latest successful
// checkpoint. Live connected trees may legitimately contain far more wrappers.
const WRAPPER_CHECKPOINT_INTERVAL: usize = 1024;
const WRAPPER_DEFERRED_LIMIT: usize = 4096;

type StorageHolder = (u64, Persistent<Value<'static>>);

struct CallbackRoots {
    id: u64,
    values: Vec<Persistent<Value<'static>>>,
    stored_value_lease: Option<Vec<u64>>,
    stored_value_transferred: bool,
    buffer_lease_id: Option<u64>,
}

pub(crate) struct World {
    pub host: SharedContext,
    wrappers: RefCell<HashMap<u64, Wrapper>>,
    next_wrapper: Cell<u64>,
    new_wrapper_materializations: Cell<usize>,
    pub collecting: Rc<Cell<bool>>,
    interrupt: Arc<AtomicBool>,
    deadline: Rc<Cell<Option<Instant>>>,
    pub gc_epoch: Cell<u64>,
    prototypes: RefCell<HashMap<&'static str, Prototype>>,
    legacy_methods: RefCell<HashMap<&'static str, QuickJsLegacyMethods>>,
    // HTML gives the active Document one local/session Storage holder. These
    // objects remain observable through Window even without a script reference.
    storage_holders: RefCell<[Option<StorageHolder>; 2]>,
    crypto_holder: RefCell<Option<StorageHolder>>,
    callback_roots: RefCell<Vec<CallbackRoots>>,
    next_callback_frame: Cell<u64>,
    intrinsics: RefCell<HashMap<&'static str, Persistent<Value<'static>>>>,
    values: RefCell<HashMap<u64, ValueSlot>>,
    next_value: Cell<u64>,
    pub dead: Rc<Cell<Vec<(u64, u64)>>>,
    // A partially applied death survives unwind/poison for teardown. New GC
    // deaths remain in `dead` without borrowing this processing batch.
    pending_dead: RefCell<Vec<(u64, u64)>>,
    pub native_failure: Rc<Cell<Option<&'static str>>>,
    pub active: Cell<*mut rquickjs::qjs::JSContext>,
    weak_constructor: RefCell<Option<Persistent<rquickjs::function::Constructor<'static>>>>,
    weak_deref: RefCell<Option<Persistent<Function<'static>>>>,
    cycle_intrinsics: RefCell<Option<cycles::Intrinsics>>,
    cycle: RefCell<Option<cycles::Collection>>,
}
impl World {
    pub fn new(
        host: SharedContext,
        interrupt: Arc<AtomicBool>,
        deadline: Rc<Cell<Option<Instant>>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            host,
            wrappers: RefCell::default(),
            next_wrapper: Cell::new(0),
            new_wrapper_materializations: Cell::new(0),
            collecting: Rc::new(Cell::new(false)),
            interrupt,
            deadline,
            gc_epoch: Cell::new(0),
            prototypes: RefCell::default(),
            legacy_methods: RefCell::default(),
            storage_holders: RefCell::default(),
            crypto_holder: RefCell::default(),
            callback_roots: RefCell::default(),
            next_callback_frame: Cell::new(0),
            intrinsics: RefCell::default(),
            values: RefCell::default(),
            next_value: Cell::new(0),
            dead: Rc::default(),
            pending_dead: RefCell::default(),
            native_failure: Rc::new(Cell::new(None)),
            active: Cell::new(std::ptr::null_mut()),
            weak_constructor: RefCell::new(None),
            weak_deref: RefCell::new(None),
            cycle_intrinsics: RefCell::new(None),
            cycle: RefCell::new(None),
        })
    }
    pub fn initialize(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let global = ctx.globals();
        let mut intrinsics = self.intrinsics.borrow_mut();
        for name in [
            "DataView",
            "Uint8ClampedArray",
            "Int8Array",
            "Uint8Array",
            "Int16Array",
            "Uint16Array",
            "Int32Array",
            "Uint32Array",
            "Float32Array",
            "Float64Array",
            "TypeError",
        ] {
            intrinsics.insert(name, Persistent::save(ctx, global.get::<_, Value>(name)?));
        }
        let object: Object = global.get("Object")?;
        let descriptor: Function = object.get("getOwnPropertyDescriptor")?;
        let view: Object = global.get("DataView")?;
        let prototype: Object = view.get("prototype")?;
        for (property, name) in [
            ("buffer", "DataView.buffer"),
            ("byteOffset", "DataView.byteOffset"),
            ("byteLength", "DataView.byteLength"),
        ] {
            let property: Object = descriptor.call((prototype.clone(), property))?;
            intrinsics.insert(
                name,
                Persistent::save(ctx, property.get::<_, Value>("get")?),
            );
        }
        // Capture the native typed-array buffer accessor before page code can
        // replace constructors or prototype properties. The World owns the
        // Persistent and clears it before its Context is dropped.
        let uint8: Object = global.get("Uint8Array")?;
        let uint8_prototype: Object = uint8.get("prototype")?;
        let typed_prototype = uint8_prototype
            .get_prototype()
            .ok_or(rquickjs::Error::Unknown)?;
        let typed_descriptor: Object = descriptor.call((typed_prototype, "buffer"))?;
        intrinsics.insert(
            "TypedArray.buffer",
            Persistent::save(ctx, typed_descriptor.get::<_, Value>("get")?),
        );
        let array: Object = global.get("Array")?;
        let prototype: Object = array.get("prototype")?;
        intrinsics.insert(
            "Array.values",
            Persistent::save(ctx, prototype.get::<_, Value>("values")?),
        );
        let reflect: Object = global.get("Reflect")?;
        for (property, key) in [
            ("get", "Reflect.get"),
            ("set", "Reflect.set"),
            ("has", "Reflect.has"),
            (
                "getOwnPropertyDescriptor",
                "Reflect.getOwnPropertyDescriptor",
            ),
            ("getPrototypeOf", "Reflect.getPrototypeOf"),
            ("defineProperty", "Reflect.defineProperty"),
            ("deleteProperty", "Reflect.deleteProperty"),
        ] {
            intrinsics.insert(
                key,
                Persistent::save(ctx, reflect.get::<_, Value>(property)?),
            );
        }
        let weak: rquickjs::function::Constructor = ctx.globals().get("WeakRef")?;
        let proto: Object = weak.get("prototype")?;
        *self.weak_deref.borrow_mut() =
            Some(Persistent::save(ctx, proto.get::<_, Function>("deref")?));
        *self.weak_constructor.borrow_mut() = Some(Persistent::save(ctx, weak));
        self.initialize_cycles(ctx)?;
        Ok(())
    }
    pub fn release(&self, bits: u64) {
        let mut values = self.values.borrow_mut();
        if let Some(slot) = values.get_mut(&bits) {
            slot.refs -= 1;
            if slot.refs == 0 {
                values.remove(&bits);
            }
        }
    }
    /// Remove exactly one leased reference, taking the QuickJS root
    /// out of the value-table borrow before dropping it. A missing bit means a
    /// lease/host ownership invariant was violated.
    fn release_stored_reference(&self, bits: u64) -> Result<(), ()> {
        if bits == AnyValue::NULL.0 {
            return Ok(());
        }
        let root = {
            let Ok(mut values) = self.values.try_borrow_mut() else {
                return Err(());
            };
            let Some(slot) = values.get_mut(&bits) else {
                return Err(());
            };
            if slot.refs == 0 {
                return Err(());
            }
            slot.refs -= 1;
            if slot.refs == 0 {
                values.remove(&bits).and_then(|mut slot| slot.value.take())
            } else {
                None
            }
        };
        drop(root);
        if self.native_failure.get().is_some() {
            Err(())
        } else {
            Ok(())
        }
    }

    /// Finish the stored-value lease while its callback frame remains
    /// published. Releasing a root can run a native finalizer, so each drop is
    /// outside both frame and value-table RefCell borrows.
    fn finish_stored_value_lease(&self, id: u64, initially_failed: bool) -> bool {
        loop {
            let next = {
                let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
                    self.record_native_failure("stored value frame borrow");
                    return false;
                };
                let Some(frame) = frames.last_mut().filter(|frame| frame.id == id) else {
                    self.record_native_failure("stored value frame order");
                    return false;
                };
                if initially_failed || frame.stored_value_transferred {
                    // On poison the actual value-table roots remain until
                    // teardown. A transferred lease belongs to the host.
                    frame.stored_value_lease = None;
                    frame.stored_value_transferred = false;
                    frame.buffer_lease_id = None;
                    return true;
                }
                match frame
                    .stored_value_lease
                    .as_ref()
                    .and_then(|values| values.last())
                    .copied()
                {
                    Some(bits) => bits,
                    None => {
                        frame.stored_value_lease = None;
                        frame.buffer_lease_id = None;
                        return true;
                    }
                }
            };
            if self.release_stored_reference(next).is_err() {
                self.record_native_failure("stored value cleanup");
                return false;
            }
            let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
                self.record_native_failure("stored value frame borrow");
                return false;
            };
            let Some(frame) = frames.last_mut().filter(|frame| frame.id == id) else {
                self.record_native_failure("stored value frame order");
                return false;
            };
            if frame.stored_value_lease.as_mut().and_then(Vec::pop) != Some(next) {
                self.record_native_failure("stored value cleanup order");
                return false;
            }
            if self.native_failure.get().is_some() {
                return false;
            }
        }
    }

    /// Roll back only a newly constructed Event/Host whose wrapper could not
    /// be published. Its original owner remains valid even when DocumentSet
    /// no longer maps the slot. A poisoned engine retains records and roots
    /// until teardown instead of mutating slabs after native failure.
    fn rollback_failed_constructor(&self, owner: &SharedContext, bits: u64) -> Result<(), ()> {
        if self.native_failure.get().is_some() {
            return Err(());
        }
        let decoded = decode(bits);
        match decoded.tag {
            TAG_EVENT => {
                let removed = owner.events.borrow_mut().remove(decoded.event_key());
                if self.native_failure.get().is_some() {
                    return Err(());
                }
                if let Some(value) = removed.and_then(|record| record.kind.owned_stored_value()) {
                    self.release_stored_reference(value)?;
                }
            }
            TAG_HOST => {
                let reclaimed = owner.reclaim_host(decoded.host_key());
                if self.native_failure.get().is_some() {
                    return Err(());
                }
                let callbacks = owner.drain_host_listeners(&reclaimed.keys);
                if self.native_failure.get().is_some() {
                    return Err(());
                }
                for callback in callbacks {
                    self.release_stored_reference(callback.0)?;
                }
                for value in reclaimed.values {
                    self.release_stored_reference(value)?;
                }
            }
            // Node/Document arena ownership is outside the slab transaction.
            _ => {}
        }
        if self.native_failure.get().is_some() {
            Err(())
        } else {
            Ok(())
        }
    }

    pub fn storage_wrapper<'js>(
        &self,
        ctx: &Ctx<'js>,
        local: bool,
        handle: Handle<Storage>,
    ) -> rquickjs::Result<Value<'js>> {
        let index = usize::from(!local);
        let cached = self.storage_holders.borrow()[index]
            .as_ref()
            .filter(|(bits, _)| *bits == handle.bits())
            .map(|(_, value)| value.clone());
        if let Some(value) = cached {
            return value.restore(ctx);
        }
        let value = handle_to_quickjs(ctx, handle)?;
        let previous = self.storage_holders.borrow_mut()[index]
            .replace((handle.bits(), Persistent::save(ctx, value.clone())));
        // Releasing the old holder can finalize a class. Hold no cache borrow
        // across that release, or across conversion/creation of the new one.
        drop(previous);
        Ok(value)
    }
    pub fn crypto_wrapper<'js>(
        &self,
        ctx: &Ctx<'js>,
        handle: Handle<tenta_js::bindings::Crypto>,
    ) -> rquickjs::Result<Value<'js>> {
        let cached = self
            .crypto_holder
            .borrow()
            .as_ref()
            .filter(|(bits, _)| *bits == handle.bits())
            .map(|(_, value)| value.clone());
        if let Some(value) = cached {
            return value.restore(ctx);
        }
        let value = handle_to_quickjs(ctx, handle)?;
        let previous = self
            .crypto_holder
            .borrow_mut()
            .replace((handle.bits(), Persistent::save(ctx, value.clone())));
        drop(previous);
        Ok(value)
    }

    fn allocation_limit(ctx: &Ctx<'_>) -> rquickjs::Error {
        rquickjs::Exception::throw_range(ctx, "native wrapper allocation limit exceeded")
    }

    fn reserve_wrapper_materialization(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let count = self.new_wrapper_materializations.get();
        if self.collecting.get() {
            self.record_native_failure("wrapper publication during collection");
            return quickjs_check_native_failure(ctx);
        }
        if count >= WRAPPER_DEFERRED_LIMIT {
            return Err(Self::allocation_limit(ctx));
        }
        self.new_wrapper_materializations.set(count + 1);
        Ok(())
    }

    pub fn script_interrupted(&self) -> bool {
        self.host.native_memory.first_failure().is_some()
            || self.interrupt.load(Ordering::Acquire)
            || self.deadline.get().is_some_and(|end| Instant::now() >= end)
    }

    fn uncatchable_error(ctx: &Ctx<'_>, message: &'static str) -> rquickjs::Error {
        let _ = rquickjs::Exception::throw_internal(ctx, message);
        let error = ctx.catch();
        // SAFETY: this runtime owns the error. Hard budget and ordinary
        // watchdog termination skip page catch/finally without native poison.
        unsafe {
            rquickjs::qjs::JS_SetUncatchableError(ctx.as_raw().as_ptr(), error.as_raw());
        }
        ctx.throw(error)
    }

    pub(crate) fn interrupted_error(ctx: &Ctx<'_>) -> rquickjs::Error {
        Self::uncatchable_error(ctx, "script execution interrupted")
    }

    pub(crate) fn native_memory_budget_error(ctx: &Ctx<'_>) -> rquickjs::Error {
        Self::uncatchable_error(ctx, "page memory budget exceeded")
    }

    fn admit_callback(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        if self.collecting.get() {
            self.record_native_failure("callback entered during wrapper collection");
            return quickjs_check_native_failure(ctx);
        }
        if self.host.native_memory.first_failure().is_some() {
            return Err(Self::native_memory_budget_error(ctx));
        }
        if self.script_interrupted() {
            return Err(Self::interrupted_error(ctx));
        }
        // An outer native frame can own unwrapped outgoing handles as well as
        // converted arguments. Never collect from a reentrant getter in it.
        let nested = match self.callback_roots.try_borrow() {
            Ok(frames) => !frames.is_empty(),
            Err(_) => {
                self.record_native_failure("callback argument frame borrow");
                return quickjs_check_native_failure(ctx);
            }
        };
        if !nested && self.new_wrapper_materializations.get() >= WRAPPER_CHECKPOINT_INTERVAL {
            self.collect_checkpoint(ctx, true)?;
            // Interruption can arrive after the collector has weakened roots.
            // Finish its closed recovery, then refuse before any host effect.
            if self.script_interrupted() {
                return Err(Self::interrupted_error(ctx));
            }
        }
        if self.new_wrapper_materializations.get() >= WRAPPER_DEFERRED_LIMIT {
            return Err(Self::allocation_limit(ctx));
        }
        Ok(())
    }

    fn existing<'js>(&self, ctx: &Ctx<'js>, bits: u64) -> rquickjs::Result<Option<Object<'js>>> {
        let (root, weak) = {
            let cache = self.wrappers.borrow();
            let Some(wrapper) = cache.get(&bits) else {
                return Ok(None);
            };
            (wrapper.root.clone(), wrapper.weak.clone())
        };
        if let Some(root) = root {
            return root.restore(ctx).map(Some);
        }
        let weak = weak.restore(ctx)?;
        let deref = self
            .weak_deref
            .borrow()
            .as_ref()
            .ok_or(rquickjs::Error::Unknown)?
            .clone()
            .restore(ctx)?;
        deref.call((rquickjs::function::This(weak),))
    }
    /// Adds one value-table reference to an already converted EventTarget
    /// wrapper. A missing weak-cache entry is an invariant failure: the
    /// callback argument frame still holds the original JS object.
    pub(crate) fn retain_existing_target(
        &self,
        ctx: &Ctx<'_>,
        handle: u64,
    ) -> Result<Option<u64>, JsError> {
        if self.native_failure.get().is_some() {
            return Err(JsError::type_error("relatedTarget wrapper is unavailable"));
        }
        if handle == encode_window() {
            return Ok(None);
        }
        let wrapper = match self.existing(ctx, handle) {
            Ok(Some(wrapper)) => wrapper,
            _ => {
                self.record_native_failure("relatedTarget wrapper retention");
                return Err(JsError::type_error("relatedTarget wrapper is unavailable"));
            }
        };
        if self.native_failure.get().is_some() {
            return Err(JsError::type_error("relatedTarget wrapper is unavailable"));
        }
        let bits = self.store_value(ctx, wrapper.into_value());
        if self.native_failure.get().is_some() {
            // The poisoned world owns this root until teardown.
            return Err(JsError::type_error("relatedTarget wrapper is unavailable"));
        }
        Ok(Some(bits))
    }

    fn make<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        host: HostObject,
    ) -> Option<Object<'js>> {
        let bits = host.handle_bits();
        if let Some(object) = self.existing(ctx, bits).ok()? {
            return Some(object);
        }
        let decoded = decode(bits);
        let graph_root = decoded.node_anchor().is_some()
            || (decoded.tag == TAG_HOST
                && (cssom::graph::is_css_host(&host) || host.is_standalone_event_target()));
        if decoded.tag != TAG_WINDOW {
            // Every newly materialized native wrapper consumes admission debt,
            // even when its kind does not need a provisional graph root.
            // Reserve before allocating a class or publishing a raw handle.
            self.reserve_wrapper_materialization(ctx).ok()?;
        }
        let (prototype, ancestors) = {
            let p = self.prototypes.borrow();
            let p = p.get(interface)?;
            (p.value.clone().restore(ctx).ok()?, p.ancestors)
        };
        if interface == "CSSStyleProperties" {
            tenta_js::bindings::style_properties::ensure_quickjs::<HostObject>(&prototype).ok()?;
        }
        let owner = host.context().clone();
        let generation = self.next_wrapper.get();
        self.next_wrapper.set(generation.wrapping_add(1));
        let native = QuickJsHost {
            host: Some(Rc::new(host)),
            generation,
            native_failure: self.native_failure.clone(),
            handle: bits,
            interfaces: ancestors,
            dead: self.dead.clone(),
        };
        let methods = self.legacy_methods.borrow().get(interface).copied();
        let object = match methods {
            Some(methods) => Class::instance(
                ctx.clone(),
                legacy::LegacyHost::new(ctx, native, methods).ok()?,
            )
            .ok()?
            .into_inner(),
            None => Class::instance(ctx.clone(), native).ok()?.into_inner(),
        };
        object.set_prototype(Some(&prototype)).ok()?;
        let ctor = self
            .weak_constructor
            .borrow()
            .as_ref()?
            .clone()
            .restore(ctx)
            .ok()?;
        let weak: Object = ctor.construct((object.clone(),)).ok()?;
        let wrapper = Wrapper {
            weak: Persistent::save(ctx, weak),
            generation,
            root: graph_root.then(|| Persistent::save(ctx, object.clone())),
        };
        // A remint of the same exact handle transfers its cached claim. Its
        // queued old-generation death must not release the replacement's claim.
        // Failed constructions have no cache entry and never own a claim.
        let previous = {
            let mut wrappers = self.wrappers.borrow_mut();
            if decoded.node_anchor().is_some() && !wrappers.contains_key(&bits) {
                let node = tenta_core::NodeRef {
                    id: decoded.node_id,
                    generation: u32::from(decoded.generation),
                };
                if owner.doc.borrow_mut().retain_wrapper(node).is_err() {
                    self.record_native_failure("node wrapper claim");
                    return None;
                }
            }
            wrappers.insert(bits, wrapper)
        };
        drop(previous);
        // Every fallible allocation completed before the cache entry and its
        // native ownership were published.
        match decoded.tag {
            TAG_EVENT => {
                if let Some(record) = owner.events.borrow_mut().get_mut(decoded.event_key()) {
                    record.has_wrapper = true;
                }
            }
            TAG_HOST => {
                if let Some(record) = owner.host_records.borrow_mut().get_mut(decoded.host_key()) {
                    match record {
                        HostRecord::Url(v) => v.has_wrapper = true,
                        HostRecord::Signal(v) => v.has_wrapper = true,
                        HostRecord::Headers(v) => v.has_wrapper = true,
                        HostRecord::File(v) => v.has_wrapper = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Some(object)
    }
    /// Drains exact wrapper deaths and reports released native node pins.
    pub fn reclaim(&self) -> bool {
        self.reclaim_with_failure(false)
    }

    fn reclaim_with_failure(&self, teardown: bool) -> bool {
        let mut pins_released = false;
        let failed = || !teardown && self.native_failure.get().is_some();
        loop {
            if failed() {
                return pins_released;
            }
            if self.pending_dead.borrow().is_empty() {
                *self.pending_dead.borrow_mut() = self.dead.take();
            }
            loop {
                if failed() {
                    return pins_released;
                }
                let Some((bits, generation)) = self.pending_dead.borrow().last().copied() else {
                    break;
                };
                // The current death stays published until every native step
                // succeeds. A panic cannot lose the remaining processing batch.
                if self
                    .wrappers
                    .borrow()
                    .get(&bits)
                    .is_some_and(|live| live.generation != generation)
                {
                    self.pending_dead.borrow_mut().pop();
                    continue;
                }
                let d = decode(bits);
                let removed = if d.node_anchor().is_some() {
                    if let Some(ctx) = self.host.documents.get(d.slot) {
                        // Borrow the document before taking ownership out of
                        // the cache. Release before dropping JS handles: that
                        // drop can poison the world and leave this death queued
                        // for teardown, which must not release the claim twice.
                        let mut doc = ctx.doc.borrow_mut();
                        let removed = self.wrappers.borrow_mut().remove(&bits);
                        if removed.is_some() {
                            doc.release_wrapper(tenta_core::NodeRef {
                                id: d.node_id,
                                generation: u32::from(d.generation),
                            });
                        }
                        removed
                    } else {
                        self.wrappers.borrow_mut().remove(&bits)
                    }
                } else {
                    self.wrappers.borrow_mut().remove(&bits)
                };
                drop(removed);
                if failed() {
                    return pins_released;
                }
                if let Some(ctx) = self.host.documents.get(d.slot) {
                    match d.tag {
                        TAG_STATIC_NODE_LIST | TAG_STATIC_HTML_COLLECTION => {
                            {
                                let mut snapshots = ctx.snapshots.borrow_mut();
                                pins_released |= snapshots
                                    .get(d.snapshot_key())
                                    .is_some_and(|nodes| !nodes.is_empty());
                                snapshots.remove(d.snapshot_key());
                            }
                            if failed() {
                                return pins_released;
                            }
                        }
                        TAG_EVENT => {
                            let removed = ctx.events.borrow_mut().remove(d.event_key());
                            if let Some(bits) =
                                removed.and_then(|record| record.kind.owned_stored_value())
                            {
                                self.release(bits);
                                if failed() {
                                    return pins_released;
                                }
                            }
                        }
                        TAG_HOST => {
                            let owns_pins = matches!(
                                ctx.host_records.borrow().get(d.host_key()),
                                Some(
                                    HostRecord::Attr(_)
                                        | HostRecord::Range(_)
                                        | HostRecord::StaticRange(_)
                                )
                            );
                            let reclaimed = ctx.reclaim_host(d.host_key());
                            if failed() {
                                return pins_released;
                            }
                            pins_released |= owns_pins && reclaimed.keys.contains(&d.host_key());
                            for callback in ctx.drain_host_listeners(&reclaimed.keys) {
                                self.release(callback.0);
                                if failed() {
                                    return pins_released;
                                }
                            }
                            for value in reclaimed.values {
                                self.release(value);
                                if failed() {
                                    return pins_released;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                self.pending_dead.borrow_mut().pop();
            }
            for context in self.host.documents.all() {
                for callback in context.take_swept_listeners() {
                    self.release(callback.0);
                    if failed() {
                        return pins_released;
                    }
                }
            }
            // Releases can enqueue more deaths. Drain before another plan can
            // observe a finalized wrapper whose provisional root was removed.
            let queued = self.dead.take();
            if queued.is_empty() {
                return pins_released;
            }
            *self.pending_dead.borrow_mut() = queued;
        }
    }
    pub fn clear(&self) {
        // No observer callback may enter while the world releases JS roots.
        self.host.documents.retire_observers();
        self.reclaim_with_failure(true);
        // A failed cleanup can retain a frame until teardown. Contexts remain
        // alive here, and no frame-stack borrow may span value destruction.
        let frames = std::mem::take(&mut *self.callback_roots.borrow_mut());
        drop(frames);
        let holders = std::mem::take(&mut *self.storage_holders.borrow_mut());
        drop(holders);
        let crypto_holder = self.crypto_holder.borrow_mut().take();
        drop(crypto_holder);
        for host in self.host.documents.all() {
            *host.script.borrow_mut() = None;
        }
        let values = std::mem::take(&mut *self.values.borrow_mut());
        drop(values);
        let cycle = self.cycle.borrow_mut().take();
        drop(cycle);
        let intrinsics = self.cycle_intrinsics.borrow_mut().take();
        drop(intrinsics);
        // A document may outlive this runtime. Live wrappers will finalize
        // when its context is freed, so clear their node pins explicitly now.
        let wrappers = std::mem::take(&mut *self.wrappers.borrow_mut());
        for (bits, wrapper) in wrappers {
            let d = decode(bits);
            if d.node_anchor().is_some()
                && let Some(owner) = self.host.documents.get(d.slot)
            {
                owner.doc.borrow_mut().release_wrapper(tenta_core::NodeRef {
                    id: d.node_id,
                    generation: u32::from(d.generation),
                });
            }
            drop(wrapper);
        }
        self.prototypes.borrow_mut().clear();
        self.legacy_methods.borrow_mut().clear();
        self.intrinsics.borrow_mut().clear();
        self.weak_constructor.borrow_mut().take();
        self.weak_deref.borrow_mut().take();
        // Ranges retain arena endpoints; release records before breaking the
        // minted-context / document-set cycle while their documents are live.
        for host in self.host.documents.all() {
            host.host_records.replace(Default::default());
        }
        self.host.documents.clear_minted();
    }
}
impl QuickJsWrapperWorld for World {
    fn begin_callback_roots(&self, ctx: &Ctx<'_>) -> rquickjs::Result<u64> {
        self.admit_callback(ctx)?;
        let Some(id) = self.next_callback_frame.get().checked_add(1) else {
            self.record_native_failure("callback argument frame overflow");
            return Err(rquickjs::Error::Unknown);
        };
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("callback argument frame borrow");
            return Err(rquickjs::Error::Unknown);
        };
        frames
            .try_reserve(1)
            .map_err(|_| rquickjs::Error::Allocation)?;
        frames.push(CallbackRoots {
            id,
            values: Vec::new(),
            stored_value_lease: None,
            stored_value_transferred: false,
            buffer_lease_id: None,
        });
        self.next_callback_frame.set(id);
        Ok(id)
    }

    fn begin_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("stored value frame borrow");
            return quickjs_check_native_failure(ctx);
        };
        let Some(frame) = frames.last_mut() else {
            self.record_native_failure("stored value frame missing");
            return quickjs_check_native_failure(ctx);
        };
        if frame.stored_value_lease.is_some() {
            self.record_native_failure("stored value frame nested without callback");
            return quickjs_check_native_failure(ctx);
        }
        frame.stored_value_lease = Some(Vec::new());
        frame.stored_value_transferred = false;
        frame.buffer_lease_id = None;
        Ok(())
    }

    fn begin_buffer_reference_lease(&self, ctx: &Ctx<'_>, lease_id: u64) -> rquickjs::Result<()> {
        self.begin_stored_value_lease(ctx)?;
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("buffer reference frame borrow");
            return quickjs_check_native_failure(ctx);
        };
        let Some(frame) = frames.last_mut() else {
            self.record_native_failure("buffer reference frame missing");
            return quickjs_check_native_failure(ctx);
        };
        frame.buffer_lease_id = Some(lease_id);
        Ok(())
    }

    fn current_buffer_reference_lease(&self) -> Option<u64> {
        self.callback_roots.borrow().last().and_then(|frame| {
            frame.stored_value_lease.as_ref()?;
            frame.buffer_lease_id
        })
    }

    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool {
        self.callback_roots.borrow().iter().any(|frame| {
            frame.stored_value_lease.is_some() && frame.buffer_lease_id == Some(lease_id)
        })
    }

    fn track_stored_value(&self, ctx: &Ctx<'_>, bits: u64) -> rquickjs::Result<()> {
        // Value conversion also runs outside leased calls. Only the active
        // callback's stored-value lease may own this newly stored reference.
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("stored value frame borrow");
            return quickjs_check_native_failure(ctx);
        };
        let Some(frame) = frames.last_mut() else {
            return Ok(());
        };
        let Some(values) = frame.stored_value_lease.as_mut() else {
            return Ok(());
        };
        if values.try_reserve(1).is_err() {
            // The acquired value-table reference cannot be forgotten and the
            // frame cannot recover its missing bit. Poison and retain it until
            // teardown rather than continuing with a leak in a healthy world.
            self.record_native_failure("stored value lease allocation");
            return quickjs_check_native_failure(ctx);
        }
        values.push(bits);
        Ok(())
    }

    fn transfer_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("stored value frame borrow");
            return quickjs_check_native_failure(ctx);
        };
        let Some(frame) = frames.last_mut() else {
            self.record_native_failure("stored value frame missing");
            return quickjs_check_native_failure(ctx);
        };
        if frame.stored_value_lease.is_none() || frame.stored_value_transferred {
            self.record_native_failure("stored value transfer order");
            return quickjs_check_native_failure(ctx);
        }
        frame.stored_value_transferred = true;
        Ok(())
    }

    fn before_constructor_host(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        quickjs_check_native_failure(ctx)?;
        if self.script_interrupted() {
            return Err(Self::interrupted_error(ctx));
        }
        if self.new_wrapper_materializations.get() >= WRAPPER_DEFERRED_LIMIT {
            return Err(Self::allocation_limit(ctx));
        }
        quickjs_check_native_failure(ctx)
    }

    fn retain_callback_argument<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Value<'js>,
    ) -> rquickjs::Result<()> {
        let root = Persistent::save(ctx, value);
        let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
            self.record_native_failure("callback argument frame borrow");
            return Err(rquickjs::Error::Unknown);
        };
        let Some(frame) = frames.last_mut() else {
            return Err(rquickjs::Error::new_from_js_message(
                "interface instance",
                "native handle",
                "interface conversion requires a callback scope",
            ));
        };
        frame
            .values
            .try_reserve(1)
            .map_err(|_| rquickjs::Error::Allocation)?;
        frame.values.push(root);
        Ok(())
    }

    fn end_callback_roots(&self, id: u64) {
        let initially_failed = self.native_failure.get().is_some();
        if !self.finish_stored_value_lease(id, initially_failed) {
            return;
        }
        loop {
            let value = {
                let Ok(mut frames) = self.callback_roots.try_borrow_mut() else {
                    self.record_native_failure("callback argument frame borrow");
                    return;
                };
                let Some(frame) = frames.last_mut().filter(|frame| frame.id == id) else {
                    self.record_native_failure("callback argument frame order");
                    return;
                };
                match frame.values.pop() {
                    Some(value) => value,
                    None => {
                        frames.pop();
                        return;
                    }
                }
            };
            // Keep the frame and its remaining roots published while a value
            // is destroyed. A failure cannot discard an ancestor's scope or
            // release the not-yet-dropped roots during a second unwind.
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(value)))
            {
                self.record_native_failure("callback argument cleanup");
                if let Err(nested) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(nested);
                }
                return;
            }
            if !initially_failed && self.native_failure.get().is_some() {
                return;
            }
        }
    }

    fn wrapper<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        bits: u64,
    ) -> Option<Object<'js>> {
        if let Some(existing) = self.existing(ctx, bits).ok()? {
            return Some(existing);
        }
        let d = decode(bits);
        if d.tag == TAG_WINDOW {
            return matches!(interface, "EventTarget" | "Window" | "WindowProxy")
                .then(|| ctx.globals());
        }
        let owner = self.host.documents.get(d.slot)?;
        let (interface, host) = match d.tag {
            TAG_STATIC_NODE_LIST => {
                owner.snapshots.borrow().get(d.snapshot_key())?;
                (
                    "NodeList",
                    HostObject::static_node_list(owner, d.snapshot_key()),
                )
            }
            TAG_STATIC_HTML_COLLECTION => {
                owner.snapshots.borrow().get(d.snapshot_key())?;
                (
                    "HTMLCollection",
                    HostObject::static_html_collection(owner, d.snapshot_key()),
                )
            }
            TAG_EVENT => {
                let interface = {
                    let events = owner.events.borrow();
                    let record = events.get(d.event_key())?;
                    record.kind.interface_name()
                };
                (interface, HostObject::event(owner, d.event_key()))
            }
            TAG_HOST => {
                let interface = {
                    let records = owner.host_records.borrow();
                    let record = records.get(d.host_key())?;
                    record.interface_name()
                };
                (interface, HostObject::host_record(owner, d.host_key()))
            }
            _ => {
                let node = {
                    let doc = owner.doc.borrow();
                    if !doc.is_live(d.node_id)
                        || doc.node_ref(d.node_id).generation != u32::from(d.generation)
                    {
                        return None;
                    }
                    doc.node_ref(d.node_id)
                };
                match d.tag {
                    TAG_NODE_LIST => ("NodeList", HostObject::node_list(owner, node)),
                    TAG_HTML_COLLECTION => {
                        ("HTMLCollection", HostObject::html_collection(owner, node))
                    }
                    TAG_TOKEN_LIST => ("DOMTokenList", HostObject::token_list(owner, node)),
                    TAG_NAMED_NODE_MAP => ("NamedNodeMap", HostObject::named_node_map(owner, node)),
                    TAG_STYLE => ("CSSStyleProperties", HostObject::style(owner, node)),
                    TAG_NODE => {
                        let interface = node_interface(&owner, node.id);
                        (interface, HostObject::node(owner, node))
                    }
                    _ => return None,
                }
            }
        };
        self.make(ctx, interface, host)
    }
    fn handle(&self, interface: &'static str, wrapper: &Object<'_>) -> Option<u64> {
        if matches!(interface, "EventTarget" | "Window" | "WindowProxy")
            && wrapper == &wrapper.ctx().globals()
        {
            return Some(encode_window());
        }
        if let Some(class) = wrapper.as_class::<legacy::LegacyHost>() {
            let host = class.borrow();
            return host
                .host
                .interfaces
                .contains(&interface)
                .then_some(host.host.handle);
        }
        let class = wrapper.as_class::<QuickJsHost>()?;
        let host = class.borrow();
        host.interfaces.contains(&interface).then_some(host.handle)
    }
    fn store_value(&self, _ctx: &Ctx<'_>, value: Value<'_>) -> u64 {
        let ctx = value.ctx().clone();
        if value.is_null() {
            return AnyValue::NULL.0;
        }
        let mut values = self.values.borrow_mut();
        for (bits, slot) in values.iter_mut() {
            if slot.value.as_ref().is_some_and(|stored| {
                stored
                    .clone()
                    .restore(&ctx)
                    .is_ok_and(|stored| stored == value)
            }) {
                slot.refs += 1;
                return *bits;
            }
        }
        let bits = self.next_value.get();
        self.next_value.set(bits + 1);
        values.insert(
            bits,
            ValueSlot {
                value: Some(Persistent::save(&ctx, value)),
                refs: 1,
            },
        );
        bits
    }
    fn load_value<'js>(&self, ctx: &Ctx<'js>, bits: u64) -> Option<Value<'js>> {
        if bits == AnyValue::NULL.0 {
            return Some(Value::new_null(ctx.clone()));
        }
        self.values
            .borrow()
            .get(&bits)?
            .value
            .as_ref()?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn native_host(
        &self,
        interface: &'static str,
        value: &Value<'_>,
    ) -> Option<Rc<dyn std::any::Any>> {
        if interface == "EventTarget" && value.as_object().is_some_and(|o| o == &o.ctx().globals())
        {
            return Some(Rc::new(HostObject::window(self.host.clone())));
        }
        let class = value.as_object()?.as_class::<legacy::LegacyHost>()?;
        let native = class.borrow();
        if native.host.interfaces.contains(&interface) {
            native.host.host.clone()
        } else {
            None
        }
    }
    fn realm(&self) -> Realm {
        self.host.clone()
    }
    fn adopt<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        host: Box<dyn std::any::Any>,
    ) -> Option<Object<'js>> {
        let host = host.downcast::<HostObject>().ok()?;
        let owner = host.context().clone();
        let bits = host.handle_bits();
        let decoded = decode(bits);
        let matching_owner = self
            .host
            .documents
            .get(decoded.slot)
            .is_some_and(|current| Rc::ptr_eq(&current, &owner));
        let live_record = matching_owner
            && match decoded.tag {
                TAG_EVENT => owner.events.borrow().get(decoded.event_key()).is_some(),
                TAG_HOST => owner
                    .host_records
                    .borrow()
                    .get(decoded.host_key())
                    .is_some(),
                TAG_NODE => {
                    let doc = owner.doc.borrow();
                    doc.is_live(decoded.node_id)
                        && doc.node_ref(decoded.node_id).generation == u32::from(decoded.generation)
                }
                _ => false,
            };
        let result = live_record
            .then(|| self.make(ctx, interface, *host))
            .flatten();
        if result.is_none()
            && self.native_failure.get().is_none()
            && self.rollback_failed_constructor(&owner, bits).is_err()
        {
            self.record_native_failure("constructor adoption rollback");
        }
        result
    }
    fn prototype<'js>(&self, ctx: &Ctx<'js>, interface: &'static str) -> Option<Object<'js>> {
        self.prototypes
            .borrow()
            .get(interface)?
            .value
            .clone()
            .restore(ctx)
            .ok()
    }
    fn cache_prototype<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        prototype: Object<'js>,
        ancestors: &'static [&'static str],
    ) {
        self.prototypes.borrow_mut().insert(
            interface,
            Prototype {
                value: Persistent::save(ctx, prototype),
                ancestors,
            },
        );
    }
    fn cache_legacy(&self, interface: &'static str, methods: QuickJsLegacyMethods) {
        self.legacy_methods.borrow_mut().insert(interface, methods);
    }
    fn native_failure(&self) -> Option<&'static str> {
        self.native_failure.get()
    }
    fn native_memory_failure(&self) -> bool {
        self.host.native_memory.first_failure().is_some()
    }
    fn script_interrupted(&self) -> bool {
        World::script_interrupted(self)
    }
    fn intrinsic<'js>(&self, ctx: &Ctx<'js>, name: &str) -> Option<Value<'js>> {
        self.intrinsics
            .borrow()
            .get(name)?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn record_native_failure(&self, name: &'static str) {
        if self.native_failure.get().is_none() {
            self.native_failure.set(Some(name));
        }
    }
}

fn node_interface(ctx: &SharedContext, node: tenta_core::NodeId) -> &'static str {
    let doc = ctx.doc.borrow();
    match doc.kind(node) {
        NodeKind::Element => match doc.namespace(node) {
            Some(ns) if ns == atoms::NS_HTML => match doc.tag(node) {
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
        NodeKind::CData => "CDATASection",
        NodeKind::Comment => "Comment",
        NodeKind::Fragment => "DocumentFragment",
        NodeKind::DocType => "DocumentType",
        NodeKind::ProcessingInstruction => "ProcessingInstruction",
        _ => "Node",
    }
}

/// The userdata owns an Rc while the trait implementation stays on a local type.
pub(crate) struct Bridge(pub Rc<World>);
impl QuickJsWrapperWorld for Bridge {
    fn begin_callback_roots(&self, ctx: &Ctx<'_>) -> rquickjs::Result<u64> {
        self.0.begin_callback_roots(ctx)
    }
    fn begin_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        self.0.begin_stored_value_lease(ctx)
    }
    fn begin_buffer_reference_lease(&self, ctx: &Ctx<'_>, lease_id: u64) -> rquickjs::Result<()> {
        self.0.begin_buffer_reference_lease(ctx, lease_id)
    }
    fn current_buffer_reference_lease(&self) -> Option<u64> {
        self.0.current_buffer_reference_lease()
    }
    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool {
        self.0.buffer_reference_lease_active(lease_id)
    }
    fn track_stored_value(&self, ctx: &Ctx<'_>, bits: u64) -> rquickjs::Result<()> {
        self.0.track_stored_value(ctx, bits)
    }
    fn transfer_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        self.0.transfer_stored_value_lease(ctx)
    }
    fn before_constructor_host(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        self.0.before_constructor_host(ctx)
    }
    fn retain_callback_argument<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Value<'js>,
    ) -> rquickjs::Result<()> {
        self.0.retain_callback_argument(ctx, value)
    }
    fn end_callback_roots(&self, frame: u64) {
        self.0.end_callback_roots(frame)
    }
    fn intrinsic<'js>(&self, ctx: &Ctx<'js>, name: &str) -> Option<Value<'js>> {
        self.0.intrinsic(ctx, name)
    }
    fn wrapper<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        bits: u64,
    ) -> Option<Object<'js>> {
        self.0.wrapper(ctx, interface, bits)
    }
    fn handle(&self, interface: &'static str, wrapper: &Object<'_>) -> Option<u64> {
        self.0.handle(interface, wrapper)
    }
    fn store_value(&self, ctx: &Ctx<'_>, value: Value<'_>) -> u64 {
        self.0.store_value(ctx, value)
    }
    fn load_value<'js>(&self, ctx: &Ctx<'js>, bits: u64) -> Option<Value<'js>> {
        self.0.load_value(ctx, bits)
    }
    fn realm(&self) -> Realm {
        self.0.realm()
    }
    fn adopt<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        host: Box<dyn std::any::Any>,
    ) -> Option<Object<'js>> {
        self.0.adopt(ctx, interface, host)
    }
    fn prototype<'js>(&self, ctx: &Ctx<'js>, interface: &'static str) -> Option<Object<'js>> {
        self.0.prototype(ctx, interface)
    }
    fn cache_prototype<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        prototype: Object<'js>,
        ancestors: &'static [&'static str],
    ) {
        self.0.cache_prototype(ctx, interface, prototype, ancestors)
    }
    fn cache_legacy(&self, interface: &'static str, methods: QuickJsLegacyMethods) {
        self.0.cache_legacy(interface, methods);
    }
    fn native_failure(&self) -> Option<&'static str> {
        self.0.native_failure()
    }
    fn native_memory_failure(&self) -> bool {
        self.0.host.native_memory.first_failure().is_some()
    }
    fn script_interrupted(&self) -> bool {
        self.0.script_interrupted()
    }
    fn record_native_failure(&self, name: &'static str) {
        self.0.record_native_failure(name)
    }
    fn native_host(
        &self,
        interface: &'static str,
        value: &Value<'_>,
    ) -> Option<Rc<dyn std::any::Any>> {
        self.0.native_host(interface, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_callback_failure_precedes_argument_cleanup_failure() {
        struct PanickingHost;
        impl Drop for PanickingHost {
            fn drop(&mut self) {
                panic!("secondary argument finalizer failure");
            }
        }
        let mut engine = crate::Engine::new().unwrap();
        let world = engine.state.world.clone();
        let error = engine
            .entry::<()>(None, |ctx| {
                quickjs_callback_boundary(ctx, "primary argument callback", || {
                    let instance = Class::instance(
                        ctx.clone(),
                        QuickJsHost {
                            host: Some(Rc::new(PanickingHost)),
                            handle: u64::MAX,
                            generation: 0,
                            interfaces: &[],
                            dead: world.dead.clone(),
                            native_failure: world.native_failure.clone(),
                        },
                    )?;
                    world.retain_callback_argument(ctx, instance.into_value())?;
                    panic!("primary callback failure before argument cleanup");
                })
            })
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert_eq!(engine.native_failure(), Some("primary argument callback"));
        assert!(world.callback_roots.borrow().is_empty());
        assert_eq!(
            engine.evaluate("1 + 1").unwrap_err().kind,
            tenta_js::engine::ScriptErrorKind::Terminated,
        );
    }

    #[test]
    fn argument_cleanup_failure_retains_remaining_roots_until_teardown() {
        struct KeptHost(Rc<Cell<bool>>);
        impl Drop for KeptHost {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        struct PanickingHost;
        impl Drop for PanickingHost {
            fn drop(&mut self) {
                panic!("argument root cleanup failure");
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let mut engine = crate::Engine::new().unwrap();
        let world = engine.state.world.clone();
        let error = engine
            .entry::<()>(None, |ctx| {
                quickjs_callback_boundary(ctx, "cleanup callback", || {
                    for host in [
                        Rc::new(KeptHost(dropped.clone())) as Rc<dyn std::any::Any>,
                        Rc::new(PanickingHost),
                    ] {
                        let instance = Class::instance(
                            ctx.clone(),
                            QuickJsHost {
                                host: Some(host),
                                handle: u64::MAX,
                                generation: 0,
                                interfaces: &[],
                                dead: world.dead.clone(),
                                native_failure: world.native_failure.clone(),
                            },
                        )?;
                        world.retain_callback_argument(ctx, instance.into_value())?;
                    }
                    Ok(())
                })
            })
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert_eq!(engine.native_failure(), Some("native host finalizer"));
        assert!(!dropped.get(), "cleanup must retain the remaining argument");
        assert_eq!(world.callback_roots.borrow().len(), 1);
        assert_eq!(
            engine.evaluate("1 + 1").unwrap_err().kind,
            tenta_js::engine::ScriptErrorKind::Terminated,
        );
        drop(engine);
        assert!(
            dropped.get(),
            "engine teardown must release retained arguments"
        );
        assert!(world.callback_roots.borrow().is_empty());
    }

    #[test]
    fn native_failure_reporting_never_calls_error_accessors() {
        let mut engine = crate::Engine::new().unwrap();
        engine.evaluate("Object.defineProperty(InternalError.prototype,'name',{get(){globalThis.errorNameRan=true;return 'InternalError'}});Error.prototype.toString=function(){globalThis.errorStringRan=true;return 'tampered'}").unwrap();
        let document = engine.state.world.host.doc.clone();
        let borrow = document.borrow_mut();
        let error = engine
            .evaluate("document.createElement('div')")
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        engine.context.with(|ctx| {
            assert!(
                ctx.globals()
                    .get::<_, Value>("errorNameRan")
                    .unwrap()
                    .is_undefined()
            );
            assert!(
                ctx.globals()
                    .get::<_, Value>("errorStringRan")
                    .unwrap()
                    .is_undefined()
            );
        });
        drop(borrow);
    }

    #[test]
    fn failure_during_exception_name_lookup_skips_message_coercion() {
        let mut engine = crate::Engine::new().unwrap();
        let document = engine.state.world.host.doc.clone();
        let borrow = document.borrow_mut();
        let error = engine.evaluate("throw {get name(){document.createElement('div')},toString(){globalThis.afterPoison=true;return 'tampered'}}").unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        engine.context.with(|ctx| {
            assert!(
                ctx.globals()
                    .get::<_, Value>("afterPoison")
                    .unwrap()
                    .is_undefined()
            );
        });
        drop(borrow);
    }

    #[test]
    fn native_poison_prevents_later_event_listeners() {
        let mut engine = crate::Engine::new().unwrap();
        let document = engine.state.world.host.doc.clone();
        engine.context.with(|ctx| {
            ctx.globals()
                .set(
                    "panicNative",
                    Function::new(ctx.clone(), move |ctx: Ctx<'_>| {
                        let _conflict = document.borrow_mut();
                        ctx.eval::<(), _>("document.createElement('div')")
                    })
                    .unwrap(),
                )
                .unwrap();
        });
        let error = engine.evaluate("document.addEventListener('probe',()=>panicNative());document.addEventListener('probe',()=>{globalThis.later=true});document.dispatchEvent(new Event('probe'));globalThis.continued=true").unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        engine.context.with(|ctx| {
            assert!(
                ctx.globals()
                    .get::<_, Value>("later")
                    .unwrap()
                    .is_undefined()
            );
            assert!(
                ctx.globals()
                    .get::<_, Value>("continued")
                    .unwrap()
                    .is_undefined()
            );
        });
    }

    #[test]
    fn reentrant_native_poison_skips_promise_and_iterator_error_cleanup() {
        for source in [
            "try{fetch('https://example.test/',{get method(){document.createElement('div');return 'GET'}})}catch(e){globalThis.caught=true}globalThis.continued=true",
            r#"try {
                new Blob({
                    [Symbol.iterator]() {
                        return {
                            next() { return {done: false, value: {toString() { document.createElement('div'); }}}; },
                            return() { globalThis.closed = true; return {}; }
                        };
                    }
                });
            } catch (e) { globalThis.caught = true; }
            globalThis.continued = true;"#,
        ] {
            let mut engine = crate::Engine::new().unwrap();
            let document = engine.state.world.host.doc.clone();
            let borrow = document.borrow_mut();
            let error = engine.evaluate(source).unwrap_err();
            assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
            engine.context.with(|ctx| {
                for name in ["caught", "continued", "closed"] {
                    assert!(
                        ctx.globals().get::<_, Value>(name).unwrap().is_undefined(),
                        "poisoned script ran {name}"
                    );
                }
            });
            drop(borrow);
        }
    }

    #[test]
    fn native_finalizer_failure_interrupts_running_javascript() {
        struct PanickingHost;
        impl Drop for PanickingHost {
            fn drop(&mut self) {
                panic!("native finalizer interrupt probe");
            }
        }
        let mut engine = crate::Engine::new().unwrap();
        let world = engine.state.world.clone();
        engine.context.with(|ctx| {
            ctx.globals()
                .set(
                    "makePanickingHost",
                    Function::new(ctx.clone(), move |ctx: Ctx<'_>| {
                        let instance = Class::instance(
                            ctx,
                            QuickJsHost {
                                host: Some(Rc::new(PanickingHost)),
                                handle: u64::MAX,
                                generation: 0,
                                interfaces: &[],
                                dead: world.dead.clone(),
                                native_failure: world.native_failure.clone(),
                            },
                        )?;
                        drop(instance);
                        Ok::<_, rquickjs::Error>(())
                    })
                    .unwrap(),
                )
                .unwrap();
        });
        // A separate watchdog bounds a regression without being the mechanism
        // that makes the test pass: native failure must interrupt on its own.
        let watchdog = tenta_js::engine::PageEngine::watchdog(&engine).unwrap();
        let (done, completion) = std::sync::mpsc::channel();
        let fallback = std::thread::spawn(move || {
            if completion
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_err()
            {
                watchdog.terminate();
                true
            } else {
                false
            }
        });
        let error = engine
            .evaluate("makePanickingHost();for(;;){}")
            .unwrap_err();
        let _ = done.send(());
        assert!(
            !fallback.join().unwrap(),
            "native failure did not interrupt the loop"
        );
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert_eq!(engine.native_failure(), Some("native host finalizer"));
        engine.trim_memory();
        assert_eq!(engine.native_failure(), Some("native host finalizer"));
    }

    #[test]
    fn teardown_releases_minted_document_contexts_and_live_node_pins() {
        let mut engine = crate::Engine::new().unwrap();
        engine.evaluate("globalThis.child=new DOMParser().parseFromString('<root/>','application/xml');globalThis.detached=document.createElement('div')").unwrap();
        let root = engine.state.world.host.clone();
        let child = Rc::downgrade(&root.documents.get(1).expect("minted XML document"));
        let detached = engine.context.with(|ctx| {
            let wrapper: Object = ctx.globals().get("detached").unwrap();
            decode(root_handle(&wrapper)).node_id
        });
        assert!(root.doc.borrow().has_wrapper(detached));
        drop(engine);
        assert!(
            child.upgrade().is_none(),
            "minted context must not form an ownership cycle"
        );
        assert!(!root.doc.borrow().has_wrapper(detached));
        root.doc.borrow_mut().sweep_detached();
        assert!(!root.doc.borrow().is_live(detached));
    }

    #[test]
    fn failed_wrapper_construction_does_not_pin_a_node() {
        let mut engine = crate::Engine::new().unwrap();
        engine
            .evaluate("globalThis.detached=document.createElement('div')")
            .unwrap();
        let world = &engine.state.world;
        let bits = engine.context.with(|ctx| {
            let wrapper: Object = ctx.globals().get("detached").unwrap();
            let bits = root_handle(&wrapper);
            ctx.globals().remove("detached").unwrap();
            bits
        });
        // Drop the cached WeakRef so the construction path cannot return an
        // existing object. Removing the saved constructor injects a failure
        // after native class allocation without depending on allocator limits.
        world.wrappers.borrow_mut().remove(&bits);
        let id = decode(bits).node_id;
        world
            .host
            .doc
            .borrow_mut()
            .release_wrapper(tenta_core::NodeRef {
                id,
                generation: u32::from(decode(bits).generation),
            });
        let constructor = world.weak_constructor.borrow_mut().take();
        engine
            .context
            .with(|ctx| assert!(world.wrapper(&ctx, "Node", bits).is_none()));
        assert!(!world.host.doc.borrow().has_wrapper(id));
        *world.weak_constructor.borrow_mut() = constructor;
    }

    #[test]
    fn failed_wrapper_finalizer_cannot_release_another_worlds_claim() {
        let engine = crate::Engine::new().unwrap();
        let document = engine.state.world.host.doc.clone();
        let mut other = crate::Engine::with_document(document.clone()).unwrap();
        other
            .evaluate(
                "globalThis.kept = document.appendChild(document.createElement('div')); \
                 kept.note = 'foreign'",
            )
            .unwrap();
        let node = {
            let doc = document.borrow();
            doc.node_ref(doc.document_element().unwrap())
        };
        let world = engine.state.world.clone();
        let bits = encode_node(world.host.slot, node);
        let constructor = world.weak_constructor.borrow_mut().take();
        engine
            .context
            .with(|ctx| assert!(world.wrapper(&ctx, "Node", bits).is_none()));
        *world.weak_constructor.borrow_mut() = constructor;
        world.reclaim();
        assert_eq!(document.borrow().wrapper_count(node.id), 1);
        other.evaluate("document.removeChild(kept)").unwrap();
        drop(engine);
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
        assert_eq!(other.evaluate("kept.note").unwrap(), "foreign");
        drop(other);
        assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
    }

    #[test]
    fn node_rewrap_transfers_its_claim_before_old_finalizer_reclamation() {
        let mut engine = crate::Engine::new().unwrap();
        engine
            .evaluate("document.appendChild(document.createElement('div'))")
            .unwrap();
        let world = engine.state.world.clone();
        let node = {
            let doc = world.host.doc.borrow();
            doc.node_ref(doc.document_element().unwrap())
        };
        let bits = encode_node(world.host.slot, node);
        // Connected-node roots ordinarily prevent this race; remove only the
        // engine's provisional JS root to exercise a queued exact-cache remint.
        // Native claims remain owned by the cache until transfer/reclamation.
        let root = world
            .wrappers
            .borrow_mut()
            .get_mut(&bits)
            .unwrap()
            .root
            .take();
        drop(root);
        engine.runtime.run_gc();
        let deaths = world.dead.take();
        assert!(deaths.iter().any(|(handle, _)| *handle == bits));
        world.dead.set(deaths);
        engine.context.with(|ctx| {
            let wrapper = world.wrapper(&ctx, "Node", bits).unwrap();
            ctx.globals().set("rewrapped", wrapper).unwrap();
        });
        world.reclaim();
        assert_eq!(world.host.doc.borrow().wrapper_count(node.id), 1);
        assert_eq!(
            engine
                .evaluate("rewrapped === document.documentElement")
                .unwrap(),
            "true"
        );
        engine.evaluate("document.removeChild(rewrapped)").unwrap();
        drop(engine);
        assert_eq!(world.host.doc.borrow_mut().sweep_detached().nodes_freed, 1);
    }

    fn root_handle(wrapper: &Object<'_>) -> u64 {
        wrapper.as_class::<QuickJsHost>().unwrap().borrow().handle
    }

    #[test]
    fn a_dead_wrapper_generation_cannot_reclaim_its_replacement() {
        let mut engine = crate::Engine::new().unwrap();
        engine
            .evaluate(
                "document.appendChild(document.createElement('div')).setAttribute('id','saved')",
            )
            .unwrap();
        engine.context.with(|ctx| {
            let _active = crate::host::ActiveContext::new(&engine.state.world, &ctx);
            // Attr remains weak: native ownership pins its owner, while its
            // canonical record can be rewrapped before the queued death drains.
            ctx.eval::<Value, _>("void document.documentElement.attributes.item(0)")
                .unwrap();
            let queued = engine.state.world.dead.take();
            assert!(!queued.is_empty());
            engine.state.world.dead.set(queued);
            ctx.eval::<Value, _>("globalThis.saved=document.documentElement.attributes.item(0)")
                .unwrap();
        });
        engine.state.world.reclaim();
        assert_eq!(
            engine
                .evaluate("document.documentElement.attributes.item(0)===saved")
                .unwrap(),
            "true"
        );
        engine.trim_memory();
        assert_eq!(
            engine.evaluate("saved.name+'|'+saved.value").unwrap(),
            "id|saved"
        );
    }

    #[test]
    fn an_orphaned_wrapper_does_not_pin_a_reused_native_slot() {
        let mut engine = crate::Engine::new().unwrap();
        engine
            .evaluate("document.appendChild(document.createElement('div')).note='old'")
            .unwrap();
        let world = engine.state.world.clone();
        let old = world.host.doc.borrow().document_element().unwrap();
        let reference = world.host.doc.borrow().node_ref(old);
        let old_bits = encode_node(world.host.slot, reference);
        // Exercise defensive stale-generation handling through real arena
        // reuse. An embedder can invalidate pins using this native primitive;
        // the stale wrapper must never clear the new generation's wrapper bit.
        {
            let mut doc = world.host.doc.borrow_mut();
            doc.mutate(tenta_dom::Mutation::Remove { child: old })
                .unwrap();
            doc.release_wrapper(reference);
            assert_eq!(doc.sweep_detached().nodes_freed, 1);
        }
        assert!(world.wrappers.borrow().contains_key(&old_bits));
        engine
            .evaluate("document.appendChild(document.createElement('span')).note='new'")
            .unwrap();
        let doc = world.host.doc.borrow();
        let new = doc.document_element().unwrap();
        assert_eq!(old, new, "the native free list must reuse the old slot");
        assert_ne!(reference.generation, doc.node_ref(new).generation);
        assert!(doc.has_wrapper(new));
        drop(doc);
        assert!(!world.wrappers.borrow().contains_key(&old_bits));
        assert_eq!(
            engine.evaluate("document.documentElement.note").unwrap(),
            "new"
        );
        assert_eq!(engine.native_failure(), None);
    }

    #[test]
    fn watchdog_requested_during_pressure_collection_waits_for_recovery() {
        struct RequestDuringGc {
            collecting: Rc<Cell<bool>>,
            watchdog: Box<dyn tenta_js::engine::EngineWatchdog>,
            observed: Rc<Cell<bool>>,
        }
        impl Drop for RequestDuringGc {
            fn drop(&mut self) {
                self.observed.set(self.collecting.get());
                self.watchdog.terminate();
            }
        }
        let mut engine = crate::Engine::new().unwrap();
        engine.runtime.set_gc_threshold(usize::MAX);
        let world = engine.state.world.clone();
        let observed = Rc::new(Cell::new(false));
        engine.context.with(|ctx| {
            let _active = crate::host::ActiveContext::new(&world, &ctx);
            // Real publications reach the threshold; no production fault hook
            // or counter override requests the subsequent pressure checkpoint.
            while world.new_wrapper_materializations.get() < WRAPPER_CHECKPOINT_INTERVAL {
                ctx.eval::<Value, _>("document.createElement('div')")
                    .unwrap();
            }
            let instance = Class::instance(
                ctx.clone(),
                QuickJsHost {
                    host: Some(Rc::new(RequestDuringGc {
                        collecting: world.collecting.clone(),
                        watchdog: tenta_js::engine::PageEngine::watchdog(&engine).unwrap(),
                        observed: observed.clone(),
                    })),
                    handle: u64::MAX,
                    generation: 0,
                    interfaces: &[],
                    dead: world.dead.clone(),
                    native_failure: world.native_failure.clone(),
                },
            )
            .unwrap();
            instance.set("cycle", instance.clone()).unwrap();
        });
        let error = engine
            .evaluate("document.createElement('i');globalThis.afterAdmission=true")
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert!(
            observed.get(),
            "request must arrive inside the actual GC transaction"
        );
        assert_eq!(engine.native_failure(), None);
        assert!(!world.collecting.get());
        assert!(
            world.cycle.borrow().is_none(),
            "recovery completed before termination"
        );
        assert_eq!(
            world.host.doc.borrow().len(),
            1,
            "interrupted admission must not create a host node"
        );
        engine.context.with(|ctx| {
            assert!(
                ctx.globals()
                    .get::<_, Value>("afterAdmission")
                    .unwrap()
                    .is_undefined()
            )
        });
        assert_eq!(
            engine
                .evaluate("document.createElement('b').nodeName")
                .unwrap(),
            "B"
        );
    }

    #[test]
    fn a_nonempty_wrapper_graph_survives_ordinary_timeout_without_native_poison() {
        let mut engine = crate::Engine::new().unwrap();
        engine.evaluate("globalThis.kept=document.createElement('div');kept.appendChild(document.createElement('b')).note='live'").unwrap();
        let error = engine
            .evaluate_with_timeout("for(;;){}", Some(std::time::Duration::from_millis(5)))
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert_eq!(engine.native_failure(), None);
        assert!(!engine.state.world.collecting.get());
        assert!(engine.state.world.cycle.borrow().is_none());
        assert_eq!(engine.evaluate("kept.firstChild.note").unwrap(), "live");
        let error = engine
            .evaluate_with_timeout(
                "document.createElement('i')",
                Some(std::time::Duration::ZERO),
            )
            .unwrap_err();
        assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
        assert_eq!(engine.native_failure(), None);
        assert_eq!(engine.state.world.host.doc.borrow().len(), 3);
        engine.evaluate("kept=null").unwrap();
        assert_eq!(
            engine
                .state
                .world
                .host
                .doc
                .borrow_mut()
                .sweep_detached()
                .nodes_freed,
            2
        );
    }
    #[test]
    fn a_multi_handle_return_preserves_publication_failure_and_recovers() {
        let mut engine = crate::Engine::new().unwrap();
        engine.evaluate("globalThis.root=document.createElement('div');root.innerHTML='<b></b>'.repeat(5000)").unwrap();
        // Use the actual generated sequence/interface conversion helpers in
        // one callback. NodeList's lazy JS iterator does not exercise a batch
        // of raw handles published before another native entry safe point.
        let handles: Vec<Handle<Node>> = {
            let world = &engine.state.world;
            let root_bits = engine
                .context
                .with(|ctx| root_handle(&ctx.globals().get::<_, Object>("root").unwrap()));
            let doc = world.host.doc.borrow();
            let mut node = doc.first_child(decode(root_bits).node_id);
            let mut handles = Vec::new();
            while node.is_some() {
                handles.push(Handle::from_bits(encode_node(
                    world.host.slot,
                    doc.node_ref(node),
                )));
                node = doc.next_sibling(node);
            }
            handles
        };
        assert_eq!(handles.len(), 5000);
        engine.context.with(|ctx| {
            let error = quickjs_callback_boundary(&ctx, "batch interface return", || {
                quickjs_sequence_value(&ctx, handles.clone(), |handle| {
                    handle_to_quickjs(&ctx, handle)
                })
            })
            .unwrap_err();
            assert!(error.is_exception());
            let exception = ctx.catch().into_object().unwrap();
            assert_eq!(exception.get::<_, String>("name").unwrap(), "RangeError");
            assert_eq!(
                exception.get::<_, String>("message").unwrap(),
                "native wrapper allocation limit exceeded"
            );
        });
        assert_eq!(engine.native_failure(), None);
        engine.trim_memory();
        engine.context.with(|ctx| {
            let value = quickjs_callback_boundary(&ctx, "batch interface return", || {
                quickjs_sequence_value(&ctx, handles, |handle| handle_to_quickjs(&ctx, handle))
            })
            .unwrap();
            assert_eq!(value.into_array().unwrap().len(), 5000);
        });
        assert_eq!(engine.native_failure(), None);
    }

    include!("world/css_pressure_tests.rs");
    include!("world/generic_materialization_tests.rs");
    include!("world/listener_value_lease_tests.rs");
    include!("world/standalone_event_target_tests.rs");
}
