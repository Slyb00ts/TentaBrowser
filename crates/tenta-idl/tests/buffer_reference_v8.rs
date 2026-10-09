//! The generated annotation retains native buffer identity for one callback.

#![cfg(feature = "v8")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
    include!("golden/toy/prelude.rs");
    include!("golden/toy/types.rs");
    include!("golden/toy/buffer_reference_probe_ops.rs");
    include!("golden/toy/buffer_reference_probe_register.rs");
    include!("golden/toy/buffer_reference_probe_v8.rs");
}
use bindings::*;

#[derive(Default)]
struct Roots {
    values: RefCell<Vec<Option<v8::Global<v8::Value>>>>,
    free: RefCell<Vec<u64>>,
    resizable_getter: RefCell<Option<v8::Global<v8::Function>>>,
    immutable_getter: RefCell<Option<v8::Global<v8::Function>>>,
}

impl Roots {
    fn live(&self) -> usize {
        self.values
            .borrow()
            .iter()
            .filter(|value| value.is_some())
            .count()
    }

    fn release(&self, bits: u64) {
        let root = self.values.borrow_mut()[bits as usize].take();
        assert!(root.is_some(), "one leased reference is released once");
        self.free.borrow_mut().push(bits);
        drop(root);
    }
}

struct World(Rc<Roots>);
impl WrapperWorld for World {
    fn wrapper<'s>(
        &self,
        _: &mut v8::PinScope<'s, '_>,
        _: &'static str,
        _: u64,
    ) -> Result<Option<v8::Local<'s, v8::Object>>, JsError> {
        Ok(None)
    }

    fn handle(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
    ) -> Option<u64> {
        None
    }

    fn store_value(&self, scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> u64 {
        let root = v8::Global::new(scope, value);
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

    fn load_value<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        bits: u64,
    ) -> Option<v8::Local<'s, v8::Value>> {
        self.0
            .values
            .borrow()
            .get(bits as usize)
            .and_then(|root| root.as_ref().map(|root| v8::Local::new(scope, root)))
    }

    fn array_buffer_resizable<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        let getter = {
            let saved = self.0.resizable_getter.borrow();
            v8::Local::new(
                scope,
                saved.as_ref().expect("getter installed before page script"),
            )
        };
        let value = call_js_function(scope, getter, buffer.into(), &[])?;
        Ok(value.boolean_value(scope))
    }

    fn array_buffer_immutable<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        buffer: v8::Local<'s, v8::ArrayBuffer>,
    ) -> Result<bool, JsError> {
        let getter = {
            let saved = self.0.immutable_getter.borrow();
            saved.as_ref().map(|getter| v8::Local::new(scope, getter))
        };
        let Some(getter) = getter else {
            return Ok(false);
        };
        let value = call_js_function(scope, getter, buffer.into(), &[])?;
        Ok(value.boolean_value(scope))
    }

    fn realm(&self) -> Realm {
        Rc::new(())
    }

    fn adopt(
        &self,
        _: &mut v8::PinScope<'_, '_>,
        _: &'static str,
        _: v8::Local<v8::Object>,
        _: Box<dyn std::any::Any>,
    ) -> Result<(), JsError> {
        Ok(())
    }
}

