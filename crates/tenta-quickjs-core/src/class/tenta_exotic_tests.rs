//! Behavioral tests for the minimal TentaBrowser native property extensions.

use super::{
    ClassKind, DefinePropertyDescriptor, JsCell, JsClass, PropertyDescriptor, PropertyName, Trace,
    Tracer, Writable,
};
use crate::{
    value::Constructor, Atom, Class, Context, Ctx, JsLifetime, Object, Result, Runtime, Value,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Debug, PartialEq, Eq)]
struct SeenDescriptor {
    value: bool,
    getter: bool,
    setter: bool,
    configurable: Option<bool>,
    enumerable: Option<bool>,
    writable: Option<bool>,
}

struct Probe<'js> {
    value: Value<'js>,
    seen: Option<SeenDescriptor>,
    panic_on_prevent: bool,
}

impl<'js> Trace<'js> for Probe<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.value.trace(tracer);
    }
}

// The only lifetime-bearing field is the traced JavaScript value.
unsafe impl<'js> JsLifetime<'js> for Probe<'js> {
    type Changed<'to> = Probe<'to>;
}

impl<'js> JsClass<'js> for Probe<'js> {
    const NAME: &'static str = "NativePropertyProbe";
    const KIND: ClassKind = ClassKind::Exotic;
    type Mutable = Writable;

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }

    fn exotic_get_property(
        this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        object: Object<'js>,
        atom: Atom<'js>,
        receiver: Value<'js>,
    ) -> Result<Value<'js>> {
        Ok(if atom.to_string()? == "object" {
            object.into_value()
        } else if atom.to_string()? == "receiver" {
            receiver
        } else if atom.to_string()? == "virtual" {
            this.borrow().value.clone()
        } else {
            Value::new_undefined(ctx.clone())
        })
    }

    fn exotic_set_property(
        this: &JsCell<'js, Self>,
        _ctx: &Ctx<'js>,
        _object: Object<'js>,
        atom: Atom<'js>,
        receiver: Value<'js>,
        value: Value<'js>,
    ) -> Result<bool> {
        if atom.to_string()? == "receiver" {
            this.borrow_mut().value = receiver;
            return Ok(true);
        }
        if atom.to_string()? != "virtual" {
            return Ok(false);
        }
        this.borrow_mut().value = value;
        Ok(true)
    }

    fn exotic_get_own_property(
        this: &JsCell<'js, Self>,
        _ctx: &Ctx<'js>,
        _object: Object<'js>,
        atom: Atom<'js>,
    ) -> Result<Option<PropertyDescriptor<'js>>> {
        Ok((atom.to_string()? == "virtual")
            .then(|| PropertyDescriptor::new_value(this.borrow().value.clone(), true, true, true)))
    }

    fn exotic_get_own_property_names(
        _this: &JsCell<'js, Self>,
        ctx: &Ctx<'js>,
        _object: crate::Object<'js>,
    ) -> Result<Vec<PropertyName<'js>>> {
        Ok(vec![PropertyName {
            atom: Atom::from_str(ctx.clone(), "virtual")?,
            is_enumerable: true,
        }])
    }

    fn exotic_define_own_property(
        this: &JsCell<'js, Self>,
        _ctx: &Ctx<'js>,
        _object: Object<'js>,
        atom: Atom<'js>,
        descriptor: DefinePropertyDescriptor<'js>,
    ) -> Result<Option<bool>> {
        match atom.to_string()?.as_str() {
            "panic" => panic!("native define hook panic"),
            "reject" => Ok(Some(false)),
            "virtual" | "inspect" => {
                let mut state = this.borrow_mut();
                state.seen = Some(SeenDescriptor {
                    value: descriptor.value.is_some(),
                    getter: descriptor.getter.is_some(),
                    setter: descriptor.setter.is_some(),
                    configurable: descriptor.configurable,
                    enumerable: descriptor.enumerable,
                    writable: descriptor.writable,
                });
                if let Some(value) = descriptor.value {
                    state.value = value;
                }
                Ok(Some(true))
            }
            _ => Ok(None),
        }
    }

    fn exotic_can_prevent_extensions(
        this: &JsCell<'js, Self>,
        _ctx: &Ctx<'js>,
        _object: Object<'js>,
    ) -> Result<bool> {
        assert!(!this.borrow().panic_on_prevent, "native prevent hook panic");
        Ok(false)
    }
}

struct DefaultExotic;

