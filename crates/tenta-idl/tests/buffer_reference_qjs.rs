//! The generated QuickJS binding retains exact native buffer identities.

#![cfg(feature = "quickjs")]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::{Class, Ctx, Function, Object, Persistent, Value};

#[allow(
    dead_code,
    unused_variables,
    clippy::wrong_self_convention,
    clippy::unnecessary_cast,
    clippy::single_match,
    clippy::unnecessary_map_on_constructor,
    clippy::clone_on_copy
)]
mod bindings {
    use rquickjs::FromJs;
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
    include!("golden/toy/buffer_reference_probe_ops.rs");
    include!("golden/toy/buffer_reference_probe_register.rs");
    include!("golden/toy/buffer_reference_probe_qjs.rs");
}
use bindings::*;

struct Frame {
    id: u64,
    lease_id: Option<u64>,
    bits: Vec<u64>,
}

#[derive(Default)]
struct World {
    values: RefCell<Vec<Option<Persistent<Value<'static>>>>>,
    free: RefCell<Vec<u64>>,
    frames: RefCell<Vec<Frame>>,
    next_frame: Cell<u64>,
    prototypes: RefCell<HashMap<&'static str, Persistent<Object<'static>>>>,
    intrinsics: RefCell<HashMap<&'static str, Persistent<Value<'static>>>>,
    failure: Cell<Option<&'static str>>,
}
impl World {
    fn live(&self) -> usize {
        self.values
            .borrow()
            .iter()
            .filter(|value| value.is_some())
            .count()
    }

    fn capture_intrinsics(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        for (name, source) in [
            (
                "DataView.buffer",
                "Object.getOwnPropertyDescriptor(DataView.prototype, 'buffer').get",
            ),
            (
                "DataView.byteOffset",
                "Object.getOwnPropertyDescriptor(DataView.prototype, 'byteOffset').get",
            ),
            (
                "DataView.byteLength",
                "Object.getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get",
            ),
            (
                "TypedArray.buffer",
                "Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), 'buffer').get",
            ),
        ] {
            let getter: Function = ctx.eval(source)?;
            self.intrinsics
                .borrow_mut()
                .insert(name, Persistent::save(ctx, getter.into_value()));
        }
        Ok(())
    }
}

