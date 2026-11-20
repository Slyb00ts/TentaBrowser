//! Shared JavaScript engine contracts, generated WebIDL bindings and HTML event loop.
//!
//! V8 and QuickJS use the same DOM hosts, timers, fetch and XHR drivers. Other
//! P2 surfaces and conformance gates remain tracked in `docs/spec/16-roadmap.md`.
//! Dependency direction is enforced by `cargo xtask lint-deps`.

pub mod base64;
pub mod bindings;
pub mod conveniences;
pub mod dom;
pub mod engine;
pub mod event_interfaces;
pub mod event_loop;
pub mod fetch;
pub mod fonts;
pub mod geometry;
pub mod history;
pub mod image;
pub mod inspect;
pub mod location;
pub mod matrix;
pub mod modules;
pub mod rejection;
pub mod scripts;
pub mod selection;
pub mod storage;
pub mod streams;
pub mod structured_clone;
pub mod stylesheets;
pub mod subtle;
pub mod svg_animated;
pub mod traversal;
pub mod xhr;
