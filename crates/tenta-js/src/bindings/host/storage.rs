//! Window Web Storage bindings over the page's native storage lease.
//!
//! The record contains a live area view, not a copy of its entries. Both
//! methods and named-property hooks therefore observe writes from other pages
//! in the same browser context without running script while the store is locked.

use super::*;
use crate::bindings::{Storage, StorageOps};
use crate::storage::{StorageArea, StorageError, StorageKind};

/// The live area held by one Storage wrapper.
pub struct StorageRecord {
    area: StorageArea,
}

impl StorageRecord {
    /// Identifies the Window-owned root replaced when its area changes.
    pub fn kind(&self) -> StorageKind {
        self.area.kind()
    }
}

fn storage_error(error: StorageError) -> JsError {
    let name = match error {
        StorageError::Security => "SecurityError",
        StorageError::QuotaExceeded => "QuotaExceededError",
    };
    JsError::dom_exception(name, &error.to_string())
}

fn area_record(host: &HostObject) -> Result<StorageArea, JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(JsError::type_error("the receiver is not Storage"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::Storage(record)) = records.get(*key) else {
        return Err(JsError::type_error("the storage area is gone"));
    };
    Ok(record.area.clone())
}

fn storage_handle(ctx: &SharedContext, kind: StorageKind) -> Result<Handle<Storage>, JsError> {
    // A base element can change baseURI; Web Storage always keys the final
    // document URL. Drop the document borrow before entering the storage lock.
    let final_url = ctx.doc.borrow().url().cloned();
    let area = match kind {
        StorageKind::Local => ctx.storage.local_area(final_url.as_ref()),
        StorageKind::Session => ctx.storage.session_area(final_url.as_ref()),
    }
    .map_err(storage_error)?;
    let index = match kind {
        StorageKind::Local => 0,
        StorageKind::Session => 1,
    };
    if let Some(key) = ctx.storage_keys.borrow()[index]
        && let Some(HostRecord::Storage(record)) = ctx.host_records.borrow().get(key)
        && record.area.kind() == kind
        && record.area.origin() == area.origin()
    {
        return Ok(Handle::from_bits(encode_host(ctx.slot, key)));
    }
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::Storage(StorageRecord { area }));
    ctx.storage_keys.borrow_mut()[index] = Some(key);
    Ok(Handle::from_bits(encode_host(ctx.slot, key)))
}

impl HostObject {
    /// The canonical localStorage wrapper for this active Window.
    pub fn local_storage(&self) -> Result<Handle<Storage>, JsError> {
        let HostObject::Window { ctx } = self else {
            return Err(JsError::type_error("the receiver is not a Window"));
        };
        storage_handle(ctx, StorageKind::Local)
    }

    /// The canonical sessionStorage wrapper for this active Window.
    pub fn session_storage(&self) -> Result<Handle<Storage>, JsError> {
        let HostObject::Window { ctx } = self else {
            return Err(JsError::type_error("the receiver is not a Window"));
        };
        storage_handle(ctx, StorageKind::Session)
    }
}

impl StorageOps for HostObject {
    fn length(&self) -> Result<u32, JsError> {
        u32::try_from(area_record(self)?.len().map_err(storage_error)?)
            .map_err(|_| JsError::type_error("too many storage items"))
    }

    fn key(&self, index: u32) -> Result<Option<String>, JsError> {
        area_record(self)?
            .key(index as usize)
            .map_err(storage_error)
    }

    fn get_item(&self, key: &str) -> Result<Option<String>, JsError> {
        area_record(self)?.get_item(key).map_err(storage_error)
    }

    fn set_item(&self, key: &str, value: &str) -> Result<(), JsError> {
        area_record(self)?
            .set_item(key, value)
            .map_err(storage_error)?;
        Ok(())
    }

    fn remove_item(&self, key: &str) -> Result<(), JsError> {
        area_record(self)?.remove_item(key).map_err(storage_error)?;
        Ok(())
    }

    fn clear(&self) -> Result<(), JsError> {
        area_record(self)?.clear().map_err(storage_error)?;
        Ok(())
    }

    fn supported_property_names(&self) -> Result<Vec<String>, JsError> {
        area_record(self)?.keys().map_err(storage_error)
    }

    fn is_supported_property_name(&self, name: &str) -> Result<bool, JsError> {
        area_record(self)?.contains_key(name).map_err(storage_error)
    }
}
