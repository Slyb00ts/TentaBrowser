//! Code generators. Every entry point is a pure function of the [`Model`].
//!
//! [`Model`]: crate::model::Model

mod convert;
mod ops;
mod prelude;
mod quickjs;
mod register;
mod rust_type;
mod types;
mod v8;

pub use ops::{interface_ops, namespace_ops};
pub use prelude::prelude;
pub use quickjs::{interface_quickjs, namespace_quickjs};
pub use register::{interface_register, namespace_register};
pub use types::types;
pub use v8::{interface_v8, namespace_v8};
