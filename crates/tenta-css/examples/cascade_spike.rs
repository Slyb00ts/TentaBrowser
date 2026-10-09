//! Throwaway spike: run Stylo's sequential DOM traversal over the arena DOM.
//!
//! The adapter types borrow `&Document` directly (no snapshot). The program
//! prints the computed `color` and `display` of two elements and then times
//! the pieces whose cost decides the adapter design. See
//! `docs/dev/cascade-spike.md` for the findings.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::time::Instant;

use selectors::OpaqueElement;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use servo_arc::{Arc, ArcBorrow};
use style::Atom;
use style::LocalName;
use style::Namespace;
use style::animation::DocumentAnimationSet;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::context::{
    QuirksMode, RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext,
    StyleContext, StyleSystemOptions,
};
use style::data::{ElementData, ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::device::Device;
use style::device::servo::FontMetricsProvider;
use style::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use style::driver::traverse_dom;
use style::font_metrics::FontMetrics;
use style::media_queries::MediaType;
use style::properties::style_structs::Font;
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::queries::values::PrefersColorScheme;
use style::selector_parser::{AttrValue as SelAttrValue, Lang, NonTSPseudoClass, PseudoElement};
use style::selector_parser::{SelectorImpl, SnapshotMap};
use style::servo::media_features::PointerCapabilities;
use style::shared_lock::{Locked, SharedRwLock, StylesheetGuards};
use style::stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
use style::stylist::{CascadeData, Stylist};
use style::thread_state::{self, ThreadState};
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::traversal_flags::TraversalFlags;
use style::values::AtomIdent;
use style::values::AtomString;
use style::values::computed::font::GenericFontFamily;
use style::values::computed::{Au, Length};
use style::values::specified::font::QueryFontMetricsFlags;
use style_traits::ToCss;
use tenta_core::NodeId;
use tenta_core::atom::{Atom as TentaAtom, atoms};
use tenta_dom::{Document, NodeKind};
use tenta_html::{Parser, ParserOptions};

// ----------------------------------------------------------------------------
// Shared per-flush context: the side tables behind the borrowed handles.
// ----------------------------------------------------------------------------

/// Call counters for the adapter hooks, used to size the atom-mapping cost.
#[derive(Clone, Default)]
struct Counters {
    local_name: Cell<u64>,
    has_local_name: Cell<u64>,
    has_class: Cell<u64>,
    each_class: Cell<u64>,
    id: Cell<u64>,
    attr_matches: Cell<u64>,
}

struct Ctx<'a> {
    doc: &'a Document,
    lock: SharedRwLock,
    /// One record per arena slot; set once after the records are built. A
    /// `TElement` must be exactly one machine word, so a handle is a reference
    /// to its record and the record reaches the context through `ctx`.
    slots: OnceCell<&'a [Slot<'a>]>,
    /// tenta tag atom (raw) -> Stylo local name, filled on first use.
    local_names: Vec<OnceCell<LocalName>>,
    /// Per-slot element ids as Stylo atoms (`TElement::id` returns a reference).
    ids: Vec<Option<Atom>>,
    namespaces: [(TentaAtom, Namespace); 3],
    empty_namespace: Namespace,
    quirks: QuirksMode,
    counters: Counters,
    /// Class name -> Stylo atom, so `each_class` does not intern per call.
    cache_classes: bool,
    class_cache: RefCell<HashMap<Box<str>, AtomIdent>>,
}

/// The per-node record a handle points at: Stylo state that lives for one flush.
struct Slot<'a> {
    ctx: &'a Ctx<'a>,
    id: NodeId,
    /// Stylo's own cell. Its fields are private, so a plain
    /// `AtomicRefCell<Option<ElementData>>` cannot produce `ElementDataMut`.
    data: ElementDataWrapper,
    has_data: Cell<bool>,
    dirty_descendants: Cell<bool>,
    selector_flags: Cell<ElementSelectorFlags>,
    children_to_process: Cell<isize>,
}

impl<'a> Slot<'a> {
    fn doc(&self) -> &'a Document {
        self.ctx.doc
    }

    fn new(ctx: &'a Ctx<'a>, index: usize) -> Self {
        Slot {
            ctx,
            id: NodeId(index as u32),
            data: ElementDataWrapper::default(),
            has_data: Cell::new(false),
            dirty_descendants: Cell::new(false),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            children_to_process: Cell::new(0),
        }
    }
}

