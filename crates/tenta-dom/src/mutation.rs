//! The single mutation path (`docs/spec/04-dom-and-css.md` §1) and the
//! observer hook it feeds.
//!
//! Every structural or attribute change to a [`crate::Document`] is a
//! [`Mutation`] passed to `Document::mutate`. There is no second path, so the
//! dirty bits, id and class indexes, mutation generation, detached-root set
//! and observer records can never drift apart.

use tenta_core::{Atom, NodeId};
use tenta_html::QualName;

/// A requested change to the tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mutation {
    /// Appends `child` as the last child of `parent`.
    Append { parent: NodeId, child: NodeId },
    /// Inserts `child` into `parent` before `before`. `before` must be a
    /// child of `parent`; [`NodeId::NONE`] appends.
    InsertBefore {
        parent: NodeId,
        child: NodeId,
        before: NodeId,
    },
    /// Detaches `child` from its parent. A node without a parent is a no-op.
    Remove { child: NodeId },
    /// Replaces `old_child` with `new_child` in `parent`.
    ReplaceChild {
        parent: NodeId,
        new_child: NodeId,
        old_child: NodeId,
    },
    /// Sets an attribute, replacing any existing one with the same name.
    SetAttribute {
        element: NodeId,
        name: QualName,
        value: String,
    },
    /// Removes an attribute; missing attributes are a no-op.
    RemoveAttribute { element: NodeId, name: QualName },
    /// Replaces the data of a text, comment or CDATA node.
    SetText { node: NodeId, text: String },
    /// Moves every child of `from` to the end of `to`, in order. Used by the
    /// tree builder's "adopt" and `reparent_children` steps.
    ReparentChildren { from: NodeId, to: NodeId },
}

/// What happened, in the order it happened. Records own their data so an
/// observer can outlive the borrow of the document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MutationRecord {
    /// `node` was inserted into `parent` after `previous_sibling`.
    Inserted {
        parent: NodeId,
        node: NodeId,
        previous_sibling: NodeId,
    },
    /// `node` was removed from `parent`.
    Removed { parent: NodeId, node: NodeId },
    /// An attribute of `element` changed. `old_value` is `None` when the
    /// attribute did not exist before.
    Attribute {
        element: NodeId,
        name: Atom,
        namespace: Atom,
        old_value: Option<String>,
    },
    /// The data of a character-data node changed.
    CharacterData { node: NodeId, old_value: String },
}

/// Consumer of the mutation record log. Real `MutationObserver` semantics
/// (queues, microtask delivery, observer options) land with the bindings in
/// P2; Tier 1 only needs the hook to exist so `mutate` has one place to
/// report from.
pub trait MutationObserverSink: Send {
    fn record(&mut self, record: MutationRecord);
}

/// The default sink: drops every record.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopObserverSink;

impl MutationObserverSink for NoopObserverSink {
    fn record(&mut self, _record: MutationRecord) {}
}

/// A sink that keeps every record behind a shared handle, so the caller can
/// read the log while the document still owns the sink.
///
/// The handle is `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>` because a
/// document travels from the page thread that parsed it to its caller
/// (`docs/spec/02-architecture.md` §3), so every sink must be `Send`.
#[derive(Clone, Debug, Default)]
pub struct RecordingObserverSink {
    records: std::sync::Arc<std::sync::Mutex<Vec<MutationRecord>>>,
}

impl RecordingObserverSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the records collected so far.
    pub fn take(&self) -> Vec<MutationRecord> {
        std::mem::take(&mut self.records())
    }

    pub fn len(&self) -> usize {
        self.records().len()
    }

    pub fn is_empty(&self) -> bool {
        self.records().is_empty()
    }

    /// The log, recovering from a poisoned lock: a panic in one recorder must
    /// not make the log unreadable.
    fn records(&self) -> std::sync::MutexGuard<'_, Vec<MutationRecord>> {
        self.records.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl MutationObserverSink for RecordingObserverSink {
    fn record(&mut self, record: MutationRecord) {
        self.records().push(record);
    }
}
