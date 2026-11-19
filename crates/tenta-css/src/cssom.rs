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
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, ToCss, parse_one_rule,
};
use selectors::parser::{ParseRelative, SelectorList};
use servo_arc::Arc;
use style::context::QuirksMode;
use style::device::Device;
use style::invalidation::stylesheets::RuleChangeKind;
use style::media_queries::{MediaList, MediaQuery};
use style::parser::ParserContext;
use style::properties::declaration_block::{
    SourcePropertyDeclarationUpdate, parse_one_declaration_into,
};
use style::properties::{
    Importance, NonCustomPropertyId, PropertyDeclarationBlock, PropertyId,
    SourcePropertyDeclaration, parse_style_attribute,
};
use style::selector_parser::{SelectorImpl, SelectorParser};
use style::shared_lock::{Locked, SharedRwLock, SharedRwLockReadGuard, ToCssWithGuard};
use style::stylesheet_set::DocumentStylesheetSet;
use style::stylesheets::KeyframesRule;
use style::stylesheets::keyframes_rule::{Keyframe, KeyframeSelectors};
use style::stylesheets::supports_rule::parse_condition_or_declaration;
use style::stylesheets::{
    AllowImportRules, CssRule, CssRuleTypes, CssRules, CustomMediaMap, DocumentStyleSheet, Origin,
    OriginSet, RulesMutateError, Stylesheet, StylesheetContents, StylesheetLoader, UrlExtraData,
};
use style::values::KeyframesName;
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
    /// What was to be removed is not there.
    NotFound,
    /// The rule has no child rule list of its own to change.
    UnsupportedRuleContext,
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
        set_property_in_block(
            &mut self.block,
            &self.url_data,
            CssRuleType::Style,
            name,
            value,
            priority,
        )
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
    kind: CssRuleType,
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
        // A keyframe's declarations cannot be important.
        priority
            if priority.eq_ignore_ascii_case("important")
                && !CssRuleTypes::IMPORTANT_FORBIDDEN.contains(kind) =>
        {
            Importance::Important
        }
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
        kind,
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

/// The descriptors of an `@property` rule, each as it serializes. A
/// descriptor the rule leaves out is `None`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssPropertyRegistration {
    /// The custom property's name, with its `--`.
    pub name: String,
    pub syntax: Option<String>,
    /// `true` or `false`.
    pub inherits: Option<String>,
    pub initial_value: Option<String>,
}

/// Reads `name: value;` pairs out of a serialized descriptor block. The text
/// is Stylo's own serialization, so every declaration is well formed; this
/// only has to find where each value ends, which cssparser's tokenizer knows
/// (a `;` inside a string or a `url()` is not one).
fn descriptor_list(body: &str) -> Vec<(String, String)> {
    struct Descriptors;
    impl<'i> DeclarationParser<'i> for Descriptors {
        type Declaration = (String, String);
        type Error = ();
        fn parse_value<'t>(
            &mut self,
            name: CowRcStr<'i>,
            input: &mut Parser<'i, 't>,
            _start: &ParserState,
        ) -> Result<Self::Declaration, ParseError<'i, ()>> {
            let start = input.position();
            consume_generic_tokens(input);
            Ok((name.to_string(), input.slice_from(start).trim().to_owned()))
        }
    }
    impl<'i> AtRuleParser<'i> for Descriptors {
        type Prelude = ();
        type AtRule = (String, String);
        type Error = ();
    }
    impl<'i> QualifiedRuleParser<'i> for Descriptors {
        type Prelude = ();
        type QualifiedRule = (String, String);
        type Error = ();
    }
    impl<'i> RuleBodyItemParser<'i, (String, String), ()> for Descriptors {
        fn parse_declarations(&self) -> bool {
            true
        }
        fn parse_qualified(&self) -> bool {
            false
        }
    }
    let mut input = ParserInput::new(body);
    let mut input = Parser::new(&mut input);
    RuleBodyParser::new(&mut input, &mut Descriptors)
        .flatten()
        .collect()
}

/// One media query, when `text` is exactly one. Stylo's own
/// `MediaList::append_medium` stops at the first query it can read and
/// ignores what follows, which would take `print and nonsense` as `print`.
fn parse_one_medium(context: &ParserContext<'_>, text: &str) -> Option<MediaQuery> {
    let mut input = ParserInput::new(text);
    let mut input = Parser::new(&mut input);
    input
        .parse_entirely(|input| MediaQuery::parse(context, input))
        .ok()
}