impl<'a> Ctx<'a> {
    fn new(doc: &'a Document, cache_classes: bool) -> Self {
        let slots = doc.slots();
        let mut ids: Vec<Option<Atom>> = vec![None; slots];
        for node in doc.descendants(doc.document_node()) {
            if doc.is_element(node)
                && let Some(id) = doc.element_id(node)
            {
                ids[node.raw() as usize] = Some(Atom::from(id));
            }
        }
        let quirks = match doc.quirks_mode() {
            tenta_html::QuirksMode::NoQuirks => QuirksMode::NoQuirks,
            tenta_html::QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
            tenta_html::QuirksMode::Quirks => QuirksMode::Quirks,
        };
        Self {
            doc,
            lock: SharedRwLock::new(),
            slots: OnceCell::new(),
            local_names: (0..TentaAtom::count()).map(|_| OnceCell::new()).collect(),
            ids,
            namespaces: [
                (atoms::NS_HTML, Namespace::from(atoms::NS_HTML.as_str())),
                (atoms::NS_SVG, Namespace::from(atoms::NS_SVG.as_str())),
                (atoms::NS_MATHML, Namespace::from(atoms::NS_MATHML.as_str())),
            ],
            empty_namespace: Namespace::from(""),
            quirks,
            counters: Counters::default(),
            cache_classes,
            class_cache: RefCell::new(HashMap::new()),
        }
    }

    fn slot(&self, id: NodeId) -> &'a Slot<'a> {
        &self
            .slots
            .get()
            .expect("slots are installed before traversal")[id.raw() as usize]
    }

    fn tag_name(&self, tag: TentaAtom) -> &LocalName {
        self.local_names[tag.raw() as usize].get_or_init(|| LocalName::from(tag.as_str()))
    }

    fn namespace_of(&self, ns: TentaAtom) -> &Namespace {
        self.namespaces
            .iter()
            .find(|(atom, _)| *atom == ns)
            .map(|(_, ns)| ns)
            .unwrap_or(&self.empty_namespace)
    }
}

fn bump(counter: &Cell<u64>) {
    counter.set(counter.get() + 1);
}

// ----------------------------------------------------------------------------
// Handles: Copy newtypes over (&Ctx, NodeId).
// ----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Node<'a>(&'a Slot<'a>);

#[derive(Clone, Copy)]
struct Element<'a>(&'a Slot<'a>);

#[derive(Clone, Copy)]
struct Doc<'a>(&'a Slot<'a>);

/// There is no shadow DOM in the spike; the type only has to exist.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
enum NoShadow<'a> {
    Never(std::marker::PhantomData<&'a ()>, std::convert::Infallible),
}

impl<'a> Node<'a> {
    fn with(&self, id: NodeId) -> Option<Node<'a>> {
        id.is_some().then(|| Node(self.0.ctx.slot(id)))
    }
    fn doc(&self) -> &'a Document {
        self.0.ctx.doc
    }
    fn id(&self) -> NodeId {
        self.0.id
    }
}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.doc().tag(self.0.id) {
            Some(tag) => write!(f, "<{} #{}>", tag.as_str(), self.0.id.raw()),
            None => write!(f, "[{:?} #{}]", self.doc().kind(self.0.id), self.0.id.raw()),
        }
    }
}
impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}
impl std::fmt::Debug for Element<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_node().fmt(f)
    }
}
impl PartialEq for Element<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}
impl Eq for Element<'_> {}
impl Hash for Element<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.raw().hash(state);
    }
}
impl std::fmt::Debug for Doc<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[document]")
    }
}

impl NodeInfo for Node<'_> {
    fn is_element(&self) -> bool {
        self.doc().kind(self.0.id) == NodeKind::Element
    }
    fn is_text_node(&self) -> bool {
        self.doc().kind(self.0.id) == NodeKind::Text
    }
}

impl<'a> TDocument for Doc<'a> {
    type ConcreteNode = Node<'a>;
    fn as_node(&self) -> Node<'a> {
        Node(self.0)
    }
    fn is_html_document(&self) -> bool {
        self.0.doc().is_html_document()
    }
    fn quirks_mode(&self) -> QuirksMode {
        self.0.ctx.quirks
    }
    fn shared_lock(&self) -> &SharedRwLock {
        &self.0.ctx.lock
    }
}

