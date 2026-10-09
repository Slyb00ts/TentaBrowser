//! Selector matching over the arena with Servo's `selectors` engine
//! (`docs/spec/04-dom-and-css.md` §4).
//!
//! The engine is generic over a [`selectors::SelectorImpl`]; this module
//! supplies one whose names are [`Atom`]s, and an [`selectors::Element`] view
//! that is a `(&Document, NodeId)` pair. Compiled selector lists are cached
//! per document in a small LRU, and `query_selector*` narrows the candidate
//! set through the id, class and tag indexes `Document::mutate` maintains
//! before running the matcher.

use std::fmt;

use ::selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use ::selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use ::selectors::matching::{ElementSelectorFlags, matches_selector_list};
use ::selectors::parser::{
    Component, NonTSPseudoClass as NonTSPseudoClassTrait, ParseRelative,
    PseudoElement as PseudoElementTrait, Selector, SelectorParseErrorKind,
};
use ::selectors::{Element as SelectorsElement, OpaqueElement, SelectorImpl, SelectorList};
use cssparser::{CowRcStr, ParseError, Parser as CssParser, ParserInput, SourceLocation, ToCss};
use precomputed_hash::PrecomputedHash;
use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId, Result};

use crate::arena::NodeKind;
use crate::document::Document;
use crate::error::syntax;

/// FNV-1a over the bytes of a name; `selectors` wants a precomputed hash for
/// the ancestor-hash filter. Atom handles are dense small integers and would
/// make a poor hash, so the string is hashed once at construction.
fn name_hash(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in s.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

macro_rules! atom_name {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Debug)]
        pub struct $name {
            pub atom: Atom,
            hash: u32,
        }

        impl $name {
            pub fn new(atom: Atom) -> Self {
                Self {
                    hash: name_hash(atom.as_str()),
                    atom,
                }
            }

            #[inline]
            pub fn as_str(&self) -> &str {
                self.atom.as_str()
            }
        }

        impl<'a> From<&'a str> for $name {
            fn from(s: &'a str) -> Self {
                Self::new(Atom::new(s))
            }
        }

        impl PrecomputedHash for $name {
            #[inline]
            fn precomputed_hash(&self) -> u32 {
                self.hash
            }
        }

        impl ToCss for $name {
            fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
                cssparser::serialize_identifier(self.atom.as_str(), dest)
            }
        }
    };
}

atom_name!(
    /// An element or attribute local name in a selector.
    CssLocalName
);
atom_name!(
    /// A namespace URL in a selector.
    CssNamespace
);
atom_name!(
    /// A namespace prefix in a selector.
    CssPrefix
);

/// An identifier in a selector: the argument of `#id`, `.class`, `::part()`.
///
/// Owned, not interned: these are compared against ids and classes, which are
/// document-local strings, and a selector string can come from the page as
/// easily as from the embedder (`docs/spec/04-dom-and-css.md` §2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CssIdent {
    text: Box<str>,
    hash: u32,
}

impl CssIdent {
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl<'a> From<&'a str> for CssIdent {
    fn from(s: &'a str) -> Self {
        Self {
            hash: name_hash(s),
            text: s.into(),
        }
    }
}

impl PrecomputedHash for CssIdent {
    #[inline]
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

impl ToCss for CssIdent {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_identifier(&self.text, dest)
    }
}

impl Default for CssNamespace {
    fn default() -> Self {
        Self::new(atoms::EMPTY)
    }
}

impl Default for CssPrefix {
    fn default() -> Self {
        Self::new(atoms::EMPTY)
    }
}

/// An attribute value in a selector. Values are not interned: they come from
/// untrusted markup and must not grow the atom table
/// (`docs/spec/02-architecture.md` §7).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CssAttrValue(pub String);

impl<'a> From<&'a str> for CssAttrValue {
    fn from(s: &'a str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for CssAttrValue {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl ToCss for CssAttrValue {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_string(&self.0, dest)
    }
}

/// The non-tree-structural pseudo-classes Tier 1 understands.
///
/// `:hover`, `:active`, `:focus`, `:focus-visible` and `:focus-within` parse
/// but never match: Tier 1 has no user interaction and no focus
/// (`docs/spec/04-dom-and-css.md` §4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PseudoClass {
    Active,
    AnyLink,
    Checked,
    Default,
    Defined,
    Disabled,
    Enabled,
    Focus,
    FocusVisible,
    FocusWithin,
    Hover,
    Indeterminate,
    Lang(String),
    Link,
    Optional,
    PlaceholderShown,
    ReadOnly,
    ReadWrite,
    Required,
    Target,
    Visited,
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        let name = match self {
            PseudoClass::Active => ":active",
            PseudoClass::AnyLink => ":any-link",
            PseudoClass::Checked => ":checked",
            PseudoClass::Default => ":default",
            PseudoClass::Defined => ":defined",
            PseudoClass::Disabled => ":disabled",
            PseudoClass::Enabled => ":enabled",
            PseudoClass::Focus => ":focus",
            PseudoClass::FocusVisible => ":focus-visible",
            PseudoClass::FocusWithin => ":focus-within",
            PseudoClass::Hover => ":hover",
            PseudoClass::Indeterminate => ":indeterminate",
            PseudoClass::Lang(lang) => return write!(dest, ":lang({lang})"),
            PseudoClass::Link => ":link",
            PseudoClass::Optional => ":optional",
            PseudoClass::PlaceholderShown => ":placeholder-shown",
            PseudoClass::ReadOnly => ":read-only",
            PseudoClass::ReadWrite => ":read-write",
            PseudoClass::Required => ":required",
            PseudoClass::Target => ":target",
            PseudoClass::Visited => ":visited",
        };
        dest.write_str(name)
    }
}

