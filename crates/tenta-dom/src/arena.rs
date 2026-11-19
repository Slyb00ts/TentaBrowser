//! The struct-of-arrays node arena (`docs/spec/04-dom-and-css.md` §1).
//!
//! Every column is a plain `Vec` indexed by [`NodeId`]; there is no per-node
//! heap object and no `Rc` cycle. Per-kind payload lives in the side tables
//! below, addressed by the `data` column.

use std::sync::Arc;

use smallvec::SmallVec;
use tenta_core::{Atom, NodeId};

use crate::interner::LocalName;
use crate::text::TextChunk;

/// Node kinds, one byte in the arena (`docs/spec/04-dom-and-css.md` §1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum NodeKind {
    Element,
    Text,
    Comment,
    Document,
    DocType,
    Fragment,
    ShadowRoot,
    ProcessingInstruction,
    CData,
}

impl NodeKind {
    /// Whether nodes of this kind may have children.
    #[inline]
    pub const fn is_container(self) -> bool {
        matches!(
            self,
            NodeKind::Element | NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot
        )
    }

    /// Whether the node carries character data (`Text`, `Comment`, `CData`).
    #[inline]
    pub const fn is_character_data(self) -> bool {
        matches!(self, NodeKind::Text | NodeKind::Comment | NodeKind::CData)
    }

    /// Whether the node is a `CharacterData` subclass in the DOM Standard, and
    /// so carries the `data`/`length`/splice surface.
    ///
    /// This is `is_character_data` plus `ProcessingInstruction`: a processing
    /// instruction's `data` is its own string field, not a text rope span, so
    /// the two predicates differ only in how the payload is read and written
    /// ([`crate::Document::text_data`] and [`crate::Document::set_text`]).
    #[inline]
    pub const fn is_character_data_subclass(self) -> bool {
        matches!(
            self,
            NodeKind::Text | NodeKind::Comment | NodeKind::CData | NodeKind::ProcessingInstruction
        )
    }
}

/// Per-node bit flags. Bit values are internal; use the accessors.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct NodeFlags(u32);

impl NodeFlags {
    /// The node is reachable from the document node.
    pub const CONNECTED: NodeFlags = NodeFlags(1 << 0);
    /// The element has a non-empty `id` attribute (mirrored in the id index).
    pub const HAS_ID: NodeFlags = NodeFlags(1 << 1);
    /// The element has at least one class (mirrored in the class index).
    pub const HAS_CLASS: NodeFlags = NodeFlags(1 << 2);
    /// A live JavaScript wrapper exists in some world
    /// (`docs/spec/04-dom-and-css.md` §1.1).
    pub const HAS_WRAPPER: NodeFlags = NodeFlags(1 << 3);
    /// Style needs recomputation (placeholder until Tier 3).
    pub const DIRTY_STYLE: NodeFlags = NodeFlags(1 << 4);
    /// Layout needs recomputation (placeholder until Tier 3).
    pub const DIRTY_LAYOUT: NodeFlags = NodeFlags(1 << 5);
    /// The node is the topmost node of a detached subtree.
    pub const DETACHED_ROOT: NodeFlags = NodeFlags(1 << 6);
    /// The cached sweep decision below is valid.
    pub const KEEP_VALID: NodeFlags = NodeFlags(1 << 7);
    /// Cached sweep decision: the subtree must survive.
    pub const KEEP: NodeFlags = NodeFlags(1 << 8);
    /// The element is a `<template>`; its `shadow` slot holds the contents
    /// fragment rather than a shadow root.
    pub const TEMPLATE: NodeFlags = NodeFlags(1 << 9);
    /// The script element was already started (set by the tree builder).
    pub const SCRIPT_STARTED: NodeFlags = NodeFlags(1 << 10);
    /// The element is a MathML `annotation-xml` HTML integration point.
    pub const MATHML_INTEGRATION_POINT: NodeFlags = NodeFlags(1 << 11);
    /// The slot holds a live node. Cleared by [`DomArena::free`], so a
    /// recycled slot is distinguishable from a live one.
    pub const ALIVE: NodeFlags = NodeFlags(1 << 12);
    /// The script element was created by a parser, which runs it itself; a
    /// script without this flag runs when it becomes connected.
    pub const SCRIPT_PARSER_INSERTED: NodeFlags = NodeFlags(1 << 13);
    /// HTML's "force async" of a script element: set on one script creates,
    /// cleared when its `async` IDL attribute is assigned. While it is set the
    /// script runs as soon as it is fetched, with or without an `async`
    /// attribute.
    pub const SCRIPT_FORCE_ASYNC: NodeFlags = NodeFlags(1 << 14);

