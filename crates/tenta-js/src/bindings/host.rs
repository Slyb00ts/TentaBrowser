//! The engine-independent host object that backs every DOM wrapper.
//!
//! [`HostObject`] implements the generated `*Ops` traits over a shared
//! [`tenta_dom::Document`], so the same host serves both the V8 and QuickJS
//! glue (`docs/spec/05-javascript.md` §3). It is a small enum: a `Node` host
//! stands for one node, a `NodeList` host stands for the live child collection
//! of one node. Both hold a shared handle to the context (containing the document
//! and snapshot slab), so the arena and snapshots outlive every wrapper, and a
//! generational [`tenta_core::NodeRef`] so a wrapper kept across a sweep fails
//! the handle validation instead of aliasing a recycled slot.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId, NodeRef};
use tenta_dom::{Document, Mutation, NodeKind};
use tenta_html::{ElementFlags, QualName};

use super::{
    AddEventListenerOptionsOrBoolean, AnyValue, CSSStyleDeclaration, CSSStyleDeclarationOps,
    CharacterDataOps, Comment, CommentOps, CustomEventConstructor, CustomEventInit, CustomEventOps,
    DOMTokenList, DOMTokenListOps, DocumentFragment, DocumentFragmentOps, DocumentOps, Element,
    ElementOps, Event, EventConstructor, EventInit, EventListener, EventListenerOptionsOrBoolean,
    EventOps, EventTarget, EventTargetOps, HTMLAnchorElementOps, HTMLButtonElementOps,
    HTMLCollection, HTMLCollectionOps, HTMLDivElementOps, HTMLElement, HTMLElementOps,
    HTMLFormElementOps, HTMLIFrameElementOps, HTMLImageElementOps, HTMLInputElementOps,
    HTMLLabelElementOps, HTMLOptionElementOps, HTMLParagraphElementOps, HTMLScriptElementOps,
    HTMLSelectElementOps, HTMLSpanElementOps, HTMLTemplateElementOps, HTMLTextAreaElementOps,
    Handle, JsError, MouseEventConstructor, MouseEventInit, MouseEventOps, Node, NodeList,
    NodeListOps, NodeOps, Realm, SVGElementOps, Text, TextOps, UIEventConstructor, UIEventInit,
    UIEventOps,
};
use crate::dom::events::{
    CallbackRef, DispatchTarget, EventKey, EventKind, EventRecord, EventSlab, ListenerTable,
    ScriptHost, TargetKey, dispatch, dispatch_at,
};
use crate::event_loop::SharedClock;

/// A document shared between the host objects of one world and the engine that
/// drives it. Every `HostObject` for the document holds one of these clones, so
/// the arena outlives every wrapper.
///
/// Re-exported from `tenta-dom`, which parses into the same handle
/// (`tenta_dom::SharedSink`): the engine and the parser must name one type, or
/// a page would have to choose between running script and building the tree.
pub use tenta_dom::SharedDocument;

/// A key referencing a static snapshot in the slab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotKey {
    /// Index into the slab.
    pub index: u32,
    /// Generation counter to detect reuse.
    pub generation: u16,
}

/// One slot in the snapshot slab.
struct Slot {
    generation: u16,
    nodes: Option<Vec<NodeRef>>,
}

/// A slab backing static snapshot collections (querySelectorAll, getElementsByTagName, etc.).
/// Reuses slots via a free list, with generation counters to detect stale snapshot keys.
///
/// NOTE: Like [`NodeRef`], the 16-bit generation counter wraps; a snapshot key older than
/// ~65k reuses of the same slot can theoretically alias a new snapshot. This matches the
/// node-handle TODO and is acceptable for this phase.
pub struct SnapshotSlab {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

impl SnapshotSlab {
    /// Creates a new empty snapshot slab.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Inserts a new snapshot, reusing a free slot if available.
    pub fn insert(&mut self, nodes: Vec<NodeRef>) -> SnapshotKey {
        let (index, slot) = if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            (index, slot)
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                nodes: None,
            });
            (index, &mut self.slots[index as usize])
        };
        let generation = slot.generation;
        slot.nodes = Some(nodes);
        SnapshotKey { index, generation }
    }

    /// Retrieves the nodes for a snapshot key, returning None if the key is stale or out of range.
    pub fn get(&self, key: SnapshotKey) -> Option<&[NodeRef]> {
        let slot = self.slots.get(key.index as usize)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.nodes.as_deref()
    }

    /// Marks a snapshot as dead: bumps the generation, clears nodes, and returns the slot to the free list.
    pub fn remove(&mut self, key: SnapshotKey) {
        let slot = match self.slots.get_mut(key.index as usize) {
            Some(s) => s,
            None => return,
        };
        if slot.generation != key.generation {
            return; // Already reused.
        }
        slot.generation = slot.generation.wrapping_add(1);
        slot.nodes = None;
        self.free.push(key.index);
    }

    /// Clears all snapshots and the free list.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
    }
}

impl Default for SnapshotSlab {
    fn default() -> Self {
        Self::new()
    }
}

/// The shared host context: document, snapshots, event state and the script
/// callback boundary for one wrapper world.
pub struct HostContext {
    /// The document all hosts point to.
    pub doc: SharedDocument,
    /// The snapshot slab for static collections.
    pub snapshots: RefCell<SnapshotSlab>,
    /// The event listeners registered on this world's nodes.
    pub listeners: RefCell<ListenerTable>,
    /// The live event records for this world.
    pub events: RefCell<EventSlab>,
    /// The engine's callback boundary, installed at `Engine::build` and cleared
    /// at teardown. `None` before installation and after teardown.
    pub script: RefCell<Option<Rc<dyn ScriptHost>>>,
    /// The event loop's clock, which `Event.timeStamp` reads. It is the loop's
    /// clock and not a private origin so that a timestamp is directly
    /// comparable with `performance.now()`, as script assumes — and so that a
    /// virtual clock moves both together.
    pub clock: SharedClock,
    /// Elements with a synthetic `click()` in progress, so a nested `click()`
    /// on the same element during its own dispatch is a no-op (the DOM
    /// "click in progress" flag).
    pub click_in_progress: RefCell<HashSet<NodeId>>,
}

impl HostContext {
    /// Builds a fresh context over `doc`, timestamping events against `clock`,
    /// with empty event state and no script host yet.
    pub fn new(doc: SharedDocument, clock: SharedClock) -> Self {
        Self {
            doc,
            snapshots: RefCell::new(SnapshotSlab::new()),
            listeners: RefCell::new(ListenerTable::new()),
            events: RefCell::new(EventSlab::new()),
            script: RefCell::new(None),
            clock,
            click_in_progress: RefCell::new(HashSet::new()),
        }
    }

    /// Milliseconds since the loop origin, for `Event.timeStamp` — the same
    /// reading `performance.now()` returns.
    pub fn time_stamp(&self) -> f64 {
        self.clock.now().as_millis_f64()
    }

    /// Drains the listener entries of every node the document's most recent
    /// sweep freed, releasing the stored callback reference each entry owned.
    ///
    /// The entries are drained by the exact [`NodeRef`] the document recorded,
    /// generation included — [`ListenerTable::drain_target_at`] — so the key
    /// matches precisely the registrations made while that node lived. A drain
    /// on the bare id would also reach a listener registered on a node that
    /// later took the freed slot; this one cannot. The generation is known here
    /// because the document recorded it before the slot was recycled.
    ///
    /// Releasing a freed node's entries is a memory reclaim, not a correctness
    /// one: their key can never match a live target again. Without it every
    /// listener registered on a swept node keeps a V8 value alive for the life
    /// of the world, because a [`CallbackRef`] is a bare handle with no `Drop`.
    ///
    /// A no-op when the last sweep freed nothing. It is also a silent no-op
    /// when no [`ScriptHost`] is installed, because there is then no
    /// `release_value` to free a reference through — so a caller must run it at
    /// a point where the world has one, which for a page engine is any entry
    /// point after `Engine::build` installed the host.
    pub fn drain_swept_listeners(&self) {
        // The references are copied out so the document borrow ends before
        // anything else is touched, matching the borrow discipline the engine
        // boundary states for a caller; nothing below re-enters the document.
        let freed = self.doc.borrow().sweep_freed_nodes().to_vec();
        if freed.is_empty() {
            return;
        }
        // The entries leave the table first, so no borrow of it is held while a
        // released value drops (which can touch the wrapper world's own cells).
        let drained: Vec<CallbackRef> = {
            let mut listeners = self.listeners.borrow_mut();
            freed
                .iter()
                .flat_map(|node| listeners.drain_target_at(TargetKey::Node(*node)))
                .collect()
        };
        let script = self.script.borrow();
        if let Some(script) = script.as_ref() {
            for callback in drained {
                script.release_value(callback.0);
            }
        }
    }
}

/// A context shared between the host objects of one world and the engine.
pub type SharedContext = Rc<HostContext>;

/// The Rust side of a DOM wrapper.
///
/// One `HostObject` is created per live wrapper and owned by the world's host
/// map (`docs/spec/05-javascript.md` §3); the wrapper's internal field only
/// borrows a pointer to it.
pub enum HostObject {
    /// A wrapper for a single node.
    Node {
        /// The context (document and snapshots) the node lives in.
        ctx: SharedContext,
        /// A generational reference to the node this wrapper stands for.
        node: NodeRef,
    },
    /// A wrapper for the live child `NodeList` of `owner`.
    NodeList {
        /// The context the collection's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the node whose children the list holds.
        owner: NodeRef,
    },
    /// A wrapper for the live element-child `HTMLCollection` of `owner`.
    HtmlCollection {
        /// The context the collection's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the node whose element children the collection holds.
        owner: NodeRef,
    },
    /// A wrapper for a static snapshot of nodes (querySelectorAll result, etc.).
    StaticNodeList {
        /// The context holding the snapshot slab.
        ctx: SharedContext,
        /// The key to the snapshot in the slab.
        key: SnapshotKey,
    },
    /// A wrapper for a static snapshot of elements (getElementsByTagName result, etc.).
    StaticHtmlCollection {
        /// The context holding the snapshot slab.
        ctx: SharedContext,
        /// The key to the snapshot in the slab.
        key: SnapshotKey,
    },
    /// A wrapper for an `Event` (or a subclass), keyed into the event slab.
    Event {
        /// The context holding the event slab.
        ctx: SharedContext,
        /// The key to the event record in the slab.
        key: EventKey,
    },
    /// A wrapper for the live `DOMTokenList` (`classList`) of `owner`.
    TokenList {
        /// The context the token list's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose `class` the list reflects.
        owner: NodeRef,
    },
    /// A wrapper for the inline `CSSStyleDeclaration` (`style`) of `owner`.
    Style {
        /// The context the style declaration's owner lives in.
        ctx: SharedContext,
        /// A generational reference to the element whose `style` the block reflects.
        owner: NodeRef,
    },
    /// The window: the browsing context itself, not a node.
    ///
    /// It is the only host with no arena slot and no id, because the window is
    /// the JavaScript global object — `window`, `self`, `top` and `globalThis`
    /// are one object, so a wrapper for it cannot be a fresh instance of a
    /// template. The world resolves its handle to that global object instead of
    /// instantiating anything (see `WrapperWorld::wrapper`), which is why this
    /// variant carries nothing but the context.
    Window {
        /// The context whose browsing context this is.
        ctx: SharedContext,
    },
}

impl HostObject {
    /// Builds a node host for `node` in `ctx`.
    pub fn node(ctx: SharedContext, node: NodeRef) -> Self {
        Self::Node { ctx, node }
    }

    /// Builds a `NodeList` host for the children of `owner` in `ctx`.
    pub fn node_list(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::NodeList { ctx, owner }
    }

