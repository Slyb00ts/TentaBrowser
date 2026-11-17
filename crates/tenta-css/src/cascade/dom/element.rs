//! `TElement`: what Stylo's traversal and cascade ask of an element.

use selectors::matching::{ElementSelectorFlags, VisitedHandlingMode};
use selectors::sink::Push;
use servo_arc::{Arc, ArcBorrow};
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::context::SharedStyleContext;
use style::data::{ElementData, ElementDataMut, ElementDataRef};
use style::dom::{LayoutIterator, TElement, TNode};
use style::properties::PropertyDeclarationBlock;
use style::selector_parser::{AttrValue, Lang, PseudoElement, SelectorImpl};
use style::shared_lock::Locked;
use style::values::AtomIdent;
use style::values::computed::Au;
use style::{Atom, LocalName, Namespace};
use stylo_dom::ElementState;
use tenta_core::atom::atoms;

use super::node::{ChildNodes, NodeHandle, ShadowRootHandle};
use super::{Handle, state};

impl<'a> Handle<'a> {
    fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(self.0.entry.selector_flags.get())
    }

    fn is_in_namespace(&self, namespace: tenta_core::Atom) -> bool {
        self.0.view.doc.namespace(self.0.node) == Some(namespace)
    }
}

// The five `unsafe fn`s below are forced by the trait. Their bodies are safe
// code over `Cell`s and `ElementDataWrapper`; what makes the calls sound is the
// contract Stylo documents for them, which the sequential traversal keeps:
// no borrow of an element's data is held across `ensure_data`/`clear_data`
// of that same element. `ElementDataWrapper` checks that with an atomic
// borrow flag in debug builds only, so the traversal tests run with debug
// assertions on.
impl<'a> Handle<'a> {
    /// The shadow root this element hosts.
    #[inline(never)]
    fn shadow_root_of_host(self) -> Option<ShadowRootHandle<'a>> {
        let root = self.0.view.doc.shadow_root(self.0.node)?;
        self.0.slot_of(root).map(ShadowRootHandle)
    }

    /// The shadow root whose tree this element is in.
    #[inline(never)]
    fn shadow_root_around(self) -> Option<ShadowRootHandle<'a>> {
        let root = self.0.view.doc.containing_shadow_root(self.0.node)?;
        self.0.slot_of(root).map(ShadowRootHandle)
    }

    /// Computes the element's state flags and keeps them for this
    /// generation of the document.
    #[inline(never)]
    fn compute_state(self, generation: u64) -> ElementState {
        let state = state::element_state(self.0.view.doc, self.0.node);
        self.0.entry.state.set(Some((generation, state.bits())));
        state
    }

    /// The slot this element is assigned to.
    #[inline(never)]
    pub(super) fn slot_assigned_to(self) -> Option<Self> {
        let slot = self.0.view.doc.assigned_slot(self.0.node)?;
        self.0.slot_of(slot).map(Handle)
    }
}

impl<'a> TElement for Handle<'a> {
    type ConcreteNode = NodeHandle<'a>;
    type TraversalChildrenIterator = ChildNodes<'a>;