    #[inline]
    pub const fn contains(self, other: NodeFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn insert(&mut self, other: NodeFlags) {
        self.0 |= other.0;
    }

    #[inline]
    pub const fn remove(&mut self, other: NodeFlags) {
        self.0 &= !other.0;
    }

    #[inline]
    pub const fn set(&mut self, other: NodeFlags, on: bool) {
        if on {
            self.insert(other);
        } else {
            self.remove(other);
        }
    }

    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Index into a per-kind side table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DataIndex(pub u32);

impl DataIndex {
    pub const NONE: DataIndex = DataIndex(u32::MAX);

    #[inline]
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

/// Placeholder for the main-world JavaScript wrapper slot
/// (`docs/spec/05-javascript.md` §3). Tier 1 never fills it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WrapperSlot(pub u32);

impl WrapperSlot {
    pub const EMPTY: WrapperSlot = WrapperSlot(0);
}

/// Half-open range into the document's shared attribute vector.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AttrRange {
    pub start: u32,
    pub len: u32,
}

/// Half-open range into the document's shared class-atom vector.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ClassRange {
    pub start: u32,
    pub len: u32,
}

/// A packed [`tenta_html::QualName`]: `EMPTY` encodes "no namespace" and
/// "no prefix", which saves the 8 bytes an `Option<Atom>` would cost on every
/// attribute of every element (`docs/spec/04-dom-and-css.md` §8 budget).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AttrName {
    pub ns: Atom,
    pub prefix: Atom,
    pub local: Atom,
}

impl AttrName {
    #[inline]
    pub fn from_qual(name: tenta_html::QualName) -> Self {
        Self {
            ns: name.ns,
            prefix: name.prefix.unwrap_or(tenta_core::atom::atoms::EMPTY),
            local: name.local,
        }
    }

    #[inline]
    pub fn to_qual(self) -> tenta_html::QualName {
        tenta_html::QualName {
            ns: self.ns,
            prefix: if self.prefix == tenta_core::atom::atoms::EMPTY {
                None
            } else {
                Some(self.prefix)
            },
            local: self.local,
        }
    }

    /// An attribute in no namespace, the common HTML case.
    #[inline]
    pub fn local(local: Atom) -> Self {
        Self {
            ns: tenta_core::atom::atoms::EMPTY,
            prefix: tenta_core::atom::atoms::EMPTY,
            local,
        }
    }
}

/// Range of an attribute value inside the document's shared value buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AttrValue {
    pub start: u32,
    pub len: u32,
}

impl AttrValue {
    /// Marker for a dead (tombstone) attribute slot.
    pub const DEAD: AttrValue = AttrValue {
        start: u32::MAX,
        len: u32::MAX,
    };

    /// Whether this is a dead slot marker.
    #[inline]
    pub fn is_dead(self) -> bool {
        self == Self::DEAD
    }
}

/// The node columns. All vectors have the same length; freed slots stay in
/// place with a bumped generation. Reusable slots are chained through `next`;
/// exhausted identities remain unlinked tombstones.
#[derive(Default)]
pub struct Nodes {
    pub kind: Vec<NodeKind>,
    pub parent: Vec<NodeId>,
    pub first_child: Vec<NodeId>,
    pub last_child: Vec<NodeId>,
    pub prev: Vec<NodeId>,
    pub next: Vec<NodeId>,
    pub flags: Vec<NodeFlags>,
    pub generation: Vec<u32>,
    pub handles: Vec<u16>,
    pub data: Vec<DataIndex>,
}

impl Nodes {
    /// Bytes of arena columns charged to one node, without per-kind payload.
    pub const BYTES_PER_NODE: usize = size_of::<NodeKind>()
        + 5 * size_of::<NodeId>()
        + size_of::<NodeFlags>()
        + size_of::<u32>()
        + size_of::<u16>()
        + size_of::<DataIndex>();

    #[inline]
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    fn push(&mut self, kind: NodeKind, data: DataIndex) -> NodeId {
        let id = u32::try_from(self.kind.len()).expect("document exceeds u32 nodes");
        self.kind.push(kind);
        self.parent.push(NodeId::NONE);
        self.first_child.push(NodeId::NONE);
        self.last_child.push(NodeId::NONE);
        self.prev.push(NodeId::NONE);
        self.next.push(NodeId::NONE);
        self.flags.push(NodeFlags::ALIVE);
        self.generation.push(0);
        self.handles.push(0);
        self.data.push(data);
        NodeId(id)
    }

    fn reset(&mut self, id: NodeId, kind: NodeKind, data: DataIndex) {
        let i = id.raw() as usize;
        self.kind[i] = kind;
        self.parent[i] = NodeId::NONE;
        self.first_child[i] = NodeId::NONE;
        self.last_child[i] = NodeId::NONE;
        self.prev[i] = NodeId::NONE;
        self.next[i] = NodeId::NONE;
        self.flags[i] = NodeFlags::ALIVE;
        self.handles[i] = 0;
        self.data[i] = data;
    }
}

