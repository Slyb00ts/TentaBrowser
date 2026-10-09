//! `selectors::Element`: what selector matching asks of an element.

use selectors::OpaqueElement;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use style::dom::{TElement, TNode};
use style::selector_parser::{AttrValue, NonTSPseudoClass, PseudoElement, SelectorImpl};
use style::values::AtomIdent;
use style::{LocalName, Namespace};
use tenta_core::atom::atoms;
use tenta_dom::NodeKind;

use super::{Handle, state};

type BorrowedLocalName = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedLocalName;
type BorrowedNamespace = <SelectorImpl as selectors::parser::SelectorImpl>::BorrowedNamespaceUrl;

impl<'a> selectors::Element for Handle<'a> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // Stable within one flush, which is as long as selector caches keep
        // it: the store may grow and move its entries between flushes.
        OpaqueElement::new(&self.0.entry.data)
    }

    fn parent_element(&self) -> Option<Self> {
        self.as_node().traversal_parent()
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
        self.is_html_element() && self.0.view.doc.is_html_document()
    }

    fn has_local_name(&self, name: &BorrowedLocalName) -> bool {
        *TElement::local_name(self) == *name
    }

    fn has_namespace(&self, namespace: &BorrowedNamespace) -> bool {
        *TElement::namespace(self) == *namespace
    }

    fn is_same_type(&self, other: &Self) -> bool {
        let doc = self.0.view.doc;
        doc.tag(self.0.node) == doc.tag(other.0.node)
            && doc.namespace(self.0.node) == doc.namespace(other.0.node)
    }

    fn attr_matches(
        &self,
        namespace: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&AttrValue>,
    ) -> bool {
        let view = self.0.view;
        view.doc.attributes(self.0.node).any(|(name, value)| {
            // The selector's name already carries the HTML-document case
            // folding; attribute names are stored as written.
            view.atoms.local_name(name.local) == local_name
                && match namespace {
                    NamespaceConstraint::Any => true,
                    NamespaceConstraint::Specific(wanted) => {
                        view.atoms.namespace(name.ns) == *wanted
                    }
                }
                && operation.eval_str(value)
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo_class: &NonTSPseudoClass,
        _: &mut MatchingContext<SelectorImpl>,
    ) -> bool {
        let doc = self.0.view.doc;
        match pseudo_class {
            NonTSPseudoClass::Link | NonTSPseudoClass::AnyLink => self.is_link(),
            NonTSPseudoClass::Lang(range) => self.match_element_lang(None, range),
            NonTSPseudoClass::ServoNonZeroBorder => state::has_nonzero_border(doc, self.0.node),
            NonTSPseudoClass::CustomState(_) => false,
            other => self.state().contains(other.state_flag()),
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
        // Matching reports every flag against the element it was matching;
        // the ones that describe sibling structure belong to the parent.
        let record = |entry: &super::StoreEntry, flags: ElementSelectorFlags| {
            entry
                .selector_flags
                .set(entry.selector_flags.get() | flags.bits());
        };
        let own = flags.for_self();
        if !own.is_empty() {
            record(self.0.entry, own);
        }
        let inherited = flags.for_parent();
        if !inherited.is_empty()
            && let Some(parent) = self.parent_element()
        {
            record(parent.0.entry, inherited);
        }
    }

    fn is_link(&self) -> bool {
        state::is_link(self.0.view.doc, self.0.node)
    }

    fn is_html_slot_element(&self) -> bool {
        self.is_html_element() && self.0.view.doc.tag(self.0.node) == Some(atoms::SLOT)
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.0
            .view
            .doc
            .element_id(self.0.node)
            .is_some_and(|actual| case_sensitivity.eq(actual.as_bytes(), id.as_bytes()))
    }

    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.0
            .view
            .doc
            .classes(self.0.node)
            .any(|class| case_sensitivity.eq(class.as_bytes(), name.as_bytes()))
    }

    fn has_custom_state(&self, _: &AtomIdent) -> bool {
        false
    }

    fn imported_part(&self, _: &AtomIdent) -> Option<AtomIdent> {
        None
    }

    fn is_part(&self, name: &AtomIdent) -> bool {
        self.0
            .view
            .doc
            .attribute(self.0.node, *state::PART)
            .is_some_and(|parts| {
                parts
                    .split_ascii_whitespace()
                    .any(|part| part.as_bytes() == name.as_bytes())
            })
    }

    fn is_empty(&self) -> bool {
        let doc = self.0.view.doc;
        !doc.children(self.0.node)
            .any(|child| match doc.kind(child) {
                NodeKind::Element => true,
                NodeKind::Text | NodeKind::CData => !doc.text_data(child).unwrap_or("").is_empty(),
                _ => false,
            })
    }

    fn is_root(&self) -> bool {
        let doc = self.0.view.doc;
        let parent = doc.parent(self.0.node);
        parent.is_some() && doc.kind(parent) == NodeKind::Document
    }

    fn add_element_unique_hashes(&self, _: &mut BloomFilter) -> bool {
        false
    }
}