    /// Builds an `HTMLCollection` host for the element children of `owner` in `ctx`.
    pub fn html_collection(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::HtmlCollection { ctx, owner }
    }

    /// Builds a `DOMTokenList` host for the `classList` of `owner` in `ctx`.
    pub fn token_list(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::TokenList { ctx, owner }
    }

    /// Builds a `CSSStyleDeclaration` host for the `style` of `owner` in `ctx`.
    pub fn style(ctx: SharedContext, owner: NodeRef) -> Self {
        Self::Style { ctx, owner }
    }

    /// Builds a `NodeList` host for a static snapshot.
    pub fn static_node_list(ctx: SharedContext, key: SnapshotKey) -> Self {
        Self::StaticNodeList { ctx, key }
    }

    /// Builds an `HTMLCollection` host for a static snapshot.
    pub fn static_html_collection(ctx: SharedContext, key: SnapshotKey) -> Self {
        Self::StaticHtmlCollection { ctx, key }
    }

    /// Builds an `Event` host for the event record at `key`.
    pub fn event(ctx: SharedContext, key: EventKey) -> Self {
        Self::Event { ctx, key }
    }

    /// Builds the window host for `ctx`.
    pub fn window(ctx: SharedContext) -> Self {
        Self::Window { ctx }
    }

    /// The shared context this host belongs to.
    pub fn context(&self) -> &SharedContext {
        match self {
            HostObject::Node { ctx, .. }
            | HostObject::NodeList { ctx, .. }
            | HostObject::HtmlCollection { ctx, .. }
            | HostObject::StaticNodeList { ctx, .. }
            | HostObject::StaticHtmlCollection { ctx, .. }
            | HostObject::Event { ctx, .. }
            | HostObject::TokenList { ctx, .. }
            | HostObject::Style { ctx, .. }
            | HostObject::Window { ctx } => ctx,
        }
    }

    /// Reads the event record this host stands for, or returns `default` when
    /// the host is not an `Event` or the record has been reclaimed.
    fn with_event<R>(&self, default: R, f: impl FnOnce(&EventRecord) -> R) -> R {
        if let HostObject::Event { ctx, key } = self
            && let Some(record) = ctx.events.borrow().get(*key)
        {
            return f(record);
        }
        default
    }

    /// Mutates the event record this host stands for, if it is a live `Event`.
    fn with_event_mut(&self, f: impl FnOnce(&mut EventRecord)) {
        if let HostObject::Event { ctx, key } = self
            && let Some(record) = ctx.events.borrow_mut().get_mut(*key)
        {
            f(record);
        }
    }

    /// Releases one stored value through the installed script host, if any.
    fn release_stored(&self, bits: u64) {
        if let Some(script) = self.context().script.borrow().as_ref() {
            script.release_value(bits);
        }
    }

    /// The opaque handle bits this host stands for, tagged by variant so a node
    /// and its `NodeList` (and a recycled slot) never share a key.
    pub fn handle_bits(&self) -> u64 {
        match self {
            HostObject::Node { node, .. } => encode_node(*node),
            HostObject::NodeList { owner, .. } => encode_collection(*owner),
            HostObject::HtmlCollection { owner, .. } => encode_html_collection(*owner),
            HostObject::StaticNodeList { key, .. } => encode_snapshot(TAG_STATIC_NODE_LIST, *key),
            HostObject::StaticHtmlCollection { key, .. } => {
                encode_snapshot(TAG_STATIC_HTML_COLLECTION, *key)
            }
            HostObject::Event { key, .. } => encode_event(*key),
            HostObject::TokenList { owner, .. } => encode_token_list(*owner),
            HostObject::Style { owner, .. } => encode_style(*owner),
            HostObject::Window { .. } => encode_window(),
        }
    }

    /// The document and the node this host is anchored to: the node itself for
    /// a `Node` host, the owner for a `NodeList` or `HtmlCollection` host.
    /// For snapshot variants (StaticNodeList/StaticHtmlCollection), returns the document
    /// and NodeId::NONE, as snapshot hosts never participate in node-navigation ops
    /// (routing by signature brand ensures only live-collection ops call navigate).
    fn anchor(&self) -> (&SharedDocument, NodeId) {
        match self {
            HostObject::Node { ctx, node } => (&ctx.doc, node.id),
            HostObject::NodeList { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::HtmlCollection { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::TokenList { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::Style { ctx, owner } => (&ctx.doc, owner.id),
            HostObject::StaticNodeList { ctx, .. } => (&ctx.doc, NodeId::NONE),
            HostObject::StaticHtmlCollection { ctx, .. } => (&ctx.doc, NodeId::NONE),
            HostObject::Event { ctx, .. } => (&ctx.doc, NodeId::NONE),
            // The window is not in the tree, so there is no node to anchor to.
            // It only reaches the anchored paths if a generated operation that
            // needs a node is somehow reachable on the window, which none is:
            // the window's own members are installed natively.
            HostObject::Window { ctx } => (&ctx.doc, NodeId::NONE),
        }
    }

    /// The event target this host stands for, or `None` for a host that cannot
    /// be one.
    ///
    /// Only a node wrapper and the window carry the `EventTarget` prototype
    /// chain, so a `NodeList`, a snapshot, an `Event` or a style declaration
    /// reaching here means the receiver was not what the template's signature
    /// promised.
    fn dispatch_target(&self) -> Option<DispatchTarget> {
        match self {
            HostObject::Node { node, .. } => Some(DispatchTarget::node(*node)),
            HostObject::Window { .. } => Some(DispatchTarget::window()),
            _ => None,
        }
    }

    /// The listener-table key this host registers listeners under. See
    /// [`Self::dispatch_target`] for why the other variants have none.
    fn target_key(&self) -> Option<TargetKey> {
        match self {
            HostObject::Node { node, .. } => Some(TargetKey::Node(*node)),
            HostObject::Window { .. } => Some(TargetKey::Window),
            _ => None,
        }
    }

    /// Sets a reflected attribute (`id`, `class`, …) to `value` on the anchored
    /// element, ignoring any mutation error the way a fast-API reflected setter
    /// must. `name` is an already-lowercase attribute local name.
    fn reflect_attribute(&self, name: &str, value: &str) {
        let (doc, id) = self.anchor();
        let qname = QualName::new(atoms::EMPTY, Atom::new(name));
        let _ = doc.borrow_mut().mutate(Mutation::SetAttribute {
            element: id,
            name: qname,
            value: value.to_owned(),
        });
    }

    /// Reads a reflected DOMString attribute, returning "" when it is absent —
    /// the WHATWG default for a string-reflecting IDL getter. `name` is an
    /// already-lowercase local name.
    fn reflect_string(&self, name: &str) -> String {
        let (doc, id) = self.anchor();
        doc.borrow()
            .attribute_str(id, name)
            .unwrap_or("")
            .to_owned()
    }

    /// Reads a reflected URL-valued attribute — the HTML Standard's rule for a
    /// `USVString` attribute whose content attribute is defined to contain a
    /// URL, which is what backs `href`, `src` and `action`
    /// (`wpt/checkout/html/dom/reflection.js`, the `"url"` reflection case).
    /// An absent attribute yields the empty string (never the document URL, so
    /// `if (img.src)` stays falsy for `<img>` with no `src`); a present value
    /// is resolved against the element's node document and the serialized
    /// absolute URL returned; a value that will not parse is returned verbatim
    /// rather than throwing. `name` is an already-lowercase local name.
    fn reflect_url(&self, name: &str) -> String {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let Some(raw) = doc.attribute_str(id, name) else {
            return String::new();
        };
        match doc.resolve_url(raw) {
            Some(url) => url.to_string(),
            None => raw.to_owned(),
        }
    }

    /// Reads a reflected boolean attribute: present (any value) is `true`.
    fn reflect_bool(&self, name: &str) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().attribute_str(id, name).is_some()
    }

    /// Reads a form control's IDL `value` under the dirty value model
    /// (`docs/spec/05-javascript.md` §9a): the dirty value once one has been
    /// set, and the `attribute` content attribute until then. A control whose
    /// default comes from somewhere other than an attribute (a textarea's child
    /// text) reads the dirty value with [`Self::dirty_value`] instead.
    fn control_value(&self, attribute: &str) -> String {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.dirty_value(id) {
            Some(dirty) => dirty.to_owned(),
            None => doc.attribute_str(id, attribute).unwrap_or("").to_owned(),
        }
    }

    /// The dirty value alone, for a control whose default is not an attribute.
    fn dirty_value(&self) -> Option<String> {
        let (doc, id) = self.anchor();
        doc.borrow().dirty_value(id).map(str::to_owned)
    }

    /// Assigning to a control's `value` always stores the dirty value and
    /// raises the dirty value flag; the content attribute is left untouched,
    /// so `getAttribute("value")` keeps reporting the default.
    fn set_control_value(&self, value: &str) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_dirty_value(id, value.to_owned());
    }

    /// Writes a reflected boolean attribute: `true` adds it with the empty-string
    /// value, `false` removes it. Mirrors `toggleAttribute` with an explicit
    /// force and, like the other reflected setters, ignores a mutation error.
    fn set_reflect_bool(&self, name: &str, value: bool) {
        let (doc, id) = self.anchor();
        let qname = QualName::new(atoms::EMPTY, Atom::new(name));
        let mut doc = doc.borrow_mut();
        if value {
            let _ = doc.mutate(Mutation::SetAttribute {
                element: id,
                name: qname,
                value: String::new(),
            });
        } else {
            let _ = doc.mutate(Mutation::RemoveAttribute {
                element: id,
                name: qname,
            });
        }
    }

    /// Detaches the anchored node from its parent (`ChildNode.remove()`). A node
    /// with no parent is a no-op; the node stays alive as long as script holds a
    /// reference to its wrapper.
    fn remove_from_parent(&self) {
        let (doc, id) = self.anchor();
        let _ = doc.borrow_mut().mutate(Mutation::Remove { child: id });
    }

    /// The anchored element's `class` attribute parsed as a DOM ordered set:
    /// ASCII-whitespace-split, empties dropped, first occurrence of each token
    /// kept. Backs the `DOMTokenList` read and mutation ops.
    fn class_tokens(&self) -> Vec<String> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let mut tokens: Vec<String> = Vec::new();
        if let Some(value) = doc.attribute_str(id, "class") {
            for token in value.split_ascii_whitespace() {
                if !tokens.iter().any(|existing| existing == token) {
                    tokens.push(token.to_owned());
                }
            }
        }
        tokens
    }

    /// Serialises an ordered set back into the `class` attribute (space-joined).
    fn set_class_tokens(&self, tokens: &[String]) {
        self.reflect_attribute("class", &tokens.join(" "));
    }

    /// A `CSSStyleDeclaration` handle for the anchored element's inline `style`.
    fn style_handle(&self) -> Result<Handle<CSSStyleDeclaration>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_style(owner)))
    }

    /// The anchored element's `style` attribute parsed into its declaration
    /// block: an ordered list of `(name, value, important)`, property names
    /// lowercased, later declarations of a property replacing earlier ones.
    fn style_declarations(&self) -> Vec<StyleDeclaration> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let mut declarations: Vec<StyleDeclaration> = Vec::new();
        let Some(text) = doc.attribute_str(id, "style") else {
            return declarations;
        };
        for chunk in text.split(';') {
            let Some((name, value)) = chunk.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let (value, important) = split_important(value.trim());
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            declarations.retain(|declaration| declaration.name != name);
            declarations.push(StyleDeclaration {
                name,
                value: value.to_owned(),
                important,
            });
        }
        declarations
    }

    /// Serialises a declaration block back into the `style` attribute.
    fn set_style_declarations(&self, declarations: &[StyleDeclaration]) {
        self.reflect_attribute("style", &serialize_declarations(declarations));
    }

    /// Applies a navigation step to the anchored node and wraps the result as a
    /// freshly read `Node` handle (never stale), or `None` at the tree edge.
    fn navigate(&self, step: impl Fn(&Document, NodeId) -> NodeId) -> Option<Handle<Node>> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let target = step(&doc, id);
        target
            .is_some()
            .then(|| Handle::from_bits(encode_node(doc.node_ref(target))))
    }
}

