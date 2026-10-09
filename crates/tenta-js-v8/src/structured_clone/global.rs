//! Window structuredClone entry and bounded Web IDL transfer conversion.

use tenta_js::structured_clone::{
    TransferList, collect_transfer_list, deserialize, serialize_with_transfer,
};

use super::{Adapter, CloneError, CloneResult};
use crate::host::HostState;

fn method<'s>(
    adapter: &mut Adapter<'_, 's, '_>,
    object: v8::Local<'s, v8::Object>,
    key: v8::Local<'s, v8::Value>,
    message: &'static str,
) -> CloneResult<v8::Local<'s, v8::Function>> {
    let value = object.get(adapter.scope, key).ok_or(CloneError::Abrupt)?;
    adapter.check()?;
    if !value.is_function() {
        return Err(CloneError::Type(message));
    }
    v8::Local::<v8::Function>::try_from(value).map_err(|_| CloneError::Type(message))
}

fn transfer_list<'s>(
    adapter: &mut Adapter<'_, 's, '_>,
    options: Option<v8::Local<'s, v8::Value>>,
) -> CloneResult<TransferList<v8::Local<'s, v8::Value>>> {
    let reservation = adapter.reservation();
    let Some(options) = options.filter(|value| !value.is_null_or_undefined()) else {
        return Ok(TransferList::new(reservation));
    };
    if !options.is_object() {
        return Err(CloneError::Type(
            "structuredClone options must be an object",
        ));
    }
    let options = adapter.object_handle(options);
    let key = v8::String::new(adapter.scope, "transfer").expect("fixed native key");
    let transfer = options
        .get(adapter.scope, key.into())
        .ok_or(CloneError::Abrupt)?;
    adapter.check()?;
    if transfer.is_undefined() {
        return Ok(TransferList::new(reservation));
    }
    if !transfer.is_object() {
        return Err(CloneError::Type("transfer must be an iterable object"));
    }
    let iterable = adapter.object_handle(transfer);
    let symbol: v8::Local<'s, v8::Value> = v8::Symbol::get_iterator(adapter.scope).into();
    let iterator_method = method(adapter, iterable, symbol, "transfer must be iterable")?;
    let iterator_value = iterator_method
        .call(adapter.scope, iterable.into(), &[])
        .ok_or(CloneError::Abrupt)?;
    adapter.check()?;
    if !iterator_value.is_object() {
        return Err(CloneError::Type("transfer iterator must be an object"));
    }
    let iterator = adapter.object_handle(iterator_value);
    let next_key = v8::String::new(adapter.scope, "next").expect("fixed native key");
    let next = method(
        adapter,
        iterator,
        next_key.into(),
        "transfer iterator.next must be callable",
    )?;
    collect_transfer_list(adapter, reservation, move |adapter| {
        let result = next
            .call(adapter.scope, iterator.into(), &[])
            .ok_or(CloneError::Abrupt)?;
        adapter.check()?;
        if !result.is_object() {
            return Err(CloneError::Type("iterator result must be an object"));
        }
        let result = adapter.object_handle(result);
        let done_key = v8::String::new(adapter.scope, "done").expect("fixed native key");
        let done = result
            .get(adapter.scope, done_key.into())
            .ok_or(CloneError::Abrupt)?;
        adapter.check()?;
        if done.boolean_value(adapter.scope) {
            return Ok(None);
        }
        let value_key = v8::String::new(adapter.scope, "value").expect("fixed native key");
        let value = result
            .get(adapter.scope, value_key.into())
            .ok_or(CloneError::Abrupt)?;
        adapter.check()?;
        if !value.is_object() {
            // Web IDL sequence<object> propagates element-conversion failure
            // directly; its algorithm does not perform IteratorClose here.
            return Err(CloneError::Type("transfer item must be an object"));
        }
        Ok(Some(value))
    })
}

pub(super) fn run<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    state: &HostState,
) -> CloneResult<v8::Local<'s, v8::Value>> {
    let mut adapter = Adapter::new(scope, state);
    adapter.check()?;
    if args.length() == 0 {
        return Err(CloneError::Type(
            "structuredClone requires a value argument",
        ));
    }
    let value = args.get(0);
    let options = (args.length() > 1).then(|| args.get(1));
    let transfers = transfer_list(&mut adapter, options)?;
    let wire = serialize_with_transfer(&mut adapter, value, transfers)?;
    deserialize(&mut adapter, wire)
}

pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    let key = v8::String::new(scope, "structuredClone").expect("fixed native key");
    let function = v8::Function::builder(super::callback)
        .length(1)
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .build(scope)
        .expect("create structuredClone while a context is entered");
    function.set_name(key);
    global
        .set(scope, key.into(), function.into())
        .expect("install structuredClone on the new global");
}