/// Element columns (`docs/spec/04-dom-and-css.md` §1). `shadow` doubles as
/// the `<template>` contents slot; the [`NodeFlags::TEMPLATE`] flag says which.
#[derive(Default)]
pub struct Elements {
    pub tag: Vec<Atom>,
    pub ns: Vec<Atom>,
    pub attrs: Vec<AttrRange>,
    /// The `id` attribute, interned in the document's own table: ids come
    /// from untrusted markup and must never reach the process-global atom
    /// table (`docs/spec/04-dom-and-css.md` §2).
    pub id: Vec<LocalName>,
    pub classes: Vec<ClassRange>,
    pub shadow: Vec<NodeId>,
    pub main_wrapper: Vec<WrapperSlot>,
}

impl Elements {
    /// Bytes of element columns charged to one element.
    pub const BYTES_PER_ELEMENT: usize = 2 * size_of::<Atom>()
        + size_of::<LocalName>()
        + size_of::<AttrRange>()
        + size_of::<ClassRange>()
        + size_of::<NodeId>()
        + size_of::<WrapperSlot>();

    #[inline]
    pub fn len(&self) -> usize {
        self.tag.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tag.is_empty()
    }
}

/// Text columns: an immutable rope chunk plus a byte range inside it
/// (`docs/spec/02-architecture.md` §7).
#[derive(Default)]
pub struct Texts {
    pub chunk: Vec<Arc<TextChunk>>,
    pub range: Vec<(u32, u32)>,
}

impl Texts {
    #[inline]
    pub fn len(&self) -> usize {
        self.chunk.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.chunk.is_empty()
    }
}

/// A doctype record.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DocTypeData {
    pub name: String,
    pub public_id: String,
    pub system_id: String,
}

/// A processing instruction record.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PiData {
    pub target: String,
    pub data: String,
}

/// The whole node arena: columns plus per-kind side tables and their free
/// lists. Freed node ids are recycled with a bumped generation until their
/// sixteen-bit wrapper identity is exhausted, then retired permanently
/// (`docs/spec/04-dom-and-css.md` §1.1).
pub struct DomArena {
    pub nodes: Nodes,
    pub elements: Elements,
    pub texts: Texts,
    pub comments: Vec<String>,
    pub pis: Vec<PiData>,
    pub doctypes: Vec<DocTypeData>,
    /// Shared attribute storage; `Elements.attrs` indexes into it.
    pub attrs: Vec<(AttrName, AttrValue)>,
    /// Shared class storage; `Elements.classes` indexes into it. Class names
    /// are document-local for the same reason ids are.
    pub class_atoms: Vec<LocalName>,
    /// Shared attribute-value bytes; [`AttrValue`] indexes into it.
    pub attr_values: String,
    /// Entries in `attrs` no element references any more.
    pub attrs_garbage: usize,
    /// Entries in `class_atoms` no element references any more.
    pub class_atoms_garbage: usize,
    /// Bytes in `attr_values` no attribute references any more.
    pub attr_values_garbage: usize,
    free_nodes: NodeId,
    free_elements: Vec<u32>,
    free_texts: Vec<u32>,
    free_comments: Vec<u32>,
    free_pis: Vec<u32>,
    free_doctypes: Vec<u32>,
    live: usize,
    retired_nodes: usize,
}

impl Default for DomArena {
    fn default() -> Self {
        Self::new()
    }
}

impl DomArena {
    pub fn new() -> Self {
        Self {
            nodes: Nodes::default(),
            elements: Elements::default(),
            texts: Texts::default(),
            comments: Vec::new(),
            pis: Vec::new(),
            doctypes: Vec::new(),
            attrs: Vec::new(),
            class_atoms: Vec::new(),
            attr_values: String::new(),
            attrs_garbage: 0,
            class_atoms_garbage: 0,
            attr_values_garbage: 0,
            free_nodes: NodeId::NONE,
            free_elements: Vec::new(),
            free_texts: Vec::new(),
            free_comments: Vec::new(),
            free_pis: Vec::new(),
            free_doctypes: Vec::new(),
            live: 0,
            retired_nodes: 0,
        }
    }

    /// Number of live nodes.
    #[inline]
    pub fn live(&self) -> usize {
        self.live
    }

