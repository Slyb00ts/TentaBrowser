//! The single mutation path (`docs/spec/04-dom-and-css.md` §1) and the
//! observer hook it feeds.
//!
//! Every structural or attribute change to a [`crate::Document`] is a
//! [`Mutation`] passed to `Document::mutate`. There is no second path, so the
//! dirty bits, id and class indexes, mutation generation, detached-root set
//! and observer records can never drift apart.

use tenta_core::{Atom, NodeId, NodeRef};
use tenta_html::{Attribute, QualName};

/// A validated replacement source for an internal replace-all operation.
///
/// `ChildrenOf` accepts a detached fragment or the parser's private
/// construction root inside one. It is not a general node-adoption operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplaceAllWith {
    /// Remove the current children without adding a node.
    Empty,
    /// Add one freshly detached text node, or a doctype for a document parent.
    Detached(NodeId),
    /// Move all children from a detached fragment or parser root.
    ChildrenOf(NodeId),
}

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
    /// Replace every child in one DOM replace-all operation. The semantic
    /// observer record combines the complete old and new child snapshots.
    ReplaceAll {
        parent: NodeId,
        replacement: ReplaceAllWith,
    },
    /// Sets an attribute's value, preserving the existing attribute's prefix
    /// when namespace and local name already match.
    SetAttribute {
        element: NodeId,
        name: QualName,
        value: String,
    },
    /// Replaces an entire attribute node, including its prefix, or appends it
    /// when no attribute with the same namespace and local name exists.
    ReplaceAttribute {
        element: NodeId,
        name: QualName,
        value: String,
    },
    /// Removes an attribute; missing attributes are a no-op.
    RemoveAttribute { element: NodeId, name: QualName },
    /// Adds the parser's missing attributes in one relocation while queueing
    /// one semantic attribute record for each attribute actually added.
    AddMissingAttributes {
        element: NodeId,
        attributes: Vec<Attribute>,
    },
    /// Replaces the data of a text, comment or CDATA node.
    SetText { node: NodeId, text: String },
    /// Extends a parser text node without recopying its existing rope data.
    AppendTextData { node: NodeId, text: String },
    /// Replaces a UTF-16 span in a character-data node, adjusting live ranges.
    ReplaceData {
        node: NodeId,
        offset: u32,
        count: u32,
        data: String,
    },
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

/// Whether a shadow root is reachable through its host (`Element.shadowRoot`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

/// What happened to an element that a custom element definition may have to
/// react to (HTML "custom element reactions"), in the order it happened.
///
/// The log is off until [`crate::Document::enable_lifecycle_log`] turns it on,
/// which a wrapper world does when its page defines the first custom element,
/// so a document without custom elements records nothing. Records name nodes
/// by [`NodeRef`], so a consumer can tell a node that was freed and whose slot
/// was reused from the one the record was made for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LifecycleRecord {
    /// An HTML element whose local name contains a hyphen was created.
    Created(NodeRef),
    /// An element became connected, and its shadow-including subtree with
    /// it: its shadow-including root is now the document.
    Connected(NodeRef),
    /// An element stopped being connected, and its shadow-including subtree
    /// with it.
    Disconnected(NodeRef),
    /// An attribute of an element whose local name contains a hyphen changed.
    /// `old_value` is `None` when the attribute did not exist before.
    Attribute {
        element: NodeRef,
        name: Atom,
        namespace: Atom,
        old_value: Option<String>,
    },
}

/// Consumer of the mutation record log. Real `MutationObserver` semantics
/// (queues, microtask delivery, observer options) land with the bindings in
/// P2; Tier 1 only needs the hook to exist so `mutate` has one place to
/// report from.
pub trait MutationObserverSink: Send {
    fn record(&mut self, record: MutationRecord);

    /// Avoid copying an old text value solely for a diagnostic sink that
    /// discards every record. Semantic observer capture is independent.
    fn wants_record(&self) -> bool {
        true
    }
}

/// The default sink: drops every record.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopObserverSink;

impl MutationObserverSink for NoopObserverSink {
    fn record(&mut self, _record: MutationRecord) {}

    fn wants_record(&self) -> bool {
        false
    }
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