impl NodeOps for HostObject {
    fn child_nodes(&self) -> Result<Handle<NodeList>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_collection(owner)))
    }

    fn first_child(&self) -> Result<Option<Handle<Node>>, JsError> {
        Ok(self.navigate(|doc, id| doc.first_child(id)))
    }

    fn last_child(&self) -> Result<Option<Handle<Node>>, JsError> {
        Ok(self.navigate(|doc, id| doc.last_child(id)))
    }

    fn next_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        Ok(self.navigate(|doc, id| doc.next_sibling(id)))
    }

    fn node_name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(node_name(&doc.borrow(), id))
    }

    fn node_type(&self) -> u16 {
        // The DOM `nodeType` codes (https://dom.spec.whatwg.org/#dom-node-nodetype).
        // The `NodeOps` consts cover the kinds this slice exercises; the rest
        // use the fixed spec values.
        let (doc, id) = self.anchor();
        match doc.borrow().kind(id) {
            NodeKind::Element => <Self as NodeOps>::ELEMENT_NODE,
            NodeKind::Text => <Self as NodeOps>::TEXT_NODE,
            NodeKind::Comment => <Self as NodeOps>::COMMENT_NODE,
            NodeKind::Document => <Self as NodeOps>::DOCUMENT_NODE,
            NodeKind::CData => 4,
            NodeKind::ProcessingInstruction => 7,
            NodeKind::DocType => 10,
            NodeKind::Fragment | NodeKind::ShadowRoot => 11,
        }
    }

    fn owner_document(&self) -> Result<Option<Handle<super::Document>>, JsError> {
        // The Document node's `ownerDocument` is null; every other node in this
        // world belongs to the one document the host is anchored in. `Document`
        // is qualified because `tenta_dom::Document` shadows the binding marker.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let document_node = doc.document_node();
        Ok((id != document_node)
            .then(|| Handle::from_bits(encode_node(doc.node_ref(document_node)))))
    }

    fn is_connected(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().is_connected(id)
    }

    fn has_child_nodes(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().first_child(id).is_some()
    }

    fn contains(&self, other: Option<Handle<Node>>) -> Result<bool, JsError> {
        // `node.contains(other)` is true when `other` is an inclusive descendant
        // of `node`. `ancestors` excludes the node itself, so the identity case
        // is checked separately. A null argument is not contained. A swept
        // `other` is not live, so `ancestors` yields nothing and the answer is
        // false; slot reuse within the 16-bit generation window is the
        // handle-wide staleness limitation shared with every node-handle op
        // (the `pack` TODO), not specific to this method.
        let Some(other) = other else {
            return Ok(false);
        };
        let decoded = decode(other.bits());
        debug_assert_eq!(decoded.tag, TAG_NODE, "contains requires a Node handle");
        let other_id = decoded.node_id;
        let (doc, self_id) = self.anchor();
        let doc = doc.borrow();
        Ok(other_id == self_id || doc.ancestors(other_id).any(|ancestor| ancestor == self_id))
    }

    fn parent_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let parent = doc.parent(id);
        Ok((parent.is_some() && doc.kind(parent) == NodeKind::Element)
            .then(|| Handle::from_bits(encode_node(doc.node_ref(parent)))))
    }

    fn parent_node(&self) -> Result<Option<Handle<Node>>, JsError> {
        Ok(self.navigate(|doc, id| doc.parent(id)))
    }

    fn previous_sibling(&self) -> Result<Option<Handle<Node>>, JsError> {
        Ok(self.navigate(|doc, id| doc.previous_sibling(id)))
    }

    fn text_content(&self) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let kind = doc.kind(id);
        Ok(match kind {
            NodeKind::Document | NodeKind::DocType => None,
            NodeKind::Text | NodeKind::CData | NodeKind::Comment => {
                Some(doc.text_data(id).unwrap_or("").to_owned())
            }
            NodeKind::Element | NodeKind::Fragment | NodeKind::ShadowRoot => {
                let mut result = String::new();
                for child_id in doc.descendants(id) {
                    if matches!(doc.kind(child_id), NodeKind::Text | NodeKind::CData)
                        && let Some(data) = doc.text_data(child_id)
                    {
                        result.push_str(data);
                    }
                }
                Some(result)
            }
            NodeKind::ProcessingInstruction => Some(String::new()),
        })
    }

    fn set_text_content(&self, value: Option<&str>) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let kind = doc.kind(id);

        match kind {
            NodeKind::Text | NodeKind::CData | NodeKind::Comment => {
                let text = value.unwrap_or("");
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                doc.mutate(Mutation::SetText {
                    node: id,
                    text: text.to_owned(),
                })
                .map_err(|e| mutation_error(&e))
            }
            NodeKind::Element | NodeKind::Fragment | NodeKind::ShadowRoot => {
                // Remove all children
                let child_ids: Vec<NodeId> = doc.children(id).collect();
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                for child_id in child_ids {
                    doc.mutate(Mutation::Remove { child: child_id })
                        .map_err(|e| mutation_error(&e))?;
                }
                // If value is non-empty, append a text node
                if let Some(text) = value
                    && !text.is_empty()
                {
                    let text_node_id = doc.create_text(text);
                    doc.mutate(Mutation::Append {
                        parent: id,
                        child: text_node_id,
                    })
                    .map_err(|e| mutation_error(&e))?;
                }
                Ok(())
            }
            NodeKind::Document | NodeKind::DocType | NodeKind::ProcessingInstruction => {
                // No-op for Document and similar
                Ok(())
            }
        }
    }

    fn node_value(&self) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.kind(id) {
            NodeKind::Text | NodeKind::CData | NodeKind::Comment => {
                Ok(Some(doc.text_data(id).unwrap_or("").to_owned()))
            }
            _ => Ok(None),
        }
    }

    fn set_node_value(&self, value: Option<&str>) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.kind(id) {
            NodeKind::Text | NodeKind::CData | NodeKind::Comment => {
                let text = value.unwrap_or("");
                drop(doc);
                let mut doc = self.anchor().0.borrow_mut();
                doc.mutate(Mutation::SetText {
                    node: id,
                    text: text.to_owned(),
                })
                .map_err(|e| mutation_error(&e))
            }
            _ => {
                // No-op for other node types
                Ok(())
            }
        }
    }

    fn append_child(&self, node: Handle<Node>) -> Result<Handle<Node>, JsError> {
        let (doc, self_id) = self.anchor();
        let decoded = decode(node.bits());
        let child_id = decoded.node_id;

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::Append {
            parent: self_id,
            child: child_id,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        // Return the argument node's Handle (identity preserved)
        Ok(node)
    }

    fn insert_before(
        &self,
        node: Handle<Node>,
        child: Option<Handle<Node>>,
    ) -> Result<Handle<Node>, JsError> {
        let (doc, self_id) = self.anchor();
        let decoded = decode(node.bits());
        let node_id = decoded.node_id;

        let before = child
            .map(|h| decode(h.bits()).node_id)
            .unwrap_or(NodeId::NONE);

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::InsertBefore {
            parent: self_id,
            child: node_id,
            before,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        Ok(node)
    }

    fn remove_child(&self, child: Handle<Node>) -> Result<Handle<Node>, JsError> {
        let (doc, _) = self.anchor();
        let decoded = decode(child.bits());
        let child_id = decoded.node_id;

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::Remove { child: child_id })
            .map_err(|e| mutation_error(&e))?;
        drop(doc);

        Ok(child)
    }

    fn replace_child(
        &self,
        node: Handle<Node>,
        child: Handle<Node>,
    ) -> Result<Handle<Node>, JsError> {
        let (doc, self_id) = self.anchor();
        let new_decoded = decode(node.bits());
        let old_decoded = decode(child.bits());

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::ReplaceChild {
            parent: self_id,
            new_child: new_decoded.node_id,
            old_child: old_decoded.node_id,
        })
        .map_err(|e| mutation_error(&e))?;
        drop(doc);

        // Return the old child's Handle (per spec)
        Ok(child)
    }

    fn clone_node(&self, deep: Option<bool>) -> Result<Handle<Node>, JsError> {
        let (doc, self_id) = self.anchor();
        let deep_flag = deep.unwrap_or(false);

        let new_id = {
            let mut doc = doc.borrow_mut();
            doc.clone_node(self_id, deep_flag)
                .map_err(|e| mutation_error(&e))?
        };

        let doc = doc.borrow();
        Ok(Handle::from_bits(encode_node(doc.node_ref(new_id))))
    }
}

impl ElementOps for HostObject {
    fn remove(&self) {
        self.remove_from_parent();
    }

    fn child_element_count(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow().element_children(id).count() as u32
    }