impl<'a> TNode for Node<'a> {
    type ConcreteElement = Element<'a>;
    type ConcreteDocument = Doc<'a>;
    type ConcreteShadowRoot = NoShadow<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.with(self.doc().parent(self.0.id))
    }
    fn first_child(&self) -> Option<Self> {
        self.with(self.doc().first_child(self.0.id))
    }
    fn last_child(&self) -> Option<Self> {
        self.with(self.doc().last_child(self.0.id))
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.with(self.doc().previous_sibling(self.0.id))
    }
    fn next_sibling(&self) -> Option<Self> {
        self.with(self.doc().next_sibling(self.0.id))
    }
    fn owner_doc(&self) -> Doc<'a> {
        Doc(self.0.ctx.slot(self.doc().document_node()))
    }
    fn is_in_document(&self) -> bool {
        self.doc().is_connected(self.0.id)
    }
    fn traversal_parent(&self) -> Option<Element<'a>> {
        self.parent_element()
    }
    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.0.id.raw() as usize)
    }
    fn debug_id(self) -> usize {
        self.0.id.raw() as usize
    }
    fn as_element(&self) -> Option<Element<'a>> {
        self.is_element().then_some(Element(self.0))
    }
    fn as_document(&self) -> Option<Doc<'a>> {
        (self.doc().kind(self.0.id) == NodeKind::Document).then_some(Doc(self.0))
    }
    fn as_shadow_root(&self) -> Option<NoShadow<'a>> {
        None
    }
}

impl<'a> TShadowRoot for NoShadow<'a> {
    type ConcreteNode = Node<'a>;
    fn as_node(&self) -> Node<'a> {
        match *self {
            NoShadow::Never(_, never) => match never {},
        }
    }
    fn host(&self) -> Element<'a> {
        match *self {
            NoShadow::Never(_, never) => match never {},
        }
    }
    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

struct ChildNodes<'a>(Option<Node<'a>>);
impl<'a> Iterator for ChildNodes<'a> {
    type Item = Node<'a>;
    fn next(&mut self) -> Option<Node<'a>> {
        let current = self.0.take()?;
        self.0 = current.next_sibling();
        Some(current)
    }
}

impl<'a> Element<'a> {
    fn ctx(&self) -> &'a Ctx<'a> {
        self.0.ctx
    }
    fn tag(&self) -> TentaAtom {
        self.0.doc().tag(self.0.id).expect("element has a tag")
    }
    fn wrap(&self, id: NodeId) -> Option<Element<'a>> {
        let node = self.as_node().with(id)?;
        node.as_element()
    }
    fn is_ns(&self, ns: TentaAtom) -> bool {
        self.0.doc().namespace(self.0.id) == Some(ns)
    }
}

impl<'a> TElement for Element<'a> {
    type ConcreteNode = Node<'a>;
    type TraversalChildrenIterator = ChildNodes<'a>;

