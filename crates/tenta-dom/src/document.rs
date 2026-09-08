//! The document: the arena, its indexes, the single mutation path, node
//! lifetime and the sweep (`docs/spec/04-dom-and-css.md` §1 and §1.1).

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hashbrown::{HashMap, HashSet};
use tenta_core::atom::atoms;
use tenta_core::{Atom, Error, NodeId, NodeRef, Result};
use tenta_html::{Attribute, ElementFlags, QualName, QuirksMode};
use url::Url;

use crate::arena::{
    AttrName, AttrValue, DocTypeData, DomArena, NodeFlags, NodeKind, NodeList, PiData, StyleHandle,
    WrapperSlot,
};
use crate::error::{hierarchy_request, invalid_state, not_found};
use crate::interner::{DocInterner, LocalName};
use crate::mutation::{Mutation, MutationObserverSink, MutationRecord, NoopObserverSink};
use crate::selectors::SelectorCache;
use crate::text::{TextChunk, TextRope};

/// Where the document is in its lifecycle. Tier 1 only ever reports
/// `Complete` once parsing ends; the real state machine arrives with the
/// bindings.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ReadyState {
    #[default]
    Loading,
    Interactive,
    Complete,
}

/// What a sweep did (`docs/spec/04-dom-and-css.md` §1.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SweepStats {
    /// Detached roots examined.
    pub roots_examined: usize,
    /// Detached roots kept alive by a wrapper or a Rust handle.
    pub roots_kept: usize,
    /// Roots whose decision came from the cached `keep` bit.
    pub roots_cached: usize,
    /// Node slots returned to the free list.
    pub nodes_freed: usize,
}

/// Predicate consulted by [`tenta_html::TreeSink::pop`]; returning `true`
/// stops the parse (`docs/spec/08-search.md` §3 fast path).
///
/// `Send` because a parsed document moves from the page thread that built it
/// to the caller (`docs/spec/02-architecture.md` §3).
pub type StopPredicate = Box<dyn FnMut(&Document, NodeId) -> bool + Send>;

fn next_document_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A strong Rust reference to a node (`docs/spec/04-dom-and-css.md` §1.1).
///
/// A handle keeps its node, and the whole detached subtree the node belongs
/// to, out of the sweep. Handles are created and released through
/// [`Document::handle`], [`Document::clone_handle`] and
/// [`Document::release`]: the count lives in the arena's `handles` column, so
/// `Drop` alone cannot decrement it. Dropping a handle without releasing it is
/// a bug and trips a debug assertion.
#[derive(Debug)]
pub struct NodeHandle {
    node: NodeRef,
    document: u64,
    released: bool,
}

impl NodeHandle {
    /// The generational reference this handle protects.
    #[inline]
    pub fn node_ref(&self) -> NodeRef {
        self.node
    }

    /// The arena index. Use [`Document::resolve`] before trusting it across
    /// mutations.
    #[inline]
    pub fn id(&self) -> NodeId {
        self.node.id
    }

    /// Id of the document this handle belongs to.
    #[inline]
    pub fn document_id(&self) -> u64 {
        self.document
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        debug_assert!(
            self.released || std::thread::panicking(),
            "NodeHandle for {:?} dropped without Document::release; the arena handle count would \
             never reach zero and its detached subtree could never be swept",
            self.node
        );
    }
}

/// A Tier 1 DOM document: the arena, its indexes and its state.
pub struct Document {
    arena: DomArena,
    rope: TextRope,
    open_chunk: Arc<TextChunk>,
    owned_chunk: Arc<TextChunk>,
    document_node: NodeId,
    /// Id values, interned per document (`docs/spec/04-dom-and-css.md` §2).
    ids: DocInterner,
    /// Class values, interned per document.
    class_names: DocInterner,
    /// Prefixes of elements that carry one. Only XML documents fill it, so
    /// HTML pays neither the column nor the lookup.
    element_prefixes: HashMap<NodeId, Atom>,
    /// The id, class and tag indexes. Behind a `RefCell` because a query
    /// sorts the bucket it reads into tree order and caches that, and every
    /// query API takes `&self`. Nothing reachable from selector matching
    /// touches the indexes, so the borrows never nest.
    indexes: RefCell<Indexes>,
    /// Pre-order positions, rebuilt lazily after a structural change.
    order: RefCell<OrderIndex>,
    detached_roots: HashSet<NodeId>,
    /// Shadow root or `<template>` contents fragment to the element that owns
    /// it. Those fragments have no parent but are not independent detached
    /// roots: their lifetime is their host's (`docs/spec/04-dom-and-css.md` §1.1).
    hosts: HashMap<NodeId, NodeId>,
    mutation_generation: u64,
    /// Bumped only by changes to the shape of the tree, which is what
    /// invalidates document order and the sorted index buckets. Attribute and
    /// text edits leave it alone.
    structure_generation: u64,
    mutations_since_sweep: u64,
    observer: Box<dyn MutationObserverSink>,
    document_id: u64,
    url: Option<Url>,
    base_url: Option<Url>,
    /// The first connected `<base>` element in tree order, if any.
    first_connected_base: Option<NodeId>,
    quirks: QuirksMode,
    content_type: String,
    encoding: String,
    scripting_enabled: bool,
    ready_state: ReadyState,
    selector_cache: RefCell<SelectorCache>,
    pub(crate) stop_after: Option<StopPredicate>,
    pub(crate) stopped: bool,
    parse_errors: Vec<String>,
    parse_line: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("id", &self.document_id)
            .field("url", &self.url.as_ref().map(Url::as_str))
            .field("nodes", &self.arena.live())
            .field("quirks", &self.quirks)
            .finish()
    }
}

impl Document {
    /// A new HTML document with only its document node.
    pub fn new() -> Self {
        let mut arena = DomArena::new();
        let document_node = arena.alloc_container(NodeKind::Document);
        arena.nodes.flags[document_node.raw() as usize].insert(NodeFlags::CONNECTED);
        Self {
            arena,
            rope: TextRope::new(),
            open_chunk: TextChunk::open_sentinel(),
            owned_chunk: TextChunk::owned_sentinel(),
            document_node,
            ids: DocInterner::new(),
            class_names: DocInterner::new(),
            element_prefixes: HashMap::new(),
            indexes: RefCell::new(Indexes::default()),
            order: RefCell::new(OrderIndex::default()),
            detached_roots: HashSet::new(),
            hosts: HashMap::new(),
            mutation_generation: 0,
            structure_generation: 1,
            mutations_since_sweep: 0,
            observer: Box::new(NoopObserverSink),
            document_id: next_document_id(),
            url: None,
            base_url: None,
            first_connected_base: None,
            quirks: QuirksMode::NoQuirks,
            content_type: "text/html".to_owned(),
            encoding: "UTF-8".to_owned(),
            scripting_enabled: true,
            ready_state: ReadyState::Loading,
            selector_cache: RefCell::new(SelectorCache::new()),
            stop_after: None,
            stopped: false,
            parse_errors: Vec::new(),
            parse_line: 0,
        }
    }

    /// A new document with a known address; `base_url` starts as the URL.
    pub fn with_url(url: Url) -> Self {
        let mut document = Self::new();
        document.set_url(Some(url));
        document
    }

    // ---------------------------------------------------------------- state

    /// Process-unique id, used to catch handles crossing documents.
    #[inline]
    pub fn document_id(&self) -> u64 {
        self.document_id
    }

    /// The document node itself.
    #[inline]
    pub fn document_node(&self) -> NodeId {
        self.document_node
    }

    #[inline]
    pub fn url(&self) -> Option<&Url> {
        self.url.as_ref()
    }

    /// Sets the document URL and resets the base URL to it; a later
    /// [`Document::refresh_base_url`] re-applies any `<base href>`.
    pub fn set_url(&mut self, url: Option<Url>) {
        self.base_url = url.clone();
        self.url = url;
        self.refresh_base_url();
    }

    /// The base URL used to resolve relative references: `<base href>`
    /// resolved against the document URL, or the document URL.
    #[inline]
    pub fn base_url(&self) -> Option<&Url> {
        self.base_url.as_ref()
    }

    /// Re-reads the first `<base href>` in tree order. Called by the sink
    /// when a `<base>` element is inserted and by [`Document::set_url`].
    pub fn refresh_base_url(&mut self) {
        let mut base = self.url.clone();
        // Use the cached first connected base if it's still in the tree.
        // Only walk all bases if the cache is empty or the cached base was removed.
        let first_base = if let Some(cached) = self.first_connected_base {
            if self.is_connected(cached) {
                Some(cached)
            } else {
                // Cached base was removed; find the new first connected base.
                let found = self.with_elements_by_tag(atoms::BASE, |elements| {
                    elements.iter().copied().find(|id| self.is_connected(*id))
                });
                self.first_connected_base = found;
                found
            }
        } else {
            // First call or no bases yet; find the first connected base.
            let found = self.with_elements_by_tag(atoms::BASE, |elements| {
                elements.iter().copied().find(|id| self.is_connected(*id))
            });
            self.first_connected_base = found;
            found
        };

        if let Some(element) = first_base
            && let Some(href) = self.attribute(element, atoms::HREF)
        {
            let resolved = match self.url.as_ref() {
                Some(url) => url.join(href).ok(),
                None => Url::parse(href).ok(),
            };
            if let Some(resolved) = resolved {
                base = Some(resolved);
            }
        }
        self.base_url = base;
    }

    /// Resolves `href` against the base URL.
    pub fn resolve_url(&self, href: &str) -> Option<Url> {
        match self.base_url.as_ref() {
            Some(base) => base.join(href).ok(),
            None => Url::parse(href).ok(),
        }
    }

