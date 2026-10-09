use super::{DefinePropertyDescriptor, JsClass, Tracer};
use crate::{
    class::JsCell, function::Params, qjs, runtime::opaque::Opaque, Atom, Ctx, Object, Value,
};
use alloc::boxed::Box;
use core::{any::TypeId, mem, panic::AssertUnwindSafe, ptr::NonNull};

/// FFI finalizer, destroying the object once it is delete by the Gc.
pub(crate) unsafe extern "C" fn class_finalizer(rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
    let class_id = Opaque::from_runtime_ptr(rt).get_class_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    (ptr.as_ref().v_table.finalizer)(ptr);
}

/// FFI finalizer, destroying the object once it is delete by the Gc.
pub(crate) unsafe extern "C" fn exotic_class_finalizer(rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
    let class_id = Opaque::from_runtime_ptr(rt).get_exotic_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    (ptr.as_ref().v_table.finalizer)(ptr);
}

/// FFI tracing function for non callable classes.
pub(crate) unsafe extern "C" fn class_trace(
    rt: *mut qjs::JSRuntime,
    val: qjs::JSValue,
    mark_func: qjs::JS_MarkFunc,
) {
    let class_id = Opaque::from_runtime_ptr(rt).get_class_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    let tracer = Tracer::from_ffi(rt, mark_func);
    (ptr.as_ref().v_table.trace)(ptr, tracer)
}

/// FFI tracing function for non callable exotic classes.
pub(crate) unsafe extern "C" fn exotic_class_trace(
    rt: *mut qjs::JSRuntime,
    val: qjs::JSValue,
    mark_func: qjs::JS_MarkFunc,
) {
    let class_id = Opaque::from_runtime_ptr(rt).get_exotic_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    let tracer = Tracer::from_ffi(rt, mark_func);
    (ptr.as_ref().v_table.trace)(ptr, tracer)
}

/// FFI finalizer, destroying the object once it is delete by the Gc.
pub(crate) unsafe extern "C" fn callable_finalizer(rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
    let class_id = Opaque::from_runtime_ptr(rt).get_callable_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    (ptr.as_ref().v_table.finalizer)(ptr)
}

/// FFI tracing function for classes of type callable.
pub(crate) unsafe extern "C" fn callable_trace(
    rt: *mut qjs::JSRuntime,
    val: qjs::JSValue,
    mark_func: qjs::JS_MarkFunc,
) {
    let class_id = Opaque::from_runtime_ptr(rt).get_callable_id();
    let ptr = qjs::JS_GetOpaque(val, class_id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    let tracer = Tracer::from_ffi(rt, mark_func);
    (ptr.as_ref().v_table.trace)(ptr, tracer)
}

/// FFI calling function.
pub(crate) unsafe extern "C" fn call(
    ctx: *mut qjs::JSContext,
    function: qjs::JSValue,
    this: qjs::JSValue,
    argc: qjs::c_int,
    argv: *mut qjs::JSValue,
    flags: qjs::c_int,
) -> qjs::JSValue {
    let rt = qjs::JS_GetRuntime(ctx);
    let id = Opaque::from_runtime_ptr(rt).get_callable_id();
    let ptr = qjs::JS_GetOpaque(function, id);
    let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
    (ptr.as_ref().v_table.call)(ptr, ctx, function, this, argc, argv, flags)
}

/// FFI exotic get_property function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_get_property(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
    receiver: qjs::JSValueConst,
) -> qjs::JSValue {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.get_property)(ptr, ctx, obj, atom, receiver)
    }))
}

/// FFI exotic set_property function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_set_property(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
    value: qjs::JSValue,
    receiver: qjs::JSValueConst,
    flags: qjs::c_int,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.set_property)(ptr, ctx, obj, atom, receiver, value, flags)
    }))
}

/// FFI exotic has_property function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_has_property(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.has_property)(ptr, ctx, obj, atom)
    }))
}

/// FFI exotic delete_property function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_delete_property(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.delete_property)(ptr, ctx, obj, prop)
    }))
}

/// FFI exotic get_own_property function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_get_own_property(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.get_own_property)(ptr, ctx, desc, obj, prop)
    }))
}