    fn as_node(&self) -> NodeHandle<'a> {
        NodeHandle(self.0)
    }

    fn traversal_children(&self) -> LayoutIterator<ChildNodes<'a>> {
        LayoutIterator(self.as_node().flat_children())
    }

    // Inheritance follows the flat tree too: a slotted node inherits from its
    // slot, and the children of a shadow root from the host.
    #[inline]
    fn inheritance_parent(&self) -> Option<Self> {
        self.as_node().traversal_parent()
    }

    fn is_html_element(&self) -> bool {
        self.is_in_namespace(atoms::NS_HTML)
    }

    fn is_mathml_element(&self) -> bool {
        self.is_in_namespace(atoms::NS_MATHML)
    }

    fn is_svg_element(&self) -> bool {
        self.is_in_namespace(atoms::NS_SVG)
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        Handle::style_attribute(self)
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

    #[inline]
    fn state(&self) -> ElementState {
        let generation = self.0.view.generation;
        if let Some((at, bits)) = self.0.entry.state.get()
            && at == generation
        {
            return ElementState::from_bits_retain(bits);
        }
        self.compute_state(generation)
    }

    fn has_part_attr(&self) -> bool {
        self.0.view.doc.has_attribute(self.0.node, *state::PART)
    }

    fn exports_any_part(&self) -> bool {
        self.0
            .view
            .doc
            .has_attribute(self.0.node, *state::EXPORTPARTS)
    }

    fn id(&self) -> Option<&Atom> {
        let doc = self.0.view.doc;
        self.0.view.atoms.id(self.0.node.raw() as usize, || {
            doc.element_id(self.0.node).map(Atom::from)
        })
    }

    // Each call interns every class through Stylo's global atom set. The cost
    // has not been measured on a representative page; benchmark before
    // caching or making performance claims.
    fn each_class<F: FnMut(&AtomIdent)>(&self, mut callback: F) {
        for class in self.0.view.doc.classes(self.0.node) {
            callback(&AtomIdent::new(Atom::from(class)));
        }
    }

    fn each_custom_state<F: FnMut(&AtomIdent)>(&self, _callback: F) {}

    fn each_attr_name<F: FnMut(&LocalName)>(&self, mut callback: F) {
        let view = self.0.view;
        for (name, _) in view.doc.attributes(self.0.node) {
            callback(view.atoms.local_name(name.local));
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        self.0.entry.dirty_descendants.get()
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {}

    unsafe fn set_dirty_descendants(&self) {
        self.0.entry.dirty_descendants.set(true);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.0.entry.dirty_descendants.set(false);
    }

    // The bottom-up count is only read by the postorder step, and the
    // parallel pool is the only caller that reaches it for a traversal whose
    // `needs_postorder_traversal()` is false. Both hold for the sequential
    // cascade traversal, which passes no pool.
    fn store_children_to_process(&self, _n: isize) {}

    fn did_process_child(&self) -> isize {
        unreachable!("only reachable when needs_postorder_traversal() is true")
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        self.0.entry.has_data.set(true);
        self.0.entry.data.borrow_mut()
    }

    unsafe fn clear_data(&self) {
        if self.0.entry.has_data.replace(false) {
            *self.0.entry.data.borrow_mut() = ElementData::default();
        }
    }

    fn has_data(&self) -> bool {
        self.0.entry.has_data.get()
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.has_data().then(|| self.0.entry.data.borrow())
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        self.has_data().then(|| self.0.entry.data.borrow_mut())
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

    // The style-sharing cache asks the next two, and `assigned_slot`, of
    // every candidate it tests. A document without shadow trees answers from
    // one flag, inline; the walks are kept out of line.
    #[inline]
    fn shadow_root(&self) -> Option<ShadowRootHandle<'a>> {
        if !self.0.view.shadow_trees {
            return None;
        }
        self.shadow_root_of_host()
    }

    #[inline]
    fn containing_shadow(&self) -> Option<ShadowRootHandle<'a>> {
        if !self.0.view.shadow_trees {
            return None;
        }
        self.shadow_root_around()
    }

    fn lang_attr(&self) -> Option<AttrValue> {
        state::declared_language(self.0.view.doc, self.0.node).map(AttrValue::from)
    }

    fn match_element_lang(&self, override_lang: Option<Option<AttrValue>>, value: &Lang) -> bool {
        let declared = match override_lang {
            Some(lang) => lang,
            None => self.lang_attr(),
        };
        declared.is_some_and(|lang| state::language_matches(&lang, value))
    }

    fn is_html_document_body_element(&self) -> bool {
        let doc = self.0.view.doc;
        self.is_html_element()
            && doc.tag(self.0.node) == Some(atoms::BODY)
            && doc.body() == Some(self.0.node)
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        visited: VisitedHandlingMode,
        hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
        self.synthesize_hints(visited, hints);
    }

    fn local_name(&self) -> &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName {
        let view = self.0.view;
        view.atoms.local_name(
            view.doc
                .tag(self.0.node)
                .expect("TElement is only implemented for element nodes"),
        )
    }

    fn namespace(
        &self,
    ) -> &<SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl {
        let view = self.0.view;
        let namespace = view.doc.namespace(self.0.node).unwrap_or(atoms::EMPTY);
        view.atoms.namespace(namespace)
    }

    // Size container queries therefore never match in M1: every `@container`
    // size condition evaluates false.
    fn query_container_size(
        &self,
        _: &style::values::computed::Display,
    ) -> euclid::default::Size2D<Option<Au>> {
        Default::default()
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.selector_flags().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        self.selector_flags()
            .intersection(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING)
    }

    fn get_attr(&self, attr: &LocalName, namespace: &Namespace) -> Option<String> {
        let doc = self.0.view.doc;
        let local = tenta_core::Atom::get(attr)?;
        let namespace = tenta_core::Atom::get(namespace)?;
        doc.attribute_ns(self.0.node, namespace, local)
            .map(str::to_owned)
    }
}