    fn children(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_html_collection(owner)))
    }

    fn class_list(&self) -> Result<Handle<DOMTokenList>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_token_list(owner)))
    }

    fn closest(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.closest(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn first_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .next()
            .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
    }

    fn get_elements_by_class_name(
        &self,
        class_names: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        // For Element, scope the query to the element's subtree
        let tokens: Vec<&str> = class_names.split_whitespace().collect();
        let doc_ref = doc.borrow();
        let descendants: Vec<NodeRef> = doc_ref
            .descendants(id)
            .filter(|node_id| {
                // All tokens must be present on the element; an empty class
                // list (an empty or whitespace-only argument) matches nothing,
                // per the DOM `getElementsByClassName` definition.
                !tokens.is_empty()
                    && tokens
                        .iter()
                        .all(|token| doc_ref.has_class(*node_id, token))
            })
            .map(|node_id| doc_ref.node_ref(node_id))
            .collect();
        drop(doc_ref);

        // Insert snapshot and return wrapped handle
        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants);
        Ok(Handle::from_bits(encode_snapshot(
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name(
        &self,
        qualified_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        // For elements: if tag is "*", match all elements; otherwise match by tag name
        let descendants: Vec<NodeRef> = if qualified_name == "*" {
            doc_ref
                .descendants(id)
                .filter(|node_id| doc_ref.is_element(*node_id))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        } else {
            // Get the atom for the tag name; if it doesn't exist, return empty collection
            use tenta_core::atom::Atom;
            let tag_atom = match Atom::get(&qualified_name.to_ascii_lowercase()) {
                Some(atom) => atom,
                None => {
                    drop(doc_ref);
                    let ctx = self.context();
                    let key = ctx.snapshots.borrow_mut().insert(vec![]);
                    return Ok(Handle::from_bits(encode_snapshot(
                        TAG_STATIC_HTML_COLLECTION,
                        key,
                    )));
                }
            };

            doc_ref
                .descendants(id)
                .filter(|node_id| doc_ref.tag(*node_id) == Some(tag_atom))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        };
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants);
        Ok(Handle::from_bits(encode_snapshot(
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn id(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().element_id(id).unwrap_or("").to_owned())
    }

    fn set_id(&self, value: &str) {
        // A plain reflected setter: writing `id` sets the `id` attribute. Fast-API
        // eligible, so it returns void and cannot throw (any parse the attribute
        // needs is infallible).
        self.reflect_attribute("id", value);
    }

    fn class_name(&self) -> Result<String, JsError> {
        // `className` reflects the `class` attribute value verbatim (whitespace
        // and duplicates preserved), or "" when the element has no class
        // attribute. The DOM keeps the raw attribute string and the parsed class
        // index in lockstep, so the raw string is authoritative.
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attribute_str(id, "class")
            .unwrap_or("")
            .to_owned())
    }

    fn set_class_name(&self, value: &str) {
        self.reflect_attribute("class", value);
    }

    fn local_name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .tag(id)
            .map(|local| local.as_str().to_owned())
            .unwrap_or_default())
    }

    fn namespace_uri(&self) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .namespace(id)
            .map(|ns| ns.as_str().to_owned())
            .filter(|uri| !uri.is_empty()))
    }

    fn last_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .last()
            .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
    }

    fn matches(&self, selectors: &str) -> bool {
        // Spec: `matches` throws a `SyntaxError` on an invalid selector, but the
        // generated fast-API signature is infallible (`-> bool`) — the generator
        // has no way to mark a fast-API member fallible. Until it does, an
        // invalid selector reports `false` rather than throwing. Valid selectors
        // match per spec. (Follow-up: let `tenta-idl` force a `[Throws]` member
        // off the fast path so it returns `Result`.)
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        doc.matches(id, selectors).unwrap_or(false)
    }

    fn query_selector(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.query_selector(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn query_selector_all(&self, selectors: &str) -> Result<Handle<NodeList>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        // Query against this element as the root scope
        match doc_ref.query_selector_all(id, selectors) {
            Ok(result_ids) => {
                let refs: Vec<NodeRef> = result_ids
                    .iter()
                    .map(|node_id| doc_ref.node_ref(*node_id))
                    .collect();
                drop(doc_ref);

                let ctx = self.context();
                let key = ctx.snapshots.borrow_mut().insert(refs);
                Ok(Handle::from_bits(encode_snapshot(
                    TAG_STATIC_NODE_LIST,
                    key,
                )))
            }
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn tag_name(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(element_tag_name(&doc.borrow(), id))
    }

    fn set_attribute(&self, name: &str, value: &str) -> Result<(), JsError> {
        validate_attribute_name(name)?;

        let (doc, id) = self.anchor();
        let lower_name = name.to_ascii_lowercase();
        let attr_name = Atom::new(&lower_name);
        let qname = QualName::new(atoms::EMPTY, attr_name);

        let mut doc = doc.borrow_mut();
        doc.mutate(Mutation::SetAttribute {
            element: id,
            name: qname,
            value: value.to_owned(),
        })
        .map_err(|e| mutation_error(&e))
    }

    fn get_attribute(&self, name: &str) -> Result<Option<String>, JsError> {
        let (doc, id) = self.anchor();
        let lower_name = name.to_ascii_lowercase();
        Ok(doc
            .borrow()
            .attribute_str(id, &lower_name)
            .map(str::to_owned))
    }

    fn remove_attribute(&self, name: &str) {
        let (doc, id) = self.anchor();
        let lower_name = name.to_ascii_lowercase();
        let attr_name = Atom::new(&lower_name);
        let qname = QualName::new(atoms::EMPTY, attr_name);

        let mut doc = doc.borrow_mut();
        // Removing a non-existent attribute is a no-op, so we ignore errors
        let _ = doc.mutate(Mutation::RemoveAttribute {
            element: id,
            name: qname,
        });
    }

    fn has_attribute(&self, name: &str) -> bool {
        let (doc, id) = self.anchor();
        let lower_name = name.to_ascii_lowercase();
        // Check if the attribute exists
        doc.borrow().attribute_str(id, &lower_name).is_some()
    }

    fn has_attributes(&self) -> bool {
        let (doc, id) = self.anchor();
        doc.borrow().attributes(id).next().is_some()
    }

    fn toggle_attribute(&self, name: &str, force: Option<bool>) -> Result<bool, JsError> {
        validate_attribute_name(name)?;
        let (doc, id) = self.anchor();
        let lower_name = name.to_ascii_lowercase();
        let present = doc.borrow().attribute_str(id, &lower_name).is_some();
        let qname = QualName::new(atoms::EMPTY, Atom::new(&lower_name));
        let mut doc = doc.borrow_mut();

        // `force` absent toggles; present forces on; absent forces off. Adding an
        // attribute with `toggleAttribute` gives it the empty-string value.
        let add = match force {
            Some(force) => force,
            None => !present,
        };
        if add && !present {
            doc.mutate(Mutation::SetAttribute {
                element: id,
                name: qname,
                value: String::new(),
            })
            .map_err(|e| mutation_error(&e))?;
        } else if !add && present {
            let _ = doc.mutate(Mutation::RemoveAttribute {
                element: id,
                name: qname,
            });
        }
        Ok(add)
    }

    fn get_attribute_names(&self) -> Result<Vec<String>, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attributes(id)
            .map(|(name, _)| qualified_attr_name(&name))
            .collect())
    }

    fn inner_html(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().inner_html(id))
    }

    fn set_inner_html(&self, value: &str) {
        // Note: This is a fast-API eligible operation, so it must return void.
        // Errors are silently ignored to match the fast-API constraint.
        // In practice, the HTML parser handles malformed input leniently per
        // the HTML5 spec, so errors should be rare. If parsing the fragment
        // fails, no mutation is applied; if a later tree mutation fails, the
        // element may be left partially emptied (its old children are removed
        // before the parsed fragment is reparented in).
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        let _ = doc.set_inner_html(id, value);
    }

    fn outer_html(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().outer_html(id))
    }

    fn set_outer_html(&self, value: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.set_outer_html(id, value)
            .map_err(|e| mutation_error(&e))
    }

    fn insert_adjacent_html(&self, position: &str, text: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.insert_adjacent_html(id, position, text)
            .map_err(|e| mutation_error(&e))
    }
}

impl DocumentOps for HostObject {
    fn child_element_count(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow().element_children(id).count() as u32
    }

