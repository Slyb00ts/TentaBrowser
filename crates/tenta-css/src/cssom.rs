//! Stylo-backed CSS parsing and the document-independent part of CSSOM.
//!
//! A page owns its [`CssStyleSheet`] on the page thread. Rule mutations change
//! the same Stylo stylesheet later handed to a `DocumentStylesheetSet`; callers
//! notify that set with the returned [`SheetChange`] when one is attached.
//! Attaching the DOM arena to Stylo's traversal is the P3 render package.

use std::collections::HashMap;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc as StdArc, Mutex, Once};

use cssparser::{
    AtRuleParser, CowRcStr, ParseError, Parser, ParserInput, ParserState, QualifiedRuleParser,
    ToCss, parse_one_rule,
};
use selectors::parser::{ParseRelative, SelectorList};
use servo_arc::Arc;
use style::context::QuirksMode;
use style::device::Device;
use style::invalidation::stylesheets::RuleChangeKind;
use style::media_queries::MediaList;
use style::parser::ParserContext;
use style::properties::declaration_block::{
    SourcePropertyDeclarationUpdate, parse_one_declaration_into,
};
use style::properties::{
    Importance, NonCustomPropertyId, PropertyDeclarationBlock, PropertyId,
    SourcePropertyDeclaration, parse_style_attribute,
};
use style::selector_parser::{SelectorImpl, SelectorParser};
use style::shared_lock::{SharedRwLock, SharedRwLockReadGuard, ToCssWithGuard};
use style::stylesheet_set::DocumentStylesheetSet;
use style::stylesheets::supports_rule::parse_condition_or_declaration;
use style::stylesheets::{
    AllowImportRules, CssRule, CssRuleTypes, CssRules, CustomMediaMap, DocumentStyleSheet, Origin,
    OriginSet, RulesMutateError, Stylesheet, StylesheetContents, StylesheetLoader, UrlExtraData,
};
use style_traits::ParsingMode;
use url::Url;

pub use style::stylesheets::CssRuleType;

/// CSSOM mutation error categories, suitable for mapping to DOM exceptions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CssomError {
    /// The rule text did not contain exactly one valid rule.
    Syntax,
    /// The requested rule index is outside the list.
    IndexSize,
    /// The rule is not valid at this position in the stylesheet.
    HierarchyRequest,
    /// The rule cannot be removed in the sheet's current state.
    InvalidState,
    /// This native API does not model the rule's non-style nested context.
    UnsupportedRuleContext,
    /// The constructed-sheet JS bridge has no concrete interface for this rule.
    UnsupportedRuleType,
    /// A rule tree is too deep to serialize safely into a nested snapshot.
    DepthLimit,
}

impl fmt::Display for CssomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CssomError {}

impl From<RulesMutateError> for CssomError {
    fn from(error: RulesMutateError) -> Self {
        match error {
            RulesMutateError::Syntax => Self::Syntax,
            RulesMutateError::IndexSize => Self::IndexSize,
            RulesMutateError::HierarchyRequest => Self::HierarchyRequest,
            RulesMutateError::InvalidState => Self::InvalidState,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GenericRuleKind {
    Import,
    Other,
}

// CSSOM's stylesheet insertRule first runs CSS Syntax's generic single-rule
// parser, before it checks constructed @import and before list index checks.
// cssparser performs that syntax parse; Stylo remains the CSS grammar parser.
struct GenericRuleProbe;

fn consume_generic_tokens(input: &mut Parser<'_, '_>) {
    while input.next().is_ok() {}
}

impl<'i> AtRuleParser<'i> for GenericRuleProbe {
    type Prelude = GenericRuleKind;
    type AtRule = GenericRuleKind;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        consume_generic_tokens(input);
        Ok(if name.eq_ignore_ascii_case("import") {
            GenericRuleKind::Import
        } else {
            GenericRuleKind::Other
        })
    }

    fn rule_without_block(
        &mut self,
        kind: Self::Prelude,
        _start: &ParserState,
    ) -> Result<Self::AtRule, ()> {
        Ok(kind)
    }

    fn parse_block<'t>(
        &mut self,
        kind: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, Self::Error>> {
        consume_generic_tokens(input);
        Ok(kind)
    }
}

impl<'i> QualifiedRuleParser<'i> for GenericRuleProbe {
    type Prelude = ();
    type QualifiedRule = GenericRuleKind;
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        consume_generic_tokens(input);
        Ok(())
    }

    fn parse_block<'t>(
        &mut self,
        _prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, Self::Error>> {
        consume_generic_tokens(input);
        Ok(GenericRuleKind::Other)
    }
}

fn generic_rule_kind(css: &str) -> Result<GenericRuleKind, CssomError> {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    parse_one_rule(&mut parser, &mut GenericRuleProbe).map_err(|_| CssomError::Syntax)
}

/// One parsed CSS declaration, after Stylo has applied CSS declaration rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssPropertySnapshot {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// The serialized declaration block and its effective properties.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssDeclarationSnapshot {
    pub css_text: String,
    pub properties: Vec<CssPropertySnapshot>,
}

impl CssDeclarationSnapshot {
    fn from_block(block: &PropertyDeclarationBlock) -> Self {
        let mut css_text = String::new();
        block
            .to_css(&mut css_text)
            .expect("serializing CSS into a String cannot fail");
        let properties = block
            .declaration_importance_iter()
            .map(|(declaration, importance)| {
                let mut value = String::new();
                declaration
                    .to_css(&mut value)
                    .expect("serializing CSS into a String cannot fail");
                CssPropertySnapshot {
                    name: declaration.id().name().into_owned(),
                    value,
                    important: importance.important(),
                }
            })
            .collect();
        Self {
            css_text,
            properties,
        }
    }
}

/// A Stylo declaration block parsed without an enclosing stylesheet.
pub struct CssDeclarationBlock {
    block: PropertyDeclarationBlock,
    url_data: UrlExtraData,
}

impl CssDeclarationBlock {
    /// Parse an inline style declaration block with author-origin CSS rules.
    pub fn parse(css: &str, base_url: &Url) -> Self {
        configure_stylo();
        let url_data = UrlExtraData::from(base_url.clone());
        Self {
            block: parse_style_attribute(
                css,
                &url_data,
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            ),
            url_data,
        }
    }