struct Lease {
    depth: usize,
    id: Option<u64>,
    bits: Vec<u64>,
}
struct Admission {
    roots: Rc<Roots>,
    depth: Cell<usize>,
    leases: RefCell<Vec<Lease>>,
    stop_on_second_chunk: Cell<bool>,
    native_chunks: Cell<usize>,
    pending_native_stop: Cell<bool>,
}
impl V8CallbackAdmission for Admission {
    fn enter(&self, _: &mut v8::PinScope<'_, '_>) -> Result<(), V8AdmissionError> {
        self.depth.set(self.depth.get() + 1);
        Ok(())
    }
    fn before_constructor_host(
        &self,
        _: &mut v8::PinScope<'_, '_>,
    ) -> Result<(), V8AdmissionError> {
        Ok(())
    }
    fn begin_stored_value_lease(&self) {
        self.leases.borrow_mut().push(Lease {
            depth: self.depth.get(),
            id: None,
            bits: Vec::new(),
        });
    }
    fn begin_checked_stored_value_lease(&self, id: u64) -> bool {
        if self.depth.get() == 0 {
            return false;
        }
        self.leases.borrow_mut().push(Lease {
            depth: self.depth.get(),
            id: Some(id),
            bits: Vec::new(),
        });
        true
    }
    fn current_buffer_reference_lease(&self) -> Option<u64> {
        self.leases
            .borrow()
            .last()
            .filter(|lease| lease.depth == self.depth.get())
            .and_then(|lease| lease.id)
    }
    fn buffer_reference_lease_active(&self, id: u64) -> bool {
        self.leases
            .borrow()
            .iter()
            .any(|lease| lease.depth <= self.depth.get() && lease.id == Some(id))
    }
    fn poll_native_buffer_work_termination(&self) -> bool {
        if !self.stop_on_second_chunk.get() {
            return false;
        }
        let count = self.native_chunks.get() + 1;
        self.native_chunks.set(count);
        if count == 2 {
            self.pending_native_stop.set(true);
        }
        self.pending_native_stop.get()
    }
    fn take_native_buffer_work_termination(&self) -> bool {
        let pending = self.pending_native_stop.get();
        if self.depth.get() == 0 {
            self.pending_native_stop.set(false);
        }
        pending
    }
    fn track_stored_value(&self, bits: u64) {
        self.leases
            .borrow_mut()
            .last_mut()
            .expect("stored value has a lease")
            .bits
            .push(bits);
    }
    fn transfer_stored_value_lease(&self) {
        panic!("a buffer lease never transfers its temporary roots");
    }
    fn end_stored_value_lease(&self, _: &v8::Isolate) {
        let lease = self.leases.borrow_mut().pop().expect("lease ends once");
        for bits in lease.bits {
            self.roots.release(bits);
        }
    }
    fn exit(&self) {
        self.depth.set(self.depth.get() - 1);
    }
}

#[derive(Default)]
struct ProbeState {
    old: Cell<Option<BufferSourceRef>>,
    return_old: Cell<bool>,
}
#[repr(align(8))]
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

thread_local! {
    static INSPECTED: RefCell<Option<(BufferMetadata, Vec<u8>)>> = const { RefCell::new(None) };
}

fn inspect_view<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        if let Err(error) = v8_checked_begin_stored_value_lease(scope) {
            throw_js_error(scope, &error);
            return;
        }
        let result = (|| {
            let value = args.get(0);
            let source = buffer_source_ref_from_v8(scope, value)?;
            let view = array_buffer_view_ref_from_v8(scope, value)?;
            let inner = |scope: &mut v8::PinScope<'s, '_>| {
                v8_checked_begin_stored_value_lease(scope).expect("inner frame lease");
                let result = buffer_source_ref_from_v8(scope, value)
                    .and_then(|reference| v8_buffer_source_info(scope, reference));
                v8_end_stored_value_lease(scope);
                result.is_ok()
            };
            assert!(v8_callback_boundary(
                scope,
                "nested buffer reference",
                false,
                inner
            ));
            let info = v8_array_buffer_view_info(scope, view)?;
            let mut bytes = vec![0; info.byte_length];
            let copied = v8_copy_buffer_source_into(scope, source, &mut bytes)?;
            assert_eq!(copied, info);
            let written = v8_write_array_buffer_view(scope, view, &[9, 8, 7])?;
            assert_eq!(written, info);
            INSPECTED.with(|slot| *slot.borrow_mut() = Some((info, bytes)));
            Ok::<(), JsError>(())
        })();
        v8_end_stored_value_lease(scope);
        match result {
            Ok(()) => rv.set_bool(true),
            Err(error) => throw_js_error(scope, &error),
        }
    };
    // SAFETY: this FunctionTemplate callback is registered as a slow V8 callback.
    unsafe { v8_registered_slow_callback_boundary(scope, "inspect_view", (), call) };
}

fn eval(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
    let source = v8::String::new(scope, source).expect("source");
    v8::Script::compile(scope, source, None)
        .expect("compile")
        .run(scope)
        .expect("evaluate")
        .to_rust_string_lossy(scope)
}