impl NonTSPseudoClassTrait for PseudoClass {
    type Impl = TentaSelectorImpl;

    fn is_active_or_hover(&self) -> bool {
        matches!(self, PseudoClass::Active | PseudoClass::Hover)
    }

    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            PseudoClass::Active
                | PseudoClass::Hover
                | PseudoClass::Focus
                | PseudoClass::FocusVisible
                | PseudoClass::FocusWithin
        )
    }
}

/// Pseudo-elements parse but never match at Tier 1: they need style.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PseudoElement {
    Before,
    After,
    FirstLine,
    FirstLetter,
    Marker,
    Selection,
    Placeholder,
}

impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(match self {
            PseudoElement::Before => "::before",
            PseudoElement::After => "::after",
            PseudoElement::FirstLine => "::first-line",
            PseudoElement::FirstLetter => "::first-letter",
            PseudoElement::Marker => "::marker",
            PseudoElement::Selection => "::selection",
            PseudoElement::Placeholder => "::placeholder",
        })
    }
}

impl PseudoElementTrait for PseudoElement {
    type Impl = TentaSelectorImpl;
}

/// The `SelectorImpl` binding the engine to the arena's atoms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TentaSelectorImpl;

impl SelectorImpl for TentaSelectorImpl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssAttrValue;
    type Identifier = CssIdent;
    type LocalName = CssLocalName;
    type NamespaceUrl = CssNamespace;
    type NamespacePrefix = CssPrefix;
    type BorrowedNamespaceUrl = CssNamespace;
    type BorrowedLocalName = CssLocalName;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
}

/// The selector parser: enables the Selectors 4 features Tier 1 supports.
pub struct SelectorParser;

impl<'i> selectors::parser::Parser<'i> for SelectorParser {
    type Impl = TentaSelectorImpl;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_has(&self) -> bool {
        true
    }

    fn parse_nth_child_of(&self) -> bool {
        true
    }

    fn parse_part(&self) -> bool {
        true
    }

    fn parse_non_ts_pseudo_class(
        &self,
        location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> std::result::Result<PseudoClass, ParseError<'i, Self::Error>> {
        let class = match_ignore_ascii_case(&name);
        class.ok_or_else(|| {
            location.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            ))
        })
    }

    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: CowRcStr<'i>,
        parser: &mut CssParser<'i, 't>,
        _after_part: bool,
    ) -> std::result::Result<PseudoClass, ParseError<'i, Self::Error>> {
        if name.eq_ignore_ascii_case("lang") {
            let lang = parser.expect_ident_or_string()?.as_ref().to_owned();
            return Ok(PseudoClass::Lang(lang));
        }
        Err(
            parser.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            )),
        )
    }

    fn parse_pseudo_element(
        &self,
        location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> std::result::Result<PseudoElement, ParseError<'i, Self::Error>> {
        let element = if name.eq_ignore_ascii_case("before") {
            PseudoElement::Before
        } else if name.eq_ignore_ascii_case("after") {
            PseudoElement::After
        } else if name.eq_ignore_ascii_case("first-line") {
            PseudoElement::FirstLine
        } else if name.eq_ignore_ascii_case("first-letter") {
            PseudoElement::FirstLetter
        } else if name.eq_ignore_ascii_case("marker") {
            PseudoElement::Marker
        } else if name.eq_ignore_ascii_case("selection") {
            PseudoElement::Selection
        } else if name.eq_ignore_ascii_case("placeholder") {
            PseudoElement::Placeholder
        } else {
            return Err(location.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
            ));
        };
        Ok(element)
    }
}

fn match_ignore_ascii_case(name: &str) -> Option<PseudoClass> {
    let class = if name.eq_ignore_ascii_case("active") {
        PseudoClass::Active
    } else if name.eq_ignore_ascii_case("any-link") {
        PseudoClass::AnyLink
    } else if name.eq_ignore_ascii_case("checked") {
        PseudoClass::Checked
    } else if name.eq_ignore_ascii_case("default") {
        PseudoClass::Default
    } else if name.eq_ignore_ascii_case("defined") {
        PseudoClass::Defined
    } else if name.eq_ignore_ascii_case("disabled") {
        PseudoClass::Disabled
    } else if name.eq_ignore_ascii_case("enabled") {
        PseudoClass::Enabled
    } else if name.eq_ignore_ascii_case("focus") {
        PseudoClass::Focus
    } else if name.eq_ignore_ascii_case("focus-visible") {
        PseudoClass::FocusVisible
    } else if name.eq_ignore_ascii_case("focus-within") {
        PseudoClass::FocusWithin
    } else if name.eq_ignore_ascii_case("hover") {
        PseudoClass::Hover
    } else if name.eq_ignore_ascii_case("indeterminate") {
        PseudoClass::Indeterminate
    } else if name.eq_ignore_ascii_case("link") {
        PseudoClass::Link
    } else if name.eq_ignore_ascii_case("optional") {
        PseudoClass::Optional
    } else if name.eq_ignore_ascii_case("placeholder-shown") {
        PseudoClass::PlaceholderShown
    } else if name.eq_ignore_ascii_case("read-only") {
        PseudoClass::ReadOnly
    } else if name.eq_ignore_ascii_case("read-write") {
        PseudoClass::ReadWrite
    } else if name.eq_ignore_ascii_case("required") {
        PseudoClass::Required
    } else if name.eq_ignore_ascii_case("target") {
        PseudoClass::Target
    } else if name.eq_ignore_ascii_case("visited") {
        PseudoClass::Visited
    } else {
        return None;
    };
    Some(class)
}