    /// The number of effective declarations in Stylo's ordered block.
    pub fn len(&self) -> usize {
        self.block.len()
    }

    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }

    /// The property name at `index`, or an empty string past the end.
    pub fn item(&self, index: usize) -> String {
        self.block
            .declarations()
            .get(index)
            .map(|declaration| declaration.id().name().into_owned())
            .unwrap_or_default()
    }

    /// Return Stylo's CSSOM serialization and effective declarations.
    pub fn snapshot(&self) -> CssDeclarationSnapshot {
        CssDeclarationSnapshot::from_block(&self.block)
    }

    /// Return a property's CSSOM value, including shorthand serialization.
    /// Unknown or absent properties return an empty string.
    pub fn property_value(&self, name: &str) -> String {
        property_value_in_block(&self.block, name)
    }

    /// Whether the effective property has `!important` priority.
    pub fn is_important(&self, name: &str) -> bool {
        PropertyId::parse_enabled_for_all_content(name)
            .is_ok_and(|id| self.block.property_priority(&id).important())
    }

    /// Set a property using Stylo's value parser and CSSOM declaration update
    /// algorithm. Invalid names, values and priorities are no-ops. The empty
    /// value removes a property before the priority is checked, per CSSOM.
    /// Returns whether the effective declaration block changed.
    pub fn set_property(&mut self, name: &str, value: &str, priority: &str) -> bool {
        set_property_in_block(&mut self.block, &self.url_data, name, value, priority)
    }

    /// Remove a property, returning its CSSOM value before removal.
    pub fn remove_property(&mut self, name: &str) -> String {
        remove_property_from_block(&mut self.block, name).unwrap_or_default()
    }
}

fn property_value_in_block(block: &PropertyDeclarationBlock, name: &str) -> String {
    let Ok(id) = PropertyId::parse_enabled_for_all_content(name) else {
        return String::new();
    };
    let mut value = String::new();
    block
        .property_value_to_css(&id, &mut value)
        .expect("serializing CSS into a String cannot fail");
    value
}

fn set_property_in_block(
    block: &mut PropertyDeclarationBlock,
    url_data: &UrlExtraData,
    name: &str,
    value: &str,
    priority: &str,
) -> bool {
    let Ok(id) = PropertyId::parse_enabled_for_all_content(name) else {
        return false;
    };
    if value.is_empty() {
        let Some(index) = block.first_declaration_to_remove(&id) else {
            return false;
        };
        block.remove_property(&id, index);
        return true;
    }
    let importance = match priority {
        "" => Importance::Normal,
        priority if priority.eq_ignore_ascii_case("important") => Importance::Important,
        _ => return false,
    };
    let mut source = SourcePropertyDeclaration::default();
    if parse_one_declaration_into(
        &mut source,
        id,
        value,
        Origin::Author,
        url_data,
        None,
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    )
    .is_err()
    {
        return false;
    }
    let mut updates = SourcePropertyDeclarationUpdate::default();
    if !block.prepare_for_update(&source, importance, &mut updates) {
        return false;
    }
    block.update(source.drain(), importance, &mut updates);
    true
}

fn remove_property_from_block(block: &mut PropertyDeclarationBlock, name: &str) -> Option<String> {
    let Ok(id) = PropertyId::parse_enabled_for_all_content(name) else {
        return None;
    };
    let index = block.first_declaration_to_remove(&id)?;
    let value = property_value_in_block(block, name);
    block.remove_property(&id, index);
    Some(value)
}

/// A read snapshot of one Stylo rule. Child rules preserve CSSOM nesting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssRuleSnapshot {
    pub rule_type: CssRuleType,
    pub css_text: String,
    pub style: Option<CssDeclarationSnapshot>,
    pub children: Vec<CssRuleSnapshot>,
}

impl CssRuleSnapshot {
    fn from_rule(rule: &CssRule, guard: &SharedRwLockReadGuard<'_>) -> Self {
        let style = match rule {
            CssRule::Style(style_rule) => {
                let style_rule = style_rule.read_with(guard);
                Some(CssDeclarationSnapshot::from_block(
                    style_rule.block.read_with(guard),
                ))
            }
            _ => None,
        };
        // Stylo's traversal follows @import into another sheet. CSSImportRule
        // exposes that sheet through `styleSheet`, not through nested cssRules.
        let children = if matches!(rule, CssRule::Import(_)) {
            Vec::new()
        } else {
            rule.children(guard)
                .iter()
                .map(|child| Self::from_rule(child, guard))
                .collect()
        };
        Self {
            rule_type: rule.rule_type(),
            css_text: rule.to_css_string(guard),
            style,
            children,
        }
    }
}

// Stylo does not currently impose a CSS nesting cap before calling its
// recursive serializers. Check the tree iteratively before building snapshots
// so a hostile stylesheet cannot make `to_css` exhaust the page thread's
// native stack. A read fails explicitly instead of returning a partial list.
const MAX_RULE_SNAPSHOT_DEPTH: usize = 128;

fn check_rule_depth(
    rules: &[CssRule],
    guard: &SharedRwLockReadGuard<'_>,
) -> Result<(), CssomError> {
    let mut pending: Vec<_> = rules.iter().map(|rule| (rule, 1_usize)).collect();
    while let Some((rule, depth)) = pending.pop() {
        if depth > MAX_RULE_SNAPSHOT_DEPTH {
            return Err(CssomError::DepthLimit);
        }
        // Imported rules live in another stylesheet; they are not this
        // CSSImportRule's CSSOM children.
        if !matches!(rule, CssRule::Import(_)) {
            pending.extend(rule.children(guard).iter().map(|child| (child, depth + 1)));
        }
    }
    Ok(())
}

// The JavaScript bridge exposes concrete CSSStyleRule wrappers only. Inspect
// the entire candidate before publishing any part of it into a live rule list.
fn check_supported_style_tree(
    rules: &[CssRule],
    guard: &SharedRwLockReadGuard<'_>,
    initial_depth: usize,
) -> Result<(), CssomError> {
    let mut pending: Vec<_> = rules.iter().map(|rule| (rule, initial_depth)).collect();
    while let Some((rule, depth)) = pending.pop() {
        if depth > MAX_RULE_SNAPSHOT_DEPTH {
            return Err(CssomError::DepthLimit);
        }
        if !matches!(rule, CssRule::Style(_)) {
            return Err(CssomError::UnsupportedRuleType);
        }
        pending.extend(rule.children(guard).iter().map(|child| (child, depth + 1)));
    }
    Ok(())
}