    fn as_node(&self) -> Node<'a> {
        Node(self.0)
    }
    fn traversal_children(&self) -> LayoutIterator<ChildNodes<'a>> {
        LayoutIterator(ChildNodes(self.as_node().first_child()))
    }
    fn is_html_element(&self) -> bool {
        self.is_ns(atoms::NS_HTML)
    }
    fn is_mathml_element(&self) -> bool {
        self.is_ns(atoms::NS_MATHML)
    }
    fn is_svg_element(&self) -> bool {
        self.is_ns(atoms::NS_SVG)
    }
    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn animation_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn transition_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn state(&self) -> stylo_dom::ElementState {
        stylo_dom::ElementState::empty()
    }
    fn has_part_attr(&self) -> bool {
        false
    }
    fn exports_any_part(&self) -> bool {
        false
    }
    fn id(&self) -> Option<&Atom> {
        bump(&self.ctx().counters.id);
        self.ctx().ids[self.0.id.raw() as usize].as_ref()
    }
    fn each_class<F: FnMut(&AtomIdent)>(&self, mut callback: F) {
        bump(&self.ctx().counters.each_class);
        let ctx = self.ctx();
        for class in self.0.doc().classes(self.0.id) {
            if !ctx.cache_classes {
                callback(&AtomIdent::new(Atom::from(class)));
                continue;
            }
            let cached = ctx.class_cache.borrow().get(class).cloned();
            let atom = cached.unwrap_or_else(|| {
                let atom = AtomIdent::new(Atom::from(class));
                ctx.class_cache
                    .borrow_mut()
                    .insert(class.into(), atom.clone());
                atom
            });
            callback(&atom);
        }
    }
    fn each_custom_state<F: FnMut(&AtomIdent)>(&self, _callback: F) {}
    fn each_attr_name<F: FnMut(&LocalName)>(&self, mut callback: F) {
        for (name, _) in self.0.doc().attributes(self.0.id) {
            callback(&LocalName::from(name.local.as_str()));
        }
    }
    fn has_dirty_descendants(&self) -> bool {
        self.0.dirty_descendants.get()
    }
    fn has_snapshot(&self) -> bool {
        false
    }
    fn handled_snapshot(&self) -> bool {
        true
    }
    unsafe fn set_handled_snapshot(&self) {}
    unsafe fn set_dirty_descendants(&self) {
        self.0.dirty_descendants.set(true);
    }
    unsafe fn unset_dirty_descendants(&self) {
        self.0.dirty_descendants.set(false);
    }
    fn store_children_to_process(&self, n: isize) {
        self.0.children_to_process.set(n);
    }
    fn did_process_child(&self) -> isize {
        let cell = &self.0.children_to_process;
        cell.set(cell.get() - 1);
        cell.get()
    }
    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        self.0.has_data.set(true);
        self.0.data.borrow_mut()
    }
    unsafe fn clear_data(&self) {
        if self.0.has_data.replace(false) {
            *self.0.data.borrow_mut() = ElementData::default();
        }
    }
    fn has_data(&self) -> bool {
        self.0.has_data.get()
    }
    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.has_data().then(|| self.0.data.borrow())
    }
    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        self.has_data().then(|| self.0.data.borrow_mut())
    }
    fn skip_item_display_fixup(&self) -> bool {
        false
    }
    fn may_have_animations(&self) -> bool {
        false
    }
    fn has_animations(&self, _: &SharedStyleContext) -> bool {
        false
    }
    fn has_css_animations(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }
    fn has_css_transitions(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }
    fn shadow_root(&self) -> Option<NoShadow<'a>> {
        None
    }
    fn containing_shadow(&self) -> Option<NoShadow<'a>> {
        None
    }
    fn lang_attr(&self) -> Option<AtomString> {
        None
    }
    fn match_element_lang(&self, _: Option<Option<AtomString>>, _: &Lang) -> bool {
        false
    }
    fn is_html_document_body_element(&self) -> bool {
        self.tag() == atoms::BODY && self.is_html_element()
    }
    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _: selectors::matching::VisitedHandlingMode,
        _: &mut V,
    ) where
        V: selectors::sink::Push<ApplicableDeclarationBlock>,
    {
    }
    fn local_name(&self) -> &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName {
        bump(&self.ctx().counters.local_name);
        self.ctx().tag_name(self.tag())
    }
    fn namespace(
        &self,
    ) -> &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl {
        let ns = self.0.doc().namespace(self.0.id).unwrap_or(atoms::EMPTY);
        self.ctx().namespace_of(ns)
    }
    fn query_container_size(
        &self,
        _: &style::values::computed::Display,
    ) -> euclid::default::Size2D<Option<Au>> {
        Default::default()
    }
    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.0.selector_flags.get().contains(flags)
    }
    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        self.0
            .selector_flags
            .get()
            .intersection(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING)
    }
    fn get_attr(&self, attr: &LocalName, _namespace: &Namespace) -> Option<String> {
        self.0
            .doc()
            .attribute_str(self.0.id, attr)
            .map(str::to_owned)
    }
}

