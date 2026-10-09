//! QuickJS native adapter for the supported structured-clone wire kinds.
//!
//! Classification and key enumeration use native helpers; temporary copies
//! share the page memory ledger with the engine-neutral wire.

use std::hash::{Hash, Hasher};

use rquickjs::{Array, ArrayBuffer, Ctx, Object, Value};
use tenta_core::{NativeMemoryComponent, NativeMemoryReservation};
use tenta_js::bindings::quickjs_check_native_failure;

use crate::world::World;
use tenta_js::structured_clone::{CloneSource, CloneTarget, JsString, PROPERTY_CHARGE, SourceKind};

mod global;
pub(super) use global::install;

#[cfg(test)]
mod tests;

struct Adapter<'a, 'js> {
    ctx: &'a Ctx<'js>,
    world: &'a World,
}

impl<'a, 'js> Adapter<'a, 'js> {
    fn new(ctx: &'a Ctx<'js>, world: &'a World) -> Self {
        Self { ctx, world }
    }

    fn reservation(&self) -> NativeMemoryReservation {
        self.world
            .host
            .native_memory
            .begin(NativeMemoryComponent::StructuredClone)
    }

    fn charge(
        &self,
        workspace: &mut NativeMemoryReservation,
        bytes: usize,
    ) -> rquickjs::Result<()> {
        if workspace.charge(bytes) {
            Ok(())
        } else {
            Err(World::native_memory_budget_error(self.ctx))
        }
    }

    fn copy_units(
        &self,
        string: &rquickjs::String<'js>,
        workspace: &mut NativeMemoryReservation,
    ) -> rquickjs::Result<JsString> {
        // QuickJS allocates this temporary UTF-16 export in its own engine
        // heap (page engines configure that heap's limit). The native ledger
        // charges the Rust wire copy before it reserves capacity. This is
        // partial accounting.
        let units = string.utf16_units()?;
        let bytes = units.as_ref().len().checked_mul(4).ok_or_else(|| {
            workspace.fail_allocation();
            World::native_memory_budget_error(self.ctx)
        })?;
        self.charge(workspace, bytes)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(units.as_ref().len()).map_err(|_| {
            workspace.fail_allocation();
            World::native_memory_budget_error(self.ctx)
        })?;
        copy.extend_from_slice(units.as_ref());
        Ok(copy)
    }

    fn fail_clone(&self, reason: &'static str) -> rquickjs::Error {
        match tenta_js::bindings::quickjs_dom_exception(self.ctx, "DataCloneError", reason) {
            Ok(value) => self.ctx.throw(value),
            Err(error) => error,
        }
    }

    pub(super) fn check(&self) -> rquickjs::Result<()> {
        quickjs_check_native_failure(self.ctx)?;
        if self.world.host.native_memory.first_failure().is_some() {
            return Err(World::native_memory_budget_error(self.ctx));
        }
        if self.world.script_interrupted() {
            return Err(World::interrupted_error(self.ctx));
        }
        Ok(())
    }

    fn js_key(&self, key: &[u16]) -> rquickjs::Result<Value<'js>> {
        Ok(rquickjs::String::from_utf16(self.ctx.clone(), key)?.into_value())
    }
}