/// The serialization of a value that is written without a lock guard.
fn serialized<T: style_traits::ToCss>(value: &T) -> String {
    style_traits::ToCss::to_css_string(value)
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

// A rule that script inserts is checked the same way before any part of it is
// published into a live rule list, so that the list never holds a tree that
// cannot be read back.
fn check_inserted_depth(
    rules: &[CssRule],
    guard: &SharedRwLockReadGuard<'_>,
    initial_depth: usize,
) -> Result<(), CssomError> {
    let mut pending: Vec<_> = rules.iter().map(|rule| (rule, initial_depth)).collect();
    while let Some((rule, depth)) = pending.pop() {
        if depth > MAX_RULE_SNAPSHOT_DEPTH {
            return Err(CssomError::DepthLimit);
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

/// Whether this `CssRule` is the last reference to its allocation.
fn rule_is_unique(rule: &CssRule) -> bool {
    match rule {
        CssRule::Style(rule) => rule.is_unique(),
        CssRule::Namespace(rule) => rule.is_unique(),
        CssRule::Import(rule) => rule.is_unique(),
        CssRule::Media(rule) => rule.is_unique(),
        CssRule::CustomMedia(rule) => rule.is_unique(),
        CssRule::Container(rule) => rule.is_unique(),
        CssRule::FontFace(rule) => rule.is_unique(),
        CssRule::FontFeatureValues(rule) => rule.is_unique(),
        CssRule::FontPaletteValues(rule) => rule.is_unique(),
        CssRule::CounterStyle(rule) => rule.is_unique(),
        CssRule::Keyframes(rule) => rule.is_unique(),
        CssRule::Margin(rule) => rule.is_unique(),
        CssRule::Supports(rule) => rule.is_unique(),
        CssRule::Page(rule) => rule.is_unique(),
        CssRule::Property(rule) => rule.is_unique(),
        CssRule::Document(rule) => rule.is_unique(),
        CssRule::LayerBlock(rule) => rule.is_unique(),
        CssRule::LayerStatement(rule) => rule.is_unique(),
        CssRule::Scope(rule) => rule.is_unique(),
        CssRule::StartingStyle(rule) => rule.is_unique(),
        CssRule::AppearanceBase(rule) => rule.is_unique(),
        CssRule::PositionTry(rule) => rule.is_unique(),
        CssRule::NestedDeclarations(rule) => rule.is_unique(),
        CssRule::ViewTransition(rule) => rule.is_unique(),
    }
}

/// The declaration block of a rule that is made of one: a style rule, or the
/// declarations that follow a nested rule inside one.
fn declaration_block(
    rule: &CssRule,
    guard: &SharedRwLockReadGuard<'_>,
) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
    match rule {
        CssRule::Style(rule) => Some(Arc::clone(&rule.read_with(guard).block)),
        CssRule::NestedDeclarations(rule) => Some(Arc::clone(&rule.read_with(guard).block)),
        _ => None,
    }
}

/// The child rule list a rule keeps, which is what `insertRule()` and
/// `deleteRule()` on it change.
enum ChildList {
    /// The rule has no list of child rules: it is not a grouping rule.
    NotGrouping,
    /// A style rule that has had no nested rule so far.
    Absent,
    List(Arc<Locked<CssRules>>),
}

fn child_list(rule: &CssRule, guard: &SharedRwLockReadGuard<'_>) -> ChildList {
    let list = match rule {
        CssRule::Style(rule) => match &rule.read_with(guard).rules {
            Some(rules) => rules,
            None => return ChildList::Absent,
        },
        CssRule::Media(rule) => &rule.rules,
        CssRule::Supports(rule) => &rule.rules,
        CssRule::Container(rule) => &rule.rules,
        CssRule::LayerBlock(rule) => &rule.rules,
        CssRule::Scope(rule) => &rule.rules,
        CssRule::StartingStyle(rule) => &rule.rules,
        CssRule::Document(rule) => &rule.rules,
        CssRule::Page(rule) => &rule.read_with(guard).rules,
        _ => return ChildList::NotGrouping,
    };
    ChildList::List(Arc::clone(list))
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
            markers.retain(|_, rule| !rule_is_unique(rule));
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

impl RuleParent {
    /// What a rule parsed under this one is told about where it is: the kinds
    /// of this rule and all its ancestors, and the nearest of them that makes
    /// selectors relative (a style rule or an `@scope`).
    fn nesting_context(&self) -> (CssRuleTypes, Option<CssRuleType>) {
        let mut containing = CssRuleTypes::default();
        let mut relative = None;
        let mut ancestor = Some(self);
        while let Some(parent) = ancestor {
            let kind = parent.rule.rule_type();
            containing.insert(kind);
            if relative.is_none() && matches!(kind, CssRuleType::Style | CssRuleType::Scope) {
                relative = Some(kind);
            }
            ancestor = parent.parent.as_deref();
        }
        (containing, relative)
    }
}

impl CssRuleHandle {
    /// The nesting context of a child of this rule; see
    /// [`RuleParent::nesting_context`].
    fn nesting_context(&self) -> (CssRuleTypes, Option<CssRuleType>) {
        let kind = self.rule.rule_type();
        let (mut containing, mut relative) = self
            .parent
            .as_deref()
            .map(RuleParent::nesting_context)
            .unwrap_or_default();
        containing.insert(kind);
        if matches!(kind, CssRuleType::Style | CssRuleType::Scope) {
            relative = Some(kind);
        }
        (containing, relative)
    }

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

    /// Whether this rule has a declaration block: a style rule, or the
    /// declarations that follow a nested rule inside one.
    pub fn has_style(&self) -> bool {
        matches!(
            self.rule,
            CssRule::Style(_) | CssRule::NestedDeclarations(_)
        )
    }

    /// Read the effective declarations of a rule from its live Stylo block.
    pub fn style_declarations(&self) -> Option<CssDeclarationSnapshot> {
        let guard = self.links.0.sheet.shared_lock.read();
        let block = declaration_block(&self.rule, &guard)?;
        Some(CssDeclarationSnapshot::from_block(block.read_with(&guard)))
    }

    /// Return a CSSOM property value from a rule's live declarations.
    pub fn style_property_value(&self, name: &str) -> Option<String> {
        let guard = self.links.0.sheet.shared_lock.read();
        let block = declaration_block(&self.rule, &guard)?;
        Some(property_value_in_block(block.read_with(&guard), name))
    }

    /// Return a CSSOM property's important flag for a rule's declarations.
    pub fn style_property_is_important(&self, name: &str) -> Option<bool> {
        let guard = self.links.0.sheet.shared_lock.read();
        let block = declaration_block(&self.rule, &guard)?;
        let Ok(id) = PropertyId::parse_enabled_for_all_content(name) else {
            return Some(false);
        };
        Some(block.read_with(&guard).property_priority(&id).important())
    }

    /// The address of this rule's Stylo allocation: equal for two handles
    /// exactly when [`Self::same_rule`] holds within one sheet, and not reused
    /// while any handle to the rule lives.
    pub fn identity(&self) -> usize {
        rule_address(&self.rule)
    }

    /// The font families an `@font-feature-values` rule is for, serialized as
    /// the rule's prelude.
    pub fn font_feature_family(&self) -> Option<String> {
        let CssRule::FontFeatureValues(rule) = &self.rule else {
            return None;
        };
        let names: Vec<String> = rule.family_names.iter().map(serialized).collect();
        Some(names.join(", "))
    }

    /// Whether this rule keeps a list of child rules that `insertRule()` and
    /// `deleteRule()` can change: the kinds CSSOM gives `CSSGroupingRule`.
    pub fn is_grouping(&self) -> bool {
        let guard = self.links.0.sheet.shared_lock.read();
        !matches!(child_list(&self.rule, &guard), ChildList::NotGrouping)
    }

    /// The condition of a conditional group rule as CSSOM's `conditionText`
    /// has it: the media query list of `@media` and the condition of
    /// `@supports`. (Stylo's Servo configuration parses no `@container`.)
    pub fn condition_text(&self) -> Option<String> {
        match &self.rule {
            CssRule::Media(_) => self.media_text(),
            CssRule::Supports(rule) => Some(serialized(&rule.condition)),
            _ => None,
        }
    }

    /// The media query list of an `@media` rule, serialized.
    pub fn media_text(&self) -> Option<String> {
        let CssRule::Media(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        Some(serialized(rule.media_queries.read_with(&guard)))
    }

    /// Each media query of an `@media` rule, serialized on its own.
    pub fn media_queries(&self) -> Option<Vec<String>> {
        let CssRule::Media(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        Some(
            rule.media_queries
                .read_with(&guard)
                .media_queries
                .iter()
                .map(serialized)
                .collect(),
        )
    }

    /// Whether the condition of an `@supports` rule held when it was parsed.
    pub fn supports_matches(&self) -> Option<bool> {
        match &self.rule {
            CssRule::Supports(rule) => Some(rule.enabled),
            _ => None,
        }
    }

    /// The name of an `@layer` block; empty for an anonymous layer.
    pub fn layer_name(&self) -> Option<String> {
        match &self.rule {
            CssRule::LayerBlock(rule) => {
                Some(rule.name.as_ref().map(serialized).unwrap_or_default())
            }
            _ => None,
        }
    }

    /// The names an `@layer` statement declares, in order.
    pub fn layer_names(&self) -> Option<Vec<String>> {
        match &self.rule {
            CssRule::LayerStatement(rule) => Some(rule.names.iter().map(serialized).collect()),
            _ => None,
        }
    }

    /// The `(start, end)` selector lists of an `@scope` rule; a bound the
    /// rule does not give is `None`.
    pub fn scope_bounds(&self) -> Option<(Option<String>, Option<String>)> {
        let CssRule::Scope(rule) = &self.rule else {
            return None;
        };
        let text = |list: &Option<SelectorList<SelectorImpl>>| {
            list.as_ref().map(|list| list.to_css_string())
        };
        Some((text(&rule.bounds.start), text(&rule.bounds.end)))
    }

    /// The `(prefix, namespace URL)` of an `@namespace` rule; the default
    /// namespace has the empty prefix.
    pub fn namespace(&self) -> Option<(String, String)> {
        let CssRule::Namespace(rule) = &self.rule else {
            return None;
        };
        let prefix = rule
            .prefix
            .as_ref()
            .map(|prefix| prefix.to_string())
            .unwrap_or_default();
        Some((prefix, rule.url.to_string()))
    }

    /// The name of an `@keyframes` rule: the name itself, whether the rule
    /// spells it as an identifier or as a string.
    pub fn keyframes_name(&self) -> Option<String> {
        let CssRule::Keyframes(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        Some(rule.read_with(&guard).name.as_atom().to_string())
    }

    /// How many keyframes an `@keyframes` rule has.
    pub fn keyframe_count(&self) -> Option<usize> {
        let CssRule::Keyframes(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        Some(rule.read_with(&guard).keyframes.len())
    }

    /// The keyframe at `index` of an `@keyframes` rule.
    pub fn keyframe_at(&self, index: usize) -> Option<CssKeyframeHandle> {
        let CssRule::Keyframes(rule) = &self.rule else {
            return None;
        };
        let guard = self.links.0.sheet.shared_lock.read();
        let keyframe = Arc::clone(rule.read_with(&guard).keyframes.get(index)?);
        Some(CssKeyframeHandle {
            parent: self.clone(),
            keyframe,
        })
    }

    /// `CSSKeyframesRule.findRule()`: the last keyframe whose selector is
    /// `selector`, compared as parsed (`from` finds `0%`).
    pub fn find_keyframe(&self, selector: &str) -> Option<CssKeyframeHandle> {
        let CssRule::Keyframes(rule) = &self.rule else {
            return None;
        };
        let index = {
            let guard = self.links.0.sheet.shared_lock.read();
            rule.read_with(&guard).find_rule(&guard, selector)?
        };
        self.keyframe_at(index)
    }

    /// The descriptors of an `@font-face` rule, each as it serializes, in the
    /// order the rule serializes them.
    pub fn font_face_descriptors(&self) -> Option<Vec<(String, String)>> {
        if !matches!(self.rule, CssRule::FontFace(_)) {
            return None;
        }
        let text = self.css_text().ok()?;
        let body = text.strip_prefix("@font-face")?.trim();
        let body = body.strip_prefix('{')?.strip_suffix('}')?;
        Some(descriptor_list(body))
    }

    /// What an `@property` rule registers.
    pub fn property_registration(&self) -> Option<CssPropertyRegistration> {
        let CssRule::Property(rule) = &self.rule else {
            return None;
        };
        let descriptors = &rule.descriptors;
        Some(CssPropertyRegistration {
            name: serialized(&rule.name),
            syntax: descriptors.syntax.as_ref().map(serialized),
            inherits: descriptors.inherits.as_ref().map(serialized),
            initial_value: descriptors
                .initial_value
                .as_ref()
                .map(|value| serialized(&**value)),
        })
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

/// One keyframe of an `@keyframes` rule: what `CSSKeyframeRule` is backed by.
///
/// A keyframe is not a rule in Stylo's rule tree, so it has a handle of its
/// own. Like a rule handle it retains its allocation after it leaves the
/// rule's list, and stays readable and writable there.
#[derive(Clone)]
pub struct CssKeyframeHandle {
    parent: CssRuleHandle,
    keyframe: Arc<Locked<Keyframe>>,
}

impl CssKeyframeHandle {
    /// The address of the keyframe's allocation; see
    /// [`CssRuleHandle::identity`].
    pub fn identity(&self) -> usize {
        self.keyframe.raw_ptr().as_ptr() as usize
    }

    /// The `@keyframes` rule this keyframe was read from, whether or not the
    /// keyframe is still in it.
    pub fn keyframes_rule(&self) -> &CssRuleHandle {
        &self.parent
    }

    fn is_listed(&self) -> bool {
        let CssRule::Keyframes(rule) = &self.parent.rule else {
            return false;
        };
        let guard = self.parent.links.0.sheet.shared_lock.read();
        rule.read_with(&guard)
            .keyframes
            .iter()
            .any(|listed| Arc::ptr_eq(listed, &self.keyframe))
    }

    /// `parentRule`: the `@keyframes` rule while the keyframe is in its list.
    pub fn parent_rule(&self) -> Option<CssRuleHandle> {
        self.is_listed().then(|| self.parent.clone())
    }

    /// `parentStyleSheet`: the sheet of the `@keyframes` rule the keyframe is
    /// listed in.
    pub fn has_parent_style_sheet(&self) -> bool {
        self.is_listed() && self.parent.has_parent_style_sheet()
    }

    /// Whether the keyframe takes part in the sheet's current rule tree.
    pub fn is_attached(&self) -> bool {
        self.is_listed() && self.parent.is_attached()
    }

    /// `keyText`: the keyframe's selectors, as percentages.
    pub fn key_text(&self) -> String {
        let guard = self.parent.links.0.sheet.shared_lock.read();
        serialized(&self.keyframe.read_with(&guard).selector)
    }

    pub fn css_text(&self) -> String {
        let guard = self.parent.links.0.sheet.shared_lock.read();
        let mut text = String::new();
        self.keyframe
            .read_with(&guard)
            .to_css(&guard, &mut text)
            .expect("serializing CSS into a String cannot fail");
        text
    }

    /// Read the keyframe's declarations from its live Stylo block.
    pub fn style_declarations(&self) -> CssDeclarationSnapshot {
        let guard = self.parent.links.0.sheet.shared_lock.read();
        let keyframe = self.keyframe.read_with(&guard);
        CssDeclarationSnapshot::from_block(keyframe.block.read_with(&guard))
    }

    pub fn style_property_value(&self, name: &str) -> String {
        let guard = self.parent.links.0.sheet.shared_lock.read();
        let keyframe = self.keyframe.read_with(&guard);
        property_value_in_block(keyframe.block.read_with(&guard), name)
    }
}

/// Something with a declaration block the sheet can rewrite: a rule or a
/// keyframe.
trait BlockOwner {
    /// The kind of rule the declarations are parsed for.
    const KIND: CssRuleType;
    fn sheet(&self) -> &Arc<Stylesheet>;
    fn block(
        &self,
        guard: &SharedRwLockReadGuard<'_>,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>>;
    /// What the sheet's document has to be told after the block changed.
    fn declarations_changed(&self, sheet: &mut CssStyleSheet) -> Option<SheetChange>;
}

impl BlockOwner for CssRuleHandle {
    const KIND: CssRuleType = CssRuleType::Style;
    fn sheet(&self) -> &Arc<Stylesheet> {
        &self.links.0.sheet
    }
    fn block(
        &self,
        guard: &SharedRwLockReadGuard<'_>,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        declaration_block(&self.rule, guard)
    }
    fn declarations_changed(&self, sheet: &mut CssStyleSheet) -> Option<SheetChange> {
        sheet.change_after_style_declarations(self)
    }
}

impl BlockOwner for CssKeyframeHandle {
    const KIND: CssRuleType = CssRuleType::Keyframe;
    fn sheet(&self) -> &Arc<Stylesheet> {
        &self.parent.links.0.sheet
    }
    fn block(
        &self,
        guard: &SharedRwLockReadGuard<'_>,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        Some(Arc::clone(&self.keyframe.read_with(guard).block))
    }
    fn declarations_changed(&self, sheet: &mut CssStyleSheet) -> Option<SheetChange> {
        self.is_attached().then(|| sheet.full_change())
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
    /// sheet. A rule under a style rule or an `@scope` is parsed with that
    /// ancestor's relative selector syntax.
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
        let relative = match rule
            .parent
            .as_deref()
            .map(RuleParent::nesting_context)
            .and_then(|(_, relative)| relative)
        {
            Some(CssRuleType::Scope) => ParseRelative::ForScope,
            Some(_) => ParseRelative::ForNesting,
            None => ParseRelative::No,
        };
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

    /// Replace the media query list of an `@media` rule, as assigning
    /// `mediaText` does. A query that does not parse becomes `not all`.
    pub fn set_rule_media_text(
        &mut self,
        rule: &CssRuleHandle,
        text: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.change_rule_media(rule, |list, context| {
            let mut input = ParserInput::new(text);
            let mut input = Parser::new(&mut input);
            *list = MediaList::parse(context, &mut input);
            true
        })
    }

    /// `MediaList.appendMedium()`: add one query, moving it to the end when it
    /// is already listed. Text that is not one media query changes nothing.
    pub fn append_rule_medium(
        &mut self,
        rule: &CssRuleHandle,
        medium: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.change_rule_media(rule, |list, context| {
            let Some(query) = parse_one_medium(context, medium) else {
                return false;
            };
            list.media_queries.retain(|listed| listed != &query);
            list.media_queries.push(query);
            true
        })
    }

    /// `MediaList.deleteMedium()`: remove one query. `NotFound` when the list
    /// does not have it.
    pub fn delete_rule_medium(
        &mut self,
        rule: &CssRuleHandle,
        medium: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let mut found = false;
        let change = self.change_rule_media(rule, |list, context| {
            let Some(query) = parse_one_medium(context, medium) else {
                return false;
            };
            let before = list.media_queries.len();
            list.media_queries.retain(|listed| listed != &query);
            found = list.media_queries.len() != before;
            found
        })?;
        if found {
            Ok(change)
        } else {
            Err(CssomError::NotFound)
        }
    }

    fn change_rule_media(
        &mut self,
        rule: &CssRuleHandle,
        change: impl FnOnce(&mut MediaList, &mut ParserContext<'_>) -> bool,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let CssRule::Media(media_rule) = &rule.rule else {
            return Err(CssomError::InvalidState);
        };
        let url_data = UrlExtraData::from(self.base_url.clone());
        let mut context = ParserContext::new(
            Origin::Author,
            &url_data,
            Some(CssRuleType::Media),
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            Default::default(),
            None,
            None,
            Default::default(),
        );
        let changed = {
            let mut guard = self.sheet.shared_lock.write();
            change(
                media_rule.media_queries.write_with(&mut guard),
                &mut context,
            )
        };
        Ok((changed && rule.is_attached()).then(|| self.full_change()))
    }

    /// Replace a rule's declaration block without changing rule identity.
    /// Invalid declarations are discarded by Stylo, as in CSSOM's cssText setter.
    pub fn set_rule_style_css_text(
        &mut self,
        rule: &CssRuleHandle,
        css: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.set_style_css_text(rule, css)
    }

    /// Apply CSSOM setProperty to a rule's live Stylo declaration block.
    /// Invalid names, values and priorities leave the rule unchanged.
    pub fn set_rule_style_property(
        &mut self,
        rule: &CssRuleHandle,
        name: &str,
        value: &str,
        priority: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.set_style_property(rule, name, value, priority)
    }

    /// Apply CSSOM removeProperty and return its previous serialized value.
    pub fn remove_rule_style_property(
        &mut self,
        rule: &CssRuleHandle,
        name: &str,
    ) -> Result<(String, Option<SheetChange>), CssomError> {
        self.remove_style_property(rule, name)
    }

    /// [`Self::set_rule_style_css_text`] for the declarations of a keyframe.
    pub fn set_keyframe_style_css_text(
        &mut self,
        keyframe: &CssKeyframeHandle,
        css: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.set_style_css_text(keyframe, css)
    }

    /// [`Self::set_rule_style_property`] for the declarations of a keyframe,
    /// where no declaration can be important.
    pub fn set_keyframe_style_property(
        &mut self,
        keyframe: &CssKeyframeHandle,
        name: &str,
        value: &str,
        priority: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        self.set_style_property(keyframe, name, value, priority)
    }

    /// [`Self::remove_rule_style_property`] for the declarations of a keyframe.
    pub fn remove_keyframe_style_property(
        &mut self,
        keyframe: &CssKeyframeHandle,
        name: &str,
    ) -> Result<(String, Option<SheetChange>), CssomError> {
        self.remove_style_property(keyframe, name)
    }

    fn owned_block<O: BlockOwner>(
        &self,
        owner: &O,
    ) -> Result<Arc<Locked<PropertyDeclarationBlock>>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, owner.sheet()) {
            return Err(CssomError::InvalidState);
        }
        let guard = self.sheet.shared_lock.read();
        owner.block(&guard).ok_or(CssomError::InvalidState)
    }

    fn set_style_css_text<O: BlockOwner>(
        &mut self,
        owner: &O,
        css: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let block = self.owned_block(owner)?;
        let url_data = UrlExtraData::from(self.base_url.clone());
        let parsed = parse_style_attribute(css, &url_data, None, QuirksMode::NoQuirks, O::KIND);
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
        Ok(owner.declarations_changed(self))
    }

    fn set_style_property<O: BlockOwner>(
        &mut self,
        owner: &O,
        name: &str,
        value: &str,
        priority: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let block = self.owned_block(owner)?;
        let url_data = UrlExtraData::from(self.base_url.clone());
        let changed = {
            let mut guard = self.sheet.shared_lock.write();
            set_property_in_block(
                block.write_with(&mut guard),
                &url_data,
                O::KIND,
                name,
                value,
                priority,
            )
        };
        Ok(changed.then(|| owner.declarations_changed(self)).flatten())
    }

    fn remove_style_property<O: BlockOwner>(
        &mut self,
        owner: &O,
        name: &str,
    ) -> Result<(String, Option<SheetChange>), CssomError> {
        let block = self.owned_block(owner)?;
        let removed = {
            let mut guard = self.sheet.shared_lock.write();
            remove_property_from_block(block.write_with(&mut guard), name)
        };
        match removed {
            Some(value) => Ok((value, owner.declarations_changed(self))),
            None => Ok((String::new(), None)),
        }
    }

    /// Rename an `@keyframes` rule. Any text is a name: one that is not an
    /// identifier serializes as a string.
    pub fn set_keyframes_name(
        &mut self,
        rule: &CssRuleHandle,
        name: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let keyframes = self.keyframes_of(rule)?;
        {
            let mut guard = self.sheet.shared_lock.write();
            keyframes.write_with(&mut guard).name = KeyframesName::from_ident(name);
        }
        Ok(rule.is_attached().then(|| self.full_change()))
    }

    /// `CSSKeyframesRule.appendRule()`: add one keyframe at the end. Text that
    /// is not one keyframe changes nothing, as CSS Animations has it.
    pub fn append_keyframe(
        &mut self,
        rule: &CssRuleHandle,
        css: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let keyframes = self.keyframes_of(rule)?;
        let parsed = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            Keyframe::parse(css, contents, &self.sheet.shared_lock).ok()
        };
        let Some(keyframe) = parsed else {
            return Ok(None);
        };
        {
            let mut guard = self.sheet.shared_lock.write();
            keyframes.write_with(&mut guard).keyframes.push(keyframe);
        }
        Ok(rule.is_attached().then(|| self.full_change()))
    }

    /// `CSSKeyframesRule.deleteRule()`: remove the last keyframe whose
    /// selector is `selector`. No such keyframe changes nothing.
    pub fn delete_keyframe(
        &mut self,
        rule: &CssRuleHandle,
        selector: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        let keyframes = self.keyframes_of(rule)?;
        let found = {
            let guard = self.sheet.shared_lock.read();
            keyframes.read_with(&guard).find_rule(&guard, selector)
        };
        let Some(index) = found else {
            return Ok(None);
        };
        {
            let mut guard = self.sheet.shared_lock.write();
            keyframes.write_with(&mut guard).keyframes.remove(index);
        }
        Ok(rule.is_attached().then(|| self.full_change()))
    }

    /// Assign `CSSKeyframeRule.keyText`. `Syntax` when `text` is not a list
    /// of keyframe selectors.
    pub fn set_keyframe_key_text(
        &mut self,
        keyframe: &CssKeyframeHandle,
        text: &str,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &keyframe.parent.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let mut input = ParserInput::new(text);
        let selector = Parser::new(&mut input)
            .parse_entirely(KeyframeSelectors::parse)
            .map_err(|_| CssomError::Syntax)?;
        {
            let mut guard = self.sheet.shared_lock.write();
            keyframe.keyframe.write_with(&mut guard).selector = selector;
        }
        Ok(keyframe.is_attached().then(|| self.full_change()))
    }

    fn keyframes_of(&self, rule: &CssRuleHandle) -> Result<Arc<Locked<KeyframesRule>>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &rule.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        match &rule.rule {
            CssRule::Keyframes(keyframes) => Ok(Arc::clone(keyframes)),
            _ => Err(CssomError::InvalidState),
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

    /// Insert exactly one rule at the top level of the sheet.
    ///
    /// CSS Syntax's generic single-rule parse runs first, as CSSOM orders it,
    /// so malformed text is `Syntax` whatever the index; an `@import` is
    /// `Syntax` too, because a sheet without a loader cannot fetch one. Stylo
    /// then parses the grammar and checks the index and the rule's position.
    /// Nothing is published when any of these refuse.
    pub fn insert_rule(&mut self, css: &str, index: usize) -> Result<SheetChange, CssomError> {
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
            // Namespace mutations need a corresponding update to the sheet's
            // namespace map. Defer those until the sheet can preserve live
            // rule identities across that update.
            if matches!(rule, CssRule::Namespace(_)) {
                return Err(CssomError::InvalidState);
            }
            check_inserted_depth(std::slice::from_ref(&rule), &guard, 1)?;
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

    /// Insert one rule into the child list of a grouping rule: a style rule,
    /// `@media`, `@supports`, `@container`, `@layer`, `@scope`,
    /// `@starting-style`, `@document` or `@page`.
    ///
    /// The rule is parsed as it would be where it is going: Stylo is told the
    /// kinds of every ancestor, and whether a style rule or an `@scope` among
    /// them makes selectors relative. A rejected rule publishes nothing.
    pub fn insert_nested_rule(
        &mut self,
        parent: &CssRuleHandle,
        css: &str,
        index: usize,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &parent.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let parent_has_sheet = parent.has_parent_style_sheet();
        let (rules_lock, needs_install, rule) = {
            let guard = self.sheet.shared_lock.read();
            let contents = self.sheet.contents.read_with(&guard);
            let (rules_lock, needs_install) = match child_list(&parent.rule, &guard) {
                ChildList::NotGrouping => return Err(CssomError::UnsupportedRuleContext),
                ChildList::Absent => (CssRules::new(Vec::new(), &self.sheet.shared_lock), true),
                ChildList::List(list) => (list, false),
            };
            let (containing, relative) = parent.nesting_context();
            let rule = rules_lock.read_with(&guard).parse_rule_for_insert(
                &self.sheet.shared_lock,
                css,
                contents,
                index,
                containing,
                relative,
                None,
                AllowImportRules::No,
            )?;
            check_inserted_depth(
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
            if needs_install && let CssRule::Style(style_rule) = &parent.rule {
                style_rule.write_with(&mut guard).rules = Some(rules_lock);
            }
        }
        Ok(parent.is_attached().then(|| self.full_change()))
    }

    /// Remove one child from a grouping rule, retaining the removed rule's
    /// Arc for any JavaScript wrapper that already references it.
    pub fn delete_nested_rule(
        &mut self,
        parent: &CssRuleHandle,
        index: usize,
    ) -> Result<Option<SheetChange>, CssomError> {
        if !Arc::ptr_eq(&self.sheet, &parent.links.0.sheet) {
            return Err(CssomError::InvalidState);
        }
        let rules_lock = {
            let guard = self.sheet.shared_lock.read();
            match child_list(&parent.rule, &guard) {
                ChildList::NotGrouping => return Err(CssomError::UnsupportedRuleContext),
                ChildList::Absent => return Err(CssomError::IndexSize),
                ChildList::List(list) => list,
            }
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
    /// parses the replacement before the old contents are swapped out;
    /// `AllowImportRules::No` is `replaceSync()`'s removal of `@import`. A
    /// replacement nested too deeply to be read back is refused whole.
    pub fn replace_sync(&mut self, css: &str) -> Result<SheetChange, CssomError> {
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
            check_inserted_depth(contents.rules(&guard), &guard, 1)?;
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

/// `CSS.escape(ident)`: the identifier serialized as CSSOM has it
/// (<https://drafts.csswg.org/cssom/#serialize-an-identifier>), so that it can
/// be put in a selector.
pub fn escape_identifier(ident: &str) -> String {
    let mut escaped = String::with_capacity(ident.len());
    // Writing to a `String` cannot fail.
    let _ = cssparser::serialize_identifier(ident, &mut escaped);
    escaped
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
            .insert_nested_rule(&parent, "& .child { color: blue }", 0)
            .unwrap();
        let child = parent.child_at(0).unwrap();
        assert!(!child.has_parent_style_sheet());
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);

        sheet.delete_nested_rule(&parent, 0).unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);
        drop(child);
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 0);

        sheet
            .insert_nested_rule(&parent, "& .unobserved { color: green }", 0)
            .unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 1);
        sheet.delete_nested_rule(&parent, 0).unwrap();
        assert_eq!(sheet.links.marker_count.load(Ordering::Acquire), 0);
    }
}