    #[inline]
    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks
    }

    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        if self.quirks == mode {
            return;
        }
        self.quirks = mode;
        // If the mode changes after elements are indexed, re-key the id/class indexes.
        // In standards mode, keys are exact values. In quirks mode, keys are lowercase.
        self.rekey_id_class_indexes();
    }

    /// Re-keys the id and class indexes when quirks mode changes. Standards
    /// mode keys a bucket by the exact value; quirks mode by its ASCII
    /// lowercase form, with one interner reference per (element, value) held
    /// by the index. Entries for detached elements and template contents are
    /// kept.
    fn rekey_id_class_indexes(&mut self) {
        // `self.quirks` has already flipped.
        let was_quirks = self.quirks != QuirksMode::Quirks;
        let indexes = self.indexes.get_mut();
        let mut elements: Vec<NodeId> = indexes
            .ids
            .buckets
            .values()
            .chain(indexes.classes.buckets.values())
            .flat_map(|bucket| bucket.nodes.iter().copied())
            .collect();
        indexes.ids.clear();
        indexes.classes.clear();
        elements.sort_unstable();
        elements.dedup();
        for element in elements {
            let data = self.arena.nodes.data[self.index(element)];
            let id = self.arena.elements.id[data.get()];
            if id.is_some() {
                if was_quirks
                    && let Some(old_key) = self.ids.get(&self.ids.resolve(id).to_ascii_lowercase())
                {
                    self.ids.release(old_key);
                }
                let key = self.take_id_index_key(id);
                self.indexes.get_mut().ids.push(key, element);
            }
            let classes = self.arena.element_classes(data).to_vec();
            let mut keys: Vec<LocalName> = Vec::with_capacity(classes.len());
            for class in classes {
                if was_quirks
                    && let Some(old_key) = self
                        .class_names
                        .get(&self.class_names.resolve(class).to_ascii_lowercase())
                {
                    self.class_names.release(old_key);
                }
                let key = self.take_class_index_key(class);
                if !keys.contains(&key) {
                    keys.push(key);
                    self.indexes.get_mut().classes.push(key, element);
                }
            }
        }
    }

    /// The bucket key for an id an element carries: the handle itself in
    /// standards mode, a fresh reference to its lowercase form in quirks mode.
    fn take_id_index_key(&mut self, id: LocalName) -> LocalName {
        if self.quirks == QuirksMode::Quirks {
            let folded = self.ids.resolve(id).to_ascii_lowercase();
            self.ids.intern(&folded)
        } else {
            id
        }
    }

    /// The key `take_id_index_key` produced for `id`, without a new reference.
    fn id_index_key(&self, id: LocalName) -> Option<LocalName> {
        if self.quirks == QuirksMode::Quirks {
            let key = self.ids.get(&self.ids.resolve(id).to_ascii_lowercase());
            debug_assert!(key.is_some(), "quirks id keys stay interned while indexed");
            key
        } else {
            Some(id)
        }
    }

    fn take_class_index_key(&mut self, class: LocalName) -> LocalName {
        if self.quirks == QuirksMode::Quirks {
            let folded = self.class_names.resolve(class).to_ascii_lowercase();
            self.class_names.intern(&folded)
        } else {
            class
        }
    }

    fn class_index_key(&self, class: LocalName) -> Option<LocalName> {
        if self.quirks == QuirksMode::Quirks {
            let key = self
                .class_names
                .get(&self.class_names.resolve(class).to_ascii_lowercase());
            debug_assert!(
                key.is_some(),
                "quirks class keys stay interned while indexed"
            );
            key
        } else {
            Some(class)
        }
    }

    /// Drops one element from an id bucket and, in quirks mode, the reference
    /// the key held for it.
    fn drop_id_index_entry(&mut self, id: LocalName, element: NodeId) {
        let Some(key) = self.id_index_key(id) else {
            return;
        };
        self.indexes.get_mut().ids.remove(key, element);
        if self.quirks == QuirksMode::Quirks {
            self.ids.release(key);
        }
    }

    fn drop_class_index_entry(&mut self, class: LocalName, element: NodeId) {
        let Some(key) = self.class_index_key(class) else {
            return;
        };
        self.indexes.get_mut().classes.remove(key, element);
        if self.quirks == QuirksMode::Quirks {
            self.class_names.release(key);
        }
    }

    #[inline]
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub fn set_content_type(&mut self, content_type: impl Into<String>) {
        self.content_type = content_type.into();
    }

    /// Whether this document uses HTML rules for tag and attribute name case
    /// and for class matching.
    #[inline]
    pub fn is_html_document(&self) -> bool {
        !self.content_type.contains("xml")
    }

    #[inline]
    pub fn encoding(&self) -> &str {
        &self.encoding
    }

    pub fn set_encoding(&mut self, encoding: impl Into<String>) {
        self.encoding = encoding.into();
    }

    /// Whether scripting was enabled during parsing. Affects `<noscript>` and
    /// `<template>` content handling.
    pub fn scripting_enabled(&self) -> bool {
        self.scripting_enabled
    }

    pub fn set_scripting_enabled(&mut self, enabled: bool) {
        self.scripting_enabled = enabled;
    }

    #[inline]
    pub fn ready_state(&self) -> ReadyState {
        self.ready_state
    }

    pub fn set_ready_state(&mut self, state: ReadyState) {
        self.ready_state = state;
    }

    /// Bumped by every mutation; live collections cache against it
    /// (`docs/spec/04-dom-and-css.md` §4).
    #[inline]
    pub fn mutation_generation(&self) -> u64 {
        self.mutation_generation
    }

    /// Mutations since the last [`Document::sweep_detached`]; the page thread
    /// uses it to decide when to sweep.
    #[inline]
    pub fn mutations_since_sweep(&self) -> u64 {
        self.mutations_since_sweep
    }

    /// Installs the mutation record consumer, returning the previous one.
    pub fn set_observer_sink(
        &mut self,
        sink: Box<dyn MutationObserverSink>,
    ) -> Box<dyn MutationObserverSink> {
        std::mem::replace(&mut self.observer, sink)
    }

    /// Installs the parse stop predicate used by
    /// [`tenta_html::TreeSink::pop`].
    pub fn set_stop_after(&mut self, predicate: Option<StopPredicate>) {
        self.stop_after = predicate;
        self.stopped = false;
    }

    /// Whether the stop predicate fired during parsing.
    #[inline]
    pub fn parse_stopped(&self) -> bool {
        self.stopped
    }

    /// Parse errors reported by the tree builder, capped so a hostile
    /// document cannot grow the list without bound.
    #[inline]
    pub fn parse_errors(&self) -> &[String] {
        &self.parse_errors
    }

    /// The source line the tree builder last reported.
    #[inline]
    pub fn parse_line(&self) -> u64 {
        self.parse_line
    }

    pub(crate) fn record_parse_error(&mut self, message: std::borrow::Cow<'static, str>) {
        const MAX_PARSE_ERRORS: usize = 256;
        if self.parse_errors.len() < MAX_PARSE_ERRORS {
            self.parse_errors.push(message.into_owned());
        }
    }

    pub(crate) fn set_parse_line(&mut self, line: u64) {
        self.parse_line = line;
    }

    /// Sets or clears a node flag.
    ///
    /// Crate-internal: clearing `ALIVE` on a linked node would leave a live
    /// parent pointing at a dead slot, and clearing `CONNECTED`, `HAS_ID` or
    /// `HAS_CLASS` would desynchronise a subtree from the indexes. External
    /// callers get the named setters below, which maintain what they touch.
    pub(crate) fn set_flag(&mut self, id: NodeId, flag: NodeFlags, on: bool) {
        if !self.arena.is_live(id) {
            return;
        }
        let slot = self.index(id);
        self.arena.nodes.flags[slot].set(flag, on);
    }

    /// Marks a `<script>` as already started, as the tree builder requires of
    /// a script element it re-encounters.
    pub fn set_script_started(&mut self, id: NodeId, started: bool) {
        self.set_flag(id, NodeFlags::SCRIPT_STARTED, started);
    }

    /// Whether the script element was already started.
    #[inline]
    pub fn script_started(&self, id: NodeId) -> bool {
        self.flags(id).contains(NodeFlags::SCRIPT_STARTED)
    }

    /// Marks style as needing recomputation. Tier 3 clears it.
    pub fn set_dirty_style(&mut self, id: NodeId, dirty: bool) {
        self.set_flag(id, NodeFlags::DIRTY_STYLE, dirty);
    }

    /// Marks layout as needing recomputation. Tier 3 clears it.
    pub fn set_dirty_layout(&mut self, id: NodeId, dirty: bool) {
        self.set_flag(id, NodeFlags::DIRTY_LAYOUT, dirty);
    }

    // ------------------------------------------------------------ inspection

    /// Live nodes in the arena.
    #[inline]
    pub fn len(&self) -> usize {
        self.arena.live()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.arena.live() == 0
    }

    /// Node slots ever allocated, including recycled ones. Every traversal
    /// caps its visited count at this value.
    #[inline]
    pub fn slots(&self) -> usize {
        self.arena.slots()
    }

    #[inline]
    pub(crate) fn arena(&self) -> &DomArena {
        &self.arena
    }

    #[inline]
    fn index(&self, id: NodeId) -> usize {
        id.raw() as usize
    }

    /// Whether `id` addresses a live node of this document.
    #[inline]
    pub fn is_live(&self, id: NodeId) -> bool {
        self.arena.is_live(id)
    }

    /// A generational reference to `id`.
    pub fn node_ref(&self, id: NodeId) -> NodeRef {
        NodeRef::new(id, self.arena.generation(id))
    }

    /// Resolves a reference that may have gone stale. Defence in depth: a
    /// host that kept a `NodeRef` across a sweep gets an error instead of a
    /// recycled node (`docs/spec/04-dom-and-css.md` §1.1).
    pub fn resolve(&self, reference: NodeRef) -> Result<NodeId> {
        if self.arena.is_live(reference.id)
            && self.arena.generation(reference.id) == reference.generation
        {
            Ok(reference.id)
        } else {
            Err(invalid_state(format!(
                "node reference {:?} no longer resolves in document {}",
                reference, self.document_id
            )))
        }
    }

    #[inline]
    pub fn kind(&self, id: NodeId) -> NodeKind {
        self.arena.nodes.kind[self.index(id)]
    }

    #[inline]
    pub fn flags(&self, id: NodeId) -> NodeFlags {
        self.arena.nodes.flags[self.index(id)]
    }

    #[inline]
    pub fn parent(&self, id: NodeId) -> NodeId {
        self.arena.nodes.parent[self.index(id)]
    }

    #[inline]
    pub fn first_child(&self, id: NodeId) -> NodeId {
        self.arena.nodes.first_child[self.index(id)]
    }

    #[inline]
    pub fn last_child(&self, id: NodeId) -> NodeId {
        self.arena.nodes.last_child[self.index(id)]
    }

    #[inline]
    pub fn previous_sibling(&self, id: NodeId) -> NodeId {
        self.arena.nodes.prev[self.index(id)]
    }

    #[inline]
    pub fn next_sibling(&self, id: NodeId) -> NodeId {
        self.arena.nodes.next[self.index(id)]
    }

    #[inline]
    pub fn is_connected(&self, id: NodeId) -> bool {
        self.flags(id).contains(NodeFlags::CONNECTED)
    }

    #[inline]
    pub fn is_element(&self, id: NodeId) -> bool {
        self.kind(id) == NodeKind::Element
    }

    /// Live handle count for `id`.
    #[inline]
    pub fn handle_count(&self, id: NodeId) -> u16 {
        self.arena.nodes.handles[self.index(id)]
    }

    /// The children of `id`, in order.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut next = if self.is_live(id) {
            self.first_child(id)
        } else {
            NodeId::NONE
        };
        let cap = self.slots();
        let mut seen = 0usize;
        std::iter::from_fn(move || {
            if !next.is_some() || seen >= cap {
                return None;
            }
            seen += 1;
            let current = next;
            next = self.next_sibling(current);
            Some(current)
        })
    }

    /// The element children of `id`, in order.
    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id).filter(|child| self.is_element(*child))
    }

    /// Ancestors of `id`, closest first, capped at the arena length so a
    /// corrupt tree ends the walk instead of looping.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut current = if self.is_live(id) {
            self.parent(id)
        } else {
            NodeId::NONE
        };
        let cap = self.slots();
        let mut seen = 0usize;
        std::iter::from_fn(move || {
            if !current.is_some() || seen >= cap {
                return None;
            }
            seen += 1;
            let node = current;
            current = self.parent(node);
            Some(node)
        })
    }

    /// The topmost ancestor of `id` (itself when it has no parent).
    pub fn root_of(&self, id: NodeId) -> NodeId {
        self.ancestors(id).last().unwrap_or(id)
    }

    /// Whether `ancestor` is `node` or one of its ancestors.
    pub fn is_inclusive_ancestor(&self, ancestor: NodeId, node: NodeId) -> bool {
        ancestor == node || self.ancestors(node).any(|a| a == ancestor)
    }

    /// The next node of a pre-order walk of the subtree rooted at `root`,
    /// or [`NodeId::NONE`] at the end. Constant stack: the walk keeps no
    /// state beyond the current node (`docs/spec/02-architecture.md` §3).
    pub fn next_in_subtree(&self, node: NodeId, root: NodeId) -> NodeId {
        let child = self.first_child(node);
        if child.is_some() {
            return child;
        }
        let mut current = node;
        let cap = self.slots();
        for _ in 0..cap {
            if current == root {
                return NodeId::NONE;
            }
            let sibling = self.next_sibling(current);
            if sibling.is_some() {
                return sibling;
            }
            current = self.parent(current);
            if !current.is_some() {
                return NodeId::NONE;
            }
        }
        NodeId::NONE
    }

    /// Pre-order iteration of the subtree rooted at `root`, `root` included.
    pub fn descendants_inclusive(&self, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut current = if self.is_live(root) {
            root
        } else {
            NodeId::NONE
        };
        let cap = self.slots();
        let mut seen = 0usize;
        std::iter::from_fn(move || {
            if !current.is_some() || seen >= cap {
                return None;
            }
            seen += 1;
            let node = current;
            current = self.next_in_subtree(node, root);
            Some(node)
        })
    }

    /// Pre-order iteration of the strict descendants of `root`.
    pub fn descendants(&self, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.descendants_inclusive(root).skip(1)
    }

    /// The document element (`<html>`), if the document has one.
    pub fn document_element(&self) -> Option<NodeId> {
        self.element_children(self.document_node).next()
    }

    /// The `<head>` element, if present.
    pub fn head(&self) -> Option<NodeId> {
        let root = self.document_element()?;
        self.element_children(root)
            .find(|id| self.tag(*id) == Some(atoms::HEAD))
    }

    /// The `<body>` element, if present.
    pub fn body(&self) -> Option<NodeId> {
        let root = self.document_element()?;
        self.element_children(root)
            .find(|id| matches!(self.tag(*id), Some(t) if t == atoms::BODY || t == atoms::FRAMESET))
    }

    // ------------------------------------------------------------- elements

    /// The local name of an element.
    pub fn tag(&self, id: NodeId) -> Option<Atom> {
        if self.kind(id) != NodeKind::Element {
            return None;
        }
        Some(self.arena.elements.tag[self.arena.nodes.data[self.index(id)].get()])
    }

    /// The local name of an element, without the kind check. Callers that
    /// already know the node is an element use this on hot extraction paths.
    #[inline]
    pub(crate) fn element_tag(&self, id: NodeId) -> Atom {
        self.arena.elements.tag[self.arena.nodes.data[self.index(id)].get()]
    }

    /// The namespace of an element.
    pub fn namespace(&self, id: NodeId) -> Option<Atom> {
        if self.kind(id) != NodeKind::Element {
            return None;
        }
        Some(self.arena.elements.ns[self.arena.nodes.data[self.index(id)].get()])
    }

    /// The element's qualified name, as the tree builder sees it.
    pub fn element_name(&self, id: NodeId) -> Option<QualName> {
        Some(QualName {
            ns: self.namespace(id)?,
            prefix: self.element_prefix(id),
            local: self.tag(id)?,
        })
    }

    /// The element's `id` attribute, or `None` when it has none.
    ///
    /// Values are document-local strings, never process-global atoms: an id
    /// comes from untrusted markup (`docs/spec/04-dom-and-css.md` §2).
    pub fn element_id(&self, id: NodeId) -> Option<&str> {
        if self.kind(id) != NodeKind::Element {
            return None;
        }
        let name = self.arena.elements.id[self.arena.nodes.data[self.index(id)].get()];
        name.is_some().then(|| self.ids.resolve(name))
    }

    /// The element's class list, in source order.
    pub fn classes(&self, id: NodeId) -> impl Iterator<Item = &str> + '_ {
        let classes: &[LocalName] = if self.kind(id) == NodeKind::Element {
            self.arena
                .element_classes(self.arena.nodes.data[self.index(id)])
        } else {
            &[]
        };
        classes.iter().map(|name| self.class_names.resolve(*name))
    }

    /// Number of classes on an element.
    pub fn class_count(&self, id: NodeId) -> usize {
        if self.kind(id) != NodeKind::Element {
            return 0;
        }
        self.arena
            .element_classes(self.arena.nodes.data[self.index(id)])
            .len()
    }

    /// Whether the element carries `class`, compared exactly.
    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        let Some(key) = self.class_names.get(class) else {
            return false;
        };
        self.kind(id) == NodeKind::Element
            && self
                .arena
                .element_classes(self.arena.nodes.data[self.index(id)])
                .contains(&key)
    }

    /// The namespace prefix the element was written with, if any. Only XML
    /// documents carry one; it is what makes `to_xml` round-trip.
    pub fn element_prefix(&self, id: NodeId) -> Option<Atom> {
        self.element_prefixes.get(&id).copied()
    }

    /// Opaque style handle; Tier 3 fills it (`docs/spec/04-dom-and-css.md` §6).
    pub fn style_data(&self, id: NodeId) -> StyleHandle {
        if self.kind(id) != NodeKind::Element {
            return StyleHandle::NONE;
        }
        self.arena.elements.style_data[self.arena.nodes.data[self.index(id)].get()]
    }

    /// The main-world wrapper slot; the bindings own its contents (P2).
    pub fn main_wrapper(&self, id: NodeId) -> WrapperSlot {
        if self.kind(id) != NodeKind::Element {
            return WrapperSlot::EMPTY;
        }
        self.arena.elements.main_wrapper[self.arena.nodes.data[self.index(id)].get()]
    }

    /// The shadow root of an element, if it has one. `<template>` elements
    /// keep their contents fragment in the same slot; see
    /// [`Document::template_contents`].
    pub fn shadow_root(&self, id: NodeId) -> Option<NodeId> {
        if self.kind(id) != NodeKind::Element || self.flags(id).contains(NodeFlags::TEMPLATE) {
            return None;
        }
        let slot = self.arena.elements.shadow[self.arena.nodes.data[self.index(id)].get()];
        slot.is_some().then_some(slot)
    }

    /// The contents fragment of a `<template>` element.
    pub fn template_contents(&self, id: NodeId) -> Option<NodeId> {
        if !self.flags(id).contains(NodeFlags::TEMPLATE) {
            return None;
        }
        let slot = self.arena.elements.shadow[self.arena.nodes.data[self.index(id)].get()];
        slot.is_some().then_some(slot)
    }

    /// Attributes of an element in source order.
    pub fn attributes(&self, id: NodeId) -> impl Iterator<Item = (QualName, &str)> + '_ {
        let attrs: &[(AttrName, AttrValue)] = if self.kind(id) == NodeKind::Element {
            self.arena
                .element_attrs(self.arena.nodes.data[self.index(id)])
        } else {
            &[]
        };
        attrs
            .iter()
            .filter(move |(_, value)| !value.is_dead())
            .map(move |(name, value)| (name.to_qual(), self.arena.attr_value_str(*value)))
    }

    /// Number of attributes on an element (excluding dead/tombstone slots).
    pub fn attribute_count(&self, id: NodeId) -> usize {
        if self.kind(id) != NodeKind::Element {
            return 0;
        }
        self.arena
            .element_attrs(self.arena.nodes.data[self.index(id)])
            .iter()
            .filter(|(_, value)| !value.is_dead())
            .count()
    }

    /// The value of a no-namespace attribute.
    pub fn attribute(&self, id: NodeId, name: Atom) -> Option<&str> {
        self.attribute_ns(id, atoms::EMPTY, name)
    }

    /// The value of an attribute in a namespace.
    pub fn attribute_ns(&self, id: NodeId, ns: Atom, name: Atom) -> Option<&str> {
        if self.kind(id) != NodeKind::Element {
            return None;
        }
        self.arena
            .element_attrs(self.arena.nodes.data[self.index(id)])
            .iter()
            .find(|(stored, value)| !value.is_dead() && stored.local == name && stored.ns == ns)
            .map(|(_, value)| self.arena.attr_value_str(*value))
    }

    /// Whether the element carries the attribute.
    pub fn has_attribute(&self, id: NodeId, name: Atom) -> bool {
        self.attribute(id, name).is_some()
    }

    /// Convenience: an attribute looked up by string, without interning it.
    /// Untrusted names must never grow the atom table
    /// (`docs/spec/02-architecture.md` §7).
    pub fn attribute_str(&self, id: NodeId, name: &str) -> Option<&str> {
        let atom = Atom::get(name)?;
        self.attribute(id, atom)
    }

    // ------------------------------------------------------------ character

    /// The data of a text or CDATA node. The slice may point into a shared
    /// rope chunk or into the open buffer.
    pub fn text_data(&self, id: NodeId) -> Option<&str> {
        if !self.kind(id).is_character_data() {
            return None;
        }
        match self.kind(id) {
            NodeKind::Comment => {
                Some(&self.arena.comments[self.arena.nodes.data[self.index(id)].get()])
            }
            _ => {
                let data = self.arena.nodes.data[self.index(id)];
                let (start, len) = self.arena.texts.range[data.get()];
                let chunk = &self.arena.texts.chunk[data.get()];
                if Arc::ptr_eq(chunk, &self.owned_chunk) {
                    // A promoted node owns its whole buffer.
                    return Some(self.rope.owned_str(start));
                }
                let source = if Arc::ptr_eq(chunk, &self.open_chunk) {
                    self.rope.open_str()
                } else {
                    chunk.as_str()
                };
                Some(&source[start as usize..(start + len) as usize])
            }
        }
    }

    /// The rope chunk backing a text node, sealing the open buffer first so
    /// that the caller can hold an `Arc` across later mutations (external
    /// JavaScript strings do this, `docs/spec/02-architecture.md` §7).
    pub fn text_chunk(&mut self, id: NodeId) -> Option<Arc<TextChunk>> {
        if !matches!(self.kind(id), NodeKind::Text | NodeKind::CData) {
            return None;
        }
        self.seal_owned_text(id);
        let data = self.arena.nodes.data[self.index(id)];
        if Arc::ptr_eq(&self.arena.texts.chunk[data.get()], &self.open_chunk) {
            self.seal_open_chunk();
        }
        Some(self.arena.texts.chunk[data.get()].clone())
    }

    /// Whether the node's bytes live in a buffer of its own rather than in the
    /// shared rope (see [`crate::text`]).
    fn is_owned_text(&self, id: NodeId) -> bool {
        matches!(self.kind(id), NodeKind::Text | NodeKind::CData)
            && Arc::ptr_eq(
                &self.arena.texts.chunk[self.arena.nodes.data[self.index(id)].get()],
                &self.owned_chunk,
            )
    }

    /// Freezes a promoted node's buffer into an immutable chunk and re-points
    /// the node at it, so the bytes can be shared (a clone, or a JavaScript
    /// external string).
    fn seal_owned_text(&mut self, id: NodeId) {
        if !self.is_owned_text(id) {
            return;
        }
        let data = self.arena.nodes.data[self.index(id)];
        let (index, len) = self.arena.texts.range[data.get()];
        let chunk = self.rope.seal_owned(index);
        self.arena.texts.chunk[data.get()] = chunk;
        self.arena.texts.range[data.get()] = (0, len);
    }

    /// Releases a promoted node's buffer when the node goes away.
    fn release_owned_text(&mut self, id: NodeId) {
        if !self.is_owned_text(id) {
            return;
        }
        let data = self.arena.nodes.data[self.index(id)];
        let (index, _) = self.arena.texts.range[data.get()];
        self.rope.release_owned(index);
        self.arena.texts.chunk[data.get()] = TextChunk::empty();
        self.arena.texts.range[data.get()] = (0, 0);
    }

    /// The comment's data.
    pub fn comment_data(&self, id: NodeId) -> Option<&str> {
        (self.kind(id) == NodeKind::Comment)
            .then(|| self.arena.comments[self.arena.nodes.data[self.index(id)].get()].as_str())
    }

    /// The processing instruction's target and data.
    pub fn processing_instruction(&self, id: NodeId) -> Option<(&str, &str)> {
        (self.kind(id) == NodeKind::ProcessingInstruction).then(|| {
            let pi = &self.arena.pis[self.arena.nodes.data[self.index(id)].get()];
            (pi.target.as_str(), pi.data.as_str())
        })
    }

    /// The doctype record.
    pub fn doctype(&self, id: NodeId) -> Option<&DocTypeData> {
        (self.kind(id) == NodeKind::DocType)
            .then(|| &self.arena.doctypes[self.arena.nodes.data[self.index(id)].get()])
    }

    // --------------------------------------------------------- document order

    /// Runs `f` with a document-order table that is valid for the current
    /// shape of the tree, rebuilding it first if a structural mutation
    /// invalidated it.
    fn with_order<R>(&self, f: impl FnOnce(&OrderIndex) -> R) -> R {
        let mut order = self.order.borrow_mut();
        if order.valid_for != self.structure_generation {
            self.rebuild_order(&mut order);
        }
        f(&order)
    }

    /// Assigns a pre-order position and a subtree end to every live slot.
    ///
    /// The document tree is numbered first, then any remaining live node's
    /// own root, in slot order: shadow roots and `<template>` contents have no
    /// parent and are deliberately numbered as separate trees, which is what
    /// keeps them out of the subtree range of their host.
    fn rebuild_order(&self, order: &mut OrderIndex) {
        let slots = self.arena.slots();
        order.enter.clear();
        order.enter.resize(slots, OrderIndex::UNASSIGNED);
        order.exit.clear();
        order.exit.resize(slots, OrderIndex::UNASSIGNED);
        let mut stack = std::mem::take(&mut order.stack);
        let mut next = 0u32;
        self.number_tree(self.document_node, order, &mut stack, &mut next);
        for slot in 0..slots {
            let node = NodeId(slot as u32);
            if !self.arena.is_live(node) || order.enter[slot] != OrderIndex::UNASSIGNED {
                continue;
            }
            let root = self.root_of(node);
            self.number_tree(root, order, &mut stack, &mut next);
        }
        stack.clear();
        order.stack = stack;
        order.valid_for = self.structure_generation;
    }

    fn number_tree(
        &self,
        root: NodeId,
        order: &mut OrderIndex,
        stack: &mut Vec<(NodeId, bool)>,
        next: &mut u32,
    ) {
        if !self.arena.is_live(root) {
            return;
        }
        stack.clear();
        stack.push((root, false));
        let cap = self.arena.slots().saturating_mul(2) + 8;
        let mut visited = 0usize;
        while let Some((node, leaving)) = stack.pop() {
            visited += 1;
            if visited > cap {
                return;
            }
            let slot = node.raw() as usize;
            if leaving {
                order.exit[slot] = *next;
                continue;
            }
            if order.enter[slot] != OrderIndex::UNASSIGNED {
                continue;
            }
            order.enter[slot] = *next;
            *next += 1;
            stack.push((node, true));
            let start = stack.len();
            let mut child = self.first_child(node);
            while child.is_some() {
                stack.push((child, false));
                child = self.next_sibling(child);
            }
            stack[start..].reverse();
        }
    }

    /// Whether `a` comes before `b` in tree order. An integer compare over the
    /// document-order table; nodes in different trees keep a stable order.
    pub fn precedes(&self, a: NodeId, b: NodeId) -> bool {
        if a == b {
            return false;
        }
        self.with_order(|order| order.enter_of(a) < order.enter_of(b))
    }

    /// Sorts nodes into tree order.
    pub fn sort_tree_order(&self, nodes: &mut [NodeId]) {
        self.with_order(|order| nodes.sort_unstable_by_key(|node| order.enter_of(*node)));
    }

    // -------------------------------------------------------------- indexes

    /// Puts a bucket into tree order if a structural change disturbed it, and
    /// returns it.
    fn sorted_bucket<'a, K: std::hash::Hash + Eq + Copy>(
        &self,
        index: &'a mut Index<K>,
        key: &K,
    ) -> Option<&'a Bucket> {
        let generation = self.structure_generation;
        let needs_sort = index
            .buckets
            .get(key)
            .is_some_and(|bucket| bucket.nodes.len() > 1 && bucket.sorted_at != generation);
        if needs_sort {
            // Sorting moves the entries, so the position map a removal keeps
            // for large buckets is stale from here; it is rebuilt on demand.
            index.positions.remove(key);
            let bucket = index.buckets.get_mut(key)?;
            self.with_order(|order| {
                bucket
                    .nodes
                    .sort_unstable_by_key(|node| order.enter_of(*node))
            });
            bucket.sorted_at = generation;
        }
        index.buckets.get(key)
    }

    /// The first connected element with this id, in tree order
    /// (`docs/spec/04-dom-and-css.md` §4 budget: ≤ 50 ns).
    ///
    /// Duplicate ids are common on the real web, so the bucket is kept in tree
    /// order rather than reduced element by element: the answer is the first
    /// connected entry.
    pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
        if self.quirks_mode() == tenta_html::QuirksMode::Quirks {
            // In quirks mode, look up the folded bucket but compare exact ids
            let lookup_key = self.ids.get(&id.to_ascii_lowercase())?;
            let mut indexes = self.indexes.borrow_mut();
            let bucket = self.sorted_bucket(&mut indexes.ids, &lookup_key)?;
            bucket
                .nodes
                .iter()
                .copied()
                .find(|node| self.is_connected(*node) && self.element_id(*node) == Some(id))
        } else {
            let key = self.ids.get(id)?;
            let mut indexes = self.indexes.borrow_mut();
            let bucket = self.sorted_bucket(&mut indexes.ids, &key)?;
            bucket
                .nodes
                .iter()
                .copied()
                .find(|node| self.is_connected(*node))
        }
    }

    /// Every element with this class, in tree order. A fresh `Vec`: live
    /// collections belong to the bindings (P2).
    pub fn get_elements_by_class_name(&self, class: &str) -> Vec<NodeId> {
        // In quirks mode, fold the lookup to lowercase, just like with_elements_by_class
        let lookup = if self.quirks_mode() == tenta_html::QuirksMode::Quirks {
            class.to_ascii_lowercase()
        } else {
            class.to_owned()
        };
        let Some(key) = self.class_names.get(&lookup) else {
            return Vec::new();
        };
        let mut indexes = self.indexes.borrow_mut();
        match self.sorted_bucket(&mut indexes.classes, &key) {
            Some(bucket) => bucket
                .nodes
                .iter()
                .copied()
                .filter(|node| self.is_connected(*node))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Every element with this local name, in tree order.
    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<NodeId> {
        let Some(atom) = Atom::get(tag) else {
            return Vec::new();
        };
        let mut indexes = self.indexes.borrow_mut();
        match self.sorted_bucket(&mut indexes.tags, &atom) {
            Some(bucket) => bucket
                .nodes
                .iter()
                .copied()
                .filter(|node| self.is_connected(*node))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Runs `f` over the elements with this local name, in tree order and
    /// including detached ones.
    ///
    /// The bucket is handed over as a slice rather than copied: a class shared
    /// by fifty thousand elements would otherwise be cloned on every query
    /// before the matcher saw a single element.
    pub(crate) fn with_elements_by_tag<R>(&self, tag: Atom, f: impl FnOnce(&[NodeId]) -> R) -> R {
        let mut indexes = self.indexes.borrow_mut();
        match self.sorted_bucket(&mut indexes.tags, &tag) {
            Some(bucket) => f(&bucket.nodes),
            None => f(&[]),
        }
    }

    pub(crate) fn with_elements_by_class<R>(
        &self,
        class: &str,
        f: impl FnOnce(&[NodeId]) -> R,
    ) -> R {
        let lookup = if self.quirks_mode() == tenta_html::QuirksMode::Quirks {
            class.to_ascii_lowercase().into_boxed_str()
        } else {
            class.into()
        };
        let Some(key) = self.class_names.get(lookup.as_ref()) else {
            return f(&[]);
        };
        let mut indexes = self.indexes.borrow_mut();
        match self.sorted_bucket(&mut indexes.classes, &key) {
            Some(bucket) => f(&bucket.nodes),
            None => f(&[]),
        }
    }

    pub(crate) fn with_elements_by_id<R>(&self, id: &str, f: impl FnOnce(&[NodeId]) -> R) -> R {
        let lookup = if self.quirks_mode() == tenta_html::QuirksMode::Quirks {
            id.to_ascii_lowercase().into_boxed_str()
        } else {
            id.into()
        };
        let Some(key) = self.ids.get(lookup.as_ref()) else {
            return f(&[]);
        };
        let mut indexes = self.indexes.borrow_mut();
        match self.sorted_bucket(&mut indexes.ids, &key) {
            Some(bucket) => f(&bucket.nodes),
            None => f(&[]),
        }
    }

    /// Whether `node` is a strict descendant of `root`, as an integer compare
    /// against the document-order table.
    pub(crate) fn is_in_subtree(&self, root: NodeId, node: NodeId) -> bool {
        node != root && self.with_order(|order| order.contains(root, node))
    }

    // ---------------------------------------------------------- node create

    /// Creates a detached element. Every created node starts as a detached
    /// root (`docs/spec/04-dom-and-css.md` §1.1).
    pub fn create_element(
        &mut self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Result<NodeId> {
        // Validate XML element names for XML documents
        self.validate_xml_name(name.local.as_str())?;

        let element = self.arena.alloc_element(name.local, name.ns);
        self.indexes.get_mut().tags.push(name.local, element);
        if let Some(prefix) = name.prefix.filter(|prefix| *prefix != atoms::EMPTY) {
            self.element_prefixes.insert(element, prefix);
        }
        self.set_attributes_internal(element, attrs);
        let slot = self.index(element);
        if flags.mathml_annotation_xml_integration_point {
            self.arena.nodes.flags[slot].insert(NodeFlags::MATHML_INTEGRATION_POINT);
        }
        self.mark_detached_root(element);
        if flags.template {
            let contents = self.arena.alloc_container(NodeKind::Fragment);
            let data = self.arena.nodes.data[slot];
            self.arena.elements.shadow[data.get()] = contents;
            self.arena.nodes.flags[slot].insert(NodeFlags::TEMPLATE);
            self.hosts.insert(contents, element);
            self.bump_structure();
            // The contents fragment is owned by its template element: it is
            // never an independent detached root, so a sweep cannot free it
            // while the template is alive.
        }
        Ok(element)
    }

    /// Creates a detached HTML element with no attributes.
    pub fn create_html_element(&mut self, local: Atom) -> NodeId {
        // HTML element names are always valid, so unwrap is safe
        self.create_element(
            QualName::new(atoms::NS_HTML, local),
            Vec::new(),
            ElementFlags::default(),
        )
        .expect("HTML element name should always be valid")
    }

    /// Creates a detached text node.
    pub fn create_text(&mut self, data: &str) -> NodeId {
        let span = self.rope.append(data);
        let node = self.arena.alloc_text(
            NodeKind::Text,
            self.open_chunk.clone(),
            (span.start, span.len),
        );
        self.rope.register_pending(node);
        self.mark_detached_root(node);
        self.maybe_seal();
        node
    }

    /// Creates a detached CDATA section (XML documents only).
    pub fn create_cdata(&mut self, data: &str) -> NodeId {
        let span = self.rope.append(data);
        let node = self.arena.alloc_text(
            NodeKind::CData,
            self.open_chunk.clone(),
            (span.start, span.len),
        );
        self.rope.register_pending(node);
        self.mark_detached_root(node);
        self.maybe_seal();
        node
    }

    /// Creates a detached comment node.
    pub fn create_comment(&mut self, data: &str) -> NodeId {
        let node = self.arena.alloc_comment(data.to_owned());
        self.mark_detached_root(node);
        node
    }

    /// Creates a detached processing instruction.
    pub fn create_processing_instruction(&mut self, target: &str, data: &str) -> NodeId {
        let node = self.arena.alloc_pi(PiData {
            target: target.to_owned(),
            data: data.to_owned(),
        });
        self.mark_detached_root(node);
        node
    }

    /// Creates a detached doctype node.
    pub fn create_doctype(&mut self, name: &str, public_id: &str, system_id: &str) -> NodeId {
        let node = self.arena.alloc_doctype(DocTypeData {
            name: name.to_owned(),
            public_id: public_id.to_owned(),
            system_id: system_id.to_owned(),
        });
        self.mark_detached_root(node);
        node
    }

    /// Creates a detached document fragment.
    pub fn create_fragment(&mut self) -> NodeId {
        let node = self.arena.alloc_container(NodeKind::Fragment);
        self.mark_detached_root(node);
        node
    }

    /// Attaches a shadow root to `host` and returns it. The shadow root is
    /// owned by its host, exactly like `<template>` contents.
    pub fn attach_shadow_root(&mut self, host: NodeId) -> Result<NodeId> {
        if self.kind(host) != NodeKind::Element {
            return Err(hierarchy_request("only elements can host a shadow root"));
        }
        if self.flags(host).contains(NodeFlags::TEMPLATE) {
            return Err(hierarchy_request(
                "<template> keeps its contents in the shadow slot",
            ));
        }
        let data = self.arena.nodes.data[self.index(host)];
        if self.arena.elements.shadow[data.get()].is_some() {
            return Err(invalid_state("element already has a shadow root"));
        }
        let root = self.arena.alloc_container(NodeKind::ShadowRoot);
        self.arena.elements.shadow[data.get()] = root;
        self.hosts.insert(root, host);
        self.bump_structure();
        Ok(root)
    }

    /// Copies `node` without its children. Text nodes share the rope chunk
    /// they already point at, so cloning text copies no bytes.
    pub fn shallow_clone(&mut self, node: NodeId) -> Result<NodeId> {
        if !self.arena.is_live(node) {
            return Err(invalid_state(format!("no live node at {node:?}")));
        }
        let clone = match self.kind(node) {
            NodeKind::Element => {
                let name = self.element_name(node).expect("elements have a name");
                let attrs: Vec<Attribute> = self
                    .attributes(node)
                    .map(|(name, value)| Attribute {
                        name,
                        value: value.to_owned(),
                    })
                    .collect();
                let flags = ElementFlags {
                    template: self.flags(node).contains(NodeFlags::TEMPLATE),
                    mathml_annotation_xml_integration_point: self
                        .flags(node)
                        .contains(NodeFlags::MATHML_INTEGRATION_POINT),
                };
                self.create_element(name, attrs, flags)?
            }
            NodeKind::Text | NodeKind::CData => {
                // A promoted node owns a mutable buffer that cannot be shared;
                // freezing it first gives both nodes an immutable chunk.
                self.seal_owned_text(node);
                let data = self.arena.nodes.data[self.index(node)];
                let chunk = self.arena.texts.chunk[data.get()].clone();
                let range = self.arena.texts.range[data.get()];
                let pending = Arc::ptr_eq(&chunk, &self.open_chunk);
                let clone = self.arena.alloc_text(self.kind(node), chunk, range);
                if pending {
                    self.rope.register_pending(clone);
                }
                self.mark_detached_root(clone);
                clone
            }
            NodeKind::Comment => {
                let text = self.comment_data(node).unwrap_or_default().to_owned();
                self.create_comment(&text)
            }
            NodeKind::ProcessingInstruction => {
                let (target, data) = self
                    .processing_instruction(node)
                    .map(|(t, d)| (t.to_owned(), d.to_owned()))
                    .expect("processing instructions have data");
                self.create_processing_instruction(&target, &data)
            }
            NodeKind::DocType => {
                let doctype = self.doctype(node).cloned().unwrap_or_default();
                self.create_doctype(&doctype.name, &doctype.public_id, &doctype.system_id)
            }
            NodeKind::Document | NodeKind::Fragment | NodeKind::ShadowRoot => {
                self.create_fragment()
            }
        };
        Ok(clone)
    }

    /// Copies `node`, and its subtree when `deep`. The copy is detached.
    /// Iterative, so a deep tree clones without touching the native stack.
    pub fn clone_node(&mut self, node: NodeId, deep: bool) -> Result<NodeId> {
        let root = self.shallow_clone(node)?;
        if !deep {
            return Ok(root);
        }
        let cap = self.slots().saturating_mul(2) + 8;
        let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
        let push_children =
            |document: &Document, stack: &mut Vec<(NodeId, NodeId)>, source, parent| {
                let start = stack.len();
                for child in document.children(source) {
                    stack.push((child, parent));
                }
                stack[start..].reverse();
            };
        push_children(self, &mut stack, node, root);
        let mut visited = 0usize;
        while let Some((source, parent)) = stack.pop() {
            visited += 1;
            if visited > cap {
                break;
            }
            let clone = self.shallow_clone(source)?;
            self.mutate(Mutation::Append {
                parent,
                child: clone,
            })?;
            push_children(self, &mut stack, source, clone);
        }
        Ok(root)
    }

    /// Mirrors the children of a selected `<option>` into the
    /// `<selectedcontent>` of its `<select>`, as a customizable select
    /// requires. The parser reports the option's close through
    /// [`tenta_html::TreeSink::maybe_clone_option_into_selectedcontent`]
    /// because cloning needs tree access it does not have.
    pub fn clone_option_into_selectedcontent(&mut self, option: NodeId) {
        if !self.arena.is_live(option) || !self.has_attribute(option, atoms::SELECTED) {
            return;
        }
        let Some(select) = self
            .ancestors(option)
            .find(|node| self.tag(*node) == Some(atoms::SELECT))
        else {
            return;
        };
        // A `<selectedcontent>` lives in the select's display `<button>` at the
        // top of the subtree, so it is found in the first handful of nodes when
        // it exists at all. Bounding the scan keeps this linear on a `<select>`
        // with tens of thousands of pre-selected options and no
        // `<selectedcontent>` (the common case), where an unbounded per-option
        // pre-order walk of the growing subtree is quadratic (N5-1/N5-2).
        const MAX_SELECTEDCONTENT_SCAN: usize = 4096;
        let Some(target) = self
            .descendants(select)
            .take(MAX_SELECTEDCONTENT_SCAN)
            .find(|node| self.tag(*node) == Some(atoms::SELECTEDCONTENT))
        else {
            return;
        };
        for child in self.children(target).collect::<Vec<_>>() {
            let _ = self.mutate(Mutation::Remove { child });
        }
        for child in self.children(option).collect::<Vec<_>>() {
            let Ok(clone) = self.clone_node(child, true) else {
                continue;
            };
            let _ = self.mutate(Mutation::Append {
                parent: target,
                child: clone,
            });
        }
    }

    // --------------------------------------------------------- node lifetime

    /// Takes a strong handle to `id`, blocking the sweep for its detached
    /// subtree. Release it with [`Document::release`].
    pub fn handle(&mut self, id: NodeId) -> Result<NodeHandle> {
        if !self.arena.is_live(id) {
            return Err(invalid_state(format!("no live node at {id:?}")));
        }
        let slot = self.index(id);
        let handle_count = self.arena.nodes.handles[slot];
        if handle_count == u16::MAX {
            return Err(invalid_state(format!(
                "handle count for {id:?} at capacity ({}); cannot create more handles",
                u16::MAX
            )));
        }
        self.arena.nodes.handles[slot] = handle_count + 1;
        self.invalidate_keep(id);
        Ok(NodeHandle {
            node: self.node_ref(id),
            document: self.document_id,
            released: false,
        })
    }

    /// A second handle to the same node.
    pub fn clone_handle(&mut self, handle: &NodeHandle) -> Result<NodeHandle> {
        debug_assert_eq!(
            handle.document, self.document_id,
            "NodeHandle cloned against a different document"
        );
        let id = self.resolve(handle.node)?;
        self.handle(id)
    }

    /// Drops a handle, decrementing the arena count.
    pub fn release(&mut self, mut handle: NodeHandle) {
        debug_assert_eq!(
            handle.document, self.document_id,
            "NodeHandle released against a different document"
        );
        handle.released = true;
        if let Ok(id) = self.resolve(handle.node) {
            let slot = self.index(id);
            debug_assert!(
                self.arena.nodes.handles[slot] > 0,
                "handle count underflow at {id:?}"
            );
            if self.arena.nodes.handles[slot] != u16::MAX {
                self.arena.nodes.handles[slot] = self.arena.nodes.handles[slot].saturating_sub(1);
            }
            self.invalidate_keep(id);
        }
    }

    /// Sets or clears the "a JavaScript wrapper exists" bit. The bindings
    /// call it on wrapper creation and from the engine's weak callback
    /// (`docs/spec/04-dom-and-css.md` §1.1).
    pub fn set_has_wrapper(&mut self, id: NodeId, has_wrapper: bool) {
        if !self.arena.is_live(id) {
            return;
        }
        let slot = self.index(id);
        self.arena.nodes.flags[slot].set(NodeFlags::HAS_WRAPPER, has_wrapper);
        self.invalidate_keep(id);
    }

    #[inline]
    pub fn has_wrapper(&self, id: NodeId) -> bool {
        self.flags(id).contains(NodeFlags::HAS_WRAPPER)
    }

    /// The detached roots currently tracked.
    pub fn detached_root_count(&self) -> usize {
        self.detached_roots.len()
    }

    fn mark_detached_root(&mut self, id: NodeId) {
        self.detached_roots.insert(id);
        let slot = self.index(id);
        self.arena.nodes.flags[slot].insert(NodeFlags::DETACHED_ROOT);
        // Only bump structure_generation if the node was previously connected.
        // A newly created node has no position in the tree yet, so marking it
        // detached doesn't invalidate the order index. Unlinking a connected
        // node is handled separately in unlink().
    }

    fn unmark_detached_root(&mut self, id: NodeId) {
        self.detached_roots.remove(&id);
        let slot = self.index(id);
        self.arena.nodes.flags[slot].remove(NodeFlags::DETACHED_ROOT);
    }

    /// The topmost node whose lifetime governs `id`: the root of its parent
    /// chain, following host links out of shadow roots and `<template>`
    /// contents fragments.
    pub fn lifetime_root(&self, id: NodeId) -> NodeId {
        let mut node = id;
        let cap = self.slots();
        for _ in 0..cap {
            let top = self.root_of(node);
            match self.hosts.get(&top) {
                Some(&host) if host != top => node = host,
                _ => return top,
            }
        }
        node
    }

    /// Clears the cached sweep decision of the detached root that owns `id`.
    /// Connected nodes are never sweep candidates, so they cost nothing here.
    fn invalidate_keep(&mut self, id: NodeId) {
        if !self.arena.is_live(id) || self.is_connected(id) {
            return;
        }
        let root = self.lifetime_root(id);
        let slot = self.index(root);
        self.arena.nodes.flags[slot].remove(NodeFlags::KEEP_VALID);
        self.arena.nodes.flags[slot].remove(NodeFlags::KEEP);
    }

    /// Walks every detached root and frees the subtrees no wrapper and no
    /// Rust handle references (`docs/spec/04-dom-and-css.md` §1.1).
    pub fn sweep_detached(&mut self) -> SweepStats {
        let mut stats = SweepStats::default();
        self.mutations_since_sweep = 0;
        let roots: Vec<NodeId> = self.detached_roots.iter().copied().collect();
        let mut stack: Vec<NodeId> = Vec::new();
        let mut subtree: Vec<NodeId> = Vec::new();
        let mut freed: HashSet<NodeId> = HashSet::new();
        let mut touched = TouchedKeys::default();
        for root in roots {
            if !self.arena.is_live(root) || self.is_connected(root) {
                self.unmark_detached_root(root);
                continue;
            }
            stats.roots_examined += 1;
            let flags = self.flags(root);
            if flags.contains(NodeFlags::KEEP_VALID) {
                stats.roots_cached += 1;
                if flags.contains(NodeFlags::KEEP) {
                    stats.roots_kept += 1;
                    continue;
                }
            }
            subtree.clear();
            self.collect_subtree(root, &mut stack, &mut subtree);
            let keep = subtree.iter().any(|node| {
                let slot = self.index(*node);
                self.arena.nodes.handles[slot] > 0
                    || self.arena.nodes.flags[slot].contains(NodeFlags::HAS_WRAPPER)
            });
            if keep {
                let slot = self.index(root);
                self.arena.nodes.flags[slot].insert(NodeFlags::KEEP);
                self.arena.nodes.flags[slot].insert(NodeFlags::KEEP_VALID);
                stats.roots_kept += 1;
                continue;
            }
            for node in subtree.iter().rev() {
                freed.insert(*node);
                self.note_deindex(*node, &mut touched);
                self.release_owned_text(*node);
                self.hosts.remove(node);
                self.element_prefixes.remove(node);
                self.detached_roots.remove(node);
                self.arena.free(*node);
                stats.nodes_freed += 1;
            }
            self.detached_roots.remove(&root);
        }
        // One pass per touched index key rather than a linear search per
        // freed node: sweeping 100 000 siblings that share a tag or class
        // would otherwise be quadratic (04 §1.1 test (a)).
        self.purge_indexes(&freed, &touched);
        // Release folded keys first (if in quirks mode), so a reclaimed slot
        // cannot be re-interned under a live key while the bucket still exists.
        for folded_id in touched.released_folded_ids {
            self.ids.release(folded_id);
        }
        for folded_class in touched.released_folded_classes {
            self.class_names.release(folded_class);
        }
        // Then release the exact handles
        for id in touched.released_ids {
            self.ids.release(id);
        }
        for class in touched.released_classes {
            self.class_names.release(class);
        }
        if stats.nodes_freed > 0 {
            self.bump_structure();
        }
        if self.arena.wants_compaction() {
            self.arena.compact();
        }
        stats
    }

    /// Collects the subtree rooted at `root`, descending into shadow roots
    /// and `<template>` contents, with an explicit stack: the tree can be a
    /// million nodes deep (`docs/spec/02-architecture.md` §3).
    fn collect_subtree(&self, root: NodeId, stack: &mut Vec<NodeId>, out: &mut Vec<NodeId>) {
        stack.clear();
        stack.push(root);
        let cap = self.slots();
        while let Some(node) = stack.pop() {
            if out.len() >= cap {
                return;
            }
            out.push(node);
            if self.kind(node) == NodeKind::Element {
                let slot =
                    self.arena.elements.shadow[self.arena.nodes.data[self.index(node)].get()];
                if slot.is_some() {
                    stack.push(slot);
                }
            }
            let mut child = self.first_child(node);
            while child.is_some() {
                stack.push(child);
                child = self.next_sibling(child);
            }
        }
    }

    /// Records which index keys a node about to be freed appears under, and
    /// the name references its columns hold.
    fn note_deindex(&mut self, node: NodeId, touched: &mut TouchedKeys) {
        if self.kind(node) != NodeKind::Element {
            return;
        }
        let data = self.arena.nodes.data[self.index(node)];
        touched.tags.insert(self.arena.elements.tag[data.get()]);
        let quirks = self.quirks == QuirksMode::Quirks;
        let id = self.arena.elements.id[data.get()];
        if id.is_some() {
            if let Some(key) = self.id_index_key(id) {
                touched.ids.insert(key);
                if quirks {
                    touched.released_folded_ids.push(key);
                }
            }
            touched.released_ids.push(id);
        }
        for class in self.arena.element_classes(data) {
            if let Some(key) = self.class_index_key(*class) {
                touched.classes.insert(key);
                if quirks {
                    touched.released_folded_classes.push(key);
                }
            }
            touched.released_classes.push(*class);
        }
    }

    fn purge_indexes(&mut self, freed: &HashSet<NodeId>, touched: &TouchedKeys) {
        let indexes = self.indexes.get_mut();
        indexes.tags.purge(&touched.tags, freed);
        indexes.ids.purge(&touched.ids, freed);
        indexes.classes.purge(&touched.classes, freed);
    }

    // ---------------------------------------------------------------- mutate

    /// The single mutation path (`docs/spec/04-dom-and-css.md` §1).
    pub fn mutate(&mut self, mutation: Mutation) -> Result<()> {
        match mutation {
            Mutation::Append { parent, child } => self.insert(parent, child, NodeId::NONE),
            Mutation::InsertBefore {
                parent,
                child,
                before,
            } => self.insert(parent, child, before),
            Mutation::Remove { child } => self.remove(child),
            Mutation::ReplaceChild {
                parent,
                new_child,
                old_child,
            } => {
                if self.parent(old_child) != parent {
                    return Err(not_found(
                        "the node to replace is not a child of the parent",
                    ));
                }
                let anchor = self.next_sibling(old_child);
                self.remove(old_child)?;
                self.insert(parent, new_child, anchor)
            }
            Mutation::SetAttribute {
                element,
                name,
                value,
            } => self.set_attribute(element, name, &value),
            Mutation::RemoveAttribute { element, name } => self.remove_attribute(element, name),
            Mutation::SetText { node, text } => self.set_text(node, &text),
            Mutation::ReparentChildren { from, to } => self.reparent_children(from, to),
        }
    }

    /// Parse an HTML fragment and set it as the innerHTML of an element.
    ///
    /// This method parses the given HTML string as a fragment in the context of
    /// the specified element, then replaces all children of the element with
    /// the parsed fragment.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not an Element node or if parsing
    /// or mutation operations fail.
    pub fn set_inner_html(&mut self, element: NodeId, html: &str) -> Result<()> {
        use tenta_html::{Parser, ParserOptions};

        // Verify that the node is an element
        let ctx_name = self.element_name(element).ok_or_else(|| {
            tenta_core::Error::invalid("cannot set innerHTML on non-Element node")
        })?;

        // Snapshot the document node's current children before parsing
        let before: Vec<NodeId> = self.children(self.document_node()).collect();

        // Parse the fragment into this Document's own arena.
        // The parser consumes the sink by value, so we move self out and back.
        let taken = std::mem::take(self);
        let opts = ParserOptions {
            fragment_context: Some(ctx_name),
            scripting_enabled: false,
            ..ParserOptions::default()
        };
        let mut parser = Parser::new(taken, opts);
        parser.feed_str(html);
        *self = parser.finish();

        // Find the new fragment root: the bare <html> created by create_root
        // that was appended to document_node after the snapshot.
        let frag_root = self
            .children(self.document_node())
            .find(|c| !before.contains(c))
            .ok_or_else(|| tenta_core::Error::invalid("fragment parsing produced no root"))?;

        // Remove all existing children of the element
        let child_ids: Vec<NodeId> = self.children(element).collect();
        for child_id in child_ids {
            self.mutate(Mutation::Remove { child: child_id })?;
        }

        // Reparent the fragment's children to the element
        self.mutate(Mutation::ReparentChildren {
            from: frag_root,
            to: element,
        })?;

        // Remove the now-empty fragment root
        self.mutate(Mutation::Remove { child: frag_root })?;

        Ok(())
    }

    fn bump(&mut self) {
        self.mutation_generation = self.mutation_generation.wrapping_add(1);
        self.mutations_since_sweep = self.mutations_since_sweep.wrapping_add(1);
    }

    /// Marks the shape of the tree as changed: document order and every
    /// sorted index bucket have to be recomputed before the next query.
    fn bump_structure(&mut self) {
        self.structure_generation = self.structure_generation.wrapping_add(1);
        if self.structure_generation == 0 {
            self.structure_generation = 1;
        }
    }

    /// Validate that a name is a valid XML Name production for XML documents.
    /// HTML documents accept any name.
    fn validate_xml_name(&self, name: &str) -> Result<()> {
        if !self.is_html_document() {
            // XML Name productions: must start with letter/underscore/colon,
            // and contain only name characters.
            if name.is_empty() {
                return Err(Error::invalid("XML name cannot be empty"));
            }
            let first = name.chars().next().unwrap();
            if !matches!(first, 'a'..='z' | 'A'..='Z' | '_' | ':') {
                return Err(Error::invalid(format!(
                    "XML name must start with letter, underscore or colon, got '{}'",
                    first
                )));
            }
            // Check for invalid characters in the name
            if name
                .chars()
                .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '>' | '<' | '&'))
            {
                return Err(Error::invalid(
                    "XML name contains invalid characters (space, quote, angle bracket, ampersand)",
                ));
            }
        }
        Ok(())
    }

    /// Validate that an attribute value is safe for XML (no NUL bytes).
    fn validate_xml_attribute_value(&self, value: &str) -> Result<()> {
        if !self.is_html_document() && value.contains('\0') {
            return Err(Error::invalid(
                "XML attribute value cannot contain NUL byte",
            ));
        }
        Ok(())
    }

    fn validate_insert(&self, parent: NodeId, child: NodeId, before: NodeId) -> Result<()> {
        if !self.arena.is_live(parent) || !self.arena.is_live(child) {
            return Err(hierarchy_request("insert with a stale node"));
        }
        if !self.kind(parent).is_container() {
            return Err(hierarchy_request(format!(
                "{:?} cannot have children",
                self.kind(parent)
            )));
        }
        if matches!(self.kind(child), NodeKind::Document | NodeKind::ShadowRoot) {
            return Err(hierarchy_request(format!(
                "{:?} cannot be inserted into a tree",
                self.kind(child)
            )));
        }
        if self.kind(child) == NodeKind::DocType && self.kind(parent) != NodeKind::Document {
            return Err(hierarchy_request(
                "a doctype may only be a child of the document",
            ));
        }
        // The cycle guard Obscura lacked: a node may not contain itself.
        // A childless node cannot be an ancestor of anything, so the ancestor
        // walk is skipped for it; that keeps building a deep tree one fresh
        // node at a time linear rather than quadratic.
        if child == parent
            || (self.first_child(child).is_some() && self.is_inclusive_ancestor(child, parent))
        {
            return Err(hierarchy_request(
                "cannot insert a node into its own subtree",
            ));
        }
        if before.is_some() && self.parent(before) != parent {
            return Err(not_found("the reference node is not a child of the parent"));
        }
        Ok(())
    }

    fn insert(&mut self, parent: NodeId, child: NodeId, before: NodeId) -> Result<()> {
        self.validate_insert(parent, child, before)?;
        if self.kind(child) == NodeKind::Fragment {
            // Inserting a fragment moves its children, as the DOM standard
            // requires, and leaves the fragment empty.
            let children: Vec<NodeId> = self.children(child).collect();
            for node in children {
                self.unlink(node)?;
                self.link(parent, node, before);
            }
            self.bump();
            return Ok(());
        }
        self.unlink(child)?;
        self.link(parent, child, before);
        self.bump();
        Ok(())
    }

    /// Detaches `child` from its parent, updating connectivity and the
    /// detached-root set. Records the removal.
    fn remove(&mut self, child: NodeId) -> Result<()> {
        if !self.arena.is_live(child) {
            return Err(hierarchy_request("remove of a stale node"));
        }
        self.unlink(child)?;
        self.bump();
        Ok(())
    }

    fn unlink(&mut self, child: NodeId) -> Result<()> {
        let parent = self.parent(child);
        if !parent.is_some() {
            // Already detached; it is its own detached root.
            self.mark_detached_root(child);
            return Ok(());
        }
        let (i, p) = (self.index(child), self.index(parent));
        let prev = self.arena.nodes.prev[i];
        let next = self.arena.nodes.next[i];
        if prev.is_some() {
            self.arena.nodes.next[prev.raw() as usize] = next;
        } else {
            self.arena.nodes.first_child[p] = next;
        }
        if next.is_some() {
            self.arena.nodes.prev[next.raw() as usize] = prev;
        } else {
            self.arena.nodes.last_child[p] = prev;
        }
        self.arena.nodes.parent[i] = NodeId::NONE;
        self.arena.nodes.prev[i] = NodeId::NONE;
        self.arena.nodes.next[i] = NodeId::NONE;
        if self.is_connected(child) {
            // If we're unlinking the cached first BASE element, invalidate it.
            if self.first_connected_base == Some(child) {
                self.first_connected_base = None;
            }
            self.set_connected(child, false);
        }
        self.mark_detached_root(child);
        self.bump_structure();
        self.invalidate_keep(child);
        self.observer.record(MutationRecord::Removed {
            parent,
            node: child,
        });
        Ok(())
    }

    fn link(&mut self, parent: NodeId, child: NodeId, before: NodeId) {
        let (i, p) = (self.index(child), self.index(parent));
        let prev = if before.is_some() {
            self.arena.nodes.prev[before.raw() as usize]
        } else {
            self.arena.nodes.last_child[p]
        };
        self.arena.nodes.parent[i] = parent;
        self.arena.nodes.prev[i] = prev;
        self.arena.nodes.next[i] = before;
        if prev.is_some() {
            self.arena.nodes.next[prev.raw() as usize] = child;
        } else {
            self.arena.nodes.first_child[p] = child;
        }
        if before.is_some() {
            self.arena.nodes.prev[before.raw() as usize] = child;
        } else {
            self.arena.nodes.last_child[p] = child;
        }
        self.unmark_detached_root(child);
        self.bump_structure();
        if self.is_connected(parent) {
            self.set_connected(child, true);
        } else {
            self.invalidate_keep(child);
        }
        self.observer.record(MutationRecord::Inserted {
            parent,
            node: child,
            previous_sibling: prev,
        });
        if self.tag(child) == Some(atoms::BASE) && self.is_connected(child) {
            self.refresh_base_url();
        }
    }

    /// Propagates the connected bit down a subtree, iteratively.
    fn set_connected(&mut self, root: NodeId, connected: bool) {
        let cap = self.slots();
        let mut node = root;
        let mut visited = 0usize;
        while node.is_some() && visited < cap {
            visited += 1;
            let slot = self.index(node);
            self.arena.nodes.flags[slot].set(NodeFlags::CONNECTED, connected);
            if connected {
                self.arena.nodes.flags[slot].remove(NodeFlags::KEEP_VALID);
                self.arena.nodes.flags[slot].remove(NodeFlags::KEEP);
            }
            node = self.next_in_subtree(node, root);
        }
    }

    fn reparent_children(&mut self, from: NodeId, to: NodeId) -> Result<()> {
        if !self.arena.is_live(from) || !self.arena.is_live(to) {
            return Err(hierarchy_request("reparent with a stale node"));
        }
        if !self.kind(to).is_container() {
            return Err(hierarchy_request("reparent target cannot have children"));
        }
        if self.is_inclusive_ancestor(from, to) && from != to {
            return Err(hierarchy_request(
                "cannot reparent a node's children into its own subtree",
            ));
        }
        let children: Vec<NodeId> = self.children(from).collect();
        for child in children {
            self.unlink(child)?;
            self.link(to, child, NodeId::NONE);
        }
        self.bump();
        Ok(())
    }

    // ------------------------------------------------------------ attributes

    fn set_attributes_internal(&mut self, element: NodeId, attrs: Vec<Attribute>) {
        let mut stored: Vec<(AttrName, AttrValue)> = Vec::with_capacity(attrs.len());
        for attr in &attrs {
            let value = self.arena.push_attr_value(&attr.value);
            stored.push((AttrName::from_qual(attr.name), value));
        }
        let data = self.arena.nodes.data[self.index(element)];
        self.arena.set_element_attrs(data, &stored);
        for attr in &attrs {
            if attr.name.ns != atoms::EMPTY {
                continue;
            }
            if attr.name.local == atoms::ID {
                self.reindex_id(element, &attr.value);
            } else if attr.name.local == atoms::CLASS {
                self.reindex_classes(element, &attr.value);
            }
        }
    }

    fn set_attribute(&mut self, element: NodeId, name: QualName, value: &str) -> Result<()> {
        if self.kind(element) != NodeKind::Element {
            return Err(hierarchy_request("attributes belong to elements"));
        }
        // Validate XML attribute values for XML documents
        self.validate_xml_attribute_value(value)?;
        // Validate attribute names for XML documents
        self.validate_xml_name(name.local.as_str())?;

        let stored_name = AttrName::from_qual(name);
        let data = self.arena.nodes.data[self.index(element)];
        // Overwriting an existing attribute patches its slot in the shared
        // buffer. Copying the whole list out and appending it again made a
        // page that writes one attribute repeatedly quadratic in the number of
        // writes and in the number of attributes on the element.
        // Skip dead (tombstone) slots when looking for existing attribute.
        let attrs_slice = self.arena.element_attrs(data);
        let position = attrs_slice
            .iter()
            .position(|(n, v)| !v.is_dead() && *n == stored_name);
        let old_value = position
            .map(|index| attrs_slice[index].1)
            .and_then(|value| {
                if value.is_dead() {
                    None
                } else {
                    Some(self.arena.attr_value_str(value).to_owned())
                }
            });
        match position {
            Some(index) => {
                let new_value = self.arena.push_attr_value(value);
                self.arena.replace_element_attr(data, index, new_value);
            }
            None => {
                let new_value = self.arena.push_attr_value(value);
                self.arena.append_element_attr(data, stored_name, new_value);
            }
        }
        if stored_name.ns == atoms::EMPTY {
            if stored_name.local == atoms::ID {
                self.reindex_id(element, value);
            } else if stored_name.local == atoms::CLASS {
                self.reindex_classes(element, value);
            }
        }
        self.observer.record(MutationRecord::Attribute {
            element,
            name: stored_name.local,
            namespace: stored_name.ns,
            old_value,
        });
        self.after_attribute_change(element, stored_name.local);
        Ok(())
    }

    fn remove_attribute(&mut self, element: NodeId, name: QualName) -> Result<()> {
        if self.kind(element) != NodeKind::Element {
            return Err(hierarchy_request("attributes belong to elements"));
        }
        let stored_name = AttrName::from_qual(name);
        let data = self.arena.nodes.data[self.index(element)];
        let attrs_slice = self.arena.element_attrs(data);
        let Some(position) = attrs_slice
            .iter()
            .position(|(n, v)| !v.is_dead() && *n == stored_name)
        else {
            return Ok(());
        };
        let old_value = self
            .arena
            .attr_value_str(attrs_slice[position].1)
            .to_owned();

        // Mark the attribute slot as dead (tombstone) instead of copying the entire list
        self.arena.mark_attribute_dead(data, position);

        if stored_name.ns == atoms::EMPTY {
            if stored_name.local == atoms::ID {
                self.reindex_id(element, "");
            } else if stored_name.local == atoms::CLASS {
                self.reindex_classes(element, "");
            }
        }
        self.observer.record(MutationRecord::Attribute {
            element,
            name: stored_name.local,
            namespace: stored_name.ns,
            old_value: Some(old_value),
        });
        self.after_attribute_change(element, stored_name.local);
        Ok(())
    }

    fn after_attribute_change(&mut self, element: NodeId, local: Atom) {
        let slot = self.index(element);
        self.arena.nodes.flags[slot].insert(NodeFlags::DIRTY_STYLE);
        self.invalidate_keep(element);
        self.bump();
        if local == atoms::HREF && self.tag(element) == Some(atoms::BASE) {
            self.refresh_base_url();
        }
        if self.arena.wants_compaction() {
            self.arena.compact();
        }
    }

    /// Adds only the attributes `element` does not already have, as the tree
    /// builder needs for repeated `<html>` and `<body>` start tags.
    ///
    /// One pass: the missing attributes are appended to the element's slice in
    /// a single relocation rather than one per attribute.
    pub fn add_attributes_if_missing(&mut self, element: NodeId, attrs: Vec<Attribute>) {
        if self.kind(element) != NodeKind::Element || attrs.is_empty() {
            return;
        }
        let data = self.arena.nodes.data[self.index(element)];
        let mut added: Vec<(AttrName, AttrValue, Attribute)> = Vec::new();
        for attr in attrs {
            let name = AttrName::from_qual(attr.name);
            let present = self
                .arena
                .element_attrs(data)
                .iter()
                .any(|(stored, _)| *stored == name)
                || added.iter().any(|(stored, _, _)| *stored == name);
            if present {
                continue;
            }
            let value = self.arena.push_attr_value(&attr.value);
            added.push((name, value, attr));
        }
        if added.is_empty() {
            return;
        }
        let stored: Vec<(AttrName, AttrValue)> = added
            .iter()
            .map(|(name, value, _)| (*name, *value))
            .collect();
        self.arena.append_element_attrs(data, &stored);
        for (name, _, attr) in &added {
            if name.ns == atoms::EMPTY {
                if name.local == atoms::ID {
                    self.reindex_id(element, &attr.value);
                } else if name.local == atoms::CLASS {
                    self.reindex_classes(element, &attr.value);
                }
            }
            self.observer.record(MutationRecord::Attribute {
                element,
                name: name.local,
                namespace: name.ns,
                old_value: None,
            });
        }
        let last = added.last().expect("added is not empty").0.local;
        self.after_attribute_change(element, last);
    }

    fn reindex_id(&mut self, element: NodeId, value: &str) {
        let data = self.arena.nodes.data[self.index(element)];
        let old = self.arena.elements.id[data.get()];
        if old.is_some() {
            self.drop_id_index_entry(old, element);
        }
        let new = if value.is_empty() {
            LocalName::NONE
        } else {
            self.ids.intern(value)
        };
        // Released only after the index entry is gone, so a reclaimed handle
        // can never be re-interned while a bucket is still keyed by it.
        if old.is_some() {
            self.ids.release(old);
        }
        self.arena.elements.id[data.get()] = new;
        let slot = self.index(element);
        self.arena.nodes.flags[slot].set(NodeFlags::HAS_ID, new.is_some());
        if new.is_some() {
            let key = self.take_id_index_key(new);
            self.indexes.get_mut().ids.push(key, element);
        }
    }

    fn reindex_classes(&mut self, element: NodeId, value: &str) {
        let data = self.arena.nodes.data[self.index(element)];
        let old: Vec<LocalName> = self.arena.element_classes(data).to_vec();
        for class in &old {
            self.drop_class_index_entry(*class, element);
        }
        let classes: Vec<LocalName> = value
            .split_ascii_whitespace()
            .map(|class| self.class_names.intern(class))
            .collect();
        for class in old {
            self.class_names.release(class);
        }
        self.arena.set_element_classes(data, &classes);
        let slot = self.index(element);
        self.arena.nodes.flags[slot].set(NodeFlags::HAS_CLASS, !classes.is_empty());
        // `class="a a"` (or `"A a"` in quirks mode) lands in its bucket once;
        // every value still takes its own key reference so removal, which
        // releases one per value, stays balanced.
        let mut keys: Vec<LocalName> = Vec::with_capacity(classes.len());
        for class in classes {
            let key = self.take_class_index_key(class);
            if !keys.contains(&key) {
                keys.push(key);
                self.indexes.get_mut().classes.push(key, element);
            }
        }
    }

    fn set_text(&mut self, node: NodeId, text: &str) -> Result<()> {
        if !self.kind(node).is_character_data() {
            return Err(hierarchy_request("only character data nodes hold text"));
        }
        let old_value = self.text_data(node).unwrap_or_default().to_owned();
        match self.kind(node) {
            NodeKind::Comment => {
                let data = self.arena.nodes.data[self.index(node)];
                self.arena.comments[data.get()] = text.to_owned();
            }
            _ if self.is_owned_text(node) => {
                // The node already owns its buffer: rewrite it in place.
                let data = self.arena.nodes.data[self.index(node)];
                let (index, _) = self.arena.texts.range[data.get()];
                let len = self.rope.set_owned(index, text);
                self.arena.texts.range[data.get()] = (index, len);
            }
            _ => {
                // Chunks are immutable, so a change writes a fresh copy into
                // the open buffer and releases the old chunk: copy-on-write at
                // chunk granularity (`docs/spec/02-architecture.md` §7).
                let span = self.rope.append(text);
                let data = self.arena.nodes.data[self.index(node)];
                self.arena.texts.chunk[data.get()] = self.open_chunk.clone();
                self.arena.texts.range[data.get()] = (span.start, span.len);
                self.rope.register_pending(node);
                self.maybe_seal();
            }
        }
        self.observer
            .record(MutationRecord::CharacterData { node, old_value });
        self.invalidate_keep(node);
        self.bump();
        Ok(())
    }

    /// Appends `text` to `parent`, merging with a trailing text node as the
    /// HTML tree construction algorithm requires.
    pub fn append_text(&mut self, parent: NodeId, text: &str) -> Result<()> {
        if !self.arena.is_live(parent) || !self.kind(parent).is_container() {
            return Err(hierarchy_request(
                "text can only be appended to a container",
            ));
        }
        let last = self.last_child(parent);
        if last.is_some() && self.kind(last) == NodeKind::Text {
            self.extend_text(last, text);
            self.bump();
            return Ok(());
        }
        let node = self.create_text(text);
        self.insert(parent, node, NodeId::NONE)
    }

    /// Appends `text` before `sibling`, merging with a preceding text node.
    pub fn insert_text_before(&mut self, sibling: NodeId, text: &str) -> Result<()> {
        let parent = self.parent(sibling);
        if !parent.is_some() {
            return Err(not_found("the reference node has no parent"));
        }
        let prev = self.previous_sibling(sibling);
        if prev.is_some() && self.kind(prev) == NodeKind::Text {
            self.extend_text(prev, text);
            self.bump();
            return Ok(());
        }
        let node = self.create_text(text);
        self.insert(parent, node, sibling)
    }

    /// Extends an existing text node with `text`.
    ///
    /// Three cases, all O(1) amortised in the length of the node
    /// (`docs/spec/04-dom-and-css.md` §8): the node already owns a buffer, so
    /// the bytes are pushed onto it; the node's bytes are the tail of the open
    /// rope buffer, so the append is free; or the node has been left behind by
    /// a seal, in which case it is promoted to a buffer of its own with one
    /// copy and never copied again. Text arriving in pieces — `\r\n`
    /// normalisation, character references, CDATA splits, `feed()` boundaries
    /// — used to re-copy the whole node on every piece.
    fn extend_text(&mut self, node: NodeId, text: &str) {
        if text.is_empty() {
            return;
        }
        let data = self.arena.nodes.data[self.index(node)];
        if self.is_owned_text(node) {
            let (index, _) = self.arena.texts.range[data.get()];
            let len = self.rope.extend_owned(index, text);
            self.arena.texts.range[data.get()] = (index, len);
            return;
        }
        let (start, len) = self.arena.texts.range[data.get()];
        let contiguous = Arc::ptr_eq(&self.arena.texts.chunk[data.get()], &self.open_chunk)
            && (start + len) as usize == self.rope.open_len();
        if contiguous {
            let span = self.rope.append(text);
            debug_assert_eq!(span.start, start + len);
            self.arena.texts.range[data.get()] = (start, len + span.len);
            self.maybe_seal();
            return;
        }
        let existing = self.text_data(node).unwrap_or_default().to_owned();
        let span = self.rope.promote(&existing, text);
        self.arena.texts.chunk[data.get()] = self.owned_chunk.clone();
        self.arena.texts.range[data.get()] = (span.start, span.len);
    }

    fn maybe_seal(&mut self) {
        if self.rope.should_seal() {
            self.seal_open_chunk();
        }
    }

    /// Seals the open buffer and re-points every text node that referenced it.
    pub fn seal_open_chunk(&mut self) {
        let Some((chunk, pending)) = self.rope.seal() else {
            return;
        };
        for node in pending {
            if !self.arena.is_live(node)
                || !matches!(self.kind(node), NodeKind::Text | NodeKind::CData)
            {
                continue;
            }
            let data = self.arena.nodes.data[self.index(node)];
            if Arc::ptr_eq(&self.arena.texts.chunk[data.get()], &self.open_chunk) {
                self.arena.texts.chunk[data.get()] = chunk.clone();
            }
        }
    }

    /// Rope statistics: sealed chunks, bytes ever written, bytes still open.
    pub fn rope_stats(&self) -> (usize, usize, usize) {
        (
            self.rope.sealed_chunks(),
            self.rope.bytes_written(),
            self.rope.open_len(),
        )
    }

    /// Text nodes that outgrew the shared rope and own a buffer
    /// (see [`crate::text`]).
    pub fn promoted_text_nodes(&self) -> usize {
        self.rope.owned_buffers()
    }

    // ------------------------------------------------------------ selectors

    pub(crate) fn with_selector_cache<R>(&self, f: impl FnOnce(&mut SelectorCache) -> R) -> R {
        f(&mut self.selector_cache.borrow_mut())
    }

    /// Runs `f` with the matching engine's caches.
    ///
    /// The borrow is held for the whole of a query, so nothing reachable from
    /// selector matching may compile a selector: `get_or_compile` shares this
    /// cell. Nothing does — the matcher only reads the arena.
    pub(crate) fn with_matching_caches<R>(
        &self,
        f: impl FnOnce(&mut ::selectors::context::SelectorCaches) -> R,
    ) -> R {
        self.selector_cache
            .borrow_mut()
            .with_matching_caches(self.mutation_generation, f)
    }

    // -------------------------------------------------------------- memory

    /// Bytes the arena and its indexes hold, by area. Used by the budget
    /// tests (`docs/spec/04-dom-and-css.md` §8).
    pub fn memory_report(&self) -> MemoryReport {
        let nodes = self.arena.nodes.len();
        MemoryReport {
            nodes,
            elements: self.arena.elements.len(),
            node_columns: nodes * crate::arena::Nodes::BYTES_PER_NODE,
            element_columns: self.arena.elements.len() * crate::arena::Elements::BYTES_PER_ELEMENT,
            attributes: self.arena.attrs.len() * size_of::<(AttrName, AttrValue)>(),
            attribute_values: self.arena.attr_values.len(),
            classes: self.arena.class_atoms.len() * size_of::<LocalName>(),
            text_columns: self.arena.texts.len()
                * (size_of::<Arc<TextChunk>>() + 2 * size_of::<u32>()),
            text_bytes: self.live_text_bytes(),
            text_bytes_written: self.rope.bytes_written(),
            names: self.ids.memory_bytes() + self.class_names.memory_bytes(),
            indexes: {
                let indexes = self.indexes.borrow();
                indexes.ids.bytes()
                    + indexes.classes.bytes()
                    + indexes.tags.bytes()
                    + self.detached_roots.capacity() * size_of::<NodeId>()
                    + self.element_prefixes.capacity()
                        * (size_of::<NodeId>() + size_of::<Atom>() + 1)
            },
        }
    }

    /// Bytes of text the document actually holds.
    ///
    /// Chunks are immutable and shared, so the figure is the size of every
    /// distinct chunk a live text node still references, plus the open buffer
    /// and the buffers of promoted nodes. It is not a running total: bytes
    /// released by a sweep or overwritten by a mutation drop out of it
    /// (`docs/spec/04-dom-and-css.md` §8).
    pub fn live_text_bytes(&self) -> usize {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut total = self.rope.open_len() + self.rope.owned_bytes();
        for slot in 0..self.arena.slots() {
            let node = NodeId(slot as u32);
            if !self.arena.is_live(node)
                || !matches!(self.kind(node), NodeKind::Text | NodeKind::CData)
            {
                continue;
            }
            let chunk = &self.arena.texts.chunk[self.arena.nodes.data[slot].get()];
            if Arc::ptr_eq(chunk, &self.open_chunk) || Arc::ptr_eq(chunk, &self.owned_chunk) {
                continue;
            }
            if seen.insert(Arc::as_ptr(chunk) as usize) {
                total += chunk.len();
            }
        }
        total
    }
}

/// Byte accounting for one document (`docs/spec/04-dom-and-css.md` §8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MemoryReport {
    pub nodes: usize,
    pub elements: usize,
    pub node_columns: usize,
    pub element_columns: usize,
    pub attributes: usize,
    pub attribute_values: usize,
    pub classes: usize,
    pub text_columns: usize,
    /// Bytes of text the document holds now (`Document::live_text_bytes`).
    pub text_bytes: usize,
    /// Every byte ever written into the rope, copies included. Diagnostic
    /// only; deliberately not part of [`MemoryReport::total`].
    pub text_bytes_written: usize,
    /// The document-local id and class tables.
    pub names: usize,
    pub indexes: usize,
}

impl MemoryReport {
    /// Everything charged to the arena, including the id, class and tag
    /// indexes.
    pub fn total(&self) -> usize {
        self.node_columns
            + self.element_columns
            + self.attributes
            + self.attribute_values
            + self.classes
            + self.text_columns
            + self.text_bytes
            + self.names
            + self.indexes
    }

    /// The arena proper: node and element columns plus attribute storage.
    /// This is the "arena memory per node" of the budget table.
    pub fn arena_core(&self) -> usize {
        self.node_columns + self.element_columns + self.attributes + self.attribute_values
    }
}

#[derive(Default)]
struct TouchedKeys {
    tags: HashSet<Atom>,
    ids: HashSet<LocalName>,
    classes: HashSet<LocalName>,
    /// One entry per element column reference, released once the index keys
    /// are gone so a reclaimed slot cannot be re-interned under a live key.
    released_ids: Vec<LocalName>,
    released_classes: Vec<LocalName>,
    /// Folded keys in quirks mode that need to be released (the lowercase versions)
    released_folded_ids: Vec<LocalName>,
    released_folded_classes: Vec<LocalName>,
}

/// One index bucket: the elements carrying a key, plus the structure
/// generation the list was last put into tree order for.
///
/// Buckets are attacker-shaped: duplicate ids are legal-if-invalid markup and
/// a class shared by tens of thousands of elements is an ordinary list page.
/// Sorting on demand and remembering that the order is still good makes
/// `getElementById` and the first-match exit of `querySelector` constant time
/// between mutations (`docs/spec/04-dom-and-css.md` §8).
///
/// The bucket is 32 bytes so a page of unique ids stays inside the §8 memory
/// budget; the position map a large bucket needs for constant-time removal
/// lives in [`Index::positions`], keyed by the same key.
#[derive(Default)]
struct Bucket {
    nodes: NodeList,
    /// `0` means "not in tree order"; otherwise the structure generation the
    /// order was computed for.
    sorted_at: u64,
}

/// Buckets with at most this many nodes are searched linearly on removal.
const POSITION_MAP_THRESHOLD: usize = 8;

/// Node → position in `Bucket::nodes`, kept only for buckets past
/// [`POSITION_MAP_THRESHOLD`] and built on the first removal from one.
type PositionMap = HashMap<NodeId, usize>;

impl Bucket {
    fn bytes(&self) -> usize {
        if self.nodes.spilled() {
            self.nodes.capacity() * size_of::<NodeId>()
        } else {
            0
        }
    }
}

/// One of the id, class or tag indexes: the buckets plus the position maps
/// of the large ones.
struct Index<K> {
    buckets: HashMap<K, Bucket>,
    positions: HashMap<K, PositionMap>,
}

impl<K> Default for Index<K> {
    fn default() -> Self {
        Self {
            buckets: HashMap::new(),
            positions: HashMap::new(),
        }
    }
}

impl<K: std::hash::Hash + Eq + Copy> Index<K> {
    fn clear(&mut self) {
        self.buckets.clear();
        self.positions.clear();
    }

    fn push(&mut self, key: K, node: NodeId) {
        let bucket = self.buckets.entry(key).or_default();
        bucket.nodes.push(node);
        bucket.sorted_at = 0;
        if let Some(map) = self.positions.get_mut(&key) {
            map.insert(node, bucket.nodes.len() - 1);
        }
    }

    /// Removes a node from one bucket, dropping the bucket when it empties.
    ///
    /// Buckets are re-sorted on demand, so removal swaps the last entry into
    /// the hole instead of shifting. Large buckets keep a position map so the
    /// swap is constant time.
    fn remove(&mut self, key: K, node: NodeId) {
        let Some(bucket) = self.buckets.get_mut(&key) else {
            return;
        };
        let position = if bucket.nodes.len() <= POSITION_MAP_THRESHOLD {
            bucket.nodes.iter().position(|n| *n == node)
        } else {
            let map = self.positions.entry(key).or_insert_with(|| {
                bucket
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(i, &n)| (n, i))
                    .collect()
            });
            let position = map.remove(&node);
            if let Some(position) = position
                && position + 1 < bucket.nodes.len()
            {
                map.insert(bucket.nodes[bucket.nodes.len() - 1], position);
            }
            position
        };
        let Some(position) = position else {
            return;
        };
        bucket.nodes.swap_remove(position);
        bucket.sorted_at = 0;
        if bucket.nodes.is_empty() {
            self.buckets.remove(&key);
            self.positions.remove(&key);
        } else if bucket.nodes.len() <= POSITION_MAP_THRESHOLD {
            // Drop the position map when bucket shrinks to the linear-search threshold
            self.positions.remove(&key);
        }
    }

    /// Drops freed nodes from the buckets of the touched keys.
    fn purge(&mut self, keys: &HashSet<K>, freed: &HashSet<NodeId>) {
        for key in keys {
            let Some(bucket) = self.buckets.get_mut(key) else {
                continue;
            };
            let before = bucket.nodes.len();
            bucket.nodes.retain(|node| !freed.contains(node));
            if bucket.nodes.len() != before {
                bucket.sorted_at = 0;
                // The positions shifted; the next removal rebuilds the map.
                self.positions.remove(key);
            }
            if bucket.nodes.is_empty() {
                self.buckets.remove(key);
                self.positions.remove(key);
            }
        }
    }

    fn bytes(&self) -> usize {
        self.buckets.capacity() * (size_of::<K>() + size_of::<Bucket>() + 1)
            + self.buckets.values().map(Bucket::bytes).sum::<usize>()
            + self.positions.capacity() * (size_of::<K>() + size_of::<PositionMap>() + 1)
            + self
                .positions
                .values()
                .map(|map| map.capacity() * (size_of::<NodeId>() + size_of::<usize>() + 1))
                .sum::<usize>()
    }
}