/// FFI exotic get_own_property_names function for classes with exotic behavior.
pub(crate) unsafe extern "C" fn exotic_get_own_property_names(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValueConst,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let rt = qjs::JS_GetRuntime(ctx);
        let id = Opaque::from_runtime_ptr(rt).get_exotic_id();
        let ptr = qjs::JS_GetOpaque(obj, id);
        let ptr = NonNull::new(ptr).unwrap().cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.get_own_property_names)(ptr, ctx, ptab, plen, obj)
    }))
}

/// The full new native boundary, including the class lookup, is guarded.
pub(crate) unsafe extern "C" fn exotic_define_own_property(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
    value: qjs::JSValueConst,
    getter: qjs::JSValueConst,
    setter: qjs::JSValueConst,
    flags: qjs::c_int,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let id = Opaque::from_runtime_ptr(qjs::JS_GetRuntime(ctx)).get_exotic_id();
        let ptr = NonNull::new(qjs::JS_GetOpaque(obj, id))
            .unwrap()
            .cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.define_own_property)(
            ptr, ctx, obj, prop, value, getter, setter, flags,
        )
    }))
}

/// The full new native boundary, including the class lookup, is guarded.
pub(crate) unsafe extern "C" fn exotic_can_prevent_extensions(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
) -> qjs::c_int {
    let context = Ctx::from_ptr(ctx);
    context.handle_panic_exotic(AssertUnwindSafe(|| {
        let id = Opaque::from_runtime_ptr(qjs::JS_GetRuntime(ctx)).get_exotic_id();
        let ptr = NonNull::new(qjs::JS_GetOpaque(obj, id))
            .unwrap()
            .cast::<ClassCell<()>>();
        (ptr.as_ref().v_table.can_prevent_extensions)(ptr, ctx, obj)
    }))
}

pub(crate) type FinalizerFunc = unsafe fn(this: NonNull<ClassCell<()>>);
pub(crate) type TraceFunc =
    for<'a> unsafe fn(this: NonNull<ClassCell<()>>, tracer: Tracer<'a, 'static>);
pub(crate) type CallFunc = for<'a> unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    function: qjs::JSValue,
    this: qjs::JSValue,
    argc: qjs::c_int,
    argv: *mut qjs::JSValue,
    flags: qjs::c_int,
) -> qjs::JSValue;

pub(crate) type GetPropertyFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
    receiver: qjs::JSValueConst,
) -> qjs::JSValue;

pub(crate) type SetPropertyFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
    receiver: qjs::JSValueConst,
    value: qjs::JSValue,
    flags: qjs::c_int,
) -> qjs::c_int;

pub(crate) type HasPropertyFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    atom: qjs::JSAtom,
) -> qjs::c_int;

pub(crate) type DeletePropertyFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
) -> qjs::c_int;

pub(crate) type GetOwnPropertyFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
) -> qjs::c_int;

pub(crate) type GetOwnPropertyNamesFunc = unsafe fn(
    this_ptr: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValueConst,
) -> qjs::c_int;

pub(crate) type TypeIdFn = fn() -> TypeId;

type DefineOwnPropertyFunc = unsafe fn(
    this: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
    prop: qjs::JSAtom,
    value: qjs::JSValueConst,
    getter: qjs::JSValueConst,
    setter: qjs::JSValueConst,
    flags: qjs::c_int,
) -> qjs::c_int;
type CanPreventExtensionsFunc = unsafe fn(
    this: NonNull<ClassCell<()>>,
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValueConst,
) -> qjs::c_int;

pub(crate) struct VTable {
    id_fn: TypeIdFn,
    finalizer: FinalizerFunc,
    trace: TraceFunc,
    call: CallFunc,
    get_property: GetPropertyFunc,
    set_property: SetPropertyFunc,
    has_property: HasPropertyFunc,
    delete_property: DeletePropertyFunc,
    get_own_property: GetOwnPropertyFunc,
    get_own_property_names: GetOwnPropertyNamesFunc,
    define_own_property: DefineOwnPropertyFunc,
    can_prevent_extensions: CanPreventExtensionsFunc,
}

