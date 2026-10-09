//! WebIDL legacy platform objects implemented by native QuickJS exotic hooks.
//!
//! The visible wrapper has no physical public own properties. Ordinary expando
//! descriptors live in a private, traced object, so QuickJS can collect cycles
//! through data properties, accessors and symbols. Its keys are recorded in
//! creation order because ordinary objects sort numeric keys differently.

use rquickjs::class::{
    ClassKind, DefinePropertyDescriptor, JsCell, JsClass, PropertyDescriptor, PropertyName,
    Readable, Trace, Tracer,
};
use rquickjs::{Atom, Ctx, Function, Object, Result, Value};
use std::cell::RefCell;
use tenta_js::bindings::*;

pub(super) struct LegacyHost<'js> {
    pub host: QuickJsHost,
    methods: QuickJsLegacyMethods,
    expando: Object<'js>,
    keys: RefCell<Vec<Atom<'js>>>,
}

impl<'js> LegacyHost<'js> {
    pub fn new(ctx: &Ctx<'js>, host: QuickJsHost, methods: QuickJsLegacyMethods) -> Result<Self> {
        let expando = Object::new(ctx.clone())?;
        expando.set_prototype(None)?;
        Ok(Self {
            host,
            methods,
            expando,
            keys: RefCell::default(),
        })
    }

    // End every native borrow before a reflection operation, conversion, or
    // getter can re-enter page script (or allocation can invoke GC tracing).
    fn state(this: &JsCell<'js, Self>) -> (QuickJsLegacyMethods, Object<'js>) {
        let this = this.borrow();
        (this.methods, this.expando.clone())
    }
}

// SAFETY: each lifetime-branded field changes to the same runtime lifetime.
unsafe impl<'js> rquickjs::JsLifetime<'js> for LegacyHost<'js> {
    type Changed<'to> = LegacyHost<'to>;
}
impl<'js> Trace<'js> for LegacyHost<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.expando.trace(tracer);
        // Each retained atom owns a context reference in addition to its
        // string/symbol. Expose those edges so context teardown can collect a
        // live wrapper together with the realm that owns it.
        self.keys.borrow().trace(tracer);
    }
}

/// Invoke an intrinsic captured before page script, preserving sticky poison
/// after a page-owned prototype/receiver trap has run.
fn reflect<'js, A: rquickjs::function::IntoArgs<'js>, R: rquickjs::FromJs<'js>>(
    ctx: &Ctx<'js>,
    name: &str,
    arguments: A,
) -> Result<R> {
    let function = quickjs_intrinsic(ctx, name)?
        .into_function()
        .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "missing reflection intrinsic"))?;
    let result = function.call(arguments);
    quickjs_check_native_failure(ctx)?;
    result
}

fn descriptor<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: &Value<'js>,
) -> Result<Option<PropertyDescriptor<'js>>> {
    let desc: Option<Object> = reflect(
        ctx,
        "Reflect.getOwnPropertyDescriptor",
        (object.clone(), key.clone()),
    )?;
    desc.map(|desc| {
        // The pristine builtin creates this fresh descriptor. Remove its
        // prototype before reading absent fields, which page code may pollute.
        desc.set_prototype(None)?;
        let is_getset = desc.contains_key("get")?;
        Ok(PropertyDescriptor {
            value: desc.get("value")?,
            getter: desc.get("get")?,
            setter: desc.get("set")?,
            configurable: desc.get("configurable")?,
            enumerable: desc.get("enumerable")?,
            writable: if is_getset {
                false
            } else {
                desc.get("writable")?
            },
            is_getset,
        })
    })
    .transpose()
}

fn prototype<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<Option<Object<'js>>> {
    reflect(ctx, "Reflect.getPrototypeOf", (object.clone(),))
}

fn string_key(key: &Value<'_>) -> Result<Option<String>> {
    key.as_string().map(rquickjs::String::to_string).transpose()
}

fn array_index(name: &str) -> Option<u32> {
    let index: u32 = name.parse().ok()?;
    (index != u32::MAX && index.to_string() == name).then_some(index)
}