impl<'a> selectors::Element for Element<'a> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(&self.0.data)
    }
    fn parent_element(&self) -> Option<Self> {
        self.wrap(self.0.doc().parent(self.0.id))
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        let mut sibling = self.as_node().prev_sibling();
        while let Some(node) = sibling {
            if let Some(element) = node.as_element() {
                return Some(element);
            }
            sibling = node.prev_sibling();
        }
        None
    }
    fn next_sibling_element(&self) -> Option<Self> {
        let mut sibling = self.as_node().next_sibling();
        while let Some(node) = sibling {
            if let Some(element) = node.as_element() {
                return Some(element);
            }
            sibling = node.next_sibling();
        }
        None
    }
    fn first_element_child(&self) -> Option<Self> {
        self.traversal_children().find_map(|node| node.as_element())
    }
    fn is_html_element_in_html_document(&self) -> bool {
        self.is_html_element() && self.0.doc().is_html_document()
    }
    fn has_local_name(
        &self,
        name: &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName,
    ) -> bool {
        bump(&self.ctx().counters.has_local_name);
        **self.ctx().tag_name(self.tag()) == *name
    }
    fn has_namespace(
        &self,
        ns: &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        **self.namespace() == *ns
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.tag() == other.tag()
            && self.0.doc().namespace(self.0.id) == other.0.doc().namespace(other.0.id)
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&SelAttrValue>,
    ) -> bool {
        bump(&self.ctx().counters.attr_matches);
        let wanted: &str = local_name;
        self.0.doc().attributes(self.0.id).any(|(name, value)| {
            name.local.as_str() == wanted
                && match ns {
                    NamespaceConstraint::Any => true,
                    NamespaceConstraint::Specific(ns) => {
                        **self.ctx().namespace_of(name.ns) == ***ns
                    }
                }
                && operation.eval_str(value)
        })
    }
    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _: &mut MatchingContext<SelectorImpl>,
    ) -> bool {
        match pc {
            NonTSPseudoClass::Link | NonTSPseudoClass::AnyLink => self.is_link(),
            _ => self.state().contains(pc.state_flag()),
        }
    }
    fn match_pseudo_element(
        &self,
        _: &PseudoElement,
        _: &mut MatchingContext<SelectorImpl>,
    ) -> bool {
        false
    }
    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        let cell = &self.0.selector_flags;
        cell.set(cell.get() | flags);
    }
    fn is_link(&self) -> bool {
        matches!(self.tag(), t if t == atoms::A || t == atoms::AREA || t == atoms::LINK)
            && self.0.doc().has_attribute(self.0.id, atoms::HREF)
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }
    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        bump(&self.ctx().counters.has_class);
        self.0
            .doc()
            .element_id(self.0.id)
            .is_some_and(|actual| case_sensitivity.eq(actual.as_bytes(), id.as_bytes()))
    }
    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        bump(&self.ctx().counters.has_class);
        self.0
            .doc()
            .classes(self.0.id)
            .any(|class| case_sensitivity.eq(class.as_bytes(), name.as_bytes()))
    }
    fn has_custom_state(&self, _: &AtomIdent) -> bool {
        false
    }
    fn imported_part(&self, _: &AtomIdent) -> Option<AtomIdent> {
        None
    }
    fn is_part(&self, _: &AtomIdent) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.as_node()
            .dom_children()
            .all(|child| match self.0.doc().kind(child.id()) {
                NodeKind::Element => false,
                NodeKind::Text => self.0.doc().text_data(child.id()).is_none_or(str::is_empty),
                _ => true,
            })
    }
    fn is_root(&self) -> bool {
        self.as_node()
            .parent_node()
            .is_some_and(|parent| self.0.doc().kind(parent.id()) == NodeKind::Document)
    }
    fn add_element_unique_hashes(&self, _: &mut BloomFilter) -> bool {
        false
    }
}

// ----------------------------------------------------------------------------
// Device support and the traversal itself.
// ----------------------------------------------------------------------------

#[derive(Debug)]
struct FixedMetrics;

impl FontMetricsProvider for FixedMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: style::values::computed::CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        // Proportions of a typical text face; the real provider asks the font system.
        FontMetrics {
            x_height: Some(base_size * 0.5),
            zero_advance_measure: Some(base_size * 0.5),
            cap_height: Some(base_size * 0.7),
            ic_width: Some(base_size),
            ascent: base_size * 0.9,
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }
    fn base_size_for_generic(&self, generic: GenericFontFamily) -> Length {
        Length::new(match generic {
            GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        })
    }
}

struct NoPainters;
impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