impl<'js> CloneSource for Adapter<'_, 'js> {
    type Value = Value<'js>;
    type Error = rquickjs::Error;

    fn kind(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> rquickjs::Result<SourceKind> {
        if value.is_undefined() {
            return Ok(SourceKind::Undefined);
        }
        if value.is_null() {
            return Ok(SourceKind::Null);
        }
        if let Some(boolean) = value.as_bool() {
            return Ok(SourceKind::Boolean(boolean));
        }
        if let Some(number) = value.as_number() {
            return Ok(SourceKind::Number(number.to_bits()));
        }
        if let Some(string) = value.as_string() {
            return Ok(SourceKind::String(self.copy_units(string, workspace)?));
        }
        if !value.is_object() || value.is_proxy() || value == &self.ctx.globals().into_value() {
            return Ok(SourceKind::Unsupported);
        }
        let raw = value.as_raw();
        // SAFETY: these pinned scalar helpers read native class/metadata only;
        // they neither forward through a Proxy nor invoke page JavaScript.
        unsafe {
            if rquickjs::qjs::JS_IsArrayBuffer(raw) {
                return Ok(if rquickjs::qjs::JS_IsResizableArrayBuffer(raw) {
                    SourceKind::Unsupported
                } else {
                    SourceKind::ArrayBuffer
                });
            }
            if value.is_array() {
                let length = value.as_object().expect("array is object").get("length")?;
                return Ok(SourceKind::Array { length });
            }
            if rquickjs::qjs::JS_IsOrdinaryObject(raw) {
                return Ok(SourceKind::Object);
            }
        }
        Ok(SourceKind::Unsupported)
    }

    fn identity_hash(&mut self, value: &Self::Value) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn same_object(&mut self, left: &Self::Value, right: &Self::Value) -> bool {
        left == right
    }

    fn enumerable_own_keys(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> rquickjs::Result<Vec<JsString>> {
        let object = value
            .as_object()
            .expect("native ordinary/array brand checked");
        let mut keys = Vec::new();
        for key in object.keys::<rquickjs::String>() {
            self.check()?;
            self.charge(workspace, PROPERTY_CHARGE)?;
            keys.try_reserve(1).map_err(|_| {
                workspace.fail_allocation();
                World::native_memory_budget_error(self.ctx)
            })?;
            keys.push(self.copy_units(&key?, workspace)?);
        }
        Ok(keys)
    }

    fn has_own(&mut self, value: &Self::Value, key: &[u16]) -> rquickjs::Result<bool> {
        value
            .as_object()
            .expect("native ordinary/array brand checked")
            .contains_own_key(self.js_key(key)?)
    }

    fn get(&mut self, value: &Self::Value, key: &[u16]) -> rquickjs::Result<Self::Value> {
        value
            .as_object()
            .expect("native ordinary/array brand checked")
            .get(self.js_key(key)?)
    }

    fn copy_buffer(
        &mut self,
        value: &Self::Value,
        workspace: &mut NativeMemoryReservation,
    ) -> rquickjs::Result<Vec<u8>> {
        // SAFETY: the scalar query checks the exact native ArrayBuffer class.
        if unsafe { rquickjs::qjs::JS_IsDetachedArrayBuffer(value.as_raw()) } {
            return Err(self.fail_clone("detached ArrayBuffer"));
        }
        let buffer = value
            .as_object()
            .expect("ArrayBuffer is object")
            .as_array_buffer()
            .expect("native ArrayBuffer brand checked");
        let bytes = buffer
            .as_bytes()
            .ok_or_else(|| self.fail_clone("detached ArrayBuffer"))?;
        let charge = bytes.len().checked_mul(2).ok_or_else(|| {
            workspace.fail_allocation();
            World::native_memory_budget_error(self.ctx)
        })?;
        self.charge(workspace, charge)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len()).map_err(|_| {
            workspace.fail_allocation();
            World::native_memory_budget_error(self.ctx)
        })?;
        copy.extend_from_slice(bytes);
        Ok(copy)
    }

    fn detach_buffer(&mut self, value: &Self::Value) -> rquickjs::Result<()> {
        // SAFETY: native class/immutable/detached queries do not run JS.
        let immutable = unsafe { rquickjs::qjs::JS_IsImmutableArrayBuffer(value.as_raw()) };
        if immutable != 0 {
            return Err(self.fail_clone("ArrayBuffer is not detachable"));
        }
        if unsafe { rquickjs::qjs::JS_IsDetachedArrayBuffer(value.as_raw()) } {
            return Err(self.fail_clone("detached ArrayBuffer"));
        }
        let mut buffer = value
            .as_object()
            .expect("ArrayBuffer is object")
            .as_array_buffer()
            .expect("native ArrayBuffer brand checked")
            .clone();
        buffer.detach();
        self.check()
    }

    fn data_clone_error(&mut self, reason: &'static str) -> Self::Error {
        self.fail_clone(reason)
    }

    fn budget_error(&mut self) -> Self::Error {
        World::native_memory_budget_error(self.ctx)
    }

    fn checkpoint(&mut self) -> rquickjs::Result<()> {
        self.check()
    }
}

impl<'js> CloneTarget for Adapter<'_, 'js> {
    type Value = Value<'js>;
    type Error = rquickjs::Error;

    fn undefined(&mut self) -> rquickjs::Result<Self::Value> {
        Ok(Value::new_undefined(self.ctx.clone()))
    }

    fn null(&mut self) -> rquickjs::Result<Self::Value> {
        Ok(Value::new_null(self.ctx.clone()))
    }

    fn boolean(&mut self, value: bool) -> rquickjs::Result<Self::Value> {
        Ok(Value::new_bool(self.ctx.clone(), value))
    }

    fn number(&mut self, bits: u64) -> rquickjs::Result<Self::Value> {
        Ok(Value::new_number(self.ctx.clone(), f64::from_bits(bits)))
    }

    fn string(&mut self, units: &[u16]) -> rquickjs::Result<Self::Value> {
        Ok(rquickjs::String::from_utf16(self.ctx.clone(), units)?.into_value())
    }

    fn array(&mut self, length: u32) -> rquickjs::Result<Self::Value> {
        let array = Array::new(self.ctx.clone())?;
        array.as_object().set("length", length)?;
        Ok(array.into_value())
    }

    fn object(&mut self) -> rquickjs::Result<Self::Value> {
        Ok(Object::new(self.ctx.clone())?.into_value())
    }

    fn array_buffer(&mut self, bytes: Vec<u8>) -> rquickjs::Result<Self::Value> {
        // A JS-managed copy lets the wire's native reservation end on decode.
        Ok(ArrayBuffer::new_copy(self.ctx.clone(), &bytes)?.into_value())
    }

    fn define_data(
        &mut self,
        owner: &Self::Value,
        key: &[u16],
        value: &Self::Value,
    ) -> rquickjs::Result<()> {
        owner
            .as_object()
            .expect("wire composite has an object")
            .prop(
                self.js_key(key)?,
                rquickjs::object::Property::from(value.clone())
                    .writable()
                    .enumerable()
                    .configurable(),
            )
    }

    fn invalid_wire(&mut self) -> Self::Error {
        self.fail_clone("invalid structured-clone wire")
    }

    fn budget_error(&mut self) -> Self::Error {
        World::native_memory_budget_error(self.ctx)
    }

    fn checkpoint(&mut self) -> rquickjs::Result<()> {
        self.check()
    }
}