/// The id, class and tag indexes `Document::mutate` maintains
/// (`docs/spec/04-dom-and-css.md` §4).
#[derive(Default)]
struct Indexes {
    ids: Index<LocalName>,
    classes: Index<LocalName>,
    tags: Index<Atom>,
}

/// Pre-order positions for every live slot.
///
/// `enter[node] < enter[other]` is "node comes first in tree order" and
/// `enter[root] <= enter[node] < exit[root]` is "node is in the subtree of
/// root": both are integer compares, so ordering a query's results costs no
/// allocation and no ancestor walk (the `precedes` this replaces allocated two
/// root paths per comparison). The whole table is rebuilt in one pass after a
/// structural change, which is what `structure_generation` tracks.
#[derive(Default)]
struct OrderIndex {
    enter: Vec<u32>,
    exit: Vec<u32>,
    valid_for: u64,
    stack: Vec<(NodeId, bool)>,
}

impl OrderIndex {
    const UNASSIGNED: u32 = u32::MAX;

    #[inline]
    fn enter_of(&self, node: NodeId) -> u32 {
        self.enter
            .get(node.raw() as usize)
            .copied()
            .unwrap_or(Self::UNASSIGNED)
    }

    #[inline]
    fn contains(&self, root: NodeId, node: NodeId) -> bool {
        let (start, end) = match (
            self.enter.get(root.raw() as usize),
            self.exit.get(root.raw() as usize),
        ) {
            (Some(&start), Some(&end)) if start != Self::UNASSIGNED => (start, end),
            _ => return false,
        };
        let position = self.enter_of(node);
        position != Self::UNASSIGNED && position >= start && position < end
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        // The arena is plain vectors, so dropping it is flat; nothing here
        // recurses even for a million-deep tree.
        debug_assert!(self.arena.slots() >= self.arena.live());
    }
}

