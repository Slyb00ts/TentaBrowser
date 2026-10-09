//! V8 adapter for the bounded, engine-neutral structured-clone wire.
//!
//! Classification uses the pinned native ordinary-object helper. Every
//! page-owned Get runs inside the existing callback panic and native-poison
//! boundary; temporary copies share the page memory ledger with the wire.

use tenta_core::{NativeMemoryComponent, NativeMemoryReservation};
use tenta_js::bindings::{
    JsError, native_callback_failure, throw_js_error, v8_registered_slow_callback_boundary,
};
use tenta_js::structured_clone::{CloneSource, CloneTarget, JsString, PROPERTY_CHARGE, SourceKind};

use crate::host::HostState;

mod global;
pub(crate) use global::install;

#[cfg(test)]
mod tests;

#[derive(Debug)]
enum CloneError {
    /// V8 already has the exact page-thrown value pending.
    Abrupt,
    Type(&'static str),
    DataClone(&'static str),
    /// Sticky native poison, watchdog termination, or page-memory failure.
    Terminated,
}

type CloneResult<T> = Result<T, CloneError>;

struct Adapter<'a, 's, 'i> {
    scope: &'a mut v8::PinScope<'s, 'i>,
    state: &'a HostState,
}

impl<'a, 's, 'i> Adapter<'a, 's, 'i> {
    fn new(scope: &'a mut v8::PinScope<'s, 'i>, state: &'a HostState) -> Self {
        Self { scope, state }
    }

    fn reservation(&self) -> NativeMemoryReservation {
        self.state
            .native_memory
            .begin(NativeMemoryComponent::StructuredClone)
    }

    fn fail_budget(&mut self, workspace: &NativeMemoryReservation) -> CloneError {
        workspace.fail_allocation();
        self.scope.terminate_execution();
        CloneError::Terminated
    }

    fn charge(&mut self, workspace: &mut NativeMemoryReservation, bytes: usize) -> CloneResult<()> {
        if workspace.charge(bytes) {
            Ok(())
        } else {
            self.scope.terminate_execution();
            Err(CloneError::Terminated)
        }
    }

    fn copy_units(
        &mut self,
        text: v8::Local<'s, v8::String>,
        workspace: &mut NativeMemoryReservation,
    ) -> CloneResult<JsString> {
        let length = text.length();
        let charge = length
            .checked_mul(4)
            .ok_or_else(|| self.fail_budget(workspace))?;
        self.charge(workspace, charge)?;
        let mut units = Vec::new();
        units
            .try_reserve_exact(length)
            .map_err(|_| self.fail_budget(workspace))?;
        units.resize(length, 0);
        // This V8 call reads raw UTF-16 code units; it does not perform JS
        // ToString or replace lone surrogates.
        text.write_v2(self.scope.as_ref(), 0, &mut units, v8::WriteFlags::empty());
        self.check()?;
        Ok(units)
    }

    fn string(&mut self, units: &[u16]) -> CloneResult<v8::Local<'s, v8::String>> {
        v8::String::new_from_two_byte(self.scope, units, v8::NewStringType::Normal)
            .ok_or(CloneError::DataClone("string exceeds the V8 string limit"))
    }

    fn check(&mut self) -> CloneResult<()> {
        // Native panic is dominant when it coincides with a page budget or
        // watchdog failure; none of these may become a catchable JS exception.
        if native_callback_failure(self.scope).is_some()
            || self.state.native_memory.first_failure().is_some()
            || self.state.termination.requested()
        {
            self.scope.terminate_execution();
            return Err(CloneError::Terminated);
        }
        Ok(())
    }

    fn object_handle(&self, value: v8::Local<'s, v8::Value>) -> v8::Local<'s, v8::Object> {
        v8::Local::<v8::Object>::try_from(value)
            .expect("the native source-kind check established an object")
    }
}

impl<'s, 'i> CloneSource for Adapter<'_, 's, 'i> {
    type Value = v8::Local<'s, v8::Value>;
    type Error = CloneError;