fn visible_name<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    expando: &Object<'js>,
    methods: QuickJsLegacyMethods,
    key: &Value<'js>,
    name: &str,
) -> Result<bool> {
    let Some(named) = methods.named else {
        return Ok(false);
    };
    if !(named.supports)(ctx, object, name)? || descriptor(ctx, expando, key)?.is_some() {
        return Ok(false);
    }
    if methods.override_builtins {
        return Ok(true);
    }
    let mut current = prototype(ctx, object)?;
    // Include the receiver before inspecting any prototype descriptor. A
    // Proxy can return it as a prototype; asking that object's descriptor
    // first would recursively start another visibility walk.
    let mut visited = vec![object.clone()];
    while let Some(parent) = current {
        // A Proxy can report a cyclic prototype graph even though ordinary
        // objects cannot. Bound this native walk without relying on JS polling.
        if visited.contains(&parent) {
            return Err(rquickjs::Exception::throw_type(
                ctx,
                "cyclic prototype chain",
            ));
        }
        visited.push(parent.clone());
        if descriptor(ctx, &parent, key)?.is_some() {
            return Ok(false);
        }
        current = prototype(ctx, &parent)?;
    }
    Ok(true)
}

fn own_property<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    expando: &Object<'js>,
    methods: QuickJsLegacyMethods,
    key: &Value<'js>,
    mut ignore_named: bool,
) -> Result<Option<PropertyDescriptor<'js>>> {
    let name = string_key(key)?;
    if let (Some(indexed), Some(index)) = (methods.indexed, name.as_deref().and_then(array_index)) {
        if (indexed.supports)(ctx, object, index)? {
            return Ok(Some(PropertyDescriptor::new_value(
                (indexed.get)(ctx, object, index)?,
                true,
                true,
                indexed.set.is_some(),
            )));
        }
        ignore_named = true;
    }
    if !ignore_named
        && let Some(name) = name
        && visible_name(ctx, object, expando, methods, key, &name)?
    {
        let named = methods.named.expect("visible names require a named getter");
        return Ok(Some(PropertyDescriptor::new_value(
            (named.get)(ctx, object, &name)?,
            true,
            !methods.unenumerable_names,
            named.set.is_some(),
        )));
    }
    descriptor(ctx, expando, key)
}

fn descriptor_value<'js>(
    ctx: &Ctx<'js>,
    desc: PropertyDescriptor<'js>,
    receiver: Value<'js>,
) -> Result<Value<'js>> {
    if !desc.is_getset {
        return Ok(desc.value);
    }
    if desc.getter.is_undefined() {
        return Ok(Value::new_undefined(ctx.clone()));
    }
    let get = Function::from_value(desc.getter)?;
    let result = get.call((rquickjs::function::This(receiver),));
    quickjs_check_native_failure(ctx)?;
    result
}

fn data_descriptor<'js>(ctx: &Ctx<'js>, value: Value<'js>, create: bool) -> Result<Object<'js>> {
    let desc = Object::new(ctx.clone())?;
    desc.set_prototype(None)?;
    desc.set("value", value)?;
    if create {
        for name in ["writable", "enumerable", "configurable"] {
            desc.set(name, true)?;
        }
    }
    Ok(desc)
}

fn ordinary_set<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: Value<'js>,
    value: Value<'js>,
    receiver: Value<'js>,
    desc: Option<PropertyDescriptor<'js>>,
) -> Result<bool> {
    let desc = match desc {
        Some(desc) => desc,
        None => {
            if let Some(parent) = prototype(ctx, object)? {
                return reflect(ctx, "Reflect.set", (parent, key, value, receiver));
            }
            PropertyDescriptor::new_value(Value::new_undefined(ctx.clone()), true, true, true)
        }
    };
    if desc.is_getset {
        if desc.setter.is_undefined() {
            return Ok(false);
        }
        let set = Function::from_value(desc.setter)?;
        let result: Result<Value> = set.call((rquickjs::function::This(receiver), value));
        quickjs_check_native_failure(ctx)?;
        result?;
        return Ok(true);
    }
    if !desc.writable {
        return Ok(false);
    }
    let Some(receiver) = receiver.into_object() else {
        return Ok(false);
    };
    let existing = descriptor(ctx, &receiver, &key)?;
    if existing
        .as_ref()
        .is_some_and(|desc| desc.is_getset || !desc.writable)
    {
        return Ok(false);
    }
    let update = data_descriptor(ctx, value, existing.is_none())?;
    reflect(ctx, "Reflect.defineProperty", (receiver, key, update))
}