/// Errors surface as `tenta_core::Error`; this alias keeps signatures short.
pub type DomResult<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutation::Mutation;
    use tenta_core::atom::atoms;

    fn tree() -> (Document, Vec<NodeId>) {
        let mut document = Document::new();
        let html = document.create_html_element(atoms::HTML);
        document
            .mutate(Mutation::Append {
                parent: document.document_node(),
                child: html,
            })
            .expect("append");
        let body = document.create_html_element(atoms::BODY);
        document
            .mutate(Mutation::Append {
                parent: html,
                child: body,
            })
            .expect("append");
        let first = document.create_html_element(atoms::DIV);
        let second = document.create_html_element(atoms::DIV);
        let nested = document.create_html_element(atoms::SPAN);
        document
            .mutate(Mutation::Append {
                parent: body,
                child: first,
            })
            .expect("append");
        document
            .mutate(Mutation::Append {
                parent: first,
                child: nested,
            })
            .expect("append");
        document
            .mutate(Mutation::Append {
                parent: body,
                child: second,
            })
            .expect("append");
        (document, vec![html, body, first, nested, second])
    }

    #[test]
    fn tree_order_follows_a_pre_order_walk() {
        let (document, nodes) = tree();
        let (html, body, first, nested, second) =
            (nodes[0], nodes[1], nodes[2], nodes[3], nodes[4]);
        assert!(document.precedes(html, body));
        assert!(document.precedes(body, first));
        assert!(document.precedes(first, nested));
        assert!(document.precedes(nested, second));
        assert!(!document.precedes(second, nested));
        assert!(!document.precedes(first, first));

        let mut shuffled = vec![second, nested, html, first, body];
        document.sort_tree_order(&mut shuffled);
        assert_eq!(shuffled, vec![html, body, first, nested, second]);
        assert_eq!(
            document.descendants_inclusive(html).collect::<Vec<_>>(),
            vec![html, body, first, nested, second]
        );
    }

    #[test]
    fn ancestors_and_roots() {
        let (document, nodes) = tree();
        let nested = nodes[3];
        assert_eq!(
            document.ancestors(nested).collect::<Vec<_>>(),
            vec![nodes[2], nodes[1], nodes[0], document.document_node()]
        );
        assert_eq!(document.root_of(nested), document.document_node());
        assert!(document.is_inclusive_ancestor(nodes[1], nested));
        assert!(document.is_inclusive_ancestor(nested, nested));
        assert!(!document.is_inclusive_ancestor(nested, nodes[1]));
    }

    #[test]
    fn document_state_defaults_and_setters() {
        let mut document = Document::new();
        assert_eq!(document.ready_state(), ReadyState::Loading);
        assert_eq!(document.content_type(), "text/html");
        assert_eq!(document.encoding(), "UTF-8");
        assert!(document.is_html_document());
        assert_eq!(document.url(), None);
        assert_eq!(document.base_url(), None);

        document.set_ready_state(ReadyState::Complete);
        document.set_content_type("application/xhtml+xml");
        document.set_encoding("windows-1250");
        assert_eq!(document.ready_state(), ReadyState::Complete);
        assert!(!document.is_html_document());
        assert_eq!(document.encoding(), "windows-1250");

        document.set_url(Some("https://example.test/dir/page".parse().unwrap()));
        assert_eq!(
            document.base_url().map(|url| url.as_str()),
            Some("https://example.test/dir/page")
        );
        assert_eq!(
            document.resolve_url("../other").map(|url| url.to_string()),
            Some("https://example.test/other".to_owned())
        );
    }

    #[test]
    fn attribute_lookup_by_string_never_interns() {
        const NAME: &str = "a-name-no-page-ever-used-xyzzy";
        let mut document = Document::new();
        let element = document.create_html_element(atoms::DIV);
        assert!(tenta_core::Atom::get(NAME).is_none());
        assert_eq!(document.attribute_str(element, NAME), None);
        assert!(
            tenta_core::Atom::get(NAME).is_none(),
            "an untrusted attribute name must not grow the atom table"
        );
    }

    #[test]
    fn document_ids_are_unique_and_handles_carry_them() {
        let mut first = Document::new();
        let second = Document::new();
        assert_ne!(first.document_id(), second.document_id());
        let element = first.create_html_element(atoms::DIV);
        let handle = first.handle(element).expect("live");
        assert_eq!(handle.document_id(), first.document_id());
        assert_eq!(handle.id(), element);
        first.release(handle);
    }

    #[test]
    fn a_handle_to_a_dead_node_is_refused() {
        let mut document = Document::new();
        let element = document.create_html_element(atoms::DIV);
        document.sweep_detached();
        let error = document.handle(element).expect_err("the node was freed");
        assert_eq!(
            crate::error::dom_exception_name(&error),
            Some("InvalidStateError")
        );
    }

    #[test]
    fn shadow_roots_are_attached_once_and_are_not_template_contents() {
        let mut document = Document::new();
        let host = document.create_html_element(atoms::DIV);
        let root = document.attach_shadow_root(host).expect("attach");
        assert_eq!(document.kind(root), NodeKind::ShadowRoot);
        assert_eq!(document.shadow_root(host), Some(root));
        assert_eq!(document.template_contents(host), None);
        assert!(document.attach_shadow_root(host).is_err());

        let text = document.create_text("x");
        assert!(document.attach_shadow_root(text).is_err());
    }
}