    /// Number of node slots ever allocated. Every traversal uses this as its
    /// visited-count cap (`docs/spec/04-dom-and-css.md` §1).
    #[inline]
    pub fn slots(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    pub fn generation(&self, id: NodeId) -> u32 {
        self.nodes.generation[id.raw() as usize]
    }

    /// Whether `id` addresses a live slot.
    #[inline]
    pub fn is_live(&self, id: NodeId) -> bool {
        id.is_some()
            && (id.raw() as usize) < self.nodes.len()
            && self.nodes.flags[id.raw() as usize].contains(NodeFlags::ALIVE)
    }

    fn alloc_node(&mut self, kind: NodeKind, data: DataIndex) -> NodeId {
        self.live += 1;
        if self.free_nodes.is_some() {
            let id = self.free_nodes;
            self.free_nodes = self.nodes.next[id.raw() as usize];
            self.nodes.reset(id, kind, data);
            return id;
        }
        self.nodes.push(kind, data)
    }

    pub fn alloc_element(&mut self, tag: Atom, ns: Atom) -> NodeId {
        let data = if let Some(index) = self.free_elements.pop() {
            let i = index as usize;
            self.elements.tag[i] = tag;
            self.elements.ns[i] = ns;
            self.elements.attrs[i] = AttrRange::default();
            self.elements.id[i] = LocalName::NONE;
            self.elements.classes[i] = ClassRange::default();
            self.elements.shadow[i] = NodeId::NONE;
            self.elements.main_wrapper[i] = WrapperSlot::EMPTY;
            DataIndex(index)
        } else {
            let index = u32::try_from(self.elements.len()).expect("element table overflow");
            self.elements.tag.push(tag);
            self.elements.ns.push(ns);
            self.elements.attrs.push(AttrRange::default());
            self.elements.id.push(LocalName::NONE);
            self.elements.classes.push(ClassRange::default());
            self.elements.shadow.push(NodeId::NONE);
            self.elements.main_wrapper.push(WrapperSlot::EMPTY);
            DataIndex(index)
        };
        self.alloc_node(NodeKind::Element, data)
    }

    pub fn alloc_text(
        &mut self,
        kind: NodeKind,
        chunk: Arc<TextChunk>,
        range: (u32, u32),
    ) -> NodeId {
        debug_assert!(matches!(kind, NodeKind::Text | NodeKind::CData));
        let data = if let Some(index) = self.free_texts.pop() {
            let i = index as usize;
            self.texts.chunk[i] = chunk;
            self.texts.range[i] = range;
            DataIndex(index)
        } else {
            let index = u32::try_from(self.texts.len()).expect("text table overflow");
            self.texts.chunk.push(chunk);
            self.texts.range.push(range);
            DataIndex(index)
        };
        self.alloc_node(kind, data)
    }

    pub fn alloc_comment(&mut self, text: String) -> NodeId {
        let data = if let Some(index) = self.free_comments.pop() {
            self.comments[index as usize] = text;
            DataIndex(index)
        } else {
            let index = u32::try_from(self.comments.len()).expect("comment table overflow");
            self.comments.push(text);
            DataIndex(index)
        };
        self.alloc_node(NodeKind::Comment, data)
    }

    pub fn alloc_pi(&mut self, pi: PiData) -> NodeId {
        let data = if let Some(index) = self.free_pis.pop() {
            self.pis[index as usize] = pi;
            DataIndex(index)
        } else {
            let index = u32::try_from(self.pis.len()).expect("pi table overflow");
            self.pis.push(pi);
            DataIndex(index)
        };
        self.alloc_node(NodeKind::ProcessingInstruction, data)
    }

    pub fn alloc_doctype(&mut self, doctype: DocTypeData) -> NodeId {
        let data = if let Some(index) = self.free_doctypes.pop() {
            self.doctypes[index as usize] = doctype;
            DataIndex(index)
        } else {
            let index = u32::try_from(self.doctypes.len()).expect("doctype table overflow");
            self.doctypes.push(doctype);
            DataIndex(index)
        };
        self.alloc_node(NodeKind::DocType, data)
    }

    /// Allocates a container node without payload (document, fragment,
    /// shadow root).
    pub fn alloc_container(&mut self, kind: NodeKind) -> NodeId {
        debug_assert!(matches!(
            kind,
            NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot
        ));
        self.alloc_node(kind, DataIndex::NONE)
    }

    /// Releases the slot of `id`, bumping its generation and recycling the
    /// per-kind payload. Identity slots are retired before the packed u16
    /// generation can repeat. The caller has already unlinked the node.
    pub fn free(&mut self, id: NodeId) {
        let i = id.raw() as usize;
        let data = self.nodes.data[i];
        match self.nodes.kind[i] {
            NodeKind::Element => {
                self.retire_attrs(data);
                self.class_atoms_garbage += self.elements.classes[data.get()].len as usize;
                self.elements.classes[data.get()] = ClassRange::default();
                self.elements.id[data.get()] = LocalName::NONE;
                self.free_elements.push(data.0);
            }
            NodeKind::Text | NodeKind::CData => {
                self.texts.chunk[data.get()] = TextChunk::empty();
                self.texts.range[data.get()] = (0, 0);
                self.free_texts.push(data.0);
            }
            NodeKind::Comment => {
                self.comments[data.get()] = String::new();
                self.free_comments.push(data.0);
            }
            NodeKind::ProcessingInstruction => {
                self.pis[data.get()] = PiData::default();
                self.free_pis.push(data.0);
            }
            NodeKind::DocType => {
                self.doctypes[data.get()] = DocTypeData::default();
                self.free_doctypes.push(data.0);
            }
            NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot => {}
        }
        // Live generations fit the wrapper handle's sixteen-bit field. The
        // final increment remains u32 so even the last NodeRef is invalidated.
        self.nodes.generation[i] += 1;
        self.nodes.flags[i] = NodeFlags::default();
        self.nodes.kind[i] = NodeKind::Fragment;
        self.nodes.data[i] = DataIndex::NONE;
        self.nodes.parent[i] = NodeId::NONE;
        self.nodes.first_child[i] = NodeId::NONE;
        self.nodes.last_child[i] = NodeId::NONE;
        self.nodes.prev[i] = NodeId::NONE;
        self.nodes.handles[i] = 0;
        if self.nodes.generation[i] <= u32::from(u16::MAX) {
            self.nodes.next[i] = self.free_nodes;
            self.free_nodes = id;
        } else {
            self.nodes.next[i] = NodeId::NONE;
            self.retired_nodes += 1;
        }
        self.live -= 1;
    }

    /// Number of recycled slots waiting for reuse.
    pub fn free_slots(&self) -> usize {
        self.nodes.len() - self.live - self.retired_nodes
    }

    /// Attributes of an element, as stored (including dead/tombstone slots).
    #[inline]
    pub fn element_attrs(&self, data: DataIndex) -> &[(AttrName, AttrValue)] {
        let range = self.elements.attrs[data.get()];
        &self.attrs[range.start as usize..range.start as usize + range.len as usize]
    }

    /// Attributes of an element, skipping dead (tombstone) slots.
    pub fn element_attrs_live(&self, data: DataIndex) -> Vec<(AttrName, AttrValue)> {
        self.element_attrs(data)
            .iter()
            .filter(|(_, value)| !value.is_dead())
            .copied()
            .collect()
    }

    /// Classes of an element, as stored.
    #[inline]
    pub fn element_classes(&self, data: DataIndex) -> &[LocalName] {
        let range = self.elements.classes[data.get()];
        &self.class_atoms[range.start as usize..range.start as usize + range.len as usize]
    }

    #[inline]
    pub fn attr_value_str(&self, value: AttrValue) -> &str {
        &self.attr_values[value.start as usize..value.start as usize + value.len as usize]
    }

    /// Stores `value` in the shared buffer.
    pub fn push_attr_value(&mut self, value: &str) -> AttrValue {
        let start = u32::try_from(self.attr_values.len()).expect("attribute buffer overflow");
        self.attr_values.push_str(value);
        AttrValue {
            start,
            len: value.len() as u32,
        }
    }

    /// Rewrites the three shared side vectors, dropping ranges no live
    /// element references any more. `set_element_attrs` and
    /// `set_element_classes` append rather than patch in place, so that an
    /// element's attributes stay one contiguous slice; this reclaims the
    /// abandoned ranges. `Document::mutate` calls it once garbage passes half
    /// of the live bytes, which keeps repeated attribute writes bounded.
    pub fn compact(&mut self) {
        let mut attrs = Vec::with_capacity(self.attrs.len().saturating_sub(self.attrs_garbage));
        let mut class_atoms: Vec<LocalName> = Vec::with_capacity(
            self.class_atoms
                .len()
                .saturating_sub(self.class_atoms_garbage),
        );
        let mut values = String::with_capacity(
            self.attr_values
                .len()
                .saturating_sub(self.attr_values_garbage),
        );
        for slot in 0..self.nodes.len() {
            if !self.nodes.flags[slot].contains(NodeFlags::ALIVE)
                || self.nodes.kind[slot] != NodeKind::Element
            {
                continue;
            }
            let data = self.nodes.data[slot].get();
            let attr_range = self.elements.attrs[data];
            let start = attrs.len() as u32;
            let mut live_count = 0u32;
            for i in 0..attr_range.len as usize {
                let (name, value) = self.attrs[attr_range.start as usize + i];
                // Skip dead (tombstone) attribute slots
                if value.is_dead() {
                    continue;
                }
                let value_start = values.len() as u32;
                values.push_str(
                    &self.attr_values[value.start as usize..(value.start + value.len) as usize],
                );
                attrs.push((
                    name,
                    AttrValue {
                        start: value_start,
                        len: value.len,
                    },
                ));
                live_count += 1;
            }
            self.elements.attrs[data] = AttrRange {
                start,
                len: live_count,
            };

            let class_range = self.elements.classes[data];
            let start = class_atoms.len() as u32;
            class_atoms.extend_from_slice(
                &self.class_atoms
                    [class_range.start as usize..(class_range.start + class_range.len) as usize],
            );
            self.elements.classes[data] = ClassRange {
                start,
                len: class_range.len,
            };
        }
        self.attrs = attrs;
        self.class_atoms = class_atoms;
        self.attr_values = values;
        self.attrs_garbage = 0;
        self.class_atoms_garbage = 0;
        self.attr_values_garbage = 0;
        // Shrink tables if they have excess capacity
        self.shrink_if_beneficial();
    }

    /// Shrinks arena tables if live data is < 1/4 of capacity.
    fn shrink_if_beneficial(&mut self) {
        let live_nodes = self
            .nodes
            .kind
            .iter()
            .filter(|k| **k != NodeKind::Element)
            .count()
            + self.elements.tag.len();
        let capacity = self.nodes.kind.capacity();

        // If live nodes are less than 1/4 of capacity, shrink the tables
        if capacity > 1024 && live_nodes * 4 < capacity {
            self.nodes.kind.shrink_to_fit();
            self.nodes.parent.shrink_to_fit();
            self.nodes.first_child.shrink_to_fit();
            self.nodes.last_child.shrink_to_fit();
            self.nodes.prev.shrink_to_fit();
            self.nodes.next.shrink_to_fit();
            self.nodes.flags.shrink_to_fit();
            self.nodes.generation.shrink_to_fit();
            self.nodes.handles.shrink_to_fit();
            self.nodes.data.shrink_to_fit();

            self.elements.tag.shrink_to_fit();
            self.elements.ns.shrink_to_fit();
            self.elements.id.shrink_to_fit();
            self.elements.classes.shrink_to_fit();
            self.elements.attrs.shrink_to_fit();
            self.elements.shadow.shrink_to_fit();
            self.elements.main_wrapper.shrink_to_fit();

            self.comments.shrink_to_fit();
            self.pis.shrink_to_fit();
            self.doctypes.shrink_to_fit();
            self.texts.chunk.shrink_to_fit();
            self.texts.range.shrink_to_fit();

            self.attrs.shrink_to_fit();
            self.class_atoms.shrink_to_fit();
        }
    }

    /// Whether compaction would reclaim a worthwhile amount of memory.
    pub fn wants_compaction(&self) -> bool {
        const FLOOR: usize = 4096;
        (self.attrs_garbage > FLOOR && self.attrs_garbage * 2 > self.attrs.len())
            || (self.class_atoms_garbage > FLOOR
                && self.class_atoms_garbage * 2 > self.class_atoms.len())
            || (self.attr_values_garbage > FLOOR
                && self.attr_values_garbage * 2 > self.attr_values.len())
    }

    /// Replaces the attribute list of an element. The previous range becomes
    /// garbage, reclaimed by [`DomArena::compact`].
    pub fn set_element_attrs(&mut self, data: DataIndex, attrs: &[(AttrName, AttrValue)]) {
        self.retire_attrs_selective(data, attrs);
        let start = u32::try_from(self.attrs.len()).expect("attribute table overflow");
        self.attrs.extend_from_slice(attrs);
        self.elements.attrs[data.get()] = AttrRange {
            start,
            len: attrs.len() as u32,
        };
    }

    /// Updates one attribute's value in place, preserving its name and list
    /// position. Old value bytes become garbage.
    pub fn replace_element_attr(&mut self, data: DataIndex, index: usize, value: AttrValue) {
        let range = self.elements.attrs[data.get()];
        debug_assert!(index < range.len as usize, "attribute index out of range");
        let slot = range.start as usize + index;
        self.attr_values_garbage += self.attrs[slot].1.len as usize;
        self.attrs[slot].1 = value;
    }

    /// Replaces an attribute node in place: unlike a value write, the incoming
    /// Attr supplies the row's prefix as well as its value.
    pub fn replace_element_attr_node(
        &mut self,
        data: DataIndex,
        index: usize,
        name: AttrName,
        value: AttrValue,
    ) {
        let range = self.elements.attrs[data.get()];
        debug_assert!(index < range.len as usize, "attribute index out of range");
        let slot = range.start as usize + index;
        self.attr_values_garbage += self.attrs[slot].1.len as usize;
        self.attrs[slot] = (name, value);
    }

    /// Marks an attribute slot as dead (tombstone) in place without copying.
    /// The value bytes become garbage and will be reclaimed by compaction.
    pub fn mark_attribute_dead(&mut self, data: DataIndex, index: usize) {
        let range = self.elements.attrs[data.get()];
        debug_assert!(index < range.len as usize, "attribute index out of range");
        let slot = range.start as usize + index;
        let old_value = self.attrs[slot].1;
        if !old_value.is_dead() {
            // Count both the byte size and the record itself
            self.attr_values_garbage += old_value.len as usize;
            self.attrs_garbage += 1;
            self.attrs[slot].1 = AttrValue::DEAD;
        }
    }

    /// Appends one attribute to an element.
    pub fn append_element_attr(&mut self, data: DataIndex, name: AttrName, value: AttrValue) {
        self.append_element_attrs(data, &[(name, value)]);
    }

    /// Appends attributes to an element, keeping its slice contiguous.
    ///
    /// When the element already owns the tail of the shared vector the entries
    /// are pushed in place; otherwise the slice is relocated once. The values
    /// the existing entries point at are still referenced by the copies, so
    /// only the attribute slots become garbage.
    pub fn append_element_attrs(&mut self, data: DataIndex, extra: &[(AttrName, AttrValue)]) {
        if extra.is_empty() {
            return;
        }
        let range = self.elements.attrs[data.get()];
        let end = (range.start + range.len) as usize;
        if range.len == 0 {
            let start = u32::try_from(self.attrs.len()).expect("attribute table overflow");
            self.attrs.extend_from_slice(extra);
            self.elements.attrs[data.get()] = AttrRange {
                start,
                len: extra.len() as u32,
            };
            return;
        }
        if end == self.attrs.len() {
            self.attrs.extend_from_slice(extra);
            self.elements.attrs[data.get()].len += extra.len() as u32;
            return;
        }
        let start = u32::try_from(self.attrs.len()).expect("attribute table overflow");
        for offset in 0..range.len as usize {
            let entry = self.attrs[range.start as usize + offset];
            self.attrs.push(entry);
        }
        self.attrs.extend_from_slice(extra);
        self.attrs_garbage += range.len as usize;
        self.elements.attrs[data.get()] = AttrRange {
            start,
            len: range.len + extra.len() as u32,
        };
    }

    /// Replaces the class list of an element.
    pub fn set_element_classes(&mut self, data: DataIndex, classes: &[LocalName]) {
        self.class_atoms_garbage += self.elements.classes[data.get()].len as usize;
        let start = u32::try_from(self.class_atoms.len()).expect("class table overflow");
        self.class_atoms.extend_from_slice(classes);
        self.elements.classes[data.get()] = ClassRange {
            start,
            len: classes.len() as u32,
        };
    }

    fn retire_attrs(&mut self, data: DataIndex) {
        let range = self.elements.attrs[data.get()];
        for slot in 0..range.len as usize {
            self.attr_values_garbage += self.attrs[range.start as usize + slot].1.len as usize;
        }
        self.attrs_garbage += range.len as usize;
        self.elements.attrs[data.get()] = AttrRange::default();
    }

    /// Retires attributes, counting as garbage only those that truly won't be
    /// referenced anymore. For replaced attributes (same name, different value),
    /// only the old value bytes are counted, not re-kept entries.
    fn retire_attrs_selective(&mut self, data: DataIndex, new_attrs: &[(AttrName, AttrValue)]) {
        let range = self.elements.attrs[data.get()];
        for slot in 0..range.len as usize {
            let (old_name, old_value) = self.attrs[range.start as usize + slot];
            // Find if this attribute exists in the new list with a different value
            if let Some(new_entry) = new_attrs.iter().find(|(n, _)| *n == old_name) {
                if new_entry.1 != old_value {
                    // Value is being replaced, count the old value bytes as garbage
                    self.attr_values_garbage += old_value.len as usize;
                }
                // If both name and value are the same, don't count as garbage
            } else {
                // Attribute is being removed entirely, count its bytes as garbage
                self.attr_values_garbage += old_value.len as usize;
            }
        }
        self.attrs_garbage += range.len as usize;
        self.elements.attrs[data.get()] = AttrRange::default();
    }
}

/// Small inline vector used by the id, class and tag indexes.
pub type NodeList = SmallVec<[NodeId; 1]>;

#[cfg(test)]
mod tests {
    use super::*;
    use tenta_core::atom::atoms;