/// The allocation identity of a rule in Stylo's shared rule tree.
///
/// Every `CssRule` variant contains a Servo `Arc`. Its allocation, rather
/// than its current list index or serialized text, is the identity CSSOM
/// needs when another rule is inserted before it or it is removed.
fn rule_identity(rule: &CssRule) -> (CssRuleType, NonNull<()>) {
    let allocation = match rule {
        CssRule::Style(rule) => rule.raw_ptr(),
        CssRule::Namespace(rule) => rule.raw_ptr(),
        CssRule::Import(rule) => rule.raw_ptr(),
        CssRule::Media(rule) => rule.raw_ptr(),
        CssRule::CustomMedia(rule) => rule.raw_ptr(),
        CssRule::Container(rule) => rule.raw_ptr(),
        CssRule::FontFace(rule) => rule.raw_ptr(),
        CssRule::FontFeatureValues(rule) => rule.raw_ptr(),
        CssRule::FontPaletteValues(rule) => rule.raw_ptr(),
        CssRule::CounterStyle(rule) => rule.raw_ptr(),
        CssRule::Keyframes(rule) => rule.raw_ptr(),
        CssRule::Margin(rule) => rule.raw_ptr(),
        CssRule::Supports(rule) => rule.raw_ptr(),
        CssRule::Page(rule) => rule.raw_ptr(),
        CssRule::Property(rule) => rule.raw_ptr(),
        CssRule::Document(rule) => rule.raw_ptr(),
        CssRule::LayerBlock(rule) => rule.raw_ptr(),
        CssRule::LayerStatement(rule) => rule.raw_ptr(),
        CssRule::Scope(rule) => rule.raw_ptr(),
        CssRule::StartingStyle(rule) => rule.raw_ptr(),
        CssRule::AppearanceBase(rule) => rule.raw_ptr(),
        CssRule::PositionTry(rule) => rule.raw_ptr(),
        CssRule::NestedDeclarations(rule) => rule.raw_ptr(),
        CssRule::ViewTransition(rule) => rule.raw_ptr(),
    };
    (rule.rule_type(), allocation)
}

fn rule_address(rule: &CssRule) -> usize {
    rule_identity(rule).1.as_ptr() as usize
}

/// CSSOM parent-sheet links are distinct from membership in Stylo's current
/// stylesheet tree. Removing one rule clears only that rule's link, whereas a
/// synchronous replacement detaches every rule in the old contents.
struct SheetLinks {
    // The sheet is shared here so each rule handle needs only one ownership Arc.
    sheet: Arc<Stylesheet>,
    replacement_epoch: AtomicU64,
    // A child created under a detached rule starts without a parent sheet.
    // Retaining the rule allocation avoids an ABA identity collision; entries
    // are pruned once neither a rule list nor a handle retains that rule.
    detached_insertions: Mutex<HashMap<usize, CssRule>>,
    marker_count: AtomicUsize,
}

impl SheetLinks {
    fn new(sheet: Arc<Stylesheet>) -> Self {
        Self {
            sheet,
            replacement_epoch: AtomicU64::new(0),
            detached_insertions: Mutex::new(HashMap::new()),
            marker_count: AtomicUsize::new(0),
        }
    }

    fn mark_detached_insertion(&self, rule: &CssRule) {
        let mut markers = self
            .detached_insertions
            .lock()
            .expect("CSSOM detached insertion lock poisoned");
        markers.insert(rule_address(rule), rule.clone());
        self.marker_count.store(markers.len(), Ordering::Release);
    }

    fn prune_detached_insertions(&self) {
        if self.marker_count.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut markers = self
            .detached_insertions
            .lock()
            // A drop hook can run while another native callback unwinds.
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Removing a parent can make its previously marked children unique.
        loop {
            let previous_len = markers.len();
            markers.retain(|_, rule| !matches!(rule, CssRule::Style(style) if style.is_unique()));
            if markers.len() == previous_len {
                break;
            }
        }
        self.marker_count.store(markers.len(), Ordering::Release);
    }

    fn replaced(&self) {
        self.replacement_epoch.fetch_add(1, Ordering::Release);
        self.detached_insertions
            .lock()
            .expect("CSSOM detached insertion lock poisoned")
            .clear();
        self.marker_count.store(0, Ordering::Release);
    }
}

/// Field-order drop hook: retained rule and parent-lineage Arcs are destroyed
/// before this hook, allowing a detached marker to release its final Arc as
/// soon as the last observable handle (or change record) goes away.
#[derive(Clone)]
struct LinkPruneOnDrop(StdArc<SheetLinks>);

impl Drop for LinkPruneOnDrop {
    fn drop(&mut self) {
        self.0.prune_detached_insertions();
    }
}

/// A live CSSOM rule identity backed by Stylo's shared rule allocation.
///
/// A handle retains the rule after it leaves a rule list. Its serialized text
/// and nested rules remain readable, while [`Self::is_attached`] reflects
/// whether that same allocation is still in the stylesheet's current tree.
/// This is the native basis for stable JavaScript `CSSRule` wrappers.
#[derive(Clone)]
pub struct CssRuleHandle {
    replacement_epoch: u64,
    rule: CssRule,
    parent: Option<Arc<RuleParent>>,
    // This must drop after the rule and its parent lineage.
    links: LinkPruneOnDrop,
}

#[derive(Clone)]
struct RuleParent {
    rule: CssRule,
    parent: Option<Arc<RuleParent>>,
}

impl CssRuleHandle {
    /// The highest rule still joined to this rule by live `parentRule` links.
    /// A directly deleted child becomes its own component, while deleting an
    /// ancestor leaves the descendants in that ancestor's component.
    pub fn graph_component_identity(&self) -> usize {
        let guard = self.links.0.sheet.shared_lock.read();
        let mut current = &self.rule;
        let mut parent = self.parent.as_deref();
        while let Some(ancestor) = parent {
            let identity = rule_identity(current);
            if !ancestor
                .rule
                .children(&guard)
                .iter()
                .any(|child| rule_identity(child) == identity)
            {
                break;
            }
            current = &ancestor.rule;
            parent = ancestor.parent.as_deref();
        }
        rule_address(current)
    }