impl<'js> Trace<'js> for DefaultExotic {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

// This class carries no JavaScript references.
unsafe impl<'js> JsLifetime<'js> for DefaultExotic {
    type Changed<'to> = DefaultExotic;
}

impl<'js> JsClass<'js> for DefaultExotic {
    const NAME: &'static str = "DefaultExotic";
    const KIND: ClassKind = ClassKind::Exotic;
    type Mutable = Writable;

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

fn install<'js>(ctx: &Ctx<'js>, panic_on_prevent: bool) -> Class<'js, Probe<'js>> {
    let class = Class::instance(
        ctx.clone(),
        Probe {
            value: Value::new_undefined(ctx.clone()),
            seen: None,
            panic_on_prevent,
        },
    )
    .unwrap();
    ctx.globals().set("native", class.clone()).unwrap();
    ctx.eval::<(), _>(
        "globalThis.check = (ok) => { if (!ok) throw new Error('native hook assertion'); }",
    )
    .unwrap();
    class
}

#[test]
fn native_define_accepts_nonconfigurable_virtual_property() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, false);
        ctx.eval::<(), _>(
            r#"
            check(Reflect.defineProperty(native, 'virtual', {value: 'first', configurable: false}));
            let first = Object.getOwnPropertyDescriptor(native, 'virtual');
            check(first.value === 'first' && first.configurable && first.writable && first.enumerable);
            check(Object.defineProperty(native, 'virtual', {value: 'second', configurable: false}) === native);
            check(native.virtual === 'second');
            Object.defineProperty(native, 'ordinary', {value: 17, configurable: false});
            let ordinary = Object.getOwnPropertyDescriptor(native, 'ordinary');
            check(ordinary.value === 17 && !ordinary.configurable && !ordinary.writable);
            check(!Reflect.defineProperty(native, 'ordinary', {value: 18}));
            check(Object.keys(native).join(',') === 'virtual');
            "#,
        )
        .unwrap();
    });
}

#[test]
fn native_define_preserves_absent_descriptor_fields() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let class = install(&ctx, false);
        ctx.eval::<(), _>("Object.defineProperty(native, 'inspect', {})")
            .unwrap();
        assert_eq!(
            class.borrow().seen,
            Some(SeenDescriptor {
                value: false,
                getter: false,
                setter: false,
                configurable: None,
                enumerable: None,
                writable: None,
            })
        );
        ctx.eval::<(), _>(
            "Object.defineProperty(native, 'inspect', {value: undefined, writable: false, enumerable: true, configurable: false})",
        )
        .unwrap();
        assert_eq!(
            class.borrow().seen,
            Some(SeenDescriptor {
                value: true,
                getter: false,
                setter: false,
                configurable: Some(false),
                enumerable: Some(true),
                writable: Some(false),
            })
        );
        ctx.eval::<(), _>(
            "Object.defineProperty(native, 'inspect', {get: undefined, set: undefined})",
        )
        .unwrap();
        assert_eq!(
            class.borrow().seen,
            Some(SeenDescriptor {
                value: false,
                getter: true,
                setter: true,
                configurable: None,
                enumerable: None,
                writable: None,
            })
        );
    });
}

#[test]
fn native_prevent_extensions_refuses_without_changing_ordinary_objects() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, false);
        ctx.globals()
            .set(
                "defaultExotic",
                Class::instance(ctx.clone(), DefaultExotic).unwrap(),
            )
            .unwrap();
        ctx.eval::<(), _>(
            r#"
            check(Reflect.preventExtensions(native) === false);
            check(Object.isExtensible(native));
            for (let operation of [Object.preventExtensions, Object.seal, Object.freeze]) {
                let failed = false;
                try { operation(native); } catch (error) { failed = error instanceof TypeError; }
                check(failed && Object.isExtensible(native));
            }
            let ordinary = {};
            check(Reflect.preventExtensions(ordinary) && !Object.isExtensible(ordinary));
            check(Reflect.preventExtensions(defaultExotic) && !Object.isExtensible(defaultExotic));
            check(!Reflect.defineProperty(defaultExotic, 'new', {value: 1}));
            "#,
        )
        .unwrap();
    });
}

#[test]
fn native_property_refusal_preserves_strict_and_reflect_semantics() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, false);
        ctx.eval::<(), _>(
            r#"
            Function('native', 'native.reject = 1')(native);
            check(Reflect.set(native, 'reject', 2) === false);
            check(Reflect.defineProperty(native, 'reject', {value: 3}) === false);
            for (let operation of [
                () => { 'use strict'; native.reject = 4; },
                () => Object.defineProperty(native, 'reject', {value: 5}),
            ]) {
                let failed = false;
                try { operation(); } catch (error) { failed = error instanceof TypeError; }
                check(failed);
            }
            "#,
        )
        .unwrap();
    });
}

