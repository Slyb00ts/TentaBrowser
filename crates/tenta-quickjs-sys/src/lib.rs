#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::upper_case_acronyms)]
#![allow(clippy::uninlined_format_args)]
#![no_std]

use ::core::ptr;

/// Common error message for converting between C `size_t` and Rust `usize`;
pub const SIZE_T_ERROR: &str =
    "conversion between C type 'size_t' and Rust type 'usize' overflowed.";

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(not(feature = "bindgen"))]
include!(concat!("bindings/", bindings_env!("TARGET"), ".rs"));

#[cfg(target_pointer_width = "64")]
include!("inlines/ptr_64.rs");

#[cfg(target_pointer_width = "32")]
include!("inlines/ptr_32_nan_boxing.rs");

include!("inlines/common.rs");

// The extension adds no fields to public ABI structs. Its scalar ABI is
// shared by bundled and generated bindings (the latter blocklist these names).
mod host_extensions {
    use super::{JSClassID, JSContext, JSRuntime, JSValueConst};

    pub type JSClassCanPreventExtensions =
        Option<unsafe extern "C" fn(ctx: *mut JSContext, obj: JSValueConst) -> core::ffi::c_int>;

    unsafe extern "C" {
        pub fn JS_SetClassCanPreventExtensions(
            rt: *mut JSRuntime,
            class_id: JSClassID,
            callback: JSClassCanPreventExtensions,
        ) -> core::ffi::c_int;
        pub fn JS_RejectPropertyChange(
            ctx: *mut JSContext,
            flags: core::ffi::c_int,
        ) -> core::ffi::c_int;
        pub fn JS_IsResizableArrayBuffer(obj: JSValueConst) -> bool;
        pub fn JS_IsDetachedArrayBuffer(obj: JSValueConst) -> bool;
        pub fn JS_IsOrdinaryObject(obj: JSValueConst) -> bool;
    }
}

pub use host_extensions::*;