    /// A retained handle to the highest rule still joined by `parentRule`.
    /// The host graph uses one root per detached component when checking
    /// whether any unmaterialized descendant can expose the stylesheet.
    pub fn graph_component_root(&self) -> Self {
        let mut root = self.clone();
        while let Some(parent) = root.parent_rule() {
            root = parent;
        }
        root
    }

    fn lineage_depth(&self) -> usize {
        let mut depth = 1;
        let mut parent = self.parent.as_deref();
        while let Some(ancestor) = parent {
            depth += 1;
            parent = ancestor.parent.as_deref();
        }
        depth
    }

    /// Return the current CSSOM parent. Removing this rule from its parent's
    /// list clears parentRule; removing the parent from the sheet does not.
    pub fn parent_rule(&self) -> Option<Self> {
        let parent = self.parent.as_ref()?;
        let guard = self.links.0.sheet.shared_lock.read();
        if !parent
            .rule
            .children(&guard)
            .iter()
            .any(|child| rule_identity(child) == rule_identity(&self.rule))
        {
            return None;
        }
        Some(Self {
            replacement_epoch: self.replacement_epoch,
            rule: parent.rule.clone(),
            parent: parent.parent.clone(),
            links: self.links.clone(),
        })
    }

    /// Whether two handles refer to the same native rule in the same sheet.
    pub fn same_rule(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.links.0.sheet, &other.links.0.sheet)
            && rule_identity(&self.rule) == rule_identity(&other.rule)
    }

    pub fn rule_type(&self) -> CssRuleType {
        self.rule.rule_type()
    }

    /// Serialize the selector list of a style rule, including a detached one.
    pub fn selector_text(&self) -> Option<String> {
        let CssRule::Style(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        Some(rule.read_with(&guard).selectors.to_css_string())
    }

    /// Read the effective declarations of a style rule from its live Stylo block.
    pub fn style_declarations(&self) -> Option<CssDeclarationSnapshot> {
        let CssRule::Style(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        let rule = rule.read_with(&guard);
        Some(CssDeclarationSnapshot::from_block(
            rule.block.read_with(&guard),
        ))
    }

    /// Return a CSSOM property value from a style rule's live declarations.
    pub fn style_property_value(&self, name: &str) -> Option<String> {
        let CssRule::Style(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        let rule = rule.read_with(&guard);
        Some(property_value_in_block(rule.block.read_with(&guard), name))
    }

    /// Return a CSSOM property's important flag for a style rule.
    pub fn style_property_is_important(&self, name: &str) -> Option<bool> {
        let CssRule::Style(rule) = &self.rule else {
            return None;
        };
        let Ok(id) = PropertyId::parse_enabled_for_all_content(name) else {
            return Some(false);
        };
        let guard = self.links.0.sheet.shared_lock.read();
        let rule = rule.read_with(&guard);
        Some(
            rule.block
                .read_with(&guard)
                .property_priority(&id)
                .important(),
        )
    }

    /// Read this rule from its retained Stylo allocation, even if removed.
    /// A deeply nested rule fails explicitly before Stylo's recursive writer.
    pub fn css_text(&self) -> Result<String, CssomError> {
        let guard = self.links.0.sheet.shared_lock.read();
        check_rule_depth(std::slice::from_ref(&self.rule), &guard)?;
        Ok(self.rule.to_css_string(&guard))
    }

    /// Read the current declarations and nested rules of this rule.
    pub fn snapshot(&self) -> Result<CssRuleSnapshot, CssomError> {
        let guard = self.links.0.sheet.shared_lock.read();
        check_rule_depth(std::slice::from_ref(&self.rule), &guard)?;
        Ok(CssRuleSnapshot::from_rule(&self.rule, &guard))
    }

    /// The number of child rules in this rule's own CSSOM rule list.
    pub fn child_count(&self) -> usize {
        if matches!(self.rule, CssRule::Import(_)) {
            return 0;
        }
        let guard = self.links.0.sheet.shared_lock.read();
        self.rule.children(&guard).len()
    }

    /// A child rule at `index`, retaining its own Stylo identity.
    pub fn child_at(&self, index: usize) -> Option<Self> {
        if matches!(self.rule, CssRule::Import(_)) {
            return None;
        }
        let guard = self.links.0.sheet.shared_lock.read();
        let rule = self.rule.children(&guard).get(index)?.clone();
        Some(Self {
            replacement_epoch: self.replacement_epoch,
            rule,
            parent: Some(Arc::new(RuleParent {
                rule: self.rule.clone(),
                parent: self.parent.clone(),
            })),
            links: self.links.clone(),
        })
    }

    /// Return this rule's CSSOM parent-sheet link. This intentionally does not
    /// use `is_attached`: a child of a directly removed ancestor retains its
    /// own link, even though it no longer participates in sheet invalidation.
    pub fn has_parent_style_sheet(&self) -> bool {
        if self.replacement_epoch != self.links.0.replacement_epoch.load(Ordering::Acquire) {
            return false;
        }
        {
            let markers = self
                .links
                .0
                .detached_insertions
                .lock()
                .expect("CSSOM detached insertion lock poisoned");
            if markers.contains_key(&rule_address(&self.rule)) {
                return false;
            }
            let mut parent = self.parent.as_deref();
            while let Some(ancestor) = parent {
                if markers.contains_key(&rule_address(&ancestor.rule)) {
                    return false;
                }
                parent = ancestor.parent.as_deref();
            }
        }
        let guard = self.links.0.sheet.shared_lock.read();
        let identity = rule_identity(&self.rule);
        match self.parent.as_deref() {
            Some(parent) => parent
                .rule
                .children(&guard)
                .iter()
                .any(|child| rule_identity(child) == identity),
            None => self
                .links
                .0
                .sheet
                .contents
                .read_with(&guard)
                .rules(&guard)
                .iter()
                .any(|rule| rule_identity(rule) == identity),
        }
    }

    /// Whether this rule or any rule currently reachable through its child
    /// list can expose the sheet via `parentStyleSheet`. This differs from
    /// attachment: removing an ancestor leaves a descendant's sheet link.
    /// The walk stays iterative even for deeply nested hostile CSS.
    pub fn graph_reaches_parent_style_sheet(&self) -> bool {
        if self.replacement_epoch != self.links.0.replacement_epoch.load(Ordering::Acquire) {
            return false;
        }
        let mut pending = vec![self.clone()];
        while let Some(rule) = pending.pop() {
            if rule.has_parent_style_sheet() {
                return true;
            }
            for index in 0..rule.child_count() {
                if let Some(child) = rule.child_at(index) {
                    pending.push(child);
                }
            }
        }
        false
    }

    /// Whether this exact rule remains in the stylesheet's current tree.
    /// The walk is iterative, so deeply nested hostile CSS cannot overflow
    /// the page thread's stack while checking a detached wrapper.
    pub fn is_attached(&self) -> bool {
        let guard = self.links.0.sheet.shared_lock.read();
        let contents = self.links.0.sheet.contents.read_with(&guard);
        let mut pending: Vec<_> = contents.rules(&guard).iter().collect();
        let identity = rule_identity(&self.rule);
        while let Some(rule) = pending.pop() {
            if rule_identity(rule) == identity {
                return true;
            }
            // An @import's rules belong to its imported stylesheet, not this
            // sheet's CSSRuleList or parent chain.
            if !matches!(rule, CssRule::Import(_)) {
                pending.extend(rule.children(&guard));
            }
        }
        false
    }
}

/// What changed in a sheet. Keep it until the owning document notifies Stylo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SheetChangeKind {
    Insertion,
    Removal,
    StyleDeclarations,
    Full,
}

/// A CSSOM mutation that can drive Stylo's document stylesheet invalidation.
pub struct SheetChange {
    pub kind: SheetChangeKind,
    pub index: Option<usize>,
    pub revision: u64,
    rule: Option<CssRule>,
    sheet: Arc<Stylesheet>,
    _prune_on_drop: LinkPruneOnDrop,
}

impl SheetChange {
    /// Notify an attached Stylo document set. At Tier 2 `device` can be `None`;
    /// P3 supplies a device to collect targeted element invalidations.
    pub fn notify(
        &self,
        set: &mut DocumentStylesheetSet<DocumentStyleSheet>,
        device: Option<&Device>,
        custom_media: &CustomMediaMap,
    ) {
        match (&self.rule, self.kind) {
            (
                Some(rule),
                SheetChangeKind::Insertion
                | SheetChangeKind::Removal
                | SheetChangeKind::StyleDeclarations,
            ) => {
                let guard = self.sheet.shared_lock.read();
                let kind = match self.kind {
                    SheetChangeKind::Insertion => RuleChangeKind::Insertion,
                    SheetChangeKind::Removal => RuleChangeKind::Removal,
                    SheetChangeKind::StyleDeclarations => RuleChangeKind::StyleRuleDeclarations,
                    SheetChangeKind::Full => unreachable!(),
                };
                set.rule_changed(
                    device,
                    custom_media,
                    &DocumentStyleSheet(Arc::clone(&self.sheet)),
                    rule,
                    &guard,
                    kind,
                    &[],
                );
            }
            _ => set.force_dirty(OriginSet::ORIGIN_AUTHOR),
        }
    }
}

/// An author stylesheet whose parsed rules are owned by Stylo.
pub struct CssStyleSheet {
    sheet: Arc<Stylesheet>,
    links: StdArc<SheetLinks>,
    base_url: Url,
    revision: u64,
}

impl CssStyleSheet {
    /// Parse a stylesheet. Without a loader, `@import` cannot fetch another
    /// sheet and Stylo ignores that rule, as it does for an invalid rule.
    pub fn parse(css: &str, base_url: Url) -> Self {
        Self::parse_with_loader(css, base_url, None)
    }

    /// Parse with a host-provided Stylo import loader. The loader must route
    /// requests through `tenta-net`; this crate does no network I/O itself.
    pub fn parse_with_loader(
        css: &str,
        base_url: Url,
        loader: Option<&dyn StylesheetLoader>,
    ) -> Self {
        configure_stylo();
        let lock = SharedRwLock::new();
        let media = Arc::new(lock.wrap(MediaList::empty()));
        let allow_import = if loader.is_some() {
            AllowImportRules::Yes
        } else {
            AllowImportRules::No
        };
        let sheet = Arc::new(Stylesheet::from_str(
            css,
            UrlExtraData::from(base_url.clone()),
            Origin::Author,
            media,
            lock,
            loader,
            None,
            QuirksMode::NoQuirks,
            allow_import,
        ));
        let links = StdArc::new(SheetLinks::new(Arc::clone(&sheet)));
        Self {
            sheet,
            links,
            base_url,
            revision: 0,
        }
    }

    /// The same Stylo sheet used by CSSOM, ready for a document stylesheet set.
    pub fn document_sheet(&self) -> DocumentStyleSheet {
        DocumentStyleSheet(Arc::clone(&self.sheet))
    }

    /// Attach this sheet to a document's Stylo set, retaining its CSSOM identity.
    pub fn attach_to_document(
        &self,
        set: &mut DocumentStylesheetSet<DocumentStyleSheet>,
        device: Option<&Device>,
        custom_media: &CustomMediaMap,
    ) {
        let guard = self.sheet.shared_lock.read();
        set.append_stylesheet(device, custom_media, self.document_sheet(), &guard);
    }

    /// Remove this sheet from a document's Stylo set.
    pub fn detach_from_document(
        &self,
        set: &mut DocumentStylesheetSet<DocumentStyleSheet>,
        device: Option<&Device>,
        custom_media: &CustomMediaMap,
    ) {
        let guard = self.sheet.shared_lock.read();
        set.remove_stylesheet(device, custom_media, self.document_sheet(), &guard);
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn disabled(&self) -> bool {
        self.sheet.disabled()
    }

    /// Toggle stylesheet participation in a future cascade.
    pub fn set_disabled(&mut self, disabled: bool) -> Option<SheetChange> {
        if !self.sheet.set_disabled(disabled) {
            return None;
        }
        Some(self.full_change())
    }

    /// Snapshot the current top-level `cssRules`, including nested rule trees.
    pub fn css_rules(&self) -> Result<Vec<CssRuleSnapshot>, CssomError> {
        let guard = self.sheet.shared_lock.read();
        let contents = self.sheet.contents.read_with(&guard);
        let rules = contents.rules(&guard);
        check_rule_depth(rules, &guard)?;
        Ok(rules
            .iter()
            .map(|rule| CssRuleSnapshot::from_rule(rule, &guard))
            .collect())
    }

    pub fn rule_count(&self) -> usize {
        let guard = self.sheet.shared_lock.read();
        self.sheet.contents.read_with(&guard).rules(&guard).len()
    }

    /// Top-level allocation identities for a host graph snapshot. Collecting
    /// them under one Stylo read lock avoids repeated handle clones and an
    /// O(rules x wrappers) attachment scan at a collector safe point.
    pub fn graph_top_rule_identities(&self) -> Vec<usize> {
        let guard = self.sheet.shared_lock.read();
        self.sheet
            .contents
            .read_with(&guard)
            .rules(&guard)
            .iter()
            .map(rule_address)
            .collect()
    }

    /// Get a rule by its current top-level index. The returned handle keeps
    /// the rule's identity when later insertions shift its index.
    pub fn rule_at(&self, index: usize) -> Option<CssRuleHandle> {
        let guard = self.sheet.shared_lock.read();
        let rule = self
            .sheet
            .contents
            .read_with(&guard)
            .rules(&guard)
            .get(index)?
            .clone();
        Some(CssRuleHandle {
            replacement_epoch: self.links.replacement_epoch.load(Ordering::Acquire),
            rule,
            parent: None,
            links: LinkPruneOnDrop(StdArc::clone(&self.links)),
        })
    }

    /// Change the selector list on the existing Stylo style-rule allocation.
    /// CSSOM ignores an invalid selector list instead of throwing. A detached
    /// top-level rule remains mutable, but it cannot invalidate its former
    /// sheet. Nested style rules use Stylo's relative selector context.
    pub fn set_rule_selector_text(
        &mut self,
        rule: &CssRuleHandle,
        selectors: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &rule.rule else {
            return Err(CssomError::InvalidState);
        };
        if rule.parent.is_some()
            && !rule
                .parent
                .as_ref()
                .is_some_and(|parent| matches!(&parent.rule, CssRule::Style(_)))
        {
            return Err(CssomError::UnsupportedRuleContext);
        }
        let parsed = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            let parser = SelectorParser {
                stylesheet_origin: Origin::Author,
                namespaces: &contents.namespaces,
                url_data: &contents.url_data,
                for_supports_rule: false,
            };
            let mut input = ParserInput::new(selectors);
            let mut input = Parser::new(&mut input);
            let relative = if rule.parent.is_some() {
                ParseRelative::ForNesting
            } else {
                ParseRelative::No
            };
            SelectorList::<SelectorImpl>::parse(&parser, &mut input, relative)
        };
        let Ok(parsed) = parsed else {
            return Ok(None);
        };
        {
            let guard = self.sheet.shared_lock.read();
            if style_rule.read_with(&guard).selectors.to_css_string() == parsed.to_css_string() {
                return Ok(None);
            }
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            style_rule.write_with(&mut guard).selectors = parsed;
        }
        // Replacing a selector can stop matching elements that matched the
        // old list. A full author-origin invalidation covers both old and new
        // matches until Stylo can receive both lists for targeted invalidation.
        Ok(rule.is_attached().then(|| self.full_change()))
    }

    /// Replace a style rule's declaration block without changing rule identity.
    /// Invalid declarations are discarded by Stylo, as in CSSOM's cssText setter.
    pub fn set_rule_style_css_text(
        &mut self,
        rule: &CssRuleHandle,
        css: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &rule.rule else {
            return Err(CssomError::InvalidState);
        };
        let url_data = UrlExtraData::from(self.base_url.clone());
        let parsed = parse_style_attribute(
            css,
            &url_data,
            None,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        );
        let block = {
            let guard = self.sheet.shared_lock.read();
            Arc::clone(&style_rule.read_with(&guard).block)
        };
        {
            let guard = self.sheet.shared_lock.read();
            if CssDeclarationSnapshot::from_block(block.read_with(&guard)).css_text
                == CssDeclarationSnapshot::from_block(&parsed).css_text
            {
                return Ok(None);
            }
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            *block.write_with(&mut guard) = parsed;
        }
        Ok(self.change_after_style_declarations(rule))
    }

    /// Apply CSSOM setProperty to a style rule's live Stylo declaration block.
    /// Invalid names, values and priorities leave the rule unchanged.
    pub fn set_rule_style_property(
        &mut self,
        rule: &CssRuleHandle,
        name: &str,
        value: &str,
        priority: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &rule.rule else {
            return Err(CssomError::InvalidState);
        };
        let block = {
            let guard = self.sheet.shared_lock.read();
            Arc::clone(&style_rule.read_with(&guard).block)
        };
        let url_data = UrlExtraData::from(self.base_url.clone());
        let changed = {
            let mut guard = self.sheet.shared_lock.write();
            set_property_in_block(
                block.write_with(&mut guard),
                &url_data,
                name,
                value,
                priority,
            )
        };
        Ok(changed
            .then(|| self.change_after_style_declarations(rule))
            .flatten())
    }

    /// Apply CSSOM removeProperty and return its previous serialized value.
    pub fn remove_rule_style_property(
        &mut self,
        rule: &CssRuleHandle,
        name: &str,
    ) -> Result<(String, Option<SheetChange>), CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &rule.rule else {
            return Err(CssomError::InvalidState);
        };
        let block = {
            let guard = self.sheet.shared_lock.read();
            Arc::clone(&style_rule.read_with(&guard).block)
        };
        let removed = {
            let mut guard = self.sheet.shared_lock.write();
            remove_property_from_block(block.write_with(&mut guard), name)
        };
        match removed {
            Some(value) => Ok((value, self.change_after_style_declarations(rule))),
            None => Ok((String::new(), None)),
        }
    }

    fn change_after_style_declarations(&mut self, rule: &CssRuleHandle) -> Option<SheetChange> {
        if !rule.is_attached() {
            return None;
        }
        let is_top_level = {
            let guard = self.sheet.shared_lock.read();
            self.sheet
                .contents
                .read_with(&guard)
                .rules(&guard)
                .iter()
                .any(|candidate| rule_identity(candidate) == rule_identity(&rule.rule))
        };
        if !is_top_level {
            // Stylo's targeted rule_changed API needs the nested ancestor path.
            // Rebuild conservatively until the native API carries that path.
            return Some(self.full_change());
        }
        self.revision = self.revision.saturating_add(1);
        Some(SheetChange {
            kind: SheetChangeKind::StyleDeclarations,
            index: None,
            revision: self.revision,
            rule: Some(rule.rule.clone()),
            sheet: Arc::clone(&self.sheet),
            _prune_on_drop: LinkPruneOnDrop(StdArc::clone(&self.links)),
        })
    }

    /// Insert exactly one rule using Stylo's CSSOM parser and ordering checks.
    pub fn insert_rule(&mut self, css: &str, index: usize) -> Result<SheetChange, CssomError> {
        let (rules_lock, rule) = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            let rules_lock = Arc::clone(&contents.rules);
            let rule = rules_lock.read_with(&guard).parse_rule_for_insert(
                &self.sheet.shared_lock,
                css,
                contents,
                index,
                CssRuleTypes::default(),
                None,
                None,
                AllowImportRules::No,
            )?;
            (rules_lock, rule)
        };
        // Namespace mutations need a corresponding update to the sheet's
        // namespace map. Defer those until the sheet can preserve live rule
        // identities across that update.
        if matches!(rule, CssRule::Namespace(_)) {
            return Err(CssomError::InvalidState);
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            rules_lock
                .write_with(&mut guard)
                .0
                .insert(index, rule.clone());
        }
        self.revision = self.revision.saturating_add(1);
        Ok(SheetChange {
            kind: SheetChangeKind::Insertion,
            index: Some(index),
            revision: self.revision,
            rule: Some(rule),
            sheet: Arc::clone(&self.sheet),
            _prune_on_drop: LinkPruneOnDrop(StdArc::clone(&self.links)),
        })
    }

    /// Insert a style-only rule tree for the constructed-sheet JS subset.
    /// CSS Syntax checks the generic rule and constructed import
    /// first; Stylo then parses CSS grammar and checks hierarchy. The
    /// supported-kind gate runs before mutation, leaving rejected rules atomic.
    pub fn insert_supported_style_rule(
        &mut self,
        css: &str,
        index: usize,
    ) -> Result<SheetChange, CssomError> {
        if generic_rule_kind(css)? == GenericRuleKind::Import {
            return Err(CssomError::Syntax);
        }
        let (rules_lock, rule) = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            let rules_lock = Arc::clone(&contents.rules);
            let rule = rules_lock.read_with(&guard).parse_rule_for_insert(
                &self.sheet.shared_lock,
                css,
                contents,
                index,
                CssRuleTypes::default(),
                None,
                None,
                AllowImportRules::No,
            )?;
            check_supported_style_tree(std::slice::from_ref(&rule), &guard, 1)?;
            (rules_lock, rule)
        };
        {
            let mut guard = self.sheet.shared_lock.write();
            rules_lock
                .write_with(&mut guard)
                .0
                .insert(index, rule.clone());
        }
        self.revision = self.revision.saturating_add(1);
        Ok(SheetChange {
            kind: SheetChangeKind::Insertion,
            index: Some(index),
            revision: self.revision,
            rule: Some(rule),
            sheet: Arc::clone(&self.sheet),
            _prune_on_drop: LinkPruneOnDrop(StdArc::clone(&self.links)),
        })
    }

    /// Insert a nested style rule into a CSSStyleRule's live child list.
    /// Stylo parses relative selectors and checks the insertion index before
    /// the supported-kind preflight; rejection never publishes a partial tree.
    pub fn insert_supported_nested_style_rule(
        &mut self,
        parent: &CssRuleHandle,
        css: &str,
        index: usize,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &parent.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &parent.rule else {
            return Err(CssomError::UnsupportedRuleContext);
        };
        let parent_has_sheet = parent.has_parent_style_sheet();
        let (rules_lock, needs_install, rule) = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            let existing = style_rule.read_with(&guard).rules.clone();
            let needs_install = existing.is_none();
            let rules_lock =
                existing.unwrap_or_else(|| CssRules::new(Vec::new(), &self.sheet.shared_lock));
            let rule = rules_lock.read_with(&guard).parse_rule_for_insert(
                &self.sheet.shared_lock,
                css,
                contents,
                index,
                CssRuleTypes::from(CssRuleType::Style),
                Some(CssRuleType::Style),
                None,
                AllowImportRules::No,
            )?;
            check_supported_style_tree(
                std::slice::from_ref(&rule),
                &guard,
                parent.lineage_depth() + 1,
            )?;
            (rules_lock, needs_install, rule)
        };
        if !parent_has_sheet {
            self.links.mark_detached_insertion(&rule);
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            rules_lock.write_with(&mut guard).0.insert(index, rule);
            if needs_install {
                style_rule.write_with(&mut guard).rules = Some(rules_lock);
            }
        }
        Ok(parent.is_attached().then(|| self.full_change()))
    }

    /// Remove one child from a style rule, retaining the removed rule's Arc
    /// for any JavaScript wrapper that already references it.
    pub fn delete_supported_nested_style_rule(
        &mut self,
        parent: &CssRuleHandle,
        index: usize,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &parent.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Style(style_rule) = &parent.rule else {
            return Err(CssomError::UnsupportedRuleContext);
        };
        let rules_lock = {
            let guard = self.sheet.shared_lock.read();
            style_rule
                .read_with(&guard)
                .rules
                .clone()
                .ok_or(CssomError::IndexSize)?
        };
        {
            let mut guard = self.sheet.shared_lock.write();
            rules_lock.write_with(&mut guard).remove_rule(index)?;
        }
        self.links.prune_detached_insertions();
        Ok(parent.is_attached().then(|| self.full_change()))
    }

    /// Remove a top-level rule using Stylo's CSSOM state checks.
    pub fn delete_rule(&mut self, index: usize) -> Result<SheetChange, CssomError> {
        let rules_lock = {
            let guard = self.sheet.shared_lock.read();
            Arc::clone(&self.sheet.contents.read_with(&guard).rules)
        };
        let rule = {
            let guard = self.sheet.shared_lock.read();
            rules_lock
                .read_with(&guard)
                .0
                .get(index)
                .cloned()
                .ok_or(CssomError::IndexSize)?
        };
        if matches!(rule, CssRule::Namespace(_)) {
            return Err(CssomError::InvalidState);
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            rules_lock.write_with(&mut guard).remove_rule(index)?;
        }
        self.links.prune_detached_insertions();
        self.revision = self.revision.saturating_add(1);
        Ok(SheetChange {
            kind: SheetChangeKind::Removal,
            index: Some(index),
            revision: self.revision,
            rule: Some(rule),
            sheet: Arc::clone(&self.sheet),
            _prune_on_drop: LinkPruneOnDrop(StdArc::clone(&self.links)),
        })
    }

    /// Replace the sheet synchronously, as constructed stylesheets do. Stylo
    /// parses the replacement before the old contents are swapped out.
    pub fn replace_sync(&mut self, css: &str) -> SheetChange {
        let contents = StylesheetContents::from_str(
            css,
            UrlExtraData::from(self.base_url.clone()),
            Origin::Author,
            &self.sheet.shared_lock,
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
            None,
        );
        {
            let mut guard = self.sheet.shared_lock.write();
            *self.sheet.contents.write_with(&mut guard) = contents;
        }
        self.links.replaced();
        self.full_change()
    }

    /// Replace a constructed sheet while keeping unsupported concrete rule
    /// kinds out of the first JS bridge. `AllowImportRules::No` implements
    /// replaceSync's import removal before the supported-kind preflight.
    pub fn replace_sync_supported_style_rules(
        &mut self,
        css: &str,
    ) -> Result<SheetChange, CssomError> {
        let contents = StylesheetContents::from_str(
            css,
            UrlExtraData::from(self.base_url.clone()),
            Origin::Author,
            &self.sheet.shared_lock,
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
            None,
        );
        {
            let guard = self.sheet.shared_lock.read();
            check_supported_style_tree(contents.rules(&guard), &guard, 1)?;
        }
        {
            let mut guard = self.sheet.shared_lock.write();
            *self.sheet.contents.write_with(&mut guard) = contents;
        }
        self.links.replaced();
        Ok(self.full_change())
    }

    fn full_change(&mut self) -> SheetChange {
        self.revision = self.revision.saturating_add(1);
        SheetChange {
            kind: SheetChangeKind::Full,
            index: None,
            revision: self.revision,
            rule: None,
            sheet: Arc::clone(&self.sheet),
            _prune_on_drop: LinkPruneOnDrop(StdArc::clone(&self.links)),
        }
    }
}