impl VTable {
    unsafe fn finalizer_impl<'js, C: JsClass<'js>>(this: NonNull<ClassCell<()>>) {
        let this = this.cast::<ClassCell<JsCell<C>>>();
        let _ = Box::from_raw(this.as_ptr());
    }

    unsafe fn trace_impl<'js, C: JsClass<'js>>(
        this: NonNull<ClassCell<()>>,
        tracer: Tracer<'_, 'static>,
    ) {
        let this = this.cast::<ClassCell<JsCell<C>>>();
        if let Ok(x) = this.as_ref().data.try_borrow() {
            x.trace(tracer.cast_js_lifetime())
        }
    }

    unsafe fn call_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        function: qjs::JSValue,
        this: qjs::JSValue,
        argc: qjs::c_int,
        argv: *mut qjs::JSValue,
        flags: qjs::c_int,
    ) -> qjs::JSValue {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let params = Params::from_ffi_class(ctx, function, this, argc, argv, flags);
        let ctx = params.ctx().clone();

        ctx.handle_panic(AssertUnwindSafe(|| {
            C::call(&this_ptr.as_ref().data, params)
                .map(Value::into_js_value)
                .unwrap_or_else(|e| e.throw(&ctx))
        }))
    }

    unsafe fn get_property_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        obj: qjs::JSValueConst,
        atom: qjs::JSAtom,
        receiver: qjs::JSValueConst,
    ) -> qjs::JSValue {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);
        let atom = Atom::from_atom_val_dup(ctx.clone(), atom);
        let receiver = Value::from_js_value_const(ctx.clone(), receiver);

        ctx.handle_panic(AssertUnwindSafe(|| {
            C::exotic_get_property(&this_ptr.as_ref().data, &ctx, object, atom, receiver)
                .map(Value::into_js_value)
                .unwrap_or_else(|e| e.throw(&ctx))
        }))
    }

    unsafe fn set_property_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        obj: qjs::JSValueConst,
        atom: qjs::JSAtom,
        receiver: qjs::JSValueConst,
        value: qjs::JSValue,
        flags: qjs::c_int,
    ) -> qjs::c_int {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);
        let atom = Atom::from_atom_val_dup(ctx.clone(), atom);
        let receiver = Value::from_js_value_const(ctx.clone(), receiver);
        // QuickJS frees its owned assignment value after invoking the exotic
        // callback. This callback borrows it and must retain its own reference.
        let value = Value::from_js_value_const(ctx.clone(), value);

        ctx.handle_panic_exotic(AssertUnwindSafe(|| {
            match C::exotic_set_property(
                &this_ptr.as_ref().data,
                &ctx,
                object,
                atom,
                receiver,
                value,
            ) {
                Ok(v) => {
                    if v {
                        1
                    } else {
                        qjs::JS_RejectPropertyChange(ctx.as_ptr(), flags)
                    }
                }
                Err(e) => {
                    e.throw(&ctx);
                    -1
                }
            }
        }))
    }

    unsafe fn has_property_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        obj: qjs::JSValueConst,
        atom: qjs::JSAtom,
    ) -> qjs::c_int {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);
        let atom = Atom::from_atom_val_dup(ctx.clone(), atom);

        ctx.handle_panic_exotic(AssertUnwindSafe(|| {
            match C::exotic_has_property(&this_ptr.as_ref().data, &ctx, object, atom) {
                Ok(v) => {
                    if v {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    e.throw(&ctx);
                    -1
                }
            }
        }))
    }

    unsafe fn delete_property_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        obj: qjs::JSValueConst,
        atom: qjs::JSAtom,
    ) -> qjs::c_int {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);
        let atom = Atom::from_atom_val_dup(ctx.clone(), atom);

        ctx.handle_panic_exotic(AssertUnwindSafe(|| {
            match C::exotic_delete_property(&this_ptr.as_ref().data, &ctx, object, atom) {
                Ok(v) => {
                    if v {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    e.throw(&ctx);
                    -1
                }
            }
        }))
    }

    unsafe fn get_own_property_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        desc: *mut qjs::JSPropertyDescriptor,
        obj: qjs::JSValueConst,
        prop: qjs::JSAtom,
    ) -> qjs::c_int {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);
        let atom = Atom::from_atom_val_dup(ctx.clone(), prop);

        ctx.handle_panic_exotic(AssertUnwindSafe(|| {
            match C::exotic_get_own_property(&this_ptr.as_ref().data, &ctx, object, atom) {
                Ok(Some(property)) => {
                    if !desc.is_null() {
                        let mut flags: qjs::c_int = 0;
                        if property.configurable {
                            flags |= qjs::JS_PROP_CONFIGURABLE as qjs::c_int;
                        }
                        if property.enumerable {
                            flags |= qjs::JS_PROP_ENUMERABLE as qjs::c_int;
                        }
                        if property.is_getset {
                            flags |= qjs::JS_PROP_GETSET as qjs::c_int;
                            (*desc).getter = property.getter.into_js_value();
                            (*desc).setter = property.setter.into_js_value();
                            (*desc).value = qjs::JS_UNDEFINED;
                        } else {
                            if property.writable {
                                flags |= qjs::JS_PROP_WRITABLE as qjs::c_int;
                            }
                            (*desc).value = property.value.into_js_value();
                            (*desc).getter = qjs::JS_UNDEFINED;
                            (*desc).setter = qjs::JS_UNDEFINED;
                        }
                        (*desc).flags = flags;
                    }
                    1 // TRUE - property found
                }
                Ok(None) => 0, // FALSE - property not found
                Err(e) => {
                    e.throw(&ctx);
                    -1
                }
            }
        }))
    }

    unsafe fn get_own_property_names_impl<'js, C: JsClass<'js>>(
        this_ptr: NonNull<ClassCell<()>>,
        ctx: *mut qjs::JSContext,
        ptab: *mut *mut qjs::JSPropertyEnum,
        plen: *mut u32,
        obj: qjs::JSValueConst,
    ) -> qjs::c_int {
        let this_ptr = this_ptr.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(ctx);
        let object = Object::from_js_value_const(ctx.clone(), obj);

        ctx.handle_panic_exotic(AssertUnwindSafe(|| {
            match C::exotic_get_own_property_names(&this_ptr.as_ref().data, &ctx, object) {
                Ok(names) => {
                    let len = names.len();
                    // js_malloc(ctx, 0) may return null and install an OOM
                    // exception even though an empty enumeration succeeded.
                    if len == 0 {
                        *ptab = core::ptr::null_mut();
                        *plen = 0;
                        return 0;
                    }
                    let Some((len_u32, size)) = u32::try_from(len)
                        .ok()
                        .zip(mem::size_of::<qjs::JSPropertyEnum>().checked_mul(len))
                    else {
                        qjs::JS_ThrowOutOfMemory(ctx.as_ptr());
                        return -1;
                    };
                    let tab = qjs::js_malloc(ctx.as_ptr(), size as qjs::size_t)
                        as *mut qjs::JSPropertyEnum;
                    if tab.is_null() {
                        return -1;
                    }
                    for (i, name) in names.into_iter().enumerate() {
                        let entry = tab.add(i);
                        (*entry).is_enumerable = name.is_enumerable;
                        // Dup the atom for QuickJS ownership; Rust Atom drop will free the original
                        (*entry).atom = qjs::JS_DupAtom(ctx.as_ptr(), name.atom.atom);
                    }
                    *ptab = tab;
                    *plen = len_u32;
                    0 // success
                }
                Err(e) => {
                    e.throw(&ctx);
                    -1
                }
            }
        }))
    }

    // Mirrors QuickJS's native define-property ABI without changing ownership.
    #[allow(clippy::too_many_arguments)]
    unsafe fn define_own_property_impl<'js, C: JsClass<'js>>(
        this: NonNull<ClassCell<()>>,
        raw_ctx: *mut qjs::JSContext,
        object: qjs::JSValueConst,
        prop: qjs::JSAtom,
        value: qjs::JSValueConst,
        getter: qjs::JSValueConst,
        setter: qjs::JSValueConst,
        flags: qjs::c_int,
    ) -> qjs::c_int {
        let this = this.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(raw_ctx);
        let atom = Atom::from_atom_val_dup(ctx.clone(), prop);
        let object_value = Object::from_js_value_const(ctx.clone(), object);
        let has = |flag: u32| flags & flag as qjs::c_int != 0;
        let descriptor = DefinePropertyDescriptor {
            value: has(qjs::JS_PROP_HAS_VALUE)
                .then(|| Value::from_js_value_const(ctx.clone(), value)),
            getter: has(qjs::JS_PROP_HAS_GET)
                .then(|| Value::from_js_value_const(ctx.clone(), getter)),
            setter: has(qjs::JS_PROP_HAS_SET)
                .then(|| Value::from_js_value_const(ctx.clone(), setter)),
            configurable: has(qjs::JS_PROP_HAS_CONFIGURABLE)
                .then(|| has(qjs::JS_PROP_CONFIGURABLE)),
            enumerable: has(qjs::JS_PROP_HAS_ENUMERABLE).then(|| has(qjs::JS_PROP_ENUMERABLE)),
            writable: has(qjs::JS_PROP_HAS_WRITABLE).then(|| has(qjs::JS_PROP_WRITABLE)),
        };
        match C::exotic_define_own_property(
            &this.as_ref().data,
            &ctx,
            object_value,
            atom,
            descriptor,
        ) {
            Ok(Some(true)) => 1,
            Ok(Some(false)) => qjs::JS_RejectPropertyChange(raw_ctx, flags),
            Ok(None) => qjs::JS_DefineProperty(
                raw_ctx,
                object,
                prop,
                value,
                getter,
                setter,
                flags | qjs::JS_PROP_NO_EXOTIC as qjs::c_int,
            ),
            Err(error) => {
                error.throw(&ctx);
                -1
            }
        }
    }

    unsafe fn can_prevent_extensions_impl<'js, C: JsClass<'js>>(
        this: NonNull<ClassCell<()>>,
        raw_ctx: *mut qjs::JSContext,
        object: qjs::JSValueConst,
    ) -> qjs::c_int {
        let this = this.cast::<ClassCell<JsCell<C>>>();
        let ctx = Ctx::from_ptr(raw_ctx);
        let object = Object::from_js_value_const(ctx.clone(), object);
        match C::exotic_can_prevent_extensions(&this.as_ref().data, &ctx, object) {
            Ok(allowed) => allowed as qjs::c_int,
            Err(error) => {
                error.throw(&ctx);
                -1
            }
        }
    }

    pub fn get<'js, C: JsClass<'js>>() -> &'static VTable {
        trait HasVTable {
            const VTABLE: VTable;
        }

        impl<'js, C: JsClass<'js>> HasVTable for C {
            const VTABLE: VTable = VTable {
                id_fn: TypeId::of::<C::Changed<'static>>,
                finalizer: VTable::finalizer_impl::<'js, C>,
                trace: VTable::trace_impl::<C>,
                call: VTable::call_impl::<C>,
                get_property: VTable::get_property_impl::<C>,
                set_property: VTable::set_property_impl::<C>,
                has_property: VTable::has_property_impl::<C>,
                delete_property: VTable::delete_property_impl::<C>,
                get_own_property: VTable::get_own_property_impl::<C>,
                get_own_property_names: VTable::get_own_property_names_impl::<C>,
                define_own_property: VTable::define_own_property_impl::<C>,
                can_prevent_extensions: VTable::can_prevent_extensions_impl::<C>,
            };
        }
        &<C as HasVTable>::VTABLE
    }

    pub fn id(&self) -> TypeId {
        (self.id_fn)()
    }

    pub fn is_of_class<'js, C: JsClass<'js>>(&self) -> bool {
        (self.id_fn)() == TypeId::of::<C::Changed<'static>>()
    }
}

#[repr(C)]
pub(crate) struct ClassCell<T> {
    pub(crate) v_table: &'static VTable,
    pub(crate) data: T,
}

impl<'js, T: JsClass<'js>> ClassCell<JsCell<'js, T>> {
    pub(crate) fn new(class: T) -> Self {
        ClassCell {
            v_table: VTable::get::<T>(),
            data: JsCell::new(class),
        }
    }
}