    #[test]
    fn nodes_and_elements_are_recycled_with_a_bumped_generation() {
        let mut arena = DomArena::new();
        let element = arena.alloc_element(atoms::DIV, atoms::NS_HTML);
        assert!(arena.is_live(element));
        assert_eq!(arena.live(), 1);
        let generation = arena.generation(element);

        arena.free(element);
        assert!(!arena.is_live(element));
        assert_eq!(arena.live(), 0);
        assert_eq!(arena.free_slots(), 1);

        let recycled = arena.alloc_element(atoms::SPAN, atoms::NS_HTML);
        assert_eq!(recycled, element, "the free list hands the slot back");
        assert_eq!(arena.generation(recycled), generation + 1);
        assert_eq!(arena.slots(), 1, "no new slot was allocated");
    }

    #[test]
    fn exhausted_node_identity_retires_but_its_element_payload_is_reused() {
        let mut arena = DomArena::new();
        let first = arena.alloc_element(atoms::DIV, atoms::NS_HTML);
        let payload = arena.nodes.data[first.raw() as usize];
        let mut current = first;
        for generation in 0..=u16::MAX {
            assert_eq!(current, first);
            assert_eq!(arena.generation(current), u32::from(generation));
            arena.free(current);
            if generation != u16::MAX {
                current = arena.alloc_element(atoms::DIV, atoms::NS_HTML);
            }
        }
        assert!(!arena.is_live(first));
        assert_eq!(arena.generation(first), u32::from(u16::MAX) + 1);
        assert_eq!(arena.live(), 0);
        assert_eq!(arena.free_slots(), 0, "retired indices are not reusable");
        assert_eq!(arena.slots(), 1);

        let replacement = arena.alloc_element(atoms::SPAN, atoms::NS_HTML);
        assert_ne!(replacement, first);
        assert_eq!(arena.generation(replacement), 0);
        assert_eq!(arena.nodes.data[replacement.raw() as usize], payload);
        assert_eq!(arena.elements.len(), 1, "payload storage is still recycled");
        assert_eq!(arena.live(), 1);
        arena.free(replacement);
        assert_eq!(arena.free_slots(), 1);
    }

