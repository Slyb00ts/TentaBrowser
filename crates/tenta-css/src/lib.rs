//! CSS parsing and CSSOM objects backed by Stylo.
//!
//! The Stylo model is opt-in so Tier 0/1 browser hosts do not pull in a style
//! engine. V8 and QuickJS hosts enable `stylo` for Tier 2 CSSOM; P3 attaches
//! the resulting sheets to the arena-backed cascade.

#[cfg(feature = "stylo")]
pub mod cascade;
#[cfg(feature = "stylo")]
mod cssom;

#[cfg(feature = "stylo")]
pub use cssom::*;
