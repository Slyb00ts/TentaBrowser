//! Shared JavaScript engine contracts, generated WebIDL bindings and HTML event loop.
//!
//! V8 and QuickJS use the same DOM hosts, timers, fetch and XHR drivers. Other
//! P2 surfaces and conformance gates remain tracked in `docs/spec/16-roadmap.md`.
//! Dependency direction is enforced by `cargo xtask lint-deps`.

pub mod bindings;
pub mod dom;
pub mod engine;
pub mod event_loop;
pub mod fetch;
pub mod storage;
pub mod structured_clone;
pub mod xhr;