// Stylo keeps candidate elements in a thread-local cache typed as `usize`
// (sharing/mod.rs): `size_of::<E>() == size_of::<usize>()` is asserted at run time.
const _: () = assert!(std::mem::size_of::<Element<'static>>() == std::mem::size_of::<usize>());

struct RecalcStyle<'a> {
    shared: SharedStyleContext<'a>,
}

impl<'a, 'd> DomTraversal<Element<'d>> for RecalcStyle<'a> {
    fn process_preorder<F>(
        &self,
        data: &PerLevelTraversalData,
        context: &mut StyleContext<Element<'d>>,
        node: Node<'d>,
        note_child: F,
    ) where
        F: FnMut(Node<'d>),
    {
        let Some(element) = node.as_element() else {
            return;
        };
        // SAFETY: the traversal is sequential, so nothing else holds this element's data.
        let mut element_data = unsafe { element.ensure_data() };
        recalc_style_at(self, data, context, element, &mut element_data, note_child);
        // SAFETY: same exclusivity argument.
        unsafe { element.unset_dirty_descendants() };
    }

    fn process_postorder(&self, _: &mut StyleContext<Element<'d>>, _: Node<'d>) {}

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.shared
    }
}

fn new_device(viewport: (f32, f32)) -> Device {
    let default_values = ComputedValues::initial_values_with_font_override(Font::initial_values());
    let viewport_size = euclid::Size2D::new(viewport.0, viewport.1);
    let device_size = euclid::Size2D::new(viewport.0, viewport.1);
    let ratio = euclid::Scale::new(1.0);
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        viewport_size,
        device_size,
        ratio,
        Box::new(FixedMetrics),
        default_values,
        PrefersColorScheme::Light,
        PointerCapabilities::default(),
        PointerCapabilities::default(),
    )
}

fn parse_sheet(css: &str, origin: Origin, lock: &SharedRwLock) -> DocumentStyleSheet {
    let url = url::Url::parse("https://example.test/").expect("static url");
    let media = Arc::new(lock.wrap(style::media_queries::MediaList::empty()));
    DocumentStyleSheet(Arc::new(Stylesheet::from_str(
        css,
        UrlExtraData::from(url),
        origin,
        media,
        lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::No,
    )))
}

fn parse_document(html: &str) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    parser.finish()
}

struct Timings {
    context_build_us: f64,
    stylist_flush_us: f64,
    traversal_us: f64,
}

/// Styles the whole document and calls `read` with the context and the root.
fn style_document<R>(
    doc: &Document,
    cache_classes: bool,
    ua_css: &str,
    author_css: &str,
    read: impl FnOnce(&Ctx<'_>, Element<'_>) -> R,
) -> (R, Timings, Counters) {
    let started = Instant::now();
    let ctx = Ctx::new(doc, cache_classes);
    let slot_table: Vec<Slot<'_>> = (0..doc.slots()).map(|i| Slot::new(&ctx, i)).collect();
    ctx.slots.set(&slot_table).ok().expect("installed once");
    let context_build_us = started.elapsed().as_secs_f64() * 1e6;

    let mut stylist = Stylist::new(new_device((800.0, 600.0)), ctx.quirks);
    let guard = ctx.lock.read();
    stylist.append_stylesheet(parse_sheet(ua_css, Origin::UserAgent, &ctx.lock), &guard);
    stylist.append_stylesheet(parse_sheet(author_css, Origin::Author, &ctx.lock), &guard);
    let started = Instant::now();
    let guards = StylesheetGuards::same(&guard);
    stylist.flush(&guards);
    let stylist_flush_us = started.elapsed().as_secs_f64() * 1e6;

    let snapshots = SnapshotMap::new();
    let animations = DocumentAnimationSet::default();
    let _ = &animations;
    let shared = SharedStyleContext {
        stylist: &stylist,
        visited_styles_enabled: false,
        options: StyleSystemOptions::default(),
        guards,
        current_time_for_animations: 0.0,
        traversal_flags: TraversalFlags::empty(),
        snapshot_map: &snapshots,
        animations: DocumentAnimationSet::default(),
        registered_speculative_painters: &NoPainters,
    };
    let traversal = RecalcStyle { shared };

    let root_id = doc.document_element().expect("document element");
    let root = Element(ctx.slot(root_id));

    let skip_thread_state = std::env::args().any(|a| a == "--no-thread-state");
    if !skip_thread_state {
        thread_state::enter(ThreadState::LAYOUT);
    }
    let started = Instant::now();
    let token = RecalcStyle::pre_traverse(root, traversal.shared_context());
    if token.should_traverse() {
        traverse_dom(&traversal, token, None);
    }
    let traversal_us = started.elapsed().as_secs_f64() * 1e6;
    if !skip_thread_state {
        thread_state::exit(ThreadState::LAYOUT);
    }

    let result = read(&ctx, root);
    let counters = ctx.counters.clone();
    (
        result,
        Timings {
            context_build_us,
            stylist_flush_us,
            traversal_us,
        },
        counters,
    )
}