    fn children(&self) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, id) = self.anchor();
        let owner = doc.borrow().node_ref(id);
        Ok(Handle::from_bits(encode_html_collection(owner)))
    }

    fn create_comment(&self, data: &str) -> Result<Handle<Comment>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_comment(data);
        Ok(Handle::from_bits(encode_node(doc.node_ref(id))))
    }

    fn create_event(&self, interface: &str) -> Result<Handle<Event>, JsError> {
        // Legacy `createEvent` interface aliases (https://dom.spec.whatwg.org/#dom-document-createevent).
        // The Event/CustomEvent families arrived in 3a/3c; UIEvent/MouseEvent
        // in 3d. Each returns a record whose `kind` picks the wrapper prototype.
        let kind = match interface.to_ascii_lowercase().as_str() {
            "event" | "events" | "htmlevents" => EventKind::Plain,
            "customevent" => EventKind::Custom { detail: None },
            "uievent" | "uievents" => EventKind::Ui { detail: 0 },
            "mouseevent" | "mouseevents" => EventKind::Mouse {
                detail: 0,
                screen_x: 0,
                screen_y: 0,
                client_x: 0,
                client_y: 0,
                ctrl: false,
                shift: false,
                alt: false,
                meta: false,
                button: 0,
                buttons: 0,
                related_target: None,
            },
            _ => {
                return Err(JsError::dom_exception(
                    "NotSupportedError",
                    "createEvent does not support this interface yet",
                ));
            }
        };
        let ctx = self.context();
        let record = EventRecord::new(String::new(), kind, ctx.time_stamp());
        let key = ctx.events.borrow_mut().insert(record);
        Ok(Handle::from_bits(encode_event(key)))
    }

    fn create_element(&self, local_name: &str) -> Result<Handle<Element>, JsError> {
        // Validate the name: non-empty and starts with ASCII letter
        if local_name.is_empty() {
            return Err(JsError::dom_exception(
                "InvalidCharacterError",
                "Element name must not be empty",
            ));
        }
        if let Some(first_char) = local_name.chars().next()
            && !first_char.is_ascii_alphabetic()
        {
            return Err(JsError::dom_exception(
                "InvalidCharacterError",
                "Element name must start with a letter",
            ));
        }
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        // HTML element names are lowercased.
        let atom = tenta_core::atom::Atom::new(&local_name.to_ascii_lowercase());
        // A `<template>` owns a separate contents fragment; the create path must
        // request it so a script-created template matches a parsed one and its
        // `.content` resolves.
        let flags = ElementFlags {
            template: atom == atoms::TEMPLATE,
            ..ElementFlags::default()
        };
        let id = doc
            .create_element(QualName::new(atoms::NS_HTML, atom), Vec::new(), flags)
            .map_err(|e| mutation_error(&e))?;
        Ok(Handle::from_bits(encode_node(doc.node_ref(id))))
    }

    fn create_document_fragment(&self) -> Result<Handle<DocumentFragment>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_fragment();
        Ok(Handle::from_bits(encode_node(doc.node_ref(id))))
    }

    fn create_element_ns(
        &self,
        namespace: Option<&str>,
        qualified_name: &str,
    ) -> Result<Handle<Element>, JsError> {
        let (ns, prefix, local) = validate_and_extract(namespace, qualified_name)?;
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let name = QualName {
            ns,
            prefix,
            local: Atom::new(local),
        };
        let flags = ElementFlags {
            template: ns == atoms::NS_HTML && local == atoms::TEMPLATE.as_str(),
            ..ElementFlags::default()
        };
        let id = doc
            .create_element(name, Vec::new(), flags)
            .map_err(|e| mutation_error(&e))?;
        Ok(Handle::from_bits(encode_node(doc.node_ref(id))))
    }

    fn create_text_node(&self, data: &str) -> Result<Handle<Text>, JsError> {
        let (doc, _) = self.anchor();
        let mut doc = doc.borrow_mut();
        let id = doc.create_text(data);
        Ok(Handle::from_bits(encode_node(doc.node_ref(id))))
    }

    fn document_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .document_element()
            .map(|id| Handle::from_bits(encode_node(doc.node_ref(id)))))
    }

    fn body(&self) -> Result<Option<Handle<HTMLElement>>, JsError> {
        // Only an HTML-namespace body/frameset is `document.body`, so the
        // `HTMLElement` handle is always backed by an HTML element (the wrapper
        // interface is picked by namespace at wrap time).
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .body()
            .filter(|id| doc.namespace(*id) == Some(atoms::NS_HTML))
            .map(|id| Handle::from_bits(encode_node(doc.node_ref(id)))))
    }

    fn head(&self) -> Result<Option<Handle<HTMLElement>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .head()
            .filter(|id| doc.namespace(*id) == Some(atoms::NS_HTML))
            .map(|id| Handle::from_bits(encode_node(doc.node_ref(id)))))
    }

    fn ready_state(&self) -> Result<String, JsError> {
        // No incremental load/parse model yet: a document handed to the engine is
        // already fully built, so it reports "complete" (the settle machine that
        // drives "loading"/"interactive" arrives with the page pipeline).
        Ok("complete".to_owned())
    }

    fn active_element(&self) -> Result<Option<Handle<Element>>, JsError> {
        // No focus model yet: per the spec the fallback active element is the
        // body (or the document element when there is no body).
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        let active = doc.body().or_else(|| doc.document_element());
        Ok(active.map(|id| Handle::from_bits(encode_node(doc.node_ref(id)))))
    }

    fn first_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .next()
            .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
    }

    fn get_element_by_id(&self, element_id: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, _) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .get_element_by_id(element_id)
            .map(|id| Handle::from_bits(encode_node(doc.node_ref(id)))))
    }

    fn get_elements_by_class_name(
        &self,
        class_names: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, _) = self.anchor();
        let tokens: Vec<&str> = class_names.split_whitespace().collect();
        let doc_ref = doc.borrow();
        let doc_node = doc_ref.document_node();
        let descendants: Vec<NodeRef> = doc_ref
            .descendants(doc_node)
            .filter(|node_id| {
                // All tokens must be present on the element; an empty class
                // list (an empty or whitespace-only argument) matches nothing,
                // per the DOM `getElementsByClassName` definition.
                !tokens.is_empty()
                    && tokens
                        .iter()
                        .all(|token| doc_ref.has_class(*node_id, token))
            })
            .map(|node_id| doc_ref.node_ref(node_id))
            .collect();
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants);
        Ok(Handle::from_bits(encode_snapshot(
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn get_elements_by_tag_name(
        &self,
        qualified_name: &str,
    ) -> Result<Handle<HTMLCollection>, JsError> {
        let (doc, _) = self.anchor();
        let doc_ref = doc.borrow();
        let doc_node = doc_ref.document_node();

        let descendants: Vec<NodeRef> = if qualified_name == "*" {
            doc_ref
                .descendants(doc_node)
                .filter(|node_id| doc_ref.is_element(*node_id))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        } else {
            use tenta_core::atom::Atom;
            let tag_atom = match Atom::get(&qualified_name.to_ascii_lowercase()) {
                Some(atom) => atom,
                None => {
                    drop(doc_ref);
                    let ctx = self.context();
                    let key = ctx.snapshots.borrow_mut().insert(vec![]);
                    return Ok(Handle::from_bits(encode_snapshot(
                        TAG_STATIC_HTML_COLLECTION,
                        key,
                    )));
                }
            };

            doc_ref
                .descendants(doc_node)
                .filter(|node_id| doc_ref.tag(*node_id) == Some(tag_atom))
                .map(|node_id| doc_ref.node_ref(node_id))
                .collect()
        };
        drop(doc_ref);

        let ctx = self.context();
        let key = ctx.snapshots.borrow_mut().insert(descendants);
        Ok(Handle::from_bits(encode_snapshot(
            TAG_STATIC_HTML_COLLECTION,
            key,
        )))
    }

    fn last_element_child(&self) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        Ok(doc
            .element_children(id)
            .last()
            .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
    }

    fn query_selector(&self, selectors: &str) -> Result<Option<Handle<Element>>, JsError> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.query_selector(id, selectors) {
            Ok(Some(result_id)) => Ok(Some(Handle::from_bits(encode_node(
                doc.node_ref(result_id),
            )))),
            Ok(None) => Ok(None),
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }

    fn query_selector_all(&self, selectors: &str) -> Result<Handle<NodeList>, JsError> {
        let (doc, id) = self.anchor();
        let doc_ref = doc.borrow();

        match doc_ref.query_selector_all(id, selectors) {
            Ok(result_ids) => {
                let refs: Vec<NodeRef> = result_ids
                    .iter()
                    .map(|node_id| doc_ref.node_ref(*node_id))
                    .collect();
                drop(doc_ref);

                let ctx = self.context();
                let key = ctx.snapshots.borrow_mut().insert(refs);
                Ok(Handle::from_bits(encode_snapshot(
                    TAG_STATIC_NODE_LIST,
                    key,
                )))
            }
            Err(e) => Err(JsError::syntax_error(e.to_string())),
        }
    }
}

impl NodeListOps for HostObject {
    fn item(&self, index: u32) -> Result<Option<Handle<Node>>, JsError> {
        match self {
            HostObject::StaticNodeList { ctx, key } => {
                // Static snapshot: look up in slab
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                if let Some(node_ref) = refs.get(index as usize) {
                    let doc = ctx.doc.borrow();
                    if let Ok(id) = doc.resolve(*node_ref) {
                        return Ok(Some(Handle::from_bits(encode_node(doc.node_ref(id)))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .children(id)
                    .nth(index as usize)
                    .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
            }
        }
    }

    fn length(&self) -> u32 {
        match self {
            HostObject::StaticNodeList { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                snapshots.get(*key).map(|r| r.len() as u32).unwrap_or(0)
            }
            _ => {
                let (doc, id) = self.anchor();
                doc.borrow().children(id).count() as u32
            }
        }
    }
}

impl CharacterDataOps for HostObject {
    fn remove(&self) {
        self.remove_from_parent();
    }

    fn data(&self) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        Ok(doc.borrow().text_data(id).unwrap_or("").to_owned())
    }

    fn set_data(&self, value: &str) {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        // SetText should not fail for CharacterData nodes (Text, Comment, CDATA).
        // If it does, we ignore the error since the setter is infallible.
        let _ = doc.mutate(Mutation::SetText {
            node: id,
            text: value.to_owned(),
        });
    }

    fn length(&self) -> u32 {
        let (doc, id) = self.anchor();
        doc.borrow()
            .text_data(id)
            .map(|s| s.encode_utf16().count() as u32)
            .unwrap_or(0)
    }

    fn substring_data(&self, offset: u32, count: u32) -> Result<String, JsError> {
        let (doc, id) = self.anchor();
        doc.borrow()
            .substring_data(id, offset, count)
            .map_err(|e| mutation_error(&e))
    }

    fn append_data(&self, data: &str) {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        // appendData cannot fail on a character-data node; the void signature
        // has no way to report an error regardless.
        let _ = doc.append_data(id, data);
    }

    fn insert_data(&self, offset: u32, data: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, 0, data)
            .map_err(|e| mutation_error(&e))
    }

    fn delete_data(&self, offset: u32, count: u32) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, count, "")
            .map_err(|e| mutation_error(&e))
    }

    fn replace_data(&self, offset: u32, count: u32, data: &str) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        let mut doc = doc.borrow_mut();
        doc.replace_data(id, offset, count, data)
            .map_err(|e| mutation_error(&e))
    }
}

impl TextOps for HostObject {}

impl CommentOps for HostObject {}

impl HTMLCollectionOps for HostObject {
    fn item(&self, index: u32) -> Result<Option<Handle<Element>>, JsError> {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                // Static snapshot: look up in slab
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                if let Some(node_ref) = refs.get(index as usize) {
                    let doc = ctx.doc.borrow();
                    if let Ok(id) = doc.resolve(*node_ref) {
                        return Ok(Some(Handle::from_bits(encode_node(doc.node_ref(id)))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .element_children(id)
                    .nth(index as usize)
                    .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
            }
        }
    }

    fn length(&self) -> u32 {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                let snapshots = ctx.snapshots.borrow();
                snapshots.get(*key).map(|r| r.len() as u32).unwrap_or(0)
            }
            _ => {
                let (doc, id) = self.anchor();
                doc.borrow().element_children(id).count() as u32
            }
        }
    }

    fn named_item(&self, name: &str) -> Result<Option<Handle<Element>>, JsError> {
        match self {
            HostObject::StaticHtmlCollection { ctx, key } => {
                // Static snapshot: look up by id in stored nodes
                let snapshots = ctx.snapshots.borrow();
                let refs = match snapshots.get(*key) {
                    Some(r) => r,
                    None => return Ok(None),
                };
                let doc = ctx.doc.borrow();
                for node_ref in refs {
                    if let Ok(id) = doc.resolve(*node_ref)
                        && doc
                            .element_id(id)
                            .map(|id_attr| id_attr == name)
                            .unwrap_or(false)
                    {
                        return Ok(Some(Handle::from_bits(encode_node(doc.node_ref(id)))));
                    }
                }
                Ok(None)
            }
            _ => {
                // Live collection: traverse from anchor
                let (doc, id) = self.anchor();
                let doc = doc.borrow();
                Ok(doc
                    .element_children(id)
                    .find(|child_id| {
                        doc.element_id(*child_id)
                            .map(|id_attr| id_attr == name)
                            .unwrap_or(false)
                    })
                    .map(|child| Handle::from_bits(encode_node(doc.node_ref(child)))))
            }
        }
    }
}

impl EventTargetOps for HostObject {
    fn add_event_listener(
        &self,
        r#type: &str,
        callback: Option<EventListener>,
        options: Option<AddEventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        let Some(callback) = callback else {
            return Ok(());
        };
        let (capture, once, passive) = match options {
            None => (false, false, false),
            Some(AddEventListenerOptionsOrBoolean::Boolean(capture)) => (capture, false, false),
            Some(AddEventListenerOptionsOrBoolean::AddEventListenerOptions(o)) => {
                (o.capture, o.once, o.passive.unwrap_or(false))
            }
        };
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let cb = CallbackRef(callback.bits());
        let added = self.context().listeners.borrow_mut().add_at(
            target,
            r#type.to_owned(),
            cb,
            capture,
            once,
            passive,
        );
        // The host owns the callback's stored reference: it keeps that reference
        // when the listener is added, and releases it when the entry is a
        // duplicate (the DOM "add an event listener" no-op).
        if added.is_none() {
            self.release_stored(cb.0);
        }
        Ok(())
    }

    fn remove_event_listener(
        &self,
        r#type: &str,
        callback: Option<EventListener>,
        options: Option<EventListenerOptionsOrBoolean>,
    ) -> Result<(), JsError> {
        let Some(callback) = callback else {
            return Ok(());
        };
        let capture = match options {
            None => false,
            Some(EventListenerOptionsOrBoolean::Boolean(capture)) => capture,
            Some(EventListenerOptionsOrBoolean::EventListenerOptions(o)) => o.capture,
        };
        let target = self
            .target_key()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        let cb = CallbackRef(callback.bits());
        let removed = self
            .context()
            .listeners
            .borrow_mut()
            .remove_at(target, r#type, cb, capture);
        // Release the argument's stored reference, plus the removed listener's.
        self.release_stored(cb.0);
        if let Some(removed) = removed {
            self.release_stored(removed.0);
        }
        Ok(())
    }

    fn dispatch_event(&self, event: Handle<Event>) -> Result<bool, JsError> {
        let decoded = decode(event.bits());
        if decoded.tag != TAG_EVENT {
            return Err(JsError::type_error("dispatchEvent requires an Event"));
        }
        let key = decoded.event_key();
        let target = self
            .dispatch_target()
            .ok_or_else(|| JsError::type_error("the receiver is not an event target"))?;
        dispatch_at(self.context(), target, key, false)
    }
}

impl EventOps for HostObject {
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.with_event(String::new(), |r| r.type_.clone()))
    }

    fn target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| r.target.map(Handle::from_bits)))
    }

    fn current_target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| r.current_target.map(Handle::from_bits)))
    }

    fn event_phase(&self) -> u16 {
        self.with_event(0, |r| r.phase)
    }

    fn stop_propagation(&self) {
        self.with_event_mut(|r| r.flags.stop_propagation = true);
    }

    fn stop_immediate_propagation(&self) {
        self.with_event_mut(|r| {
            r.flags.stop_propagation = true;
            r.flags.stop_immediate = true;
        });
    }

    fn bubbles(&self) -> bool {
        self.with_event(false, |r| r.bubbles)
    }

    fn cancelable(&self) -> bool {
        self.with_event(false, |r| r.cancelable)
    }

    fn prevent_default(&self) {
        self.with_event_mut(|r| {
            if r.cancelable && !r.flags.in_passive_listener {
                r.flags.canceled = true;
            }
        });
    }

    fn default_prevented(&self) -> bool {
        self.with_event(false, |r| r.flags.canceled)
    }

    fn composed(&self) -> bool {
        self.with_event(false, |r| r.composed)
    }

    fn is_trusted(&self) -> bool {
        self.with_event(false, |r| r.is_trusted)
    }

    fn time_stamp(&self) -> f64 {
        self.with_event(0.0, |r| r.time_stamp)
    }

    fn src_element(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        // Legacy alias of `target`.
        Ok(self.with_event(None, |r| r.target.map(Handle::from_bits)))
    }

    fn composed_path(&self) -> Result<Vec<Handle<EventTarget>>, JsError> {
        // The propagation path, populated during dispatch and cleared after, so
        // this returns an empty array when the event is not being dispatched.
        Ok(self.with_event(Vec::new(), |r| {
            r.path.iter().copied().map(Handle::from_bits).collect()
        }))
    }

    fn cancel_bubble(&self) -> bool {
        // Legacy alias of the "stop propagation" flag.
        self.with_event(false, |r| r.flags.stop_propagation)
    }

    fn set_cancel_bubble(&self, value: bool) {
        // Setting true stops propagation; setting false is a no-op (per the
        // legacy attribute's definition).
        if value {
            self.with_event_mut(|r| r.flags.stop_propagation = true);
        }
    }

    fn return_value(&self) -> bool {
        // Legacy inverse of `defaultPrevented`: false once the event is canceled.
        self.with_event(true, |r| !r.flags.canceled)
    }

    fn set_return_value(&self, value: bool) {
        // Setting false cancels the event (as `preventDefault`); true is a no-op.
        if !value {
            self.with_event_mut(|r| {
                if r.cancelable && !r.flags.in_passive_listener {
                    r.flags.canceled = true;
                }
            });
        }
    }

    fn init_event(
        &self,
        r#type: &str,
        bubbles: Option<bool>,
        cancelable: Option<bool>,
    ) -> Result<(), JsError> {
        self.with_event_mut(|r| {
            // Ignored while the event is being dispatched.
            if r.flags.dispatch {
                return;
            }
            reset_for_init(
                r,
                r#type,
                bubbles.unwrap_or(false),
                cancelable.unwrap_or(false),
            );
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Event and CustomEvent constructors (`new Event(...)`, `new CustomEvent(...)`)
// ---------------------------------------------------------------------------

/// Downcasts a [`Realm`] back to the [`SharedContext`] it actually is. The
/// wrapper world hands its own context as the realm, so this only fails if a
/// foreign world is installed — a wiring bug surfaced as a `TypeError` rather
/// than a panic.
///
/// Public because a backend that installs its own host members outside the
/// generated glue — the window's `addEventListener` and friends, which cannot
/// be generated while the window is not an IDL interface — needs the same
/// realm-to-context step the generated callbacks perform.
pub fn realm_context(realm: &Realm) -> Result<SharedContext, JsError> {
    realm
        .clone()
        .downcast::<HostContext>()
        .map_err(|_| JsError::type_error("event constructor: the realm is not a HostContext"))
}

/// Resets an event record to its post-`initEvent` state: initialized, every
/// propagation/cancel flag cleared, untrusted, no target, with the given type,
/// `bubbles` and `cancelable`. Shared by `initEvent` and `initCustomEvent`.
fn reset_for_init(record: &mut EventRecord, r#type: &str, bubbles: bool, cancelable: bool) {
    record.flags.initialized = true;
    record.flags.stop_propagation = false;
    record.flags.stop_immediate = false;
    record.flags.canceled = false;
    record.is_trusted = false;
    record.target = None;
    record.current_target = None;
    record.type_ = r#type.to_owned();
    record.bubbles = bubbles;
    record.cancelable = cancelable;
}

/// Builds an initialized, untrusted event record of `kind` and inserts it into
/// `ctx`'s slab, returning the host that stands for it. Shared by the `Event`
/// and `CustomEvent` constructors.
fn new_event(
    ctx: &SharedContext,
    r#type: &str,
    bubbles: bool,
    cancelable: bool,
    composed: bool,
    kind: EventKind,
) -> HostObject {
    let mut record = EventRecord::new(r#type.to_owned(), kind, ctx.time_stamp());
    record.bubbles = bubbles;
    record.cancelable = cancelable;
    record.composed = composed;
    // A script-constructed event is initialized but never trusted.
    record.flags.initialized = true;
    record.is_trusted = false;
    let key = ctx.events.borrow_mut().insert(record);
    HostObject::event(ctx.clone(), key)
}

impl EventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<EventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Plain,
        ))
    }
}

