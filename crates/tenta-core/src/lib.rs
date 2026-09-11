//! Foundation types shared by every TentaBrowser crate.
//!
//! This crate is the bottom of the dependency order
//! (`docs/spec/02-architecture.md` §2): identifiers, generational arenas,
//! interned atoms, the error type, budgets, the filesystem policy, tiers,
//! tracing helpers and the embedder traits that let lower layers call
//! services registered by higher layers.

pub mod arena;
pub mod atom;
pub mod budgets;
pub mod embedder;
pub mod error;
pub mod fs_policy;
pub mod ids;
pub mod provenance;
pub mod tier;
pub mod trace;

pub use arena::{Arena, Key};
pub use atom::Atom;
pub use budgets::{BudgetKind, Budgets};
pub use error::{Error, Result};
pub use fs_policy::{DownloadPolicy, FsPolicy};
pub use ids::{ContextId, FrameId, NodeId, NodeRef, PageId, RequestId, WorldId};
pub use provenance::{Origin, RequestClass};
pub use tier::Tier;