/// Evaluate a `CSS.supports(conditionText)` expression with Stylo.
pub fn supports_condition(condition: &str, base_url: &Url) -> bool {
    configure_stylo();
    let url_data = UrlExtraData::from(base_url.clone());
    let context = supports_context(&url_data);
    let mut input = ParserInput::new(condition);
    let mut parser = Parser::new(&mut input);
    parser
        .parse_entirely(parse_condition_or_declaration)
        .is_ok_and(|condition| condition.eval(&context))
}

/// Evaluate `CSS.supports(property, value)` with the same Stylo grammar.
pub fn supports_property(property: &str, value: &str, base_url: &Url) -> bool {
    // The two-argument form takes a property name, not arbitrary condition
    // text. Validate it before composing the declaration sent to Stylo.
    if PropertyId::parse_enabled_for_all_content(property).is_err() {
        return false;
    }
    supports_condition(&format!("{property}: {value}"), base_url)
}

/// Every longhand, shorthand and alias name Stylo accepts in content, sorted.
///
/// This is the set `CSSStyleDeclaration` needs one IDL attribute for. It is
/// read after the process-wide preferences are applied, so a property gated on
/// one (D-031 included) is listed exactly when `setProperty` would accept it.
pub fn supported_property_names() -> Vec<&'static str> {
    configure_stylo();
    let mut names: Vec<&'static str> = NonCustomPropertyId::iter()
        .map(NonCustomPropertyId::name)
        .filter(|name| PropertyId::parse_enabled_for_all_content(name).is_ok())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn supports_context(url_data: &UrlExtraData) -> ParserContext<'_> {
    ParserContext::new(
        Origin::Author,
        url_data,
        Some(CssRuleType::Style),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    )
}

