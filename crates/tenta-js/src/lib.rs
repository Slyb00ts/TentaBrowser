//! Tier 2: engine trait, InterfaceRegistry, event loop, bindings runtime, timers, fetch/XHR/WebSocket, storage, workers.
//!
//! Skeleton crate created in P0; the implementation lands in the phase named
//! for it in `docs/spec/16-roadmap.md`. Dependency direction is enforced by
//! `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

#[allow(unused_imports)]
use tenta_core as _;
#[allow(unused_imports)]
use tenta_css as _;
#[allow(unused_imports)]
use tenta_dom as _;
#[allow(unused_imports)]
use tenta_net as _;

pub mod bindings;
pub mod event_loop;
