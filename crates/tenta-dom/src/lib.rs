//! Tier 1 DOM: the arena, node lifetime, the single mutation path, selector
//! matching, extraction and serialisation
//! (`docs/spec/04-dom-and-css.md` §1, §2, §4, §5).
//!
//! A [`Document`] owns a struct-of-arrays arena. Nodes are addressed by
//! [`tenta_core::NodeId`]; a [`tenta_core::NodeRef`] adds the generation so a
//! reference kept across a sweep fails loudly instead of aliasing a recycled
//! slot. Every change goes through [`Document::mutate`], which is the only
//! place that touches links, flags, indexes and the observer log.
//!
//! No traversal in this crate recurses: the tree can be built a million nodes
//! deep through the DOM API, and a native stack overflow aborts the process
//! (`docs/spec/02-architecture.md` §3).

pub mod arena;
pub mod document;
pub mod error;
pub mod extract;
pub mod interner;
pub mod mutation;
pub mod selectors;
pub mod serialize;
pub mod shared_sink;
pub mod sink;
pub mod text;

pub use arena::{DocTypeData, NodeFlags, NodeKind, PiData, StyleHandle, WrapperSlot};
pub use document::{
    Document, MemoryReport, NodeHandle, ReadyState, StopPredicate, SweepStats,
    validate_doctype_name, validate_processing_instruction_data,
    validate_processing_instruction_target,
};
pub use interner::{DocInterner, LocalName};
pub use mutation::{
    Mutation, MutationObserverSink, MutationRecord, NoopObserverSink, RecordingObserverSink,
};
pub use selectors::{CompiledSelector, ElementRef, SelectorCache, TentaSelectorImpl};
pub use shared_sink::{SharedDocument, SharedSink};
pub use text::{CHUNK_MAX, CHUNK_TARGET, TextChunk};

/// Depth past which extraction and serialisation stop descending and emit
/// [`TRUNCATION_MARKER`] (`docs/spec/02-architecture.md` §3: DOM APIs can
/// build trees deeper than any native stack can walk).
pub const MAX_TRAVERSAL_DEPTH: usize = 4096;

/// Appended by extractors and serialisers when [`MAX_TRAVERSAL_DEPTH`] was
/// reached, so truncated output is never mistaken for complete output.
pub const TRUNCATION_MARKER: &str = "[tenta: truncated at depth 4096]";