pub(crate) fn configure_stylo() {
    // Stylo's standalone Servo defaults leave several broadly deployed CSS
    // features disabled. These preferences are process-wide, so configure them
    // once before the first stylesheet or CSS.supports parse in any page.
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        stylo_static_prefs::set_pref!("layout.grid.enabled", true);
        stylo_static_prefs::set_pref!("layout.writing-mode.enabled", true);
        stylo_static_prefs::set_pref!("layout.css.has-selector.enabled", true);
        stylo_static_prefs::set_pref!("layout.css.nth-child-of.enabled", true);
        stylo_static_prefs::set_pref!("layout.container-queries.enabled", true);
        stylo_static_prefs::set_pref!("layout.css.at-scope.enabled", true);
        stylo_static_prefs::set_pref!("layout.unimplemented", true);
    });
}

#[cfg(test)]
mod sheet_link_tests {
    use super::*;

    #[test]
    fn nth_child_of_selectors_parse_into_a_rule() {
        let base = Url::parse("https://example.test/style.css").unwrap();
        let sheet = CssStyleSheet::parse("li:nth-child(2n+1 of .x) { color: red }", base);
        assert!(sheet.rule_at(0).is_some());
    }

    #[test]
    fn detached_insertion_marker_releases_without_another_mutation() {
        let base = Url::parse("https://example.test/style.css").unwrap();
        let mut sheet = CssStyleSheet::parse(".parent { color: red }", base);
        let parent = sheet.rule_at(0).unwrap();
        sheet.delete_rule(0).unwrap();

        sheet
            .insert_supported_nested_style_rule(&parent, "& .child { color: blue }", 0)
            .unwrap();
        let child = parent.child_at(0).unwrap();
        assert!(!child.has_parent_style_sheet());
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);

        sheet
            .delete_supported_nested_style_rule(&parent, 0)
            .unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);
        drop(child);
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 0);

        sheet
            .insert_supported_nested_style_rule(&parent, "& .unobserved { color: green }", 0)
            .unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);
        sheet
            .delete_supported_nested_style_rule(&parent, 0)
            .unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 0);
    }
}
