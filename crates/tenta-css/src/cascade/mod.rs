//! The cascade: Stylo's style resolution over the arena document.
//!
//! [`StyleEngine`] turns a [`tenta_dom::Document`] plus author and
//! user-agent sheets into one [`Style`] per element. M1 restyles the whole
//! tree on every changed flush; the snapshot and restyle-hint path that
//! [`StyleDiff::Partial`] requires is a separate piece of work.
//!
//! # Contract the internals must keep
//!
//! - **One word per element handle.** Stylo's style-sharing cache stores
//!   elements as `usize` and asserts equal size at run time, in release too.
//!   [`dom::Handle`] is one reference and a compile-time assertion keeps it so.
//! - **The style store persists.** [`dom::StyleStore`] belongs to the engine,
//!   survives flushes, and is keyed by arena slot *and* generation, because a
//!   freed slot is recycled with a bumped generation and must not inherit the
//!   previous occupant's `ElementData`.
//! - **A view is per flush.** [`dom::FlushView`] holds `&Document`, the handle
//!   table and borrows of the persistent, grow-only atom maps
//!   ([`atoms::AtomMaps`]); it is rebuilt on every flush and never stored.
//! - **One lock.** The engine owns the only `SharedRwLock`. Sheet construction
//!   and `parse_with_loader` (in `cssom.rs`), `TDocument::shared_lock` and
//!   `style=""` parsing must all take [`StyleEngine::lock`]; Stylo asserts at run
//!   time that a guard belongs to the lock of the data it reads, so two locks in
//!   one `Stylist` panic.
//! - **Flush is a native boundary.** It enters `ThreadState::LAYOUT` through
//!   an RAII guard (so `exit` runs on unwind) and runs the traversal under
//!   `catch_unwind`; after a caught panic it clears the thread-local style
//!   sharing cache, whose constructor asserts emptiness and would otherwise
//!   trip on the next flush.
//! - **`Slot`-to-`FlushView` references are acyclic in safe Rust**: slots hold
//!   `&FlushView`, the view holds the slot table through a `OnceCell`, and
//!   neither type has a `Drop` impl.

mod atoms;
mod computed;
mod device;
mod diff;
pub mod dom;
mod engine;
mod imports;
mod media;
mod pseudo;
#[cfg(feature = "test-util")]
pub mod test_util;
mod traverse;
mod types;
mod ua;

pub use atoms::AtomMaps;
pub use engine::StyleEngine;
pub use imports::import_urls;
pub use media::{MediaQueryMatch, match_media};
pub use pseudo::ResolveError;
pub use types::{
    ChangeHint, FontMetrics, FontMetricsData, ImportedSheet, PseudoKind, Style, StyleDiff,
    StyleEnv, StyleSource,
};