    fn kind(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> CloneResult<SourceKind> {
        if value.is_undefined() {
            return Ok(SourceKind::Undefined);
        }
        if value.is_null() {
            return Ok(SourceKind::Null);
        }
        if value.is_boolean() {
            return Ok(SourceKind::Boolean(value.boolean_value(self.scope)));
        }
        if value.is_number() {
            return Ok(SourceKind::Number(
                value
                    .number_value(self.scope)
                    .ok_or(CloneError::Abrupt)?
                    .to_bits(),
            ));
        }
        if value.is_string() {
            let text = v8::Local::<v8::String>::try_from(*value)
                .expect("the native string brand was checked");
            return Ok(SourceKind::String(self.copy_units(text, workspace)?));
        }
        if !value.is_object() || value.is_proxy() {
            return Ok(SourceKind::Unsupported);
        }
        let global: v8::Local<'s, v8::Value> =
            self.scope.get_current_context().global(self.scope).into();
        if value.strict_equals(global) {
            return Ok(SourceKind::Unsupported);
        }
        if value.is_array_buffer() {
            let buffer = v8::Local::<v8::ArrayBuffer>::try_from(*value)
                .expect("the native ArrayBuffer brand was checked");
            return Ok(
                if buffer.get_backing_store().is_resizable_by_user_javascript() {
                    SourceKind::Unsupported
                } else {
                    SourceKind::ArrayBuffer
                },
            );
        }
        if value.is_array() {
            let array = v8::Local::<v8::Array>::try_from(*value)
                .expect("the native array brand was checked");
            return Ok(SourceKind::Array {
                length: array.length(),
            });
        }
        Ok(if value.is_structured_clone_ordinary_object() {
            SourceKind::Object
        } else {
            SourceKind::Unsupported
        })
    }

    fn identity_hash(&mut self, value: &Self::Value) -> u64 {
        u64::from(value.get_hash())
    }

    fn same_object(&mut self, left: &Self::Value, right: &Self::Value) -> bool {
        left.strict_equals(*right)
    }

    fn enumerable_own_keys(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> CloneResult<Vec<JsString>> {
        let args = v8::GetPropertyNamesArgs {
            mode: v8::KeyCollectionMode::OwnOnly,
            property_filter: v8::PropertyFilter::ONLY_ENUMERABLE | v8::PropertyFilter::SKIP_SYMBOLS,
            index_filter: v8::IndexFilter::IncludeIndices,
            key_conversion: v8::KeyConversionMode::ConvertToString,
        };
        let names = self
            .object_handle(*value)
            .get_own_property_names(self.scope, args)
            .ok_or(CloneError::Abrupt)?;
        self.check()?;
        let mut keys = Vec::new();
        for index in 0..names.length() {
            self.check()?;
            self.charge(workspace, PROPERTY_CHARGE)?;
            keys.try_reserve(1)
                .map_err(|_| self.fail_budget(workspace))?;
            let key = names
                .get_index(self.scope, index)
                .ok_or(CloneError::Abrupt)?;
            let key = v8::Local::<v8::String>::try_from(key)
                .expect("GetOwnPropertyNames converts numeric indices to strings");
            keys.push(self.copy_units(key, workspace)?);
        }
        Ok(keys)
    }

    fn has_own(&mut self, value: &Self::Value, key: &[u16]) -> CloneResult<bool> {
        let key = self.string(key)?;
        let present = self
            .object_handle(*value)
            .has_own_property(self.scope, key.into())
            .ok_or(CloneError::Abrupt)?;
        self.check()?;
        Ok(present)
    }

    fn get(&mut self, value: &Self::Value, key: &[u16]) -> CloneResult<Self::Value> {
        let key = self.string(key)?;
        let value = self
            .object_handle(*value)
            .get(self.scope, key.into())
            .ok_or(CloneError::Abrupt)?;
        self.check()?;
        Ok(value)
    }

    fn copy_buffer(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> CloneResult<Vec<u8>> {
        let buffer = v8::Local::<v8::ArrayBuffer>::try_from(*value)
            .expect("the native ArrayBuffer brand was checked");
        if buffer.was_detached() {
            return Err(CloneError::DataClone("detached ArrayBuffer"));
        }
        let backing = buffer.get_backing_store();
        let length = buffer.byte_length();
        let charge = length
            .checked_mul(2)
            .ok_or_else(|| self.fail_budget(workspace))?;
        self.charge(workspace, charge)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| self.fail_budget(workspace))?;
        for cell in backing.iter().take(length) {
            bytes.push(cell.get());
        }
        self.check()?;
        Ok(bytes)
    }

    fn detach_buffer(&mut self, value: &Self::Value) -> CloneResult<()> {
        let buffer = v8::Local::<v8::ArrayBuffer>::try_from(*value)
            .expect("the native ArrayBuffer brand was checked");
        if buffer.was_detached() || !buffer.is_detachable() {
            return Err(CloneError::DataClone("ArrayBuffer is not detachable"));
        }
        let detached = buffer
            .detach(None)
            .ok_or(CloneError::DataClone("ArrayBuffer detach key"))?;
        if !detached {
            return Err(CloneError::DataClone("ArrayBuffer detach failed"));
        }
        self.check()
    }

    fn data_clone_error(&mut self, reason: &'static str) -> Self::Error {
        CloneError::DataClone(reason)
    }

    fn budget_error(&mut self) -> Self::Error {
        self.scope.terminate_execution();
        CloneError::Terminated
    }

    fn checkpoint(&mut self) -> CloneResult<()> {
        self.check()
    }
}

impl<'s, 'i> CloneTarget for Adapter<'_, 's, 'i> {
    type Value = v8::Local<'s, v8::Value>;
    type Error = CloneError;

    fn undefined(&mut self) -> CloneResult<Self::Value> {
        Ok(v8::undefined(self.scope).into())
    }

    fn null(&mut self) -> CloneResult<Self::Value> {
        Ok(v8::null(self.scope).into())
    }

    fn boolean(&mut self, value: bool) -> CloneResult<Self::Value> {
        Ok(v8::Boolean::new(self.scope, value).into())
    }

    fn number(&mut self, bits: u64) -> CloneResult<Self::Value> {
        Ok(v8::Number::new(self.scope, f64::from_bits(bits)).into())
    }

    fn string(&mut self, units: &[u16]) -> CloneResult<Self::Value> {
        Ok(Adapter::string(self, units)?.into())
    }

    fn array(&mut self, length: u32) -> CloneResult<Self::Value> {
        let array = v8::Array::new(self.scope, 0);
        let key = v8::String::new(self.scope, "length").expect("fixed native key");
        let length = v8::Integer::new_from_unsigned(self.scope, length);
        array
            .set(self.scope, key.into(), length.into())
            .ok_or(CloneError::Abrupt)?;
        self.check()?;
        Ok(array.into())
    }

    fn object(&mut self) -> CloneResult<Self::Value> {
        Ok(v8::Object::new(self.scope).into())
    }

    fn array_buffer(&mut self, bytes: Vec<u8>) -> CloneResult<Self::Value> {
        let buffer = v8::ArrayBuffer::new(self.scope, bytes.len());
        let backing = buffer.get_backing_store();
        for (cell, byte) in backing.iter().zip(bytes) {
            cell.set(byte);
        }
        self.check()?;
        Ok(buffer.into())
    }

    fn define_data(
        &mut self,
        owner: &Self::Value,
        key: &[u16],
        value: &Self::Value,
    ) -> CloneResult<()> {
        let key = Adapter::string(self, key)?;
        let created = self
            .object_handle(*owner)
            .create_data_property(self.scope, key.into(), *value)
            .ok_or(CloneError::Abrupt)?;
        self.check()?;
        if !created {
            return Err(CloneError::Type(
                "structured-clone data property was refused",
            ));
        }
        Ok(())
    }

    fn invalid_wire(&mut self) -> Self::Error {
        CloneError::DataClone("invalid structured-clone wire")
    }

    fn budget_error(&mut self) -> Self::Error {
        self.scope.terminate_execution();
        CloneError::Terminated
    }

    fn checkpoint(&mut self) -> CloneResult<()> {
        self.check()
    }
}

pub(crate) fn callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut retval: v8::ReturnValue<'s, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let Some(state) = scope.get_current_context().get_slot::<HostState>() else {
            scope.terminate_execution();
            return;
        };
        let result = global::run(scope, args, &state);
        if state.native_memory.first_failure().is_some() {
            // A nested page getter can leave an ordinary exception pending
            // after the sticky budget failure. Hard termination dominates it.
            // Keep graph recovery closed until the task consumes the failure.
            state.termination.request();
            // SAFETY: This is the owning isolate's active slow API callback.
            // The clone has returned and dropped its temporary reservations;
            // no further JavaScript may run after the hard budget failure.
            unsafe { scope.terminate_execution_now_from_callback() };
            return;
        }
        match result {
            Ok(value) => retval.set(value),
            Err(CloneError::Abrupt) => {}
            Err(CloneError::Type(message)) => {
                let Some(message) = v8::String::new(scope, message) else {
                    return;
                };
                let error = v8::Exception::type_error(scope, message);
                scope.throw_exception(error);
            }
            Err(CloneError::DataClone(message)) => {
                throw_js_error(scope, &JsError::dom_exception("DataCloneError", message));
            }
            Err(CloneError::Terminated) => {
                scope.terminate_execution();
            }
        }
    };
    // SAFETY: V8 invokes this registered slow Function callback, and the
    // final native-panic tail runs only after the clone body has returned.
    unsafe { v8_registered_slow_callback_boundary(scope, "structuredClone", (), call) };
}