fn find_by_tag<'a>(root: Element<'a>, tag: TentaAtom) -> Element<'a> {
    root.as_node()
        .dom_descendants()
        .filter_map(|n| n.as_element())
        .find(|e| e.tag() == tag)
        .expect("tag present")
}

fn describe(element: Element<'_>) -> String {
    let data = element.borrow_data().expect("styled");
    let style = data.styles.primary();
    format!(
        "{:?}: color = {}, display = {}",
        element,
        style.clone_color().to_css_string(),
        style.clone_display().to_css_string()
    )
}

// ----------------------------------------------------------------------------
// Measurements.
// ----------------------------------------------------------------------------

fn ns_per_op(iterations: u64, f: impl Fn(u64) -> u64) -> f64 {
    let started = Instant::now();
    let mut sink = 0u64;
    for i in 0..iterations {
        sink = sink.wrapping_add(black_box(f(i)));
    }
    black_box(sink);
    started.elapsed().as_secs_f64() * 1e9 / iterations as f64
}

fn measure_atom_mapping() {
    let tags = [
        atoms::DIV,
        atoms::P,
        atoms::SPAN,
        atoms::A,
        atoms::UL,
        atoms::LI,
        atoms::TABLE,
        atoms::BODY,
    ];
    let selector_names: Vec<LocalName> = tags.iter().map(|t| LocalName::from(t.as_str())).collect();
    let table: Vec<OnceCell<LocalName>> =
        (0..TentaAtom::count()).map(|_| OnceCell::new()).collect();
    for tag in tags {
        table[tag.raw() as usize].get_or_init(|| LocalName::from(tag.as_str()));
    }
    const N: u64 = 20_000_000;
    let pick = |i: u64| (i as usize) & 7;

    let raw_compare = ns_per_op(N, |i| {
        let k = pick(i);
        (tags[k] == tags[(k + 1) & 7]) as u64
    });
    let as_str_only = ns_per_op(N, |i| tags[pick(i)].as_str().len() as u64);
    let as_str_compare = ns_per_op(N, |i| {
        let k = pick(i);
        (tags[k].as_str() == selector_names[k].as_ref()) as u64
    });
    let naive_map = ns_per_op(N, |i| {
        let k = pick(i);
        let mapped = LocalName::from(tags[k].as_str());
        (mapped == selector_names[k]) as u64
    });
    let table_map = ns_per_op(N, |i| {
        let k = pick(i);
        let mapped = table[tags[k].raw() as usize].get().expect("filled");
        (*mapped == selector_names[k]) as u64
    });

    let classes = [
        "container",
        "item-title",
        "btn btn-primary",
        "x",
        "sidebar-nav-link",
    ];
    let static_atoms = ["div", "class", "id", "href"];
    let dynamic_from = ns_per_op(N / 4, |i| {
        let atom = Atom::from(classes[(i as usize) % classes.len()]);
        atom.len() as u64
    });
    let static_from = ns_per_op(N / 4, |i| {
        let atom = Atom::from(static_atoms[(i as usize) % static_atoms.len()]);
        atom.len() as u64
    });
    let class_prebuilt: Vec<Atom> = classes.iter().map(|c| Atom::from(*c)).collect();
    let dynamic_clone = ns_per_op(N / 4, |i| {
        let atom = class_prebuilt[(i as usize) % classes.len()].clone();
        atom.len() as u64
    });
    let class_str_compare = ns_per_op(N, |i| {
        let k = (i as usize) % classes.len();
        CaseSensitivity::CaseSensitive.eq(classes[k].as_bytes(), class_prebuilt[k].as_bytes())
            as u64
    });

    println!(
        "-- tenta Atom -> style atom mapping (release={}, {N} iterations, black_box) --",
        !cfg!(debug_assertions)
    );
    println!("raw tenta atom == tenta atom (baseline)        {raw_compare:7.2} ns");
    println!("tenta as_str() only (RwLock read + index)      {as_str_only:7.2} ns");
    println!("as_str() + string compare with style name      {as_str_compare:7.2} ns");
    println!("naive map  LocalName::from(as_str()) + eq      {naive_map:7.2} ns");
    println!("table map  Vec<OnceCell<LocalName>>[raw] + eq  {table_map:7.2} ns");
    println!("style Atom::from(dynamic class string)         {dynamic_from:7.2} ns");
    println!("style Atom::from(static atom string)           {static_from:7.2} ns");
    println!("style dynamic Atom clone                       {dynamic_clone:7.2} ns");
    println!("class string compare (has_class path)          {class_str_compare:7.2} ns");
}