impl<'js> JsClass<'js> for LegacyHost<'js> {
    const NAME: &'static str = "TentaLegacyHost";
    const KIND: ClassKind = ClassKind::Exotic;
    type Mutable = Readable;
    fn constructor(_: &Ctx<'js>) -> Result<Option<rquickjs::function::Constructor<'js>>> {
        Ok(None)
    }

    fn exotic_get_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
        receiver: Value<'js>,
    ) -> Result<Value<'js>> {
        quickjs_callback_boundary(ctx, "legacy property get", || {
            let (methods, expando) = Self::state(this);
            let key = atom.to_property_key()?;
            if let Some(desc) = own_property(ctx, &object, &expando, methods, &key, false)? {
                return descriptor_value(ctx, desc, receiver);
            }
            if let Some(parent) = prototype(ctx, &object)? {
                reflect(ctx, "Reflect.get", (parent, key, receiver))
            } else {
                Ok(Value::new_undefined(ctx.clone()))
            }
        })
    }
    fn exotic_has_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
    ) -> Result<bool> {
        quickjs_callback_boundary(ctx, "legacy property has", || {
            let (methods, expando) = Self::state(this);
            let key = atom.to_property_key()?;
            if own_property(ctx, &object, &expando, methods, &key, false)?.is_some() {
                return Ok(true);
            }
            if let Some(parent) = prototype(ctx, &object)? {
                reflect(ctx, "Reflect.has", (parent, key))
            } else {
                Ok(false)
            }
        })
    }
    fn exotic_get_own_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
    ) -> Result<Option<PropertyDescriptor<'js>>> {
        quickjs_callback_boundary(ctx, "legacy property descriptor", || {
            let (methods, expando) = Self::state(this);
            own_property(
                ctx,
                &object,
                &expando,
                methods,
                &atom.to_property_key()?,
                false,
            )
        })
    }
    fn exotic_set_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
        receiver: Value<'js>,
        value: Value<'js>,
    ) -> Result<bool> {
        quickjs_callback_boundary(ctx, "legacy property set", || {
            let (methods, expando) = Self::state(this);
            let key = atom.to_property_key()?;
            let name = string_key(&key)?;
            if object.as_value() == &receiver {
                if let (Some(set), Some(index)) = (
                    methods.indexed.and_then(|indexed| indexed.set),
                    name.as_deref().and_then(array_index),
                ) {
                    set(ctx, &object, index, value)?;
                    return Ok(true);
                }
                if let (Some(set), Some(name)) = (methods.named.and_then(|named| named.set), name) {
                    set(ctx, &object, &name, value)?;
                    return Ok(true);
                }
            }
            let desc = own_property(ctx, &object, &expando, methods, &key, true)?;
            ordinary_set(ctx, &object, key, value, receiver, desc)
        })
    }
    fn exotic_define_own_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
        desc: DefinePropertyDescriptor<'js>,
    ) -> Result<Option<bool>> {
        quickjs_callback_boundary(ctx, "legacy property define", || {
            let (methods, expando) = Self::state(this);
            let key = atom.to_property_key()?;
            let name = string_key(&key)?;
            let is_data = desc.value.is_some() || desc.writable.is_some();
            if let (Some(indexed), Some(index)) =
                (methods.indexed, name.as_deref().and_then(array_index))
            {
                let Some(set) = indexed.set.filter(|_| is_data) else {
                    return Ok(Some(false));
                };
                set(
                    ctx,
                    &object,
                    index,
                    desc.value
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                )?;
                return Ok(Some(true));
            }
            let existing = descriptor(ctx, &expando, &key)?.is_some();
            if !methods.is_global
                && let (Some(named), Some(name)) = (methods.named, name)
                && !methods.unforgeable_names.contains(&name.as_str())
                && (methods.override_builtins || !existing)
            {
                let creating = !(named.supports)(ctx, &object, &name)?;
                if let Some(set) = named.set {
                    if !is_data {
                        return Ok(Some(false));
                    }
                    set(
                        ctx,
                        &object,
                        &name,
                        desc.value
                            .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                    )?;
                    return Ok(Some(true));
                }
                if !creating {
                    return Ok(Some(false));
                }
            }
            let descriptor = Object::new(ctx.clone())?;
            descriptor.set_prototype(None)?;
            for (name, value) in [
                ("value", desc.value),
                ("get", desc.getter),
                ("set", desc.setter),
            ] {
                if let Some(value) = value {
                    descriptor.set(name, value)?;
                }
            }
            for (name, value) in [
                ("configurable", desc.configurable),
                ("enumerable", desc.enumerable),
                ("writable", desc.writable),
            ] {
                if let Some(value) = value {
                    descriptor.set(name, value)?;
                }
            }
            let success = reflect(ctx, "Reflect.defineProperty", (expando, key, descriptor))?;
            if success && !existing {
                this.borrow().keys.borrow_mut().push(atom);
            }
            Ok(Some(success))
        })
    }
    fn exotic_delete_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
    ) -> Result<bool> {
        quickjs_callback_boundary(ctx, "legacy property delete", || {
            let (methods, expando) = Self::state(this);
            let key = atom.to_property_key()?;
            let name = string_key(&key)?;
            if let (Some(indexed), Some(index)) =
                (methods.indexed, name.as_deref().and_then(array_index))
            {
                return (indexed.supports)(ctx, &object, index).map(|supported| !supported);
            }
            if !methods.is_global
                && let Some(name) = name
                && visible_name(ctx, &object, &expando, methods, &key, &name)?
            {
                return match methods.named.and_then(|named| named.delete) {
                    Some(delete) => delete(ctx, &object, &name),
                    None => Ok(false),
                };
            }
            let success = reflect(ctx, "Reflect.deleteProperty", (expando, key))?;
            if success {
                this.borrow().keys.borrow_mut().retain(|key| key != &atom);
            }
            Ok(success)
        })
    }
    fn exotic_get_own_property_names(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
    ) -> Result<Vec<PropertyName<'js>>> {
        quickjs_callback_boundary(ctx, "legacy property keys", || {
            let (methods, expando) = Self::state(this);
            let mut names = Vec::new();
            if let Some(indexed) = methods.indexed {
                let mut indices = (indexed.indices)(ctx, &object)?;
                indices.sort_unstable();
                indices.dedup();
                for index in indices {
                    names.push(PropertyName {
                        atom: Atom::from_u32(ctx.clone(), index)?,
                        is_enumerable: true,
                    });
                }
            }
            if let Some(named) = methods.named {
                for name in (named.names)(ctx, &object)? {
                    let atom = Atom::from_str(ctx.clone(), &name)?;
                    if !names.iter().any(|entry| entry.atom == atom)
                        && visible_name(
                            ctx,
                            &object,
                            &expando,
                            methods,
                            &atom.to_property_key()?,
                            &name,
                        )?
                    {
                        names.push(PropertyName {
                            atom,
                            is_enumerable: !methods.unenumerable_names,
                        });
                    }
                }
            }
            let keys = this.borrow().keys.borrow().clone();
            for symbols in [false, true] {
                for atom in &keys {
                    let key = atom.to_property_key()?;
                    if key.is_symbol() != symbols {
                        continue;
                    }
                    if let Some(desc) = descriptor(ctx, &expando, &key)? {
                        names.push(PropertyName {
                            atom: atom.clone(),
                            is_enumerable: desc.enumerable,
                        });
                    }
                }
            }
            Ok(names)
        })
    }
    fn exotic_can_prevent_extensions(
        _: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        _: Object<'js>,
    ) -> Result<bool> {
        quickjs_callback_boundary(ctx, "legacy prevent extensions", || Ok(false))
    }
}