/// A `(&Document, NodeId)` view of an element, as the matching engine wants
/// it. Cheap to copy: two words.
#[derive(Clone, Copy)]
pub struct ElementRef<'a> {
    pub document: &'a Document,
    pub id: NodeId,
}

impl<'a> ElementRef<'a> {
    pub fn new(document: &'a Document, id: NodeId) -> Self {
        Self { document, id }
    }

    fn with(&self, id: NodeId) -> Self {
        Self {
            document: self.document,
            id,
        }
    }

    fn is_html(&self) -> bool {
        self.document.namespace(self.id) == Some(atoms::NS_HTML)
    }
}

impl fmt::Debug for ElementRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<{} #{:?}>",
            self.document
                .tag(self.id)
                .map(|t| t.as_str())
                .unwrap_or("?"),
            self.id.raw()
        )
    }
}

impl<'a> SelectorsElement for ElementRef<'a> {
    type Impl = TentaSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // A unique, stable address per node: its own slot in the kind column.
        OpaqueElement::new(&self.document.arena().nodes.kind[self.id.raw() as usize])
    }

    fn parent_element(&self) -> Option<Self> {
        let parent = self.document.parent(self.id);
        (parent.is_some() && self.document.is_element(parent)).then(|| self.with(parent))
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        let parent = self.document.parent(self.id);
        parent.is_some() && self.document.kind(parent) == NodeKind::ShadowRoot
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut sibling = self.document.previous_sibling(self.id);
        let cap = self.document.slots();
        for _ in 0..cap {
            if !sibling.is_some() {
                return None;
            }
            if self.document.is_element(sibling) {
                return Some(self.with(sibling));
            }
            sibling = self.document.previous_sibling(sibling);
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut sibling = self.document.next_sibling(self.id);
        let cap = self.document.slots();
        for _ in 0..cap {
            if !sibling.is_some() {
                return None;
            }
            if self.document.is_element(sibling) {
                return Some(self.with(sibling));
            }
            sibling = self.document.next_sibling(sibling);
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        self.document
            .element_children(self.id)
            .next()
            .map(|child| self.with(child))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.is_html() && self.document.is_html_document()
    }

    fn has_local_name(&self, name: &CssLocalName) -> bool {
        self.document.tag(self.id) == Some(name.atom)
    }

    fn has_namespace(&self, ns: &CssNamespace) -> bool {
        self.document.namespace(self.id) == Some(ns.atom)
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.document.tag(self.id) == other.document.tag(other.id)
            && self.document.namespace(self.id) == other.document.namespace(other.id)
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&CssNamespace>,
        local_name: &CssLocalName,
        operation: &AttrSelectorOperation<&CssAttrValue>,
    ) -> bool {
        match ns {
            NamespaceConstraint::Any => self
                .document
                .attributes(self.id)
                .any(|(name, value)| name.local == local_name.atom && operation.eval_str(value)),
            NamespaceConstraint::Specific(url) => self
                .document
                .attribute_ns(self.id, url.atom, local_name.atom)
                .is_some_and(|value| operation.eval_str(value)),
        }
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &PseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        let document = self.document;
        let tag = document.tag(self.id);
        match pc {
            // No user interaction, no focus and no history at Tier 1.
            PseudoClass::Active
            | PseudoClass::Focus
            | PseudoClass::FocusVisible
            | PseudoClass::FocusWithin
            | PseudoClass::Hover
            | PseudoClass::Target
            | PseudoClass::Visited => false,
            PseudoClass::Link | PseudoClass::AnyLink => self.is_link(),
            PseudoClass::Checked => is_checked(document, self.id),
            PseudoClass::Disabled => is_disabled(document, self.id),
            PseudoClass::Enabled => is_enabled(document, self.id),
            PseudoClass::Required => {
                matches!(tag, Some(t) if t == atoms::INPUT || t == atoms::SELECT || t == atoms::TEXTAREA)
                    && document.attribute_str(self.id, "required").is_some()
            }
            PseudoClass::Optional => {
                matches!(tag, Some(t) if t == atoms::INPUT || t == atoms::SELECT || t == atoms::TEXTAREA)
                    && document.attribute_str(self.id, "required").is_none()
            }
            PseudoClass::ReadWrite => is_read_write(document, self.id),
            PseudoClass::ReadOnly => !is_read_write(document, self.id),
            PseudoClass::Indeterminate => {
                document.attribute_str(self.id, "indeterminate").is_some()
            }
            PseudoClass::PlaceholderShown => is_placeholder_shown(document, self.id),
            PseudoClass::Default => {
                match tag {
                    // option with selected attribute
                    Some(t) if t == atoms::OPTION => {
                        document.has_attribute(self.id, atoms::SELECTED)
                    }
                    // checked checkbox or radio button
                    Some(t) if t == atoms::INPUT => {
                        let input_type = document
                            .attribute(self.id, atoms::TYPE)
                            .map(|t| t.trim().to_ascii_lowercase())
                            .unwrap_or_else(|| "text".to_owned());
                        matches!(input_type.as_str(), "checkbox" | "radio")
                            && document.has_attribute(self.id, atoms::CHECKED)
                    }
                    // submit/reset/image button (first of its type in the form)
                    // P1 doesn't have full form association, so we don't implement this
                    _ => false,
                }
            }
            // Every built-in element is "defined"; custom elements are P2.
            PseudoClass::Defined => true,
            PseudoClass::Lang(expected) => std::iter::once(self.id)
                .chain(document.ancestors(self.id))
                .find_map(|node| document.attribute(node, atoms::LANG))
                .is_some_and(|lang| lang_matches(lang, expected)),
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // Selector flags drive style invalidation; Tier 1 has no cascade and
        // matches with NeedsSelectorFlags::No, so nothing is recorded.
    }

    fn is_link(&self) -> bool {
        is_link(self.document, self.id)
    }

    fn is_html_slot_element(&self) -> bool {
        self.is_html() && self.document.tag(self.id) == Some(atoms::SLOT)
    }

    fn has_id(&self, id: &CssIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.document
            .element_id(self.id)
            .is_some_and(|actual| case_sensitivity.eq(actual.as_bytes(), id.as_str().as_bytes()))
    }

    fn has_class(&self, name: &CssIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.document
            .classes(self.id)
            .any(|class| case_sensitivity.eq(class.as_bytes(), name.as_str().as_bytes()))
    }

    fn has_custom_state(&self, _name: &CssIdent) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssIdent) -> Option<CssIdent> {
        None
    }

    fn is_part(&self, name: &CssIdent) -> bool {
        self.document
            .attribute_str(self.id, "part")
            .is_some_and(|parts| {
                parts
                    .split_ascii_whitespace()
                    .any(|part| part == name.as_str())
            })
    }

    fn is_empty(&self) -> bool {
        !self
            .document
            .children(self.id)
            .any(|child| match self.document.kind(child) {
                NodeKind::Element => true,
                NodeKind::Text | NodeKind::CData => {
                    !self.document.text_data(child).unwrap_or("").is_empty()
                }
                _ => false,
            })
    }

    fn is_root(&self) -> bool {
        let parent = self.document.parent(self.id);
        parent.is_some() && self.document.kind(parent) == NodeKind::Document
    }

    fn add_element_unique_hashes(&self, _filter: &mut ::selectors::bloom::BloomFilter) -> bool {
        // Tier 1 matches one element at a time and never builds an ancestor
        // bloom filter; Stylo's parallel traversal will (Tier 3).
        false
    }
}

fn lang_matches(actual: &str, expected: &str) -> bool {
    actual.eq_ignore_ascii_case(expected)
        || (actual.len() > expected.len()
            && actual.as_bytes()[expected.len()] == b'-'
            && actual[..expected.len()].eq_ignore_ascii_case(expected))
}

/// Form-associated elements: input, button, select, textarea, fieldset.
fn is_form_control(document: &Document, id: NodeId) -> bool {
    matches!(
        document.tag(id),
        Some(t)
            if t == atoms::INPUT
                || t == atoms::BUTTON
                || t == atoms::SELECT
                || t == atoms::TEXTAREA
                || t == atoms::FIELDSET
    )
}

/// `:link` and `:any-link`: `a` and `area` elements with an `href`. No
/// history exists, so nothing is ever visited.
pub fn is_link(document: &Document, id: NodeId) -> bool {
    matches!(document.tag(id), Some(t) if t == atoms::A || t == atoms::AREA)
        && document.has_attribute(id, atoms::HREF)
}

/// `:enabled`: the elements that can be disabled, when they are not.
pub fn is_enabled(document: &Document, id: NodeId) -> bool {
    let can_be_disabled = is_form_control(document, id)
        || matches!(document.tag(id), Some(t) if t == atoms::OPTION || t == atoms::OPTGROUP);
    can_be_disabled && !is_disabled(document, id)
}

/// `:checked`. A script-set checkedness wins over the `checked` attribute.
/// Selectedness of `option` is read from the `selected` attribute only; the
/// document does not track a dirty selectedness.
pub fn is_checked(document: &Document, id: NodeId) -> bool {
    match document.tag(id) {
        Some(t) if t == atoms::INPUT => document
            .dirty_checkedness(id)
            .unwrap_or_else(|| document.has_attribute(id, atoms::CHECKED)),
        Some(t) if t == atoms::OPTION => document.has_attribute(id, atoms::SELECTED),
        _ => false,
    }
}

const INPUT_TYPES: &[&str] = &[
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

/// Input types on which `readonly` applies.
const READONLY_TYPES: &[&str] = &[
    "text",
    "search",
    "url",
    "tel",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
];

/// Input types on which `placeholder` applies.
const PLACEHOLDER_TYPES: &[&str] = &[
    "text", "search", "url", "tel", "email", "password", "number",
];

/// Whether the input's type state, after the missing and invalid value
/// default of `text`, is one of `types`.
fn input_type_in(document: &Document, id: NodeId, types: &[&str]) -> bool {
    let value = document.attribute(id, atoms::TYPE).unwrap_or("");
    let state = INPUT_TYPES
        .iter()
        .find(|known| value.eq_ignore_ascii_case(known))
        .copied()
        .unwrap_or("text");
    types.contains(&state)
}

fn is_text_control(document: &Document, id: NodeId, types: &[&str]) -> Option<bool> {
    match document.tag(id) {
        Some(t) if t == atoms::INPUT => Some(input_type_in(document, id, types)),
        Some(t) if t == atoms::TEXTAREA => Some(true),
        _ => None,
    }
}

/// `:read-write`: a mutable text control, or an editing host. `:read-only` is
/// its complement.
pub fn is_read_write(document: &Document, id: NodeId) -> bool {
    match is_text_control(document, id, READONLY_TYPES) {
        Some(applies) => {
            applies && !document.has_attribute(id, atoms::READONLY) && !is_disabled(document, id)
        }
        None => document
            .attribute(id, atoms::CONTENTEDITABLE)
            .is_some_and(|value| {
                value.is_empty()
                    || value.eq_ignore_ascii_case("true")
                    || value.eq_ignore_ascii_case("plaintext-only")
            }),
    }
}

/// `:placeholder-shown`: a text control with a `placeholder` and an empty
/// value. A script-set value replaces the default one.
pub fn is_placeholder_shown(document: &Document, id: NodeId) -> bool {
    if is_text_control(document, id, PLACEHOLDER_TYPES) != Some(true)
        || !document.has_attribute(id, atoms::PLACEHOLDER)
    {
        return false;
    }
    if let Some(value) = document.dirty_value(id) {
        return value.is_empty();
    }
    if document.tag(id) == Some(atoms::TEXTAREA) {
        return document
            .children(id)
            .all(|child| document.text_data(child).is_none_or(str::is_empty));
    }
    document
        .attribute(id, atoms::VALUE)
        .unwrap_or("")
        .is_empty()
}

/// `:disabled`, including inheritance from a disabled `fieldset` or `optgroup`.
pub fn is_disabled(document: &Document, id: NodeId) -> bool {
    let tag = document.tag(id);

    // Option elements: disabled if they have disabled attr or parent optgroup is disabled
    if matches!(tag, Some(t) if t == atoms::OPTION) {
        if document.has_attribute(id, atoms::DISABLED) {
            return true;
        }
        return document
            .ancestors(id)
            .find(|a| document.tag(*a) == Some(atoms::OPTGROUP))
            .is_some_and(|optgroup| document.has_attribute(optgroup, atoms::DISABLED));
    }

    // Optgroup: disabled if has disabled attribute
    if matches!(tag, Some(t) if t == atoms::OPTGROUP) {
        return document.has_attribute(id, atoms::DISABLED);
    }

    // Form-associated elements: check disabled attr and parent fieldset
    if !is_form_control(document, id) {
        return false;
    }
    if document.has_attribute(id, atoms::DISABLED) {
        return true;
    }
    // A control inside a disabled <fieldset> is disabled too, unless it sits
    // in that fieldset's first <legend> child (HTML §4.10.15.3).
    let mut child = id;
    for ancestor in document.ancestors(id) {
        if document.tag(ancestor) == Some(atoms::FIELDSET)
            && document.has_attribute(ancestor, atoms::DISABLED)
            && first_legend_child(document, ancestor) != Some(child)
        {
            return true;
        }
        child = ancestor;
    }
    false
}

fn first_legend_child(document: &Document, fieldset: NodeId) -> Option<NodeId> {
    let mut child = document.first_child(fieldset);
    while child.is_some() {
        if document.tag(child) == Some(atoms::LEGEND) {
            return Some(child);
        }
        child = document.next_sibling(child);
    }
    None
}

/// A parsed selector list plus the index hint its rightmost compounds allow.
#[derive(Clone)]
pub struct CompiledSelector {
    pub list: SelectorList<TentaSelectorImpl>,
    hint: CandidateHint,
}

/// How the candidate set for a query can be narrowed before matching
/// (`docs/spec/04-dom-and-css.md` §4, rightmost-compound filtering).
///
/// A hint is only ever a superset filter: an index it names must contain
/// every element the selector could match. That is why a `LocalName` whose
/// written form differs from its lowercase form (`textPath`, `linearGradient`,
/// `foreignObject`, `annotation-xml`) contributes *both* names — the matcher
/// compares against the case-preserved name for foreign elements and against
/// the lowercase one for HTML. In quirks mode, id and class index keys are
/// folded to lowercase, so hints work correctly even though the matcher
/// compares those ASCII case-insensitively.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct CandidateHint {
    /// One id per branch, present only when *every* branch requires one.
    ids: Option<Vec<Box<str>>>,
    /// One class per branch, present only when every branch requires one.
    classes: Option<Vec<Box<str>>>,
    /// The local names to look up; a branch may contribute two.
    tags: Option<Vec<Atom>>,
}

fn rightmost_hint(list: &SelectorList<TentaSelectorImpl>) -> CandidateHint {
    let branches = list.slice().len();
    let mut ids: Vec<Box<str>> = Vec::new();
    let mut classes: Vec<Box<str>> = Vec::new();
    let mut tags: Vec<Atom> = Vec::new();
    let mut tagged = 0usize;
    for selector in list.slice() {
        let (mut id, mut class, mut tag) = (None, None, None);
        // `iter()` yields the rightmost compound first and stops at the first
        // combinator.
        for component in selector.iter() {
            match component {
                Component::ID(ident) => id = id.or(Some(ident)),
                Component::Class(ident) => class = class.or(Some(ident)),
                Component::LocalName(name) => tag = Some(name),
                _ => {}
            }
        }
        if let Some(id) = id {
            ids.push(id.as_str().into());
        }
        if let Some(class) = class {
            classes.push(class.as_str().into());
        }
        if let Some(name) = tag {
            tags.push(name.name.atom);
            if name.lower_name.atom != name.name.atom {
                tags.push(name.lower_name.atom);
            }
            tagged += 1;
        }
    }
    CandidateHint {
        ids: (ids.len() == branches).then_some(ids),
        classes: (classes.len() == branches).then_some(classes),
        tags: (tagged == branches).then_some(tags),
    }
}

/// Compiles a selector list.
pub fn compile(selector: &str) -> Result<CompiledSelector> {
    let mut input = ParserInput::new(selector);
    let mut parser = CssParser::new(&mut input);
    let list = SelectorList::parse(&SelectorParser, &mut parser, ParseRelative::No)
        .map_err(|error| syntax(format!("'{selector}' is not a valid selector: {error:?}")))?;
    let hint = rightmost_hint(&list);
    Ok(CompiledSelector { list, hint })
}

/// A small move-to-front cache of compiled selectors, per document
/// (`docs/spec/04-dom-and-css.md` §4).
pub struct SelectorCache {
    entries: Vec<(String, CompiledSelector)>,
    /// The matching engine's own caches. The nth-index cache is what turns
    /// `:nth-child` over n siblings from O(n²) into O(n), so it has to
    /// outlive a single element; it is only valid while the tree is
    /// unchanged, which is what [`SelectorCache::with_matching_caches`]
    /// enforces.
    caches: SelectorCaches,
    /// The document's mutation generation the caches were built against.
    caches_generation: u64,
    hits: u64,
    misses: u64,
}

impl SelectorCache {
    /// How many compiled selectors a document keeps.
    pub const CAPACITY: usize = 32;

    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(Self::CAPACITY),
            caches: SelectorCaches::default(),
            caches_generation: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn get_or_compile(&mut self, selector: &str) -> Result<CompiledSelector> {
        if let Some(position) = self.entries.iter().position(|(key, _)| key == selector) {
            self.hits += 1;
            let entry = self.entries.remove(position);
            let compiled = entry.1.clone();
            self.entries.insert(0, entry);
            return Ok(compiled);
        }
        self.misses += 1;
        let compiled = compile(selector)?;
        if self.entries.len() == Self::CAPACITY {
            self.entries.pop();
        }
        self.entries
            .insert(0, (selector.to_owned(), compiled.clone()));
        Ok(compiled)
    }

    /// Runs `f` with the matching engine's caches, rebuilding them first if
    /// the tree has changed since they were filled.
    ///
    /// Compiled selectors stay valid across mutations; only the candidate sets
    /// they produce change, and those are recomputed per query. The matching
    /// caches do not: the nth-index and relative-selector caches are keyed by
    /// tree position. Invalidation is checked here rather than pushed from
    /// every mutation, because building a fresh `SelectorCaches` costs more
    /// than the mutation itself and a parse performs one mutation per token.
    pub(crate) fn with_matching_caches<R>(
        &mut self,
        generation: u64,
        f: impl FnOnce(&mut SelectorCaches) -> R,
    ) -> R {
        if self.caches_generation != generation {
            self.caches = SelectorCaches::default();
            self.caches_generation = generation;
        }
        f(&mut self.caches)
    }

    /// Cache hits and misses since the document was created.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SelectorCache {
    fn default() -> Self {
        Self::new()
    }
}

fn quirks(document: &Document) -> QuirksMode {
    match document.quirks_mode() {
        tenta_html::QuirksMode::Quirks => QuirksMode::Quirks,
        tenta_html::QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
        tenta_html::QuirksMode::NoQuirks => QuirksMode::NoQuirks,
    }
}

fn matches_compiled_with_scope(
    document: &Document,
    element: NodeId,
    compiled: &CompiledSelector,
    caches: &mut SelectorCaches,
    scope: Option<NodeId>,
) -> bool {
    if !document.is_element(element) {
        return false;
    }
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        caches,
        quirks(document),
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    // The `selectors` crate parses `:scope` itself and matches it against
    // `scope_element`; unset, it falls back to the root element, which is
    // what a document-rooted query wants (`docs/spec/04-dom-and-css.md` §8).
    context.scope_element = scope
        .filter(|scope| document.is_element(*scope))
        .map(|scope| ElementRef::new(document, scope).opaque());
    matches_selector_list(
        &compiled.list,
        &ElementRef::new(document, element),
        &mut context,
    )
}

impl Document {
    /// Whether `element` matches the selector.
    /// For :scope pseudo-class, the scope is the element being tested.
    pub fn matches(&self, element: NodeId, selector: &str) -> Result<bool> {
        let compiled = self.with_selector_cache(|cache| cache.get_or_compile(selector))?;
        Ok(self.with_matching_caches(|caches| {
            matches_compiled_with_scope(self, element, &compiled, caches, Some(element))
        }))
    }

    /// The nearest inclusive ancestor of `element` matching the selector;
    /// `:scope` is `element` itself, as in `Element.closest`.
    pub fn closest(&self, element: NodeId, selector: &str) -> Result<Option<NodeId>> {
        let compiled = self.with_selector_cache(|cache| cache.get_or_compile(selector))?;
        Ok(self.with_matching_caches(|caches| {
            std::iter::once(element)
                .chain(self.ancestors(element))
                .filter(|node| self.is_element(*node))
                .find(|node| {
                    matches_compiled_with_scope(self, *node, &compiled, caches, Some(element))
                })
        }))
    }

    /// The first descendant of `root` matching the selector, in tree order.
    ///
    /// Candidates arrive in tree order, so the walk stops at the first match
    /// instead of matching every candidate and reducing afterwards
    /// (`docs/spec/04-dom-and-css.md` §8: ≤ 20 µs on a 10 000-node page, and a
    /// class shared by fifty thousand elements must not be slower).
    /// Whether an index candidate lies under `root`.
    ///
    /// Document-rooted queries only see connected nodes: template contents,
    /// shadow trees, detached subtrees and removed-but-unswept nodes sit in
    /// the index buckets but not in the document (Chrome semantics). The
    /// connected bit is maintained on attach/detach, so this costs O(1) and
    /// never touches the order index, which is what keeps interleaved
    /// mutation + query workloads linear.
    #[inline]
    fn candidate_under(&self, root: NodeId, node: NodeId) -> bool {
        if root == self.document_node() {
            self.is_connected(node)
        } else {
            self.is_in_subtree(root, node)
        }
    }

    pub fn query_selector(&self, root: NodeId, selector: &str) -> Result<Option<NodeId>> {
        let compiled = self.with_selector_cache(|cache| cache.get_or_compile(selector))?;
        let result = self.with_candidates(&compiled, |candidates| {
            self.with_matching_caches(|caches| {
                candidates
                    .iter()
                    .copied()
                    .filter(|node| self.candidate_under(root, *node))
                    .find(|node| {
                        matches_compiled_with_scope(self, *node, &compiled, caches, Some(root))
                    })
            })
        });
        let found = match result {
            Some(found) => found,
            None => self.with_matching_caches(|caches| {
                self.descendants(root).find(|node| {
                    matches_compiled_with_scope(self, *node, &compiled, caches, Some(root))
                })
            }),
        };
        Ok(found)
    }

    /// Every descendant of `root` matching the selector, in tree order.
    /// For :scope pseudo-class, the scope is the root.
    pub fn query_selector_all(&self, root: NodeId, selector: &str) -> Result<Vec<NodeId>> {
        let compiled = self.with_selector_cache(|cache| cache.get_or_compile(selector))?;
        let result = self.with_candidates(&compiled, |candidates| {
            self.with_matching_caches(|caches| {
                candidates
                    .iter()
                    .copied()
                    .filter(|node| self.candidate_under(root, *node))
                    .filter(|node| {
                        matches_compiled_with_scope(self, *node, &compiled, caches, Some(root))
                    })
                    .collect::<Vec<NodeId>>()
            })
        });
        let found = match result {
            Some(found) => found,
            None => self.with_matching_caches(|caches| {
                self.descendants(root)
                    .filter(|node| {
                        matches_compiled_with_scope(self, *node, &compiled, caches, Some(root))
                    })
                    .collect()
            }),
        };
        Ok(found)
    }

    /// Runs `f` over the candidate elements the index hint allows, in tree
    /// order. `None` means "no usable hint, walk the subtree".
    ///
    /// The bucket is passed as a slice: copying it first made every query
    /// proportional to the size of the whole index bucket before the matcher
    /// ran at all.
    fn with_candidates<R>(
        &self,
        compiled: &CompiledSelector,
        f: impl FnOnce(&[NodeId]) -> R,
    ) -> Option<R> {
        let hint = &compiled.hint;
        // In quirks mode ids and classes match ASCII case-insensitively. The
        // index keys are folded to lowercase, so hints now work correctly in
        // quirks mode (via with_elements_by_id/class which fold the lookup).
        if let Some(ids) = &hint.ids {
            return Some(match ids.split_first() {
                Some((only, [])) => self.with_elements_by_id(only, f),
                _ => f(&self.merged_buckets(ids, BucketKind::Id)),
            });
        }
        if let Some(classes) = &hint.classes {
            return Some(match classes.split_first() {
                Some((only, [])) => self.with_elements_by_class(only, f),
                _ => f(&self.merged_buckets(classes, BucketKind::Class)),
            });
        }
        if let Some(tags) = &hint.tags {
            return Some(match tags.split_first() {
                Some((only, [])) => self.with_elements_by_tag(*only, f),
                _ => f(&self.merged_tag_buckets(tags)),
            });
        }
        None
    }

    /// The union of several id or class buckets, in tree order.
    fn merged_buckets(&self, keys: &[Box<str>], kind: BucketKind) -> Vec<NodeId> {
        let mut merged: Vec<NodeId> = Vec::new();
        for key in keys {
            match kind {
                BucketKind::Id => {
                    self.with_elements_by_id(key, |nodes| merged.extend_from_slice(nodes))
                }
                BucketKind::Class => {
                    self.with_elements_by_class(key, |nodes| merged.extend_from_slice(nodes))
                }
            }
        }
        self.sort_tree_order(&mut merged);
        merged.dedup();
        merged
    }

    fn merged_tag_buckets(&self, tags: &[Atom]) -> Vec<NodeId> {
        let mut merged: Vec<NodeId> = Vec::new();
        for tag in tags {
            self.with_elements_by_tag(*tag, |nodes| merged.extend_from_slice(nodes));
        }
        self.sort_tree_order(&mut merged);
        merged.dedup();
        merged
    }

    /// Compiled-selector cache hits and misses.
    pub fn selector_cache_stats(&self) -> (u64, u64) {
        self.with_selector_cache(|cache| cache.stats())
    }

    /// The rightmost-compound index hint a query would use in this document.
    /// Exposed for tests and for the search fast path's planner.
    pub fn selector_hint(&self, selector: &str) -> Result<&'static str> {
        let compiled = self.with_selector_cache(|cache| cache.get_or_compile(selector))?;
        Ok(match &compiled.hint {
            hint if hint.ids.is_some() => "id",
            hint if hint.classes.is_some() => "class",
            hint if hint.tags.is_some() => "tag",
            _ => "walk",
        })
    }
}