impl CustomEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<CustomEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // `detail` is an `any` the glue already interned into the value store
        // (+1 ref) during dictionary conversion; the record owns that reference
        // until it is reclaimed (the world's dead-event sweep releases it). An
        // absent member and an explicit `null` both read back as `null`.
        let detail = init.detail.map(|value| value.0);
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Custom { detail },
        ))
    }
}

impl CustomEventOps for HostObject {
    fn detail(&self) -> Result<AnyValue, JsError> {
        Ok(self.with_event(AnyValue::NULL, |r| match &r.kind {
            EventKind::Custom { detail: Some(bits) } => AnyValue(*bits),
            _ => AnyValue::NULL,
        }))
    }

    fn init_custom_event(
        &self,
        r#type: &str,
        bubbles: Option<bool>,
        cancelable: Option<bool>,
        detail: Option<AnyValue>,
    ) -> Result<(), JsError> {
        let HostObject::Event { ctx, key } = self else {
            return Ok(());
        };
        let new_detail = detail.map(|value| value.0);

        // Mutate the record under a short borrow, deciding which stale detail
        // (if any) to release afterwards. The release must happen outside the
        // slab borrow, since `release_stored` borrows the script host.
        let released = {
            let mut events = ctx.events.borrow_mut();
            match events.get_mut(*key) {
                // Ignored while dispatching, or if the record is gone: keep any
                // existing detail and drop the new argument's stored reference.
                Some(record) if record.flags.dispatch => new_detail,
                None => new_detail,
                Some(record) => {
                    reset_for_init(
                        record,
                        r#type,
                        bubbles.unwrap_or(false),
                        cancelable.unwrap_or(false),
                    );
                    let old = match record.kind {
                        EventKind::Custom { detail } => detail,
                        _ => None,
                    };
                    record.kind = EventKind::Custom { detail: new_detail };
                    old
                }
            }
        };
        if let Some(bits) = released {
            self.release_stored(bits);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UIEvent / MouseEvent (constructors, getters) and HTMLElement.click()
// ---------------------------------------------------------------------------

impl UIEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<UIEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Ui {
                detail: init.detail,
            },
        ))
    }
}

impl UIEventOps for HostObject {
    fn detail(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Ui { detail } | EventKind::Mouse { detail, .. } => detail,
            _ => 0,
        })
    }
}

impl MouseEventConstructor for HostObject {
    fn construct(
        realm: &Realm,
        r#type: &str,
        event_init_dict: Option<MouseEventInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = event_init_dict.unwrap_or_default();
        // `relatedTarget?` is a nullable `EventTarget`: the outer `Option`
        // reports whether the member was present, the inner whether the value
        // was a target or `null`. A node handle needs no value-store reference
        // (it is not an interned JS value), so its bits are stored directly.
        let related_target = init.related_target.flatten().map(|handle| handle.bits());
        Ok(new_event(
            &ctx,
            r#type,
            init.bubbles,
            init.cancelable,
            init.composed,
            EventKind::Mouse {
                detail: init.detail,
                screen_x: init.screen_x,
                screen_y: init.screen_y,
                client_x: init.client_x,
                client_y: init.client_y,
                ctrl: init.ctrl_key,
                shift: init.shift_key,
                alt: init.alt_key,
                meta: init.meta_key,
                button: init.button,
                buttons: init.buttons,
                related_target,
            },
        ))
    }
}

impl MouseEventOps for HostObject {
    fn screen_x(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { screen_x, .. } => screen_x,
            _ => 0,
        })
    }

    fn screen_y(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { screen_y, .. } => screen_y,
            _ => 0,
        })
    }

    fn client_x(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { client_x, .. } => client_x,
            _ => 0,
        })
    }

    fn client_y(&self) -> i32 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { client_y, .. } => client_y,
            _ => 0,
        })
    }

    fn ctrl_key(&self) -> bool {
        self.with_event(false, |r| match r.kind {
            EventKind::Mouse { ctrl, .. } => ctrl,
            _ => false,
        })
    }

    fn shift_key(&self) -> bool {
        self.with_event(false, |r| match r.kind {
            EventKind::Mouse { shift, .. } => shift,
            _ => false,
        })
    }

    fn alt_key(&self) -> bool {
        self.with_event(false, |r| match r.kind {
            EventKind::Mouse { alt, .. } => alt,
            _ => false,
        })
    }

    fn meta_key(&self) -> bool {
        self.with_event(false, |r| match r.kind {
            EventKind::Mouse { meta, .. } => meta,
            _ => false,
        })
    }

    fn button(&self) -> i16 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { button, .. } => button,
            _ => 0,
        })
    }

    fn buttons(&self) -> u16 {
        self.with_event(0, |r| match r.kind {
            EventKind::Mouse { buttons, .. } => buttons,
            _ => 0,
        })
    }

    fn related_target(&self) -> Result<Option<Handle<EventTarget>>, JsError> {
        Ok(self.with_event(None, |r| match r.kind {
            EventKind::Mouse { related_target, .. } => related_target.map(Handle::from_bits),
            _ => None,
        }))
    }
}

impl HTMLElementOps for HostObject {
    fn click(&self) -> Result<(), JsError> {
        let (doc, id) = self.anchor();
        // The DOM "click in progress" flag: a nested `click()` on the same
        // element during its own dispatch is a no-op, so a handler that calls
        // `click()` again cannot recurse.
        if !self.context().click_in_progress.borrow_mut().insert(id) {
            return Ok(());
        }
        let result = self.dispatch_synthetic_click(doc, id);
        self.context().click_in_progress.borrow_mut().remove(&id);
        result
    }

    fn style(&self) -> Result<Handle<CSSStyleDeclaration>, JsError> {
        self.style_handle()
    }
}

// `HTMLIFrameElement` is an interface shell for now: it adds no members of its
// own, so its `Ops` trait is empty and the host satisfies it through its
// `HTMLElementOps`/`ElementOps` impls. `SVGElement` carries `style` via the
// `ElementCSSInlineStyle` mixin.
impl HTMLIFrameElementOps for HostObject {}

// Phase 4d per-tag interface shells. The block/inline elements carry no members
// of their own; the form controls reflect only the safe attributes (`name`,
// `placeholder`, `disabled`, `htmlFor`). d-2c adds the URL-valued `href`/`src`/
// `action` through `reflect_url`; the dirty value model and the enumerated
// attributes stay deferred.
impl HTMLDivElementOps for HostObject {}
impl HTMLSpanElementOps for HostObject {}
impl HTMLParagraphElementOps for HostObject {}

impl HTMLAnchorElementOps for HostObject {
    fn href(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("href"))
    }
    fn set_href(&self, value: &str) {
        self.reflect_attribute("href", value);
    }
}

impl HTMLImageElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
}

impl HTMLLabelElementOps for HostObject {
    fn html_for(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("for"))
    }
    fn set_html_for(&self, value: &str) {
        self.reflect_attribute("for", value);
    }
}

impl HTMLFormElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn action(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("action"))
    }
    fn set_action(&self, value: &str) {
        self.reflect_attribute("action", value);
    }
}

impl HTMLInputElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn placeholder(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("placeholder"))
    }
    fn set_placeholder(&self, value: &str) {
        self.reflect_attribute("placeholder", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
    fn value(&self) -> Result<String, JsError> {
        Ok(self.control_value("value"))
    }
    fn set_value(&self, value: &str) {
        self.set_control_value(value);
    }
    fn checked(&self) -> bool {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        match doc.dirty_checkedness(id) {
            Some(checked) => checked,
            None => doc.attribute_str(id, "checked").is_some(),
        }
    }
    fn set_checked(&self, value: bool) {
        let (doc, id) = self.anchor();
        doc.borrow_mut().set_dirty_checkedness(id, value);
    }
}

impl HTMLButtonElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
}

impl HTMLSelectElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
}

impl HTMLTextAreaElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn placeholder(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("placeholder"))
    }
    fn set_placeholder(&self, value: &str) {
        self.reflect_attribute("placeholder", value);
    }
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
    fn value(&self) -> Result<String, JsError> {
        // A textarea's default is its raw value - the child text - rather than
        // an attribute, so fall back to `textContent` when nothing is dirty.
        match self.dirty_value() {
            Some(dirty) => Ok(dirty),
            None => Ok(self.text_content()?.unwrap_or_default()),
        }
    }
    fn set_value(&self, value: &str) {
        self.set_control_value(value);
    }
}