#[test]
fn generated_buffer_references_preserve_identity_and_reject_stale_leases() {
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();
    let roots = Rc::new(Roots::default());
    let state = Rc::new(ProbeState::default());
    let mut isolate = v8::Isolate::new(Default::default());
    let admission = Rc::new(Admission {
        roots: roots.clone(),
        depth: Cell::new(0),
        leases: RefCell::new(Vec::new()),
        stop_on_second_chunk: Cell::new(false),
        native_chunks: Cell::new(0),
        pending_native_stop: Cell::new(false),
    });
    install_v8_callback_admission(&mut isolate, admission.clone());
    {
        v8::scope!(let handle_scope, &mut isolate);
        let bootstrap = v8::Context::new(handle_scope, Default::default());
        let mut bootstrap_scope = v8::ContextScope::new(handle_scope, bootstrap);
        v8::scope!(let scope, &mut bootstrap_scope);
        let global_template = v8::ObjectTemplate::new(scope);
        install_buffer_reference_probe::<Host>(scope, global_template, &mut Registry)
            .expect("install generated toy binding");
        let inspect_key = v8::String::new(scope, "inspectView").expect("inspect key");
        let inspect = v8::FunctionTemplate::builder(inspect_view).build(scope);
        global_template.set(inspect_key.into(), inspect.into());
        set_wrapper_world(scope, Box::new(World(roots.clone()))).expect("world");
        assert!(
            v8_checked_begin_stored_value_lease(scope).is_err(),
            "a direct embedder call cannot mint a callback token",
        );
        let context = v8::Context::new(
            scope,
            v8::ContextOptions {
                global_template: Some(global_template),
                ..Default::default()
            },
        );
        let mut context_scope = v8::ContextScope::new(scope, context);
        v8::scope!(let scope, &mut context_scope);
        let getter_source = v8::String::new(
            scope,
            "Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable').get",
        )
        .expect("getter source");
        let getter = v8::Script::compile(scope, getter_source, None)
            .expect("compile getter")
            .run(scope)
            .expect("run getter");
        let getter = v8::Local::<v8::Function>::try_from(getter).expect("native getter");
        *roots.resizable_getter.borrow_mut() = Some(v8::Global::new(scope, getter));
        let immutable_source = v8::String::new(
            scope,
            "Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'immutable')?.get",
        )
        .expect("optional immutable getter source");
        let immutable = v8::Script::compile(scope, immutable_source, None)
            .expect("compile immutable getter")
            .run(scope)
            .expect("run immutable getter");
        if !immutable.is_undefined() {
            let getter =
                v8::Local::<v8::Function>::try_from(immutable).expect("native immutable getter");
            *roots.immutable_getter.borrow_mut() = Some(v8::Global::new(scope, getter));
        }
        let template = cached_template(scope, "BufferReferenceProbe").expect("template");
        let object = template
            .instance_template(scope)
            .new_instance(scope)
            .expect("instance");
        attach_host(&object, Host(state.clone()));
        let global = context.global(scope);
        let key = v8::String::new(scope, "probe").expect("key");
        global.set(scope, key.into(), object.into());

        assert_eq!(
            eval(
                scope,
                "(() => { const b = new ArrayBuffer(8); const v = new Uint8Array(b, 2, 3); return probe.echoView(v) === v && probe.echoSource(b) === b && probe.echoSource(v) === v; })()"
            ),
            "true",
        );
        assert_eq!(roots.live(), 0, "temporary roots end after each call");
        assert_eq!(
            eval(
                scope,
                "probe.acceptSource(new ArrayBuffer(2)) === undefined"
            ),
            "true",
        );
        assert_eq!(roots.live(), 0, "undefined return keeps no root");
        assert_eq!(
            eval(
                scope,
                "(() => { const marker = {}; try { probe.acceptThenString(new ArrayBuffer(2), {toString() {throw marker}}); return false; } catch (error) {return error === marker;} })()"
            ),
            "true",
        );
        assert_eq!(
            roots.live(),
            0,
            "abrupt later argument releases the earlier buffer root"
        );
        assert_eq!(
            eval(
                scope,
                "(() => { const d = new DataView(new ArrayBuffer(6), 1, 2); return probe.echoView(d) === d; })()"
            ),
            "true",
        );
        assert_eq!(roots.live(), 0);
        assert_eq!(
            eval(
                scope,
                "(() => { try { probe.echoView({}); return false; } catch (e) { return e instanceof TypeError; } })()"
            ),
            "true",
        );
        assert_eq!(roots.live(), 0, "failed brand conversion releases roots");
        assert_eq!(
            eval(
                scope,
                "(() => { if (typeof SharedArrayBuffer !== 'function') return true; const view = new Uint8Array(new SharedArrayBuffer(4)); try { probe.echoView(view); return false; } catch (e) { return e instanceof TypeError; } })()"
            ),
            "true",
        );
        assert_eq!(
            eval(
                scope,
                "(() => { const b = new ArrayBuffer(4, {maxByteLength: 8}); if (!b.resizable) return true; try { probe.echoSource(b); return false; } catch (e) { return e instanceof TypeError; } })()"
            ),
            "true",
        );
        assert_eq!(
            roots.live(),
            0,
            "rejected shared or resizable input keeps no root"
        );
        assert_eq!(
            eval(scope, "globalThis.toDetach = new ArrayBuffer(4); true"),
            "true"
        );
        let key = v8::String::new(scope, "toDetach").expect("key");
        let buffer = global.get(scope, key.into()).expect("buffer");
        let buffer = v8::Local::<v8::ArrayBuffer>::try_from(buffer).expect("ArrayBuffer");
        assert_eq!(buffer.detach(None), Some(true));
        assert_eq!(
            eval(scope, "probe.echoSource(toDetach) === toDetach"),
            "true"
        );
        assert_eq!(roots.live(), 0, "detached fixed buffers have a zero span");
        assert_eq!(
            eval(
                scope,
                "(() => { const bytes = new Uint8Array([1,2,3,4,5,6]); const view = new DataView(bytes.buffer, 2, 3); return inspectView(view) && String(bytes) === '1,2,9,8,7,6'; })()"
            ),
            "true",
        );
        INSPECTED.with(|slot| {
            let (info, bytes) = slot.borrow_mut().take().expect("native bridge ran");
            assert_eq!(info.kind, BufferKind::DataView);
            assert_eq!((info.byte_offset, info.byte_length), (2, 3));
            assert_eq!(bytes, [3, 4, 5]);
        });
        assert_eq!(
            roots.live(),
            0,
            "metadata/copy/write roots end after callback"
        );
        assert_eq!(
            eval(
                scope,
                "(() => { if (typeof ArrayBuffer.prototype.transferToImmutable !== 'function') return true; const bytes = new Uint8Array([3,4,5]).buffer.transferToImmutable(); try { inspectView(new DataView(bytes)); return false; } catch (e) { return e instanceof TypeError; } })()"
            ),
            "true",
            "a native write must refuse an immutable backing",
        );
        assert_eq!(roots.live(), 0);
        state.return_old.set(true);
        assert_eq!(
            eval(
                scope,
                "(() => { try { probe.echoSource(new Uint8Array(2)); return false; } catch (e) { return e instanceof TypeError; } })()"
            ),
            "true",
        );
        assert_eq!(roots.live(), 0, "reused slot cannot revive an old lease");
        state.return_old.set(false);
        assert_eq!(
            eval(
                scope,
                "(() => { const b = new ArrayBuffer(1); return probe.echoSource(b) === b; })()"
            ),
            "true",
            "a later callback is healthy after catchable stale-token rejection",
        );
        assert_eq!(roots.live(), 0);

        // The test admission requests a hard stop on the second native copy
        // chunk, after metadata and the first 64 KiB have already been read.
        // This makes the callback-tail oracle deterministic without sleeps.
        assert_eq!(
            eval(
                scope,
                "globalThis.nativeProgress = {after:false,caught:false,finalized:false}; true"
            ),
            "true",
        );
        let progress_key = v8::String::new(scope, "nativeProgress").expect("key");
        let progress = global.get(scope, progress_key.into()).expect("progress");
        let progress = v8::Local::<v8::Object>::try_from(progress).expect("plain object");
        let progress = v8::Global::new(scope, progress);
        admission.stop_on_second_chunk.set(true);
        let source = v8::String::new(scope,
            "try { inspectView(new Uint8Array(131072)); nativeProgress.after = true; } catch (_) { nativeProgress.caught = true; } finally { nativeProgress.finalized = true; }"
        ).expect("source");
        let script = v8::Script::compile(scope, source, None).expect("compile");
        {
            v8::tc_scope!(let tc, scope);
            assert!(script.run(tc).is_none(), "hard stop discards the task");
            assert!(
                tc.has_terminated(),
                "hard stop cannot enter page catch/finally"
            );
            assert_eq!(
                admission.native_chunks.get(),
                2,
                "later chunks were not copied"
            );
            assert_eq!(roots.live(), 0, "the lease was released before termination");
            let progress = v8::Local::new(tc, &progress);
            for field in ["after", "caught", "finalized"] {
                let key = v8::String::new(tc, field).expect("field");
                let value = progress.get(tc, key.into()).expect("plain own data");
                assert!(value.is_false(), "page ran {field} after native hard stop");
            }
        }
        scope.cancel_terminate_execution();
        admission.stop_on_second_chunk.set(false);
        assert_eq!(
            eval(
                scope,
                "(() => { const b = new ArrayBuffer(1); return probe.echoSource(b) === b; })()"
            ),
            "true",
            "a later task is healthy after the synthetic watchdog resets",
        );
        assert_eq!(
            roots.live(),
            0,
            "the later task releases its temporary root"
        );
        // SAFETY: the sole instance was attached above and no finalizer owns it.
        drop(unsafe { detach_host::<Host>(&object) });
        drop(scope.remove_slot::<TemplateCache>());
        drop(roots.resizable_getter.borrow_mut().take());
        drop(roots.immutable_getter.borrow_mut().take());
    }
    drop(isolate);
}