/// Which value index a merged lookup reads.
#[derive(Clone, Copy)]
enum BucketKind {
    Id,
    Class,
}

/// Re-exported so callers can name the parsed type.
pub type ParsedSelector = Selector<TentaSelectorImpl>;

#[cfg(test)]
mod tests {
    use super::*;

    fn hint_of(selector: &str) -> CandidateHint {
        compile(selector).expect("valid selector").hint
    }

    #[test]
    fn names_hash_their_text_and_serialise_as_identifiers() {
        let a = CssIdent::from("main-content");
        let b = CssIdent::from("main-content");
        assert_eq!(a, b);
        assert_eq!(a.precomputed_hash(), b.precomputed_hash());
        assert_ne!(
            a.precomputed_hash(),
            CssIdent::from("other").precomputed_hash()
        );
        let mut out = String::new();
        CssIdent::from("a b").to_css(&mut out).expect("write");
        assert_eq!(out, "a\\ b", "identifiers are escaped when serialised");
    }

    #[test]
    fn selector_identifiers_and_values_never_grow_the_global_atom_table() {
        // Ids, classes and attribute values are page-controlled and unbounded,
        // so they stay owned by the selector instead of entering the
        // process-wide name table (`docs/spec/04-dom-and-css.md` §2). Local
        // names are names and are interned on purpose, so tag matching stays
        // an integer compare.
        let id = "tenta-sel-id-96f1d0";
        let class = "tenta-sel-class-96f1d0";
        let value = "tenta-sel-value-96f1d0";
        compile(&format!("#{id}.{class}[data-probe=\"{value}\"]")).expect("valid selector");
        for text in [id, class, value] {
            assert_eq!(Atom::get(text), None, "{text} must not be interned");
        }
    }