impl HTMLOptionElementOps for HostObject {
    fn disabled(&self) -> bool {
        self.reflect_bool("disabled")
    }
    fn set_disabled(&self, value: bool) {
        self.set_reflect_bool("disabled", value);
    }
}

// d-2c: `<script>` content-attribute reflection. `async`, `defer` and
// `noModule` are present-or-absent booleans here; whether a script actually
// defers or runs asynchronously is decided when it is prepared, not by this
// surface.
impl HTMLScriptElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn charset(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("charset"))
    }
    fn set_charset(&self, value: &str) {
        self.reflect_attribute("charset", value);
    }
    fn cross_origin(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("crossorigin"))
    }
    fn set_cross_origin(&self, value: &str) {
        self.reflect_attribute("crossorigin", value);
    }
    fn integrity(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("integrity"))
    }
    fn set_integrity(&self, value: &str) {
        self.reflect_attribute("integrity", value);
    }
    fn r#async(&self) -> bool {
        self.reflect_bool("async")
    }
    fn set_async(&self, value: bool) {
        self.set_reflect_bool("async", value);
    }
    fn defer(&self) -> bool {
        self.reflect_bool("defer")
    }
    fn set_defer(&self, value: bool) {
        self.set_reflect_bool("defer", value);
    }
    fn no_module(&self) -> bool {
        self.reflect_bool("nomodule")
    }
    fn set_no_module(&self, value: bool) {
        self.set_reflect_bool("nomodule", value);
    }
}

impl SVGElementOps for HostObject {
    fn style(&self) -> Result<Handle<CSSStyleDeclaration>, JsError> {
        self.style_handle()
    }
}
impl DocumentFragmentOps for HostObject {}

impl HTMLTemplateElementOps for HostObject {
    fn content(&self) -> Result<Handle<DocumentFragment>, JsError> {
        // The contents fragment is created with the template element and owned
        // by it, so an HTML `<template>` always has one.
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        let contents = doc.template_contents(id).ok_or_else(|| {
            JsError::dom_exception("InvalidStateError", "template has no contents fragment")
        })?;
        Ok(Handle::from_bits(encode_node(doc.node_ref(contents))))
    }
}

impl DOMTokenListOps for HostObject {
    fn length(&self) -> u32 {
        self.class_tokens().len() as u32
    }

    fn item(&self, index: u32) -> Result<Option<String>, JsError> {
        Ok(self.class_tokens().into_iter().nth(index as usize))
    }

    fn contains(&self, token: &str) -> bool {
        self.class_tokens().iter().any(|existing| existing == token)
    }

    fn add(&self, tokens: Vec<String>) -> Result<(), JsError> {
        for token in &tokens {
            validate_token(token)?;
        }
        let mut set = self.class_tokens();
        for token in tokens {
            if !set.contains(&token) {
                set.push(token);
            }
        }
        self.set_class_tokens(&set);
        Ok(())
    }

    fn remove(&self, tokens: Vec<String>) -> Result<(), JsError> {
        for token in &tokens {
            validate_token(token)?;
        }
        let mut set = self.class_tokens();
        set.retain(|existing| !tokens.iter().any(|token| token == existing));
        self.set_class_tokens(&set);
        Ok(())
    }

    fn toggle(&self, token: &str, force: Option<bool>) -> Result<bool, JsError> {
        validate_token(token)?;
        let mut set = self.class_tokens();
        let present = set.iter().any(|existing| existing == token);
        match force {
            // `force` pins the outcome: present-if-true, absent-if-false.
            Some(true) => {
                if !present {
                    set.push(token.to_owned());
                    self.set_class_tokens(&set);
                }
                Ok(true)
            }
            Some(false) => {
                if present {
                    set.retain(|existing| existing != token);
                    self.set_class_tokens(&set);
                }
                Ok(false)
            }
            None if present => {
                set.retain(|existing| existing != token);
                self.set_class_tokens(&set);
                Ok(false)
            }
            None => {
                set.push(token.to_owned());
                self.set_class_tokens(&set);
                Ok(true)
            }
        }
    }

    fn replace(&self, token: &str, new_token: &str) -> Result<bool, JsError> {
        validate_token(token)?;
        validate_token(new_token)?;
        let mut set = self.class_tokens();
        let Some(position) = set.iter().position(|existing| existing == token) else {
            return Ok(false);
        };
        set[position] = new_token.to_owned();
        // Replacing may collide with a later token; keep the ordered set unique.
        let mut seen: Vec<String> = Vec::new();
        set.retain(|existing| {
            if seen.iter().any(|kept| kept == existing) {
                false
            } else {
                seen.push(existing.clone());
                true
            }
        });
        self.set_class_tokens(&set);
        Ok(true)
    }

    fn value(&self) -> Result<String, JsError> {
        // The `value` getter returns the underlying attribute verbatim, not the
        // re-serialised ordered set.
        let (doc, id) = self.anchor();
        Ok(doc
            .borrow()
            .attribute_str(id, "class")
            .unwrap_or("")
            .to_owned())
    }

    fn set_value(&self, value: &str) {
        self.reflect_attribute("class", value);
    }
}

impl CSSStyleDeclarationOps for HostObject {
    fn css_text(&self) -> Result<String, JsError> {
        Ok(serialize_declarations(&self.style_declarations()))
    }

    fn set_css_text(&self, value: &str) {
        // Re-parse and re-serialise so the stored attribute is normalised.
        self.reflect_attribute("style", value);
        let declarations = self.style_declarations();
        self.set_style_declarations(&declarations);
    }

    fn length(&self) -> u32 {
        self.style_declarations().len() as u32
    }

    fn item(&self, index: u32) -> Result<String, JsError> {
        Ok(self
            .style_declarations()
            .into_iter()
            .nth(index as usize)
            .map(|declaration| declaration.name)
            .unwrap_or_default())
    }

    fn get_property_value(&self, property: &str) -> Result<String, JsError> {
        let name = property.trim().to_ascii_lowercase();
        Ok(self
            .style_declarations()
            .into_iter()
            .find(|declaration| declaration.name == name)
            .map(|declaration| declaration.value)
            .unwrap_or_default())
    }

    fn get_property_priority(&self, property: &str) -> Result<String, JsError> {
        let name = property.trim().to_ascii_lowercase();
        let important = self
            .style_declarations()
            .into_iter()
            .find(|declaration| declaration.name == name)
            .is_some_and(|declaration| declaration.important);
        Ok(if important { "important" } else { "" }.to_owned())
    }

    fn set_property(
        &self,
        property: &str,
        value: &str,
        priority: Option<&str>,
    ) -> Result<(), JsError> {
        let name = property.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Ok(());
        }
        // A non-empty priority that is not "important" is invalid: no-op.
        let important = match priority.map(str::trim) {
            None | Some("") => false,
            Some(priority) if priority.eq_ignore_ascii_case("important") => true,
            Some(_) => return Ok(()),
        };
        let mut declarations = self.style_declarations();
        // An empty value removes the property.
        if value.trim().is_empty() {
            declarations.retain(|declaration| declaration.name != name);
            self.set_style_declarations(&declarations);
            return Ok(());
        }
        let value = value.trim().to_owned();
        if let Some(declaration) = declarations
            .iter_mut()
            .find(|declaration| declaration.name == name)
        {
            declaration.value = value;
            declaration.important = important;
        } else {
            declarations.push(StyleDeclaration {
                name,
                value,
                important,
            });
        }
        self.set_style_declarations(&declarations);
        Ok(())
    }

    fn remove_property(&self, property: &str) -> Result<String, JsError> {
        let name = property.trim().to_ascii_lowercase();
        let mut declarations = self.style_declarations();
        let removed = declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .map(|declaration| declaration.value.clone())
            .unwrap_or_default();
        declarations.retain(|declaration| declaration.name != name);
        self.set_style_declarations(&declarations);
        Ok(removed)
    }
}

impl HostObject {
    /// Builds and dispatches the synthetic, untrusted `click` `MouseEvent` for
    /// [`HTMLElementOps::click`]. The event bubbles, is cancelable and composed,
    /// carries `detail` 1 and `button` 0 with every coordinate and modifier at
    /// its default, and no related target. If no listener created a wrapper for
    /// it, its slab record is removed here so a transient click leaks nothing;
    /// when a wrapper exists, the world's dead-event sweep reclaims the record.
    fn dispatch_synthetic_click(&self, doc: &SharedDocument, id: NodeId) -> Result<(), JsError> {
        let ctx = self.context();
        let target = {
            let doc = doc.borrow();
            // A dead element has no listeners that could legitimately fire, and
            // its arena slot may already belong to an unrelated node, so a
            // dispatch here would be attributed to that node's listeners.
            if !doc.is_live(id) {
                return Ok(());
            }
            doc.node_ref(id)
        };
        let mut record = EventRecord::new(
            "click".to_owned(),
            EventKind::Mouse {
                detail: 1,
                screen_x: 0,
                screen_y: 0,
                client_x: 0,
                client_y: 0,
                ctrl: false,
                shift: false,
                alt: false,
                meta: false,
                button: 0,
                buttons: 0,
                related_target: None,
            },
            ctx.time_stamp(),
        );
        record.bubbles = true;
        record.cancelable = true;
        record.composed = true;
        record.flags.initialized = true;
        let key = ctx.events.borrow_mut().insert(record);

        let dispatch_result = dispatch(ctx, target, key, false);

        // Remove the transient record unless a listener made a wrapper for it
        // (in which case the finalizer-driven sweep owns its removal).
        let orphaned = ctx
            .events
            .borrow()
            .get(key)
            .is_some_and(|record| !record.has_wrapper);
        if orphaned {
            ctx.events.borrow_mut().remove(key);
        }
        dispatch_result.map(|_| ())
    }
}

/// Maps a `tenta_core::Error` from a mutation to a `JsError`. A DOM error
/// carries its exception name in `code` and the human-readable text in
/// `message`; the name is passed separately so the throw path can build a real
/// `DOMException` rather than folding the name into the message.
fn mutation_error(e: &tenta_core::Error) -> JsError {
    match e {
        tenta_core::Error::Other { code, message } => JsError::dom_exception(code, message),
        _ => JsError::type_error(e.to_string()),
    }
}

/// The DOM `nodeName` of `id` (https://dom.spec.whatwg.org/#dom-node-nodename).
fn node_name(doc: &Document, id: NodeId) -> String {
    match doc.kind(id) {
        NodeKind::Element => element_tag_name(doc, id),
        NodeKind::Text => "#text".to_owned(),
        NodeKind::CData => "#cdata-section".to_owned(),
        NodeKind::Comment => "#comment".to_owned(),
        NodeKind::Document => "#document".to_owned(),
        NodeKind::Fragment | NodeKind::ShadowRoot => "#document-fragment".to_owned(),
        NodeKind::DocType => doc
            .doctype(id)
            .map(|doctype| doctype.name.clone())
            .unwrap_or_default(),
        NodeKind::ProcessingInstruction => doc
            .processing_instruction(id)
            .map(|(target, _)| target.to_owned())
            .unwrap_or_default(),
    }
}

/// The DOM `tagName` of an element: the qualified name, ASCII-uppercased for
/// elements in the HTML namespace (https://dom.spec.whatwg.org/#dom-element-tagname).
/// The qualified name of an attribute (`prefix:local`, or just `local` when
/// there is no prefix), as `getAttributeNames` reports it
/// (<https://dom.spec.whatwg.org/#dom-element-getattributenames>).
fn qualified_attr_name(name: &QualName) -> String {
    match &name.prefix {
        Some(prefix) => format!("{}:{}", prefix.as_str(), name.local.as_str()),
        None => name.local.as_str().to_owned(),
    }
}