    #[test]
    fn every_kind_has_its_own_recycled_side_table() {
        let mut arena = DomArena::new();
        let comment = arena.alloc_comment("note".to_owned());
        let pi = arena.alloc_pi(PiData {
            target: "t".to_owned(),
            data: "d".to_owned(),
        });
        let doctype = arena.alloc_doctype(DocTypeData {
            name: "html".to_owned(),
            ..Default::default()
        });
        let fragment = arena.alloc_container(NodeKind::Fragment);
        assert_eq!(arena.live(), 4);
        for node in [comment, pi, doctype, fragment] {
            arena.free(node);
        }
        assert_eq!(arena.live(), 0);
        assert_eq!(arena.comments.len(), 1, "the table is reused, not grown");
        let again = arena.alloc_comment("second".to_owned());
        assert_eq!(arena.comments.len(), 1);
        assert_eq!(
            arena.comments[arena.nodes.data[again.raw() as usize].get()],
            "second"
        );
    }

    #[test]
    fn attribute_storage_compacts_once_garbage_dominates() {
        let mut arena = DomArena::new();
        let element = arena.alloc_element(atoms::DIV, atoms::NS_HTML);
        let data = arena.nodes.data[element.raw() as usize];
        for round in 0..2000 {
            let value = arena.push_attr_value(&format!("value-{round}-padding-padding"));
            arena.set_element_attrs(data, &[(AttrName::local(atoms::VALUE), value)]);
        }
        assert!(arena.wants_compaction());
        let before = arena.attr_values.len();
        arena.compact();
        assert!(arena.attr_values.len() < before / 4);
        assert_eq!(arena.attrs.len(), 1);
        assert_eq!(arena.attrs_garbage, 0);
        let stored = arena.element_attrs(data);
        assert_eq!(stored.len(), 1);
        assert_eq!(
            arena.attr_value_str(stored[0].1),
            "value-1999-padding-padding"
        );
    }