    fn ids_of(selector: &str) -> Option<Vec<String>> {
        hint_of(selector)
            .ids
            .map(|ids| ids.iter().map(|id| id.to_string()).collect())
    }

    fn classes_of(selector: &str) -> Option<Vec<String>> {
        hint_of(selector)
            .classes
            .map(|classes| classes.iter().map(|class| class.to_string()).collect())
    }

    #[test]
    fn rightmost_compound_hints_use_the_narrowest_index() {
        assert_eq!(
            ids_of("#main").as_deref(),
            Some(["main".to_owned()].as_slice())
        );
        assert_eq!(
            classes_of("div#main .row").as_deref(),
            Some(["row".to_owned()].as_slice())
        );
        assert_eq!(hint_of("section > p").tags, Some(vec![atoms::P]));
        assert_eq!(
            ids_of("#a, #b").as_deref(),
            Some(["a".to_owned(), "b".to_owned()].as_slice()),
            "every branch of the list must have an id for the id index to be usable"
        );
        let mixed = hint_of("#a, .b");
        assert_eq!(
            mixed.ids, None,
            "one branch has no id, so the index is unusable"
        );
        assert_eq!(mixed.classes, None);
        for selector in ["[data-x]", "*"] {
            let hint = hint_of(selector);
            assert_eq!(hint.ids, None, "{selector}");
            assert_eq!(hint.classes, None, "{selector}");
            assert_eq!(hint.tags, None, "{selector}");
        }
    }

