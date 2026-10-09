//! The Window structuredClone entry and bounded Web IDL transfer conversion.

use std::rc::Rc;

use rquickjs::atom::PredefinedAtom;
use rquickjs::function::{Rest, This};
use rquickjs::{Coerced, Ctx, FromJs, Function, Object, Value};
use tenta_js::bindings::quickjs_callback_boundary;
use tenta_js::structured_clone::{
    TransferList, collect_transfer_list, deserialize, serialize_with_transfer,
};

use super::Adapter;
use crate::host::State;

fn type_error(ctx: &Ctx<'_>, message: &'static str) -> rquickjs::Error {
    rquickjs::Exception::throw_type(ctx, message)
}

fn method<'js, K: rquickjs::IntoAtom<'js>>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    key: K,
    message: &'static str,
) -> rquickjs::Result<Function<'js>> {
    let value: Value<'js> = object.get(key)?;
    if !value.is_function() {
        return Err(type_error(ctx, message));
    }
    // A native is_function check has established the exact QuickJS class.
    Function::from_value(value)
}

fn transfer_list<'js>(
    source: &mut Adapter<'_, 'js>,
    options: Option<&Value<'js>>,
) -> rquickjs::Result<TransferList<Value<'js>>> {
    let reservation = source.reservation();
    let Some(options) = options.filter(|value| !value.is_null() && !value.is_undefined()) else {
        return Ok(TransferList::new(reservation));
    };
    let options = options
        .as_object()
        .ok_or_else(|| type_error(source.ctx, "structuredClone options must be an object"))?;
    let transfer: Value<'js> = options.get("transfer")?;
    source.check()?;
    if transfer.is_undefined() {
        return Ok(TransferList::new(reservation));
    }
    let iterable = transfer
        .as_object()
        .ok_or_else(|| type_error(source.ctx, "transfer must be an iterable object"))?;
    let iterator_method = method(
        source.ctx,
        iterable,
        PredefinedAtom::SymbolIterator,
        "transfer must be iterable",
    )?;
    source.check()?;
    let iterator_value: Value<'js> = iterator_method.call((This(iterable.clone()),))?;
    source.check()?;
    let iterator = iterator_value
        .as_object()
        .ok_or_else(|| type_error(source.ctx, "transfer iterator must be an object"))?
        .clone();
    let next = method(
        source.ctx,
        &iterator,
        PredefinedAtom::Next,
        "transfer iterator.next must be callable",
    )?;
    source.check()?;
    collect_transfer_list(source, reservation, move |source| {
        let result: Value<'js> = next.call((This(iterator.clone()),))?;
        source.check()?;
        let result = result
            .as_object()
            .ok_or_else(|| type_error(source.ctx, "iterator result must be an object"))?;
        let done: Value<'js> = result.get(PredefinedAtom::Done)?;
        source.check()?;
        if Coerced::<bool>::from_js(source.ctx, done)?.0 {
            return Ok(None);
        }
        let value: Value<'js> = result.get(PredefinedAtom::Value)?;
        source.check()?;
        if !value.is_object() {
            // Web IDL's sequence conversion propagates the element conversion
            // error directly; it does not perform IteratorClose here.
            return Err(type_error(source.ctx, "transfer item must be an object"));
        }
        Ok(Some(value))
    })
}

pub(crate) fn install<'js>(ctx: &Ctx<'js>, state: &Rc<State>) -> rquickjs::Result<()> {
    let world = Rc::clone(&state.world);
    let callback = Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
        quickjs_callback_boundary(&ctx, "structuredClone", || {
            let value = args
                .0
                .first()
                .cloned()
                .ok_or_else(|| type_error(&ctx, "structuredClone requires a value argument"))?;
            let mut adapter = Adapter::new(&ctx, &world);
            let transfers = transfer_list(&mut adapter, args.0.get(1))?;
            let wire = serialize_with_transfer(&mut adapter, value, transfers)?;
            deserialize(&mut adapter, wire)
        })
    })?;
    callback.set_name("structuredClone")?;
    callback.set_length(1)?;
    ctx.globals().set("structuredClone", callback)
}