struct Bridge(Rc<World>);
impl QuickJsWrapperWorld for Bridge {
    fn wrapper<'js>(&self, _: &Ctx<'js>, _: &'static str, _: u64) -> Option<Object<'js>> {
        None
    }
    fn handle(&self, _: &'static str, _: &Object<'_>) -> Option<u64> {
        None
    }
    fn begin_callback_roots(&self, _: &Ctx<'_>) -> rquickjs::Result<u64> {
        let id = self.0.next_frame.get() + 1;
        self.0.next_frame.set(id);
        self.0.frames.borrow_mut().push(Frame {
            id,
            lease_id: None,
            bits: Vec::new(),
        });
        Ok(id)
    }
    fn begin_stored_value_lease(&self, ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        if self.0.frames.borrow().is_empty() {
            return Err(rquickjs::Exception::throw_type(ctx, "no callback frame"));
        }
        Ok(())
    }
    fn begin_buffer_reference_lease(&self, ctx: &Ctx<'_>, lease_id: u64) -> rquickjs::Result<()> {
        let mut frames = self.0.frames.borrow_mut();
        let Some(frame) = frames.last_mut() else {
            return Err(rquickjs::Exception::throw_type(ctx, "no callback frame"));
        };
        if frame.lease_id.is_some() {
            return Err(rquickjs::Exception::throw_type(
                ctx,
                "duplicate buffer lease",
            ));
        }
        frame.lease_id = Some(lease_id);
        Ok(())
    }
    fn current_buffer_reference_lease(&self) -> Option<u64> {
        self.0
            .frames
            .borrow()
            .last()
            .and_then(|frame| frame.lease_id)
    }
    fn buffer_reference_lease_active(&self, lease_id: u64) -> bool {
        self.0
            .frames
            .borrow()
            .iter()
            .any(|frame| frame.lease_id == Some(lease_id))
    }
    fn track_stored_value(&self, _: &Ctx<'_>, bits: u64) -> rquickjs::Result<()> {
        self.0
            .frames
            .borrow_mut()
            .last_mut()
            .expect("stored value has a callback frame")
            .bits
            .push(bits);
        Ok(())
    }
    fn transfer_stored_value_lease(&self, _: &Ctx<'_>) -> rquickjs::Result<()> {
        panic!("temporary buffer roots are never transferred")
    }
    fn before_constructor_host(&self, _: &Ctx<'_>) -> rquickjs::Result<()> {
        Ok(())
    }
    fn retain_callback_argument<'js>(&self, _: &Ctx<'js>, _: Value<'js>) -> rquickjs::Result<()> {
        Ok(())
    }
    fn end_callback_roots(&self, id: u64) {
        let frame = self.0.frames.borrow_mut().pop().expect("frame ends once");
        assert_eq!(frame.id, id);
        for bits in frame.bits {
            let root = self.0.values.borrow_mut()[bits as usize].take();
            assert!(root.is_some(), "one temporary root is released once");
            self.0.free.borrow_mut().push(bits);
            drop(root);
        }
    }
    fn store_value(&self, _ctx: &Ctx<'_>, value: Value<'_>) -> u64 {
        let value_ctx = value.ctx().clone();
        let root = Persistent::save(&value_ctx, value);
        if let Some(bits) = self.0.free.borrow_mut().pop() {
            self.0.values.borrow_mut()[bits as usize] = Some(root);
            bits
        } else {
            let mut values = self.0.values.borrow_mut();
            let bits = values.len() as u64;
            values.push(Some(root));
            bits
        }
    }
    fn load_value<'js>(&self, ctx: &Ctx<'js>, bits: u64) -> Option<Value<'js>> {
        self.0
            .values
            .borrow()
            .get(bits as usize)?
            .as_ref()?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn realm(&self) -> Realm {
        Rc::new(())
    }
    fn adopt<'js>(&self, _: &Ctx<'js>, _: &'static str, _: Box<dyn Any>) -> Option<Object<'js>> {
        None
    }
    fn prototype<'js>(&self, ctx: &Ctx<'js>, interface: &'static str) -> Option<Object<'js>> {
        self.0
            .prototypes
            .borrow()
            .get(interface)?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn cache_prototype<'js>(
        &self,
        ctx: &Ctx<'js>,
        interface: &'static str,
        prototype: Object<'js>,
        _: &'static [&'static str],
    ) {
        self.0
            .prototypes
            .borrow_mut()
            .insert(interface, Persistent::save(ctx, prototype));
    }
    fn intrinsic<'js>(&self, ctx: &Ctx<'js>, name: &str) -> Option<Value<'js>> {
        self.0
            .intrinsics
            .borrow()
            .get(name)?
            .clone()
            .restore(ctx)
            .ok()
    }
    fn cache_legacy(&self, _: &'static str, _: QuickJsLegacyMethods) {}
    fn native_failure(&self) -> Option<&'static str> {
        self.0.failure.get()
    }
    fn script_interrupted(&self) -> bool {
        false
    }
    fn record_native_failure(&self, name: &'static str) {
        self.0.failure.set(Some(name));
    }
    fn native_host(&self, _: &'static str, _: &Value<'_>) -> Option<Rc<dyn Any>> {
        None
    }
}

#[derive(Default)]
struct ProbeState {
    old: Cell<Option<BufferSourceRef>>,
    return_old: Cell<bool>,
}
struct Host(Rc<ProbeState>);
impl BufferReferenceProbeOps for Host {
    fn accept_source(&self, _: BufferSourceRef) -> Result<(), JsError> {
        Ok(())
    }
    fn accept_then_string(&self, _: BufferSourceRef, _: &str) -> Result<(), JsError> {
        panic!("host must not run after an abrupt argument conversion")
    }
    fn echo_view(&self, value: ArrayBufferViewRef) -> Result<ArrayBufferViewRef, JsError> {
        Ok(value)
    }
    fn echo_source(&self, value: BufferSourceRef) -> Result<BufferSourceRef, JsError> {
        if self.0.return_old.get() {
            Ok(self.0.old.get().expect("prior token"))
        } else {
            self.0.old.set(Some(value));
            Ok(value)
        }
    }
}
struct Registry;
impl InterfaceRegistry for Registry {
    fn register(&mut self, _: InterfaceDescriptor) {}
}