#[test]
fn native_setter_retains_borrowed_objects_and_strings_through_gc() {
    // Nextest isolates this ownership regression in its own process because
    // freeing a borrowed value can also abort during runtime teardown.
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, false);
        ctx.eval::<(), _>(
            r#"
            globalThis.kept = [];
            for (let index = 0; index < 512; index++) {
                let object = {index, text: 'value:'.repeat(128) + index};
                native.virtual = object;
                check(native.virtual === object && object.index === index);
                kept.push(object);
                native.virtual = object.text;
                check(native.virtual === object.text);
                check(Reflect.set(native, 'virtual', object));
            }
            "#,
        )
        .unwrap();
    });
    runtime.run_gc();
    context.with(|ctx| {
        ctx.eval::<(), _>(
            r#"
            for (let index = 0; index < kept.length; index++) {
                check(kept[index].index === index);
                check(kept[index].text === 'value:'.repeat(128) + index);
            }
            check(native.virtual === kept[511]);
            delete globalThis.kept;
            delete globalThis.native;
            "#,
        )
        .unwrap();
    });
    runtime.run_gc();
}

#[test]
fn native_define_panic_returns_to_rust_without_crossing_c() {
    assert_native_panic("Reflect.defineProperty(native, 'panic', {value: 1})", false);
}

#[test]
fn native_prevent_panic_returns_to_rust_without_crossing_c() {
    assert_native_panic("Reflect.preventExtensions(native)", true);
}

fn assert_native_panic(script: &str, panic_on_prevent: bool) {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, panic_on_prevent);
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        context.with(|ctx| ctx.eval::<(), _>(script).unwrap());
    }));
    assert!(
        result.is_err(),
        "the panic must resume only after the C call returns"
    );
    context.with(|ctx| {
        let _ = ctx.catch();
        assert_eq!(ctx.eval::<i32, _>("21 * 2").unwrap(), 42);
        ctx.globals().remove("native").unwrap();
    });
    runtime.run_gc();
}

#[test]
fn native_hook_object_is_distinct_from_derived_receiver() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install(&ctx, false);
        ctx.eval::<(), _>(r#"
            const receiver = {};
            const derived = Object.create(native);
            if (Reflect.get(native, "object", receiver) !== native) throw Error("original object lost");
            if (Reflect.get(native, "receiver", receiver) !== receiver) throw Error("explicit receiver lost");
            if (derived.object !== native || derived.receiver !== derived) throw Error("inherited receiver lost");
            if (!Reflect.set(native, "receiver", 1, receiver) || native.virtual !== receiver) throw Error("setter receiver lost");
            native.virtual = null;
        "#).unwrap();
    });
    runtime.run_gc();
}

#[test]
fn native_property_keys_preserve_symbol_identity() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let symbol: Value = ctx.eval("Symbol('private expando')").unwrap();
        let atom = Atom::from_value(ctx.clone(), &symbol).unwrap();
        assert_eq!(atom.to_property_key().unwrap(), symbol);
        assert_eq!(
            Atom::from_u32(ctx.clone(), 42)
                .unwrap()
                .to_property_key()
                .unwrap()
                .as_string()
                .unwrap()
                .to_string()
                .unwrap(),
            "42"
        );
    });
}

#[test]
fn native_class_predicate_does_not_throw_on_another_class_kind() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let ordinary = Object::new(ctx.clone()).unwrap();
        assert!(!ordinary.instance_of::<Probe>());
        assert!(ordinary.as_class::<Probe>().is_none());
        // SAFETY: the test owns the context on its runtime thread.
        assert!(!unsafe { crate::qjs::JS_HasException(ctx.as_raw().as_ptr()) });
        assert_eq!(ctx.eval::<i32, _>("21 * 2").unwrap(), 42);
    });
}

#[test]
fn native_empty_property_enumeration_does_not_leave_an_oom_exception() {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let object = Class::instance(ctx.clone(), DefaultExotic).unwrap();
        ctx.globals().set("empty", object).unwrap();
        assert!(ctx
            .eval::<bool, _>(
                "Object.keys(empty).length === 0 && Reflect.ownKeys(empty).length === 0"
            )
            .unwrap());
        assert!(!unsafe { crate::qjs::JS_HasException(ctx.as_ptr()) });
    });
}