fn large_html(elements: usize) -> String {
    let mut html = String::from("<!DOCTYPE html><html><head></head><body>");
    for i in 0..elements / 2 {
        html.push_str(&format!(
            "<div class=\"card-container-{} item-title-link\" id=\"n{i}\"><p class=\"body-paragraph-text\" data-k=\"{}\">text</p></div>",
            i % 20,
            i % 7
        ));
    }
    html.push_str("</body></html>");
    html
}

fn large_author_css() -> String {
    let mut css = String::new();
    for k in 0..20 {
        css.push_str(&format!(
            ".card-container-{k} > p.body-paragraph-text {{ color: rgb({}, 10, 10) }}\n",
            k * 10
        ));
        css.push_str(&format!("div.card-container-{k} {{ margin-top: {k}px }}\n"));
    }
    css.push_str("body div p[data-k=\"3\"] { color: blue }\n#n7 { display: inline }\n");
    css
}

fn main() {
    // Same preferences the CSSOM layer sets before any parse.
    stylo_static_prefs::set_pref!("layout.grid.enabled", true);
    stylo_static_prefs::set_pref!("layout.writing-mode.enabled", true);

    let ua_css = "div, p, body, html { display: block } head { display: none }";
    let author_css = "body > div.x { color: rgb(0, 128, 0) }";

    let doc = parse_document(
        "<!DOCTYPE html><html><head></head><body>\
         <div id=a class=x><p class=y>hi</p></div></body></html>",
    );
    let (lines, timings, _) = style_document(&doc, true, ua_css, author_css, |_, root| {
        let div = find_by_tag(root, atoms::DIV);
        let p = find_by_tag(root, atoms::P);
        vec![describe(div), describe(p)]
    });
    for line in lines {
        println!("{line}");
    }
    println!(
        "5-element doc: ctx build {:.1} us, stylist flush {:.1} us, traversal {:.1} us",
        timings.context_build_us, timings.stylist_flush_us, timings.traversal_us
    );

    measure_atom_mapping();

    let big = parse_document(&large_html(4000));
    let elements = big
        .descendants(big.document_node())
        .filter(|n| big.is_element(*n))
        .count();
    let author = large_author_css();
    println!("-- {elements}-element document, 42 author rules, sequential --");
    for (label, cache_classes) in [
        ("class atoms per call", false),
        ("class atoms cached", true),
    ] {
        // One warm-up run (first use of dynamic atoms, allocator), then the measured run.
        let _ = style_document(&big, cache_classes, ua_css, &author, |_, _| ());
        let (_, t, c) = style_document(&big, cache_classes, ua_css, &author, |_, _| ());
        println!(
            "[{label}] ctx build {:.0} us, flush {:.0} us, traversal {:.0} us ({:.2} us/element)",
            t.context_build_us,
            t.stylist_flush_us,
            t.traversal_us,
            t.traversal_us / elements as f64
        );
        println!(
            "  hook calls: local_name {}, has_local_name {}, has_class/has_id {}, each_class {}, id {}, attr_matches {}",
            c.local_name.get(),
            c.has_local_name.get(),
            c.has_class.get(),
            c.each_class.get(),
            c.id.get(),
            c.attr_matches.get()
        );
    }
}