fn install_probe(ctx: &Ctx<'_>, world: &Rc<World>, state: Rc<ProbeState>) {
    install_buffer_reference_probe::<Host>(ctx, &ctx.globals(), &mut Registry)
        .expect("install generated toy binding");
    let object = Class::instance(
        ctx.clone(),
        QuickJsHost {
            host: Some(Rc::new(Host(state)) as Rc<dyn Any>),
            handle: 1,
            generation: 1,
            native_failure: Rc::new(Cell::new(None)),
            interfaces: &["BufferReferenceProbe"],
            dead: Rc::new(Cell::new(Vec::new())),
        },
    )
    .expect("native host")
    .into_inner();
    let prototype = world
        .prototypes
        .borrow()
        .get("BufferReferenceProbe")
        .expect("generated prototype")
        .clone()
        .restore(ctx)
        .expect("restore prototype");
    object
        .set_prototype(Some(&prototype))
        .expect("link generated prototype");
    ctx.globals().set("probe", object).expect("publish host");
}

#[test]
fn generated_buffer_references_preserve_identity_and_reject_stale_leases() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    let state = Rc::new(ProbeState::default());
    context.with(|ctx| {
        let world = Rc::new(World::default());
        ctx.store_userdata(QuickJsWorldSlot(Box::new(Bridge(world.clone()))))
            .unwrap_or_else(|_| panic!("install world"));
        world.capture_intrinsics(&ctx).expect("capture trusted accessors");
        assert!(
            quickjs_begin_buffer_reference_lease(&ctx).is_err(),
            "a direct embedder call has no callback frame",
        );
        let _ = ctx.catch();
        install_probe(&ctx, &world, state.clone());

        assert!(ctx.eval::<bool, _>("(() => { const b = new ArrayBuffer(8); const v = new Uint8Array(b, 2, 3); return probe.echoView(v) === v && probe.echoSource(b) === b && probe.echoSource(v) === v; })()").expect("native identities"));
        assert_eq!(world.live(), 0, "temporary roots end after each call");
        assert!(ctx.eval::<bool, _>("probe.acceptSource(new ArrayBuffer(2)) === undefined").expect("undefined return keeps no value root"));
        assert_eq!(world.live(), 0, "undefined return does not transfer its lease");
        assert!(ctx.eval::<bool, _>("(() => { const marker = {}; try { probe.acceptThenString(new ArrayBuffer(2), {toString() {throw marker}}); return false; } catch (error) {return error === marker;} })()").expect("the original page exception propagates"));
        assert_eq!(world.live(), 0, "abrupt later argument releases the earlier buffer root");
        assert!(ctx.eval::<bool, _>("(() => { const d = new DataView(new ArrayBuffer(6), 1, 2); return probe.echoView(d) === d; })()").expect("DataView identity"));
        assert!(ctx.eval::<bool, _>("(() => { try { probe.echoView({}); return false; } catch (e) { return e instanceof TypeError; } })()").expect("forged brand"));
        assert_eq!(world.live(), 0);
        assert!(ctx.eval::<bool, _>("(() => { if (typeof SharedArrayBuffer !== 'function') return true; const view = new Uint8Array(new SharedArrayBuffer(4)); try { probe.echoView(view); return false; } catch (e) { return e instanceof TypeError; } })()").expect("shared rejection"));
        assert!(ctx.eval::<bool, _>("(() => { const b = new ArrayBuffer(4, {maxByteLength: 8}); if (!b.resizable) return true; try { probe.echoSource(b); return false; } catch (e) { return e instanceof TypeError; } })()").expect("resizable rejection"));
        assert_eq!(world.live(), 0, "rejected input keeps no root");
        let mut detached: rquickjs::ArrayBuffer = ctx
            .eval("globalThis.toDetach = new ArrayBuffer(4); toDetach")
            .expect("buffer to detach");
        detached.detach();
        assert!(ctx.eval::<bool, _>("probe.echoSource(toDetach) === toDetach").expect("detached fixed source"));
        assert_eq!(world.live(), 0);
        let view: Value = ctx.eval("globalThis.nativeView = new DataView(new Uint8Array([1,2,3,4,5,6]).buffer, 2, 3); nativeView").expect("view");
        quickjs_callback_boundary(&ctx, "inspect_view", || {
            quickjs_begin_buffer_reference_lease(&ctx)?;
            let source = buffer_source_ref_from_quickjs(&ctx, view.clone())?;
            let target = array_buffer_view_ref_from_quickjs(&ctx, view.clone())?;
            quickjs_callback_boundary(&ctx, "nested buffer reference", || {
                quickjs_begin_buffer_reference_lease(&ctx)?;
                let inner = buffer_source_ref_from_quickjs(&ctx, view.clone())?;
                assert_eq!(quickjs_buffer_source_info(&ctx, inner)?.byte_length, 3);
                Ok(())
            })?;
            let info = quickjs_array_buffer_view_info(&ctx, target)?;
            let mut bytes = vec![0; info.byte_length];
            assert_eq!(quickjs_copy_buffer_source_into(&ctx, source, &mut bytes)?, info);
            assert_eq!(bytes, [3, 4, 5]);
            assert_eq!(quickjs_write_array_buffer_view(&ctx, target, &[9, 8, 7])?, info);
            assert_eq!((info.kind, info.byte_offset, info.byte_length), (BufferKind::DataView, 2, 3));
            Ok(())
        }).expect("native metadata and byte bridge");
        assert!(ctx.eval::<bool, _>("String(new Uint8Array(nativeView.buffer)) === '1,2,9,8,7,6'").expect("written backing bytes"));
        assert_eq!(world.live(), 0, "native byte bridge releases both roots");
        let has_immutable: bool = ctx
            .eval("typeof ArrayBuffer.prototype.transferToImmutable === 'function'")
            .expect("immutable capability");
        if has_immutable {
            let immutable: Value = ctx
                .eval("new DataView(new Uint8Array([3,4,5]).buffer.transferToImmutable())")
                .expect("immutable native view");
            let result = quickjs_callback_boundary(&ctx, "immutable_write", || {
                quickjs_begin_buffer_reference_lease(&ctx)?;
                let target = array_buffer_view_ref_from_quickjs(&ctx, immutable.clone())?;
                assert_eq!(quickjs_array_buffer_view_info(&ctx, target)?.byte_length, 3);
                quickjs_write_array_buffer_view(&ctx, target, &[9, 8, 7])?;
                Ok(())
            });
            assert!(result.is_err(), "native write refuses immutable backing");
            if result.as_ref().is_err_and(rquickjs::Error::is_exception) {
                let _ = ctx.catch();
            }
            assert_eq!(world.live(), 0);
        }
        state.return_old.set(true);
        assert!(ctx.eval::<bool, _>("(() => { try { probe.echoSource(new Uint8Array(2)); return false; } catch (e) { return e instanceof TypeError; } })()").expect("stale lease"));
        assert_eq!(world.live(), 0, "recycled index does not revive old token");
        state.return_old.set(false);
        assert!(ctx.eval::<bool, _>("(() => { const b = new ArrayBuffer(1); return probe.echoSource(b) === b; })()").expect("healthy subsequent call"));
        assert_eq!(world.live(), 0);
        assert_eq!(world.failure.get(), None);
        world.prototypes.borrow_mut().clear();
        world.intrinsics.borrow_mut().clear();
        assert_eq!(world.live(), 0);
        drop(
            ctx.remove_userdata::<QuickJsWorldSlot>()
                .unwrap_or_else(|_| panic!("remove world userdata")),
        );
    });
    drop(context);
    drop(runtime);

    let other_runtime = rquickjs::Runtime::new().expect("second runtime");
    let other_context = rquickjs::Context::full(&other_runtime).expect("second context");
    other_context.with(|ctx| {
        let world = Rc::new(World::default());
        ctx.store_userdata(QuickJsWorldSlot(Box::new(Bridge(world.clone()))))
            .unwrap_or_else(|_| panic!("install second world"));
        world.capture_intrinsics(&ctx).expect("capture second-world accessors");
        state.return_old.set(true);
        install_probe(&ctx, &world, state.clone());
        assert!(ctx.eval::<bool, _>("(() => { try { probe.echoSource(new ArrayBuffer(1)); return false; } catch (e) { return e instanceof TypeError; } })()")
            .expect("a first-world token cannot resolve in a second world"));
        assert_eq!(world.live(), 0, "second-world root is released after refusal");
        world.prototypes.borrow_mut().clear();
        world.intrinsics.borrow_mut().clear();
        drop(
            ctx.remove_userdata::<QuickJsWorldSlot>()
                .unwrap_or_else(|_| panic!("remove second world userdata")),
        );
    });
}

#[test]
fn pending_quickjs_error_marker_keeps_the_original_thrown_object() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let marker: Value = ctx
            .eval("globalThis.marker = { unique: 73 }; marker")
            .expect("page marker");
        let _ = ctx.throw(marker);
        let converted = throw_quickjs_error(&ctx, &pending_js_exception_error());
        assert!(matches!(converted, rquickjs::Error::Exception));
        let original = ctx.catch();
        ctx.globals()
            .set("caught", original)
            .expect("observe exact exception");
        assert!(
            ctx.eval::<bool, _>("caught === marker")
                .expect("same page object")
        );
    });
}