    #[test]
    fn flags_are_independent_bits() {
        let mut flags = NodeFlags::default();
        assert!(!flags.contains(NodeFlags::CONNECTED));
        flags.insert(NodeFlags::CONNECTED);
        flags.insert(NodeFlags::HAS_ID);
        assert!(flags.contains(NodeFlags::CONNECTED));
        assert!(flags.contains(NodeFlags::HAS_ID));
        flags.set(NodeFlags::CONNECTED, false);
        assert!(!flags.contains(NodeFlags::CONNECTED));
        assert!(flags.contains(NodeFlags::HAS_ID));
        assert_ne!(flags.bits(), 0);
    }

    #[test]
    fn attr_names_round_trip_through_qualnames() {
        let plain = AttrName::local(atoms::ID);
        assert_eq!(plain.to_qual().prefix, None);
        assert_eq!(AttrName::from_qual(plain.to_qual()), plain);

        let namespaced = AttrName {
            ns: atoms::NS_XLINK,
            prefix: tenta_core::Atom::new("xlink"),
            local: atoms::HREF,
        };
        let qual = namespaced.to_qual();
        assert_eq!(qual.prefix.map(|p| p.as_str()), Some("xlink"));
        assert_eq!(AttrName::from_qual(qual), namespaced);
    }

    #[test]
    fn node_kinds_classify_containers_and_character_data() {
        assert!(NodeKind::Element.is_container());
        assert!(NodeKind::Document.is_container());
        assert!(NodeKind::Fragment.is_container());
        assert!(NodeKind::ShadowRoot.is_container());
        assert!(!NodeKind::Text.is_container());
        assert!(NodeKind::Text.is_character_data());
        assert!(NodeKind::Comment.is_character_data());
        assert!(!NodeKind::Element.is_character_data());
        // A processing instruction carries data, but not in the text rope: it
        // is a `CharacterData` subclass without being `is_character_data`.
        assert!(!NodeKind::ProcessingInstruction.is_character_data());
        assert!(NodeKind::ProcessingInstruction.is_character_data_subclass());
        assert!(NodeKind::Text.is_character_data_subclass());
        assert!(!NodeKind::Element.is_character_data_subclass());
    }
}