/// Validates an attribute name at the DOM boundary: non-empty and starting with
/// a letter, underscore or colon. Returns an `InvalidCharacterError` otherwise,
/// as `setAttribute`/`toggleAttribute` require.
fn validate_attribute_name(name: &str) -> Result<(), JsError> {
    if name.is_empty() {
        return Err(JsError::dom_exception(
            "InvalidCharacterError",
            "Attribute name must not be empty",
        ));
    }
    if let Some(first_char) = name.chars().next()
        && !first_char.is_ascii_alphabetic()
        && first_char != '_'
        && first_char != ':'
    {
        return Err(JsError::dom_exception(
            "InvalidCharacterError",
            "Attribute name must start with a letter, underscore, or colon",
        ));
    }
    Ok(())
}

/// One parsed declaration of an inline `style` block (`CSSStyleDeclaration`).
struct StyleDeclaration {
    name: String,
    value: String,
    important: bool,
}

/// Splits a trailing `!important` (ASCII case-insensitive) off a declaration
/// value, returning the remaining value and whether the flag was present.
fn split_important(value: &str) -> (&str, bool) {
    const IMPORTANT: &str = "!important";
    let trimmed = value.trim_end();
    if trimmed.len() >= IMPORTANT.len() {
        let (head, tail) = trimmed.split_at(trimmed.len() - IMPORTANT.len());
        if tail.eq_ignore_ascii_case(IMPORTANT) {
            return (head, true);
        }
    }
    (value, false)
}

/// Serialises a declaration block into the `style` attribute form
/// (`name: value;` declarations joined by a space).
fn serialize_declarations(declarations: &[StyleDeclaration]) -> String {
    declarations
        .iter()
        .map(|declaration| {
            if declaration.important {
                format!("{}: {} !important;", declaration.name, declaration.value)
            } else {
                format!("{}: {};", declaration.name, declaration.value)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Validates a `DOMTokenList` token (`add`/`remove`/`toggle`/`replace`): an
/// empty token is a `SyntaxError`, a token with ASCII whitespace is an
/// `InvalidCharacterError` (<https://dom.spec.whatwg.org/#dom-domtokenlist-add>).
fn validate_token(token: &str) -> Result<(), JsError> {
    if token.is_empty() {
        return Err(JsError::dom_exception(
            "SyntaxError",
            "The token must not be empty",
        ));
    }
    if token.chars().any(|c| c.is_ascii_whitespace()) {
        return Err(JsError::dom_exception(
            "InvalidCharacterError",
            "The token must not contain ASCII whitespace",
        ));
    }
    Ok(())
}

/// The WHATWG "validate and extract" algorithm for `createElementNS`
/// (<https://dom.spec.whatwg.org/#validate-and-extract>): resolve the namespace,
/// split off any prefix, and reject the prefix/namespace combinations the spec
/// forbids. Returns `(namespace, prefix, local name)`.
fn validate_and_extract<'a>(
    namespace: Option<&str>,
    qualified_name: &'a str,
) -> Result<(Atom, Option<Atom>, &'a str), JsError> {
    // An empty namespace string is treated as the null namespace.
    let namespace = namespace.filter(|ns| !ns.is_empty());

    validate_attribute_name(qualified_name)?;
    if qualified_name.chars().any(|c| {
        matches!(
            c,
            ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '>' | '<' | '&' | '/'
        )
    }) {
        return Err(JsError::dom_exception(
            "InvalidCharacterError",
            "Qualified name contains an invalid character",
        ));
    }

    // Split an optional single prefix. A trailing or doubled colon is invalid.
    let (prefix, local) = match qualified_name.split_once(':') {
        Some((prefix, local))
            if !prefix.is_empty() && !local.is_empty() && !local.contains(':') =>
        {
            (Some(prefix), local)
        }
        Some(_) => {
            return Err(JsError::dom_exception(
                "InvalidCharacterError",
                "Qualified name has a malformed prefix",
            ));
        }
        None => (None, qualified_name),
    };

    let ns = namespace.map(Atom::new);
    let namespace_error = |msg: &str| JsError::dom_exception("NamespaceError", msg);
    // A prefix requires a namespace; `xml`/`xmlns` are pinned to their namespaces.
    if prefix.is_some() && ns.is_none() {
        return Err(namespace_error("A prefix requires a non-null namespace"));
    }
    if prefix == Some("xml") && ns != Some(atoms::NS_XML) {
        return Err(namespace_error(
            "The `xml` prefix requires the XML namespace",
        ));
    }
    let is_xmlns = qualified_name == "xmlns" || prefix == Some("xmlns");
    if is_xmlns && ns != Some(atoms::NS_XMLNS) {
        return Err(namespace_error(
            "The `xmlns` name requires the XMLNS namespace",
        ));
    }
    if ns == Some(atoms::NS_XMLNS) && !is_xmlns {
        return Err(namespace_error(
            "The XMLNS namespace requires the `xmlns` name",
        ));
    }

    Ok((ns.unwrap_or(atoms::EMPTY), prefix.map(Atom::new), local))
}

fn element_tag_name(doc: &Document, id: NodeId) -> String {
    let Some(local) = doc.tag(id) else {
        return String::new();
    };
    if doc.namespace(id) == Some(atoms::NS_HTML) {
        local.as_str().to_ascii_uppercase()
    } else {
        local.as_str().to_owned()
    }
}

/// The bit layout of a DOM wrapper handle: `tag:8 | doc_slot:8 | node_id:32 |
/// generation:16` (`docs/spec/04-dom-and-css.md` §1, `NodeRef`). The document
/// slot is always zero until a world spans more than one document.
const TAG_SHIFT: u32 = 56;
const DOC_SLOT_SHIFT: u32 = 48;
const NODE_ID_SHIFT: u32 = 16;

/// Handle tag for a single node.
pub const TAG_NODE: u8 = 0;
/// Handle tag for a node's child `NodeList`.
pub const TAG_NODE_LIST: u8 = 1;
/// Handle tag for a node's element-child `HTMLCollection`.
pub const TAG_HTML_COLLECTION: u8 = 2;
/// Handle tag for a static snapshot `NodeList` (querySelectorAll, etc.).
pub const TAG_STATIC_NODE_LIST: u8 = 3;
/// Handle tag for a static snapshot `HTMLCollection` (getElementsByTagName, etc.).
pub const TAG_STATIC_HTML_COLLECTION: u8 = 4;
/// Handle tag for an `Event` (and its subclasses), keyed into the event slab.
pub const TAG_EVENT: u8 = 5;
/// Handle tag for an element's `DOMTokenList` (`classList`); like a live
/// collection, it anchors to the owning element's node.
pub const TAG_TOKEN_LIST: u8 = 6;
/// Handle tag for an element's inline `CSSStyleDeclaration` (`style`); it
/// anchors to the owning element's node like a live collection.
pub const TAG_STYLE: u8 = 7;
/// Handle tag for the window. The window is not a node — it has no arena slot
/// and no id — so its handle carries no payload at all: the tag alone names it.
/// It exists because the window is the outermost target of every event path,
/// and a listener's `currentTarget` has to be the window object.
pub const TAG_WINDOW: u8 = 8;

/// A handle decoded back into its parts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecodedHandle {
    /// [`TAG_NODE`], [`TAG_NODE_LIST`], [`TAG_HTML_COLLECTION`], or snapshot tags.
    pub tag: u8,
    /// For node handles: the node id. For snapshot handles: the snapshot index.
    pub node_id: NodeId,
    /// For node handles: the node generation. For snapshot handles: the snapshot generation.
    pub generation: u16,
}

impl DecodedHandle {
    /// Returns the node anchor (Some(node_id)) for live handles, None for snapshot tags.
    /// Used in tag-aware scans to avoid corrupting a real node's HAS_WRAPPER bit
    /// when sweeping dead wrappers.
    pub fn node_anchor(&self) -> Option<NodeId> {
        match self.tag {
            TAG_NODE | TAG_NODE_LIST | TAG_HTML_COLLECTION | TAG_TOKEN_LIST | TAG_STYLE => {
                Some(self.node_id)
            }
            // Snapshot tags 3 and 4, TAG_EVENT, and TAG_WINDOW: a window handle
            // is deliberately not anchored to any node, so a sweep cannot
            // mistake its zero payload for node 0.
            _ => None,
        }
    }

    /// Reconstructs a SnapshotKey from decoded snapshot handle parts.
    /// Only valid when tag is a snapshot tag.
    pub fn snapshot_key(&self) -> SnapshotKey {
        SnapshotKey {
            index: self.node_id.raw(),
            generation: self.generation,
        }
    }

    /// Reconstructs an [`EventKey`] from decoded event handle parts. Only valid
    /// when `tag` is [`TAG_EVENT`].
    pub fn event_key(&self) -> EventKey {
        EventKey {
            index: self.node_id.raw(),
            generation: self.generation,
        }
    }
}

/// Packs a `tag` and a node reference into the opaque handle bits a wrapper
/// world keys on.
fn pack(tag: u8, node: NodeRef) -> u64 {
    // TODO(phase 1): the arena generation is `u32`; the low 16 bits are enough
    // to catch slot reuse within a page. Widen alongside the `doc_slot` tag
    // when a world spans more than one document.
    let generation = node.generation as u16;
    ((tag as u64) << TAG_SHIFT)
        | (0u64 << DOC_SLOT_SHIFT)
        | ((node.id.raw() as u64) << NODE_ID_SHIFT)
        | (generation as u64)
}

/// Packs a node reference into a [`TAG_NODE`] handle.
pub fn encode_node(node: NodeRef) -> u64 {
    pack(TAG_NODE, node)
}

/// Packs a list owner's reference into a [`TAG_NODE_LIST`] handle.
pub fn encode_collection(owner: NodeRef) -> u64 {
    pack(TAG_NODE_LIST, owner)
}

/// Packs a collection owner's reference into a [`TAG_HTML_COLLECTION`] handle.
pub fn encode_html_collection(owner: NodeRef) -> u64 {
    pack(TAG_HTML_COLLECTION, owner)
}

/// Packs a token-list owner's reference into a [`TAG_TOKEN_LIST`] handle.
pub fn encode_token_list(owner: NodeRef) -> u64 {
    pack(TAG_TOKEN_LIST, owner)
}

/// Packs a style declaration owner's reference into a [`TAG_STYLE`] handle.
pub fn encode_style(owner: NodeRef) -> u64 {
    pack(TAG_STYLE, owner)
}

/// Packs the window into a [`TAG_WINDOW`] handle. Every window handle is
/// identical, which is what lets a listener compare `currentTarget` against the
/// global object.
pub fn encode_window() -> u64 {
    (TAG_WINDOW as u64) << TAG_SHIFT
}

/// Packs a snapshot tag and key into handle bits.
pub fn encode_snapshot(tag: u8, key: SnapshotKey) -> u64 {
    ((tag as u64) << TAG_SHIFT)
        | (0u64 << DOC_SLOT_SHIFT)
        | ((key.index as u64) << NODE_ID_SHIFT)
        | (key.generation as u64)
}

/// Packs an [`EventKey`] into [`TAG_EVENT`] handle bits.
pub fn encode_event(key: EventKey) -> u64 {
    ((TAG_EVENT as u64) << TAG_SHIFT)
        | (0u64 << DOC_SLOT_SHIFT)
        | ((key.index as u64) << NODE_ID_SHIFT)
        | (key.generation as u64)
}

/// Splits handle bits back into their tag, node id and generation.
pub fn decode(bits: u64) -> DecodedHandle {
    DecodedHandle {
        tag: (bits >> TAG_SHIFT) as u8,
        node_id: NodeId(((bits >> NODE_ID_SHIFT) & 0xFFFF_FFFF) as u32),
        generation: (bits & 0xFFFF) as u16,
    }
}