    #[test]
    fn unsupported_pseudo_classes_are_rejected_not_ignored() {
        assert!(compile(":totally-made-up").is_err());
        assert!(compile("p:nth-child(2n + 1)").is_ok());
        assert!(compile(":has(> p)").is_ok());
        assert!(compile(":is(a, b):not(.c)").is_ok());
    }

    #[test]
    fn the_cache_is_move_to_front_and_bounded() {
        let mut cache = SelectorCache::new();
        for index in 0..SelectorCache::CAPACITY + 4 {
            cache
                .get_or_compile(&format!(".class-{index}"))
                .expect("valid");
        }
        assert_eq!(cache.len(), SelectorCache::CAPACITY);
        let (hits, misses) = cache.stats();
        assert_eq!((hits, misses), (0, (SelectorCache::CAPACITY + 4) as u64));

        cache.get_or_compile(".class-35").expect("valid");
        assert_eq!(cache.stats().0, 1, "the most recent entry is still cached");
        cache.get_or_compile(".class-0").expect("valid");
        assert_eq!(cache.stats().1 as usize, SelectorCache::CAPACITY + 5);
    }

    #[test]
    fn lang_matching_follows_the_prefix_rule() {
        assert!(lang_matches("en", "en"));
        assert!(lang_matches("en-GB", "en"));
        assert!(lang_matches("EN-gb", "en"));
        assert!(!lang_matches("english", "en"));
        assert!(!lang_matches("fr", "en"));
    }
}
