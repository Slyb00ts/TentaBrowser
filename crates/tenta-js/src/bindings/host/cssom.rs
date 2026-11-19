//! CSS stylesheets and live rule wrappers backed by Stylo.
//!
//! Every rule of a sheet is an object of the interface CSSOM gives its kind
//! (`CSSStyleRule`, `CSSMediaRule`, `CSSKeyframesRule`, ...), chosen from the
//! Stylo rule when the wrapper is made; [`rule_interface`] is that choice. A
//! kind without an interface here is a plain `CSSRule` with `type` 0, so that a
//! loop over `cssRules` never stops at a rule it does not know.

pub mod graph;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tenta_css::{
    CssDeclarationSnapshot, CssKeyframeHandle, CssPropertySnapshot, CssRuleHandle, CssRuleType,
    CssStyleSheet, CssomError,
};

use super::*;
use crate::bindings::{
    CSSConditionRuleOps, CSSFontFaceRuleOps, CSSFontFeatureValuesRuleOps, CSSGroupingRuleOps,
    CSSImportRuleOps, CSSKeyframeRule, CSSKeyframeRuleOps, CSSKeyframesRuleOps,
    CSSLayerBlockRuleOps, CSSLayerStatementRuleOps, CSSMediaRuleOps, CSSNamespaceRuleOps,
    CSSNestedDeclarationsOps, CSSPageRuleOps, CSSPropertyRuleOps, CSSRule, CSSRuleList,
    CSSRuleListOps, CSSRuleOps, CSSScopeRuleOps, CSSStyleProperties, CSSStylePropertiesOps,
    CSSStyleRuleOps, CSSStyleSheet, CSSStyleSheetConstructor, CSSStyleSheetOps, CSSSupportsRuleOps,
    MediaList, MediaListOps, StyleSheetOps,
};

/// One constructed sheet shared by every live list, rule and declaration host.
/// Weak generational slab keys preserve JS identity without retaining wrappers.
pub struct CssSheetState {
    sheet: RefCell<CssStyleSheet>,
    keys: RefCell<CssKeys>,
    /// The element a document sheet belongs to; `None` for a constructed sheet.
    owner: Option<SheetOwner>,
}

/// What `StyleSheet.ownerNode`, `href` and `title` report for a sheet an element
/// owns.
pub struct SheetOwner {
    /// The `<style>` or `<link>`.
    pub node: NodeRef,
    /// The sheet's URL: a link's resolved `href`; `None` for `<style>`.
    pub href: Option<String>,
    /// The element's `title` attribute.
    pub title: Option<String>,
    /// The sheet came from another origin without a passing CORS check: its
    /// rules are not readable or writable by script.
    pub opaque: bool,
}

impl CssSheetState {
    /// Whether script made this sheet with `new CSSStyleSheet()`, as opposed
    /// to it being the sheet of an element.
    pub(super) fn is_constructed(&self) -> bool {
        self.owner.is_none()
    }

    /// `SecurityError` when script may not touch this sheet's rules.
    fn check_readable(&self) -> Result<(), JsError> {
        if self.owner.as_ref().is_some_and(|owner| owner.opaque) {
            return Err(JsError::dom_exception(
                "SecurityError",
                "Cannot access rules of a cross-origin stylesheet",
            ));
        }
        Ok(())
    }
}

/// The objects one rule or keyframe can have: itself, its child rule list, its
/// declaration block (for `@font-face`, its descriptors) and its media list.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RulePart {
    Rule,
    List,
    Style,
    Media,
}

/// What an object of a sheet belongs to: a rule, or a keyframe of one.
#[derive(Clone)]
enum PartOwner {
    Rule(CssRuleHandle),
    Keyframe(CssKeyframeHandle),
}

impl PartOwner {
    fn identity(&self) -> usize {
        match self {
            PartOwner::Rule(rule) => rule.identity(),
            PartOwner::Keyframe(keyframe) => keyframe.identity(),
        }
    }

    fn rule(self) -> Option<CssRuleHandle> {
        match self {
            PartOwner::Rule(rule) => Some(rule),
            PartOwner::Keyframe(_) => None,
        }
    }
}

/// The records made so far for a sheet's objects, so that asking twice gives
/// the same object.
///
/// A rule or keyframe is found by the address of its Stylo allocation. The
/// table holds a handle to it beside the key, which keeps the allocation, and
/// so the address, from being reused while the entry is there.
impl crate::stylesheets::ScriptSheet for CssSheetState {
    fn revision(&self) -> u64 {
        // A sheet being changed right now is read again when that is over.
        self.sheet
            .try_borrow()
            .map_or(u64::MAX, |sheet| sheet.revision())
    }

    fn text(&self) -> Option<String> {
        let sheet = self.sheet.try_borrow().ok()?;
        // The sheet of an element is its source until script changes a rule;
        // a constructed sheet has nothing but its rules.
        if self.owner.is_some() && sheet.content_revision() == 0 {
            return None;
        }
        // A sheet nested too deeply to serialize keeps its source.
        sheet.css_text().ok()
    }

    fn disabled(&self) -> bool {
        self.sheet.try_borrow().is_ok_and(|sheet| sheet.disabled())
    }
}

#[derive(Default)]
struct CssKeys {
    sheet: Option<HostKey>,
    top_list: Option<HostKey>,
    parts: HashMap<(usize, RulePart), (PartOwner, HostKey)>,
    /// Which entry of `parts` a key is, for forgetting it without a search.
    owners: HashMap<HostKey, (usize, RulePart)>,
}

impl CssSheetState {
    fn new(sheet: CssStyleSheet) -> Rc<Self> {
        Rc::new(Self {
            sheet: RefCell::new(sheet),
            keys: RefCell::new(CssKeys::default()),
            owner: None,
        })
    }

    /// The sheet of a `<style>` or `<link>` element.
    pub(super) fn owned(sheet: CssStyleSheet, owner: SheetOwner) -> Rc<Self> {
        Rc::new(Self {
            sheet: RefCell::new(sheet),
            keys: RefCell::new(CssKeys::default()),
            owner: Some(owner),
        })
    }

    pub(super) fn forget_key(&self, key: HostKey) {
        let mut keys = self.keys.borrow_mut();
        if keys.sheet == Some(key) {
            keys.sheet = None;
        }
        if keys.top_list == Some(key) {
            keys.top_list = None;
        }
        if let Some(owner) = keys.owners.remove(&key)
            && keys.parts.get(&owner).is_some_and(|(_, held)| *held == key)
        {
            keys.parts.remove(&owner);
        }
    }

    /// The record of one of a rule's or keyframe's objects, made on first use.
    fn part_key(
        self: &Rc<Self>,
        ctx: &SharedContext,
        owner: PartOwner,
        part: RulePart,
        record: impl FnOnce(Rc<Self>, PartOwner) -> HostRecord,
    ) -> HostKey {
        let place = (owner.identity(), part);
        if let Some((_, key)) = self.keys.borrow().parts.get(&place)
            && ctx.host_records.borrow().get(*key).is_some()
        {
            return *key;
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(record(Rc::clone(self), owner.clone()));
        let mut keys = self.keys.borrow_mut();
        if let Some((_, stale)) = keys.parts.insert(place, (owner, key)) {
            keys.owners.remove(&stale);
        }
        keys.owners.insert(key, place);
        key
    }

    pub(super) fn sheet_key(self: &Rc<Self>, ctx: &SharedContext) -> HostKey {
        if let Some(key) = self.keys.borrow().sheet
            && ctx.host_records.borrow().get(key).is_some()
        {
            return key;
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::CssSheet(Rc::clone(self)));
        self.keys.borrow_mut().sheet = Some(key);
        key
    }

    fn list_key(self: &Rc<Self>, ctx: &SharedContext, parent: Option<CssRuleHandle>) -> HostKey {
        let Some(parent) = parent else {
            if let Some(key) = self.keys.borrow().top_list
                && ctx.host_records.borrow().get(key).is_some()
            {
                return key;
            }
            let key = ctx
                .host_records
                .borrow_mut()
                .insert(HostRecord::CssRuleList(CssRuleListRecord {
                    sheet: Rc::clone(self),
                    parent: None,
                }));
            self.keys.borrow_mut().top_list = Some(key);
            return key;
        };
        self.part_key(
            ctx,
            PartOwner::Rule(parent),
            RulePart::List,
            |sheet, owner| {
                HostRecord::CssRuleList(CssRuleListRecord {
                    sheet,
                    parent: owner.rule(),
                })
            },
        )
    }

    fn rule_key(self: &Rc<Self>, ctx: &SharedContext, rule: CssRuleHandle) -> HostKey {
        self.part_key(ctx, PartOwner::Rule(rule), RulePart::Rule, rule_record_of)
    }

    fn keyframe_key(self: &Rc<Self>, ctx: &SharedContext, keyframe: CssKeyframeHandle) -> HostKey {
        self.part_key(
            ctx,
            PartOwner::Keyframe(keyframe),
            RulePart::Rule,
            rule_record_of,
        )
    }

    /// The declaration block of a style rule or of the declarations nested
    /// in one, and the descriptors of an `@font-face` rule.
    fn style_key(self: &Rc<Self>, ctx: &SharedContext, rule: CssRuleHandle) -> HostKey {
        self.part_key(ctx, PartOwner::Rule(rule), RulePart::Style, style_record_of)
    }

    fn keyframe_style_key(
        self: &Rc<Self>,
        ctx: &SharedContext,
        keyframe: CssKeyframeHandle,
    ) -> HostKey {
        self.part_key(
            ctx,
            PartOwner::Keyframe(keyframe),
            RulePart::Style,
            style_record_of,
        )
    }

    fn media_key(self: &Rc<Self>, ctx: &SharedContext, rule: CssRuleHandle) -> HostKey {
        self.part_key(
            ctx,
            PartOwner::Rule(rule.clone()),
            RulePart::Media,
            move |sheet, _| HostRecord::CssMediaList(CssRuleRecord { sheet, rule }),
        )
    }
}

fn rule_record_of(sheet: Rc<CssSheetState>, owner: PartOwner) -> HostRecord {
    match owner {
        PartOwner::Rule(rule) => HostRecord::CssRule(CssRuleRecord { sheet, rule }),
        PartOwner::Keyframe(keyframe) => {
            HostRecord::CssKeyframe(CssKeyframeRecord { sheet, keyframe })
        }
    }
}

fn style_record_of(sheet: Rc<CssSheetState>, owner: PartOwner) -> HostRecord {
    let owner = match owner {
        PartOwner::Rule(rule) if rule.rule_type() == CssRuleType::FontFace => {
            StyleOwner::Descriptors(rule)
        }
        PartOwner::Rule(rule) => StyleOwner::Rule(rule),
        PartOwner::Keyframe(keyframe) => StyleOwner::Keyframe(keyframe),
    };
    HostRecord::CssRuleStyle(CssStyleRecord { sheet, owner })
}

pub struct CssRuleListRecord {
    sheet: Rc<CssSheetState>,
    parent: Option<CssRuleHandle>,
}

#[derive(Clone)]
pub struct CssRuleRecord {
    sheet: Rc<CssSheetState>,
    rule: CssRuleHandle,
}

impl CssRuleRecord {
    /// The interface of this rule's object.
    pub(super) fn interface_name(&self) -> &'static str {
        rule_interface(self.rule.rule_type())
    }

    /// The rule the wrapper graph files this record under.
    pub(super) fn graph_rule(&self) -> CssRuleHandle {
        self.rule.clone()
    }

    pub(super) fn sheet(&self) -> Rc<CssSheetState> {
        Rc::clone(&self.sheet)
    }
}

/// One keyframe of an `@keyframes` rule: a `CSSKeyframeRule`.
#[derive(Clone)]
pub struct CssKeyframeRecord {
    sheet: Rc<CssSheetState>,
    keyframe: CssKeyframeHandle,
}

impl CssKeyframeRecord {
    /// A keyframe is filed with the `@keyframes` rule it was read from. One
    /// that has left the rule is kept with it for as long as script holds
    /// either, which is more than it has to be and never less.
    pub(super) fn graph_rule(&self) -> CssRuleHandle {
        self.keyframe.keyframes_rule().clone()
    }

    pub(super) fn sheet(&self) -> Rc<CssSheetState> {
        Rc::clone(&self.sheet)
    }
}

/// What a declaration object of a sheet reads and writes.
#[derive(Clone)]
enum StyleOwner {
    /// The declaration block of a style rule or of nested declarations.
    Rule(CssRuleHandle),
    /// The declaration block of a keyframe.
    Keyframe(CssKeyframeHandle),
    /// The descriptors of an `@font-face` rule, which are read only here.
    Descriptors(CssRuleHandle),
}

/// The `style` of a rule or of a keyframe.
#[derive(Clone)]
pub struct CssStyleRecord {
    sheet: Rc<CssSheetState>,
    owner: StyleOwner,
}

impl CssStyleRecord {
    /// Descriptors are a plain `CSSStyleDeclaration`: they have none of the
    /// property-named attributes a declaration block has.
    pub(super) fn interface_name(&self) -> &'static str {
        match self.owner {
            StyleOwner::Descriptors(_) => "CSSStyleDeclaration",
            StyleOwner::Rule(_) | StyleOwner::Keyframe(_) => "CSSStyleProperties",
        }
    }

    pub(super) fn graph_rule(&self) -> CssRuleHandle {
        match &self.owner {
            StyleOwner::Rule(rule) | StyleOwner::Descriptors(rule) => rule.clone(),
            StyleOwner::Keyframe(keyframe) => keyframe.keyframes_rule().clone(),
        }
    }

    pub(super) fn sheet(&self) -> Rc<CssSheetState> {
        Rc::clone(&self.sheet)
    }

    fn snapshot(&self) -> CssDeclarationSnapshot {
        match &self.owner {
            StyleOwner::Rule(rule) => rule.style_declarations(),
            StyleOwner::Keyframe(keyframe) => Some(keyframe.style_declarations()),
            StyleOwner::Descriptors(rule) => rule.font_face_descriptors().map(|descriptors| {
                let css_text = descriptors
                    .iter()
                    .map(|(name, value)| format!("{name}: {value};"))
                    .collect::<Vec<_>>()
                    .join(" ");
                CssDeclarationSnapshot {
                    css_text,
                    properties: descriptors
                        .into_iter()
                        .map(|(name, value)| CssPropertySnapshot {
                            name,
                            value,
                            important: false,
                        })
                        .collect(),
                }
            }),
        }
        .unwrap_or(CssDeclarationSnapshot {
            css_text: String::new(),
            properties: Vec::new(),
        })
    }

    fn property_value(&self, name: &str) -> String {
        match &self.owner {
            StyleOwner::Rule(rule) => rule.style_property_value(name).unwrap_or_default(),
            StyleOwner::Keyframe(keyframe) => keyframe.style_property_value(name),
            StyleOwner::Descriptors(_) => self
                .snapshot()
                .properties
                .into_iter()
                .find(|property| property.name.eq_ignore_ascii_case(name))
                .map(|property| property.value)
                .unwrap_or_default(),
        }
    }

    fn is_important(&self, name: &str) -> bool {
        match &self.owner {
            StyleOwner::Rule(rule) => rule.style_property_is_important(name).unwrap_or(false),
            StyleOwner::Keyframe(_) | StyleOwner::Descriptors(_) => false,
        }
    }

    fn set_css_text(&self, text: &str) {
        let mut sheet = self.sheet.sheet.borrow_mut();
        let _ = match &self.owner {
            StyleOwner::Rule(rule) => sheet.set_rule_style_css_text(rule, text),
            StyleOwner::Keyframe(keyframe) => sheet.set_keyframe_style_css_text(keyframe, text),
            StyleOwner::Descriptors(_) => return,
        };
    }

    fn set_property(&self, name: &str, value: &str, priority: &str) -> Result<(), CssomError> {
        let mut sheet = self.sheet.sheet.borrow_mut();
        match &self.owner {
            StyleOwner::Rule(rule) => sheet.set_rule_style_property(rule, name, value, priority),
            StyleOwner::Keyframe(keyframe) => {
                sheet.set_keyframe_style_property(keyframe, name, value, priority)
            }
            StyleOwner::Descriptors(_) => return Ok(()),
        }
        .map(|_| ())
    }

    fn remove_property(&self, name: &str) -> Result<String, CssomError> {
        let mut sheet = self.sheet.sheet.borrow_mut();
        match &self.owner {
            StyleOwner::Rule(rule) => sheet.remove_rule_style_property(rule, name),
            StyleOwner::Keyframe(keyframe) => sheet.remove_keyframe_style_property(keyframe, name),
            StyleOwner::Descriptors(_) => return Ok(String::new()),
        }
        .map(|(value, _)| value)
    }

    /// The record of the rule or keyframe this is the `style` of.
    fn parent_key(&self, ctx: &SharedContext) -> HostKey {
        match &self.owner {
            StyleOwner::Rule(rule) | StyleOwner::Descriptors(rule) => {
                self.sheet.rule_key(ctx, rule.clone())
            }
            StyleOwner::Keyframe(keyframe) => self.sheet.keyframe_key(ctx, keyframe.clone()),
        }
    }
}

/// How many rules a rule's `cssRules` has: its child rules, or for
/// `@keyframes` its keyframes.
pub(super) fn list_length(parent: &CssRuleHandle) -> usize {
    parent
        .keyframe_count()
        .unwrap_or_else(|| parent.child_count())
}

/// The interface CSSOM gives a rule of this kind. The kinds listed are the ones
/// Stylo's Servo configuration parses; another kind is a plain `CSSRule`.
fn rule_interface(kind: CssRuleType) -> &'static str {
    match kind {
        CssRuleType::Style => "CSSStyleRule",
        CssRuleType::Media => "CSSMediaRule",
        CssRuleType::Supports => "CSSSupportsRule",
        CssRuleType::LayerBlock => "CSSLayerBlockRule",
        CssRuleType::LayerStatement => "CSSLayerStatementRule",
        CssRuleType::Scope => "CSSScopeRule",
        CssRuleType::Keyframes => "CSSKeyframesRule",
        CssRuleType::FontFace => "CSSFontFaceRule",
        CssRuleType::FontFeatureValues => "CSSFontFeatureValuesRule",
        CssRuleType::Namespace => "CSSNamespaceRule",
        CssRuleType::Property => "CSSPropertyRule",
        CssRuleType::NestedDeclarations => "CSSNestedDeclarations",
        _ => "CSSRule",
    }
}

/// `CSSRule.type`: the number CSSOM froze for the kinds that had one when the
/// attribute was deprecated, and 0 for every kind since.
fn legacy_rule_type(kind: CssRuleType) -> u16 {
    match kind {
        CssRuleType::Style
        | CssRuleType::Import
        | CssRuleType::Media
        | CssRuleType::FontFace
        | CssRuleType::Page
        | CssRuleType::Keyframes
        | CssRuleType::Keyframe
        | CssRuleType::Margin
        | CssRuleType::Namespace
        | CssRuleType::CounterStyle
        | CssRuleType::Supports
        | CssRuleType::FontFeatureValues => kind as u16,
        _ => 0,
    }
}

pub(super) fn record_state(record: &HostRecord) -> Option<Rc<CssSheetState>> {
    match record {
        HostRecord::CssSheet(sheet) => Some(Rc::clone(sheet)),
        HostRecord::CssRuleList(list) => Some(Rc::clone(&list.sheet)),
        HostRecord::CssRule(rule) | HostRecord::CssMediaList(rule) => Some(Rc::clone(&rule.sheet)),
        HostRecord::CssKeyframe(keyframe) => Some(Rc::clone(&keyframe.sheet)),
        HostRecord::CssRuleStyle(style) => Some(Rc::clone(&style.sheet)),
        _ => None,
    }
}

fn sheet_record(host: &HostObject) -> Result<Rc<CssSheetState>, JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(JsError::type_error("the receiver is not a CSSStyleSheet"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::CssSheet(sheet)) = records.get(*key) else {
        return Err(JsError::type_error("the stylesheet is gone"));
    };
    Ok(Rc::clone(sheet))
}

fn rule_record(host: &HostObject) -> Result<CssRuleRecord, JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(JsError::type_error("the receiver is not a CSSRule"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::CssRule(rule)) = records.get(*key) else {
        return Err(JsError::type_error("the rule is gone"));
    };
    Ok(rule.clone())
}

/// What a getter answers when its receiver is a rule of another kind. The
/// generated glue checks the receiver's interface before it calls here, so this
/// is what a mismatch between that and the rule would say.
fn wrong_kind(interface: &str) -> JsError {
    JsError::type_error(format!("the receiver is not a {interface}"))
}

fn media_record(host: &HostObject) -> Result<CssRuleRecord, JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(wrong_kind("MediaList"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::CssMediaList(rule)) = records.get(*key) else {
        return Err(JsError::type_error("the media list is gone"));
    };
    Ok(rule.clone())
}

fn keyframe_record(host: &HostObject) -> Result<CssKeyframeRecord, JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(wrong_kind("CSSKeyframeRule"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::CssKeyframe(keyframe)) = records.get(*key) else {
        return Err(JsError::type_error("the keyframe is gone"));
    };
    Ok(keyframe.clone())
}

fn style_record(host: &HostObject) -> Option<CssStyleRecord> {
    let HostObject::Host { ctx, key } = host else {
        return None;
    };
    let records = ctx.host_records.borrow();
    let HostRecord::CssRuleStyle(rule) = records.get(*key)? else {
        return None;
    };
    Some(rule.clone())
}

fn list_record(host: &HostObject) -> Result<(Rc<CssSheetState>, Option<CssRuleHandle>), JsError> {
    let HostObject::Host { ctx, key } = host else {
        return Err(JsError::type_error("the receiver is not a CSSRuleList"));
    };
    let records = ctx.host_records.borrow();
    let Some(HostRecord::CssRuleList(list)) = records.get(*key) else {
        return Err(JsError::type_error("the rule list is gone"));
    };
    Ok((Rc::clone(&list.sheet), list.parent.clone()))
}

fn css_error(error: CssomError) -> JsError {
    let name = match error {
        CssomError::Syntax => "SyntaxError",
        CssomError::IndexSize => "IndexSizeError",
        CssomError::HierarchyRequest => "HierarchyRequestError",
        CssomError::InvalidState => "InvalidStateError",
        CssomError::NotFound => "NotFoundError",
        CssomError::UnsupportedRuleContext | CssomError::DepthLimit => "NotSupportedError",
    };
    JsError::dom_exception(name, &error.to_string())
}

impl StyleSheetOps for HostObject {
    fn r#type(&self) -> Result<String, JsError> {
        Ok("text/css".to_owned())
    }
    fn href(&self) -> Result<Option<String>, JsError> {
        Ok(sheet_record(self)?
            .owner
            .as_ref()
            .and_then(|owner| owner.href.clone()))
    }
    fn owner_node(&self) -> Result<Option<Handle<Element>>, JsError> {
        let sheet = sheet_record(self)?;
        let Some(owner) = &sheet.owner else {
            return Ok(None);
        };
        // The element may have been reclaimed since the sheet was made.
        let live = self.context().doc.borrow().resolve(owner.node).is_ok();
        Ok(live.then(|| Handle::from_bits(encode_node(self.slot(), owner.node))))
    }
    fn parent_style_sheet(&self) -> Result<Option<Handle<CSSStyleSheet>>, JsError> {
        Ok(None)
    }
    fn title(&self) -> Result<Option<String>, JsError> {
        Ok(sheet_record(self)?
            .owner
            .as_ref()
            .and_then(|owner| owner.title.clone()))
    }
    fn disabled(&self) -> bool {
        sheet_record(self).is_ok_and(|sheet| sheet.sheet.borrow().disabled())
    }
    fn set_disabled(&self, value: bool) {
        if let Ok(sheet) = sheet_record(self) {
            sheet.sheet.borrow_mut().set_disabled(value);
        }
    }
}

impl CSSStyleSheetConstructor for HostObject {
    fn construct(realm: &Realm) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let base_url = {
            let doc = ctx.doc.borrow();
            doc.base_url()
                .or(doc.url())
                .cloned()
                .unwrap_or_else(about_blank)
        };
        let sheet = CssSheetState::new(CssStyleSheet::parse("", base_url));
        let key = sheet.sheet_key(&ctx);
        Ok(HostObject::host_record(ctx, key))
    }
}

impl CSSStyleSheetOps for HostObject {
    fn owner_rule(&self) -> Result<Option<Handle<CSSRule>>, JsError> {
        Ok(None)
    }
    fn css_rules(&self) -> Result<Handle<CSSRuleList>, JsError> {
        let sheet = sheet_record(self)?;
        sheet.check_readable()?;
        Ok(Handle::from_bits(encode_host(
            self.slot(),
            sheet.list_key(self.context(), None),
        )))
    }
    fn insert_rule(&self, rule: &str, index: Option<u32>) -> Result<u32, JsError> {
        let sheet = sheet_record(self)?;
        sheet.check_readable()?;
        let index = index.unwrap_or(0);
        sheet
            .sheet
            .borrow_mut()
            .insert_rule(rule, index as usize)
            .map_err(css_error)?;
        Ok(index)
    }
    fn delete_rule(&self, index: u32) -> Result<(), JsError> {
        let sheet = sheet_record(self)?;
        sheet.check_readable()?;
        sheet
            .sheet
            .borrow_mut()
            .delete_rule(index as usize)
            .map_err(css_error)?;
        Ok(())
    }
    fn replace_sync(&self, text: &str) -> Result<(), JsError> {
        let sheet = sheet_record(self)?;
        // Only a sheet script constructed is replaced whole; the sheet of an
        // element follows the element's text.
        if !sheet.is_constructed() {
            return Err(JsError::dom_exception(
                "NotAllowedError",
                "Can't call replaceSync on a stylesheet that was not constructed",
            ));
        }
        sheet.check_readable()?;
        sheet
            .sheet
            .borrow_mut()
            .replace_sync(text)
            .map_err(css_error)?;
        Ok(())
    }
}

impl CSSRuleListOps for HostObject {
    fn length(&self) -> u32 {
        let Ok((sheet, parent)) = list_record(self) else {
            return 0;
        };
        let len = match parent {
            Some(parent) => list_length(&parent),
            None => sheet.sheet.borrow().rule_count(),
        };
        u32::try_from(len).unwrap_or(u32::MAX)
    }
    fn item(&self, index: u32) -> Result<Option<Handle<CSSRule>>, JsError> {
        let (sheet, parent) = list_record(self)?;
        let index = index as usize;
        let key = match parent {
            Some(parent) if parent.rule_type() == CssRuleType::Keyframes => parent
                .keyframe_at(index)
                .map(|keyframe| sheet.keyframe_key(self.context(), keyframe)),
            Some(parent) => parent
                .child_at(index)
                .map(|rule| sheet.rule_key(self.context(), rule)),
            None => {
                let rule = sheet.sheet.borrow().rule_at(index);
                rule.map(|rule| sheet.rule_key(self.context(), rule))
            }
        };
        Ok(key.map(|key| Handle::from_bits(encode_host(self.slot(), key))))
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<HostObject as CSSRuleListOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <HostObject as CSSRuleListOps>::length(self))
    }
}

impl CSSRuleOps for HostObject {
    fn css_text(&self) -> Result<String, JsError> {
        if let Ok(keyframe) = keyframe_record(self) {
            return Ok(keyframe.keyframe.css_text());
        }
        rule_record(self)?.rule.css_text().map_err(css_error)
    }
    // CSSOM explicitly makes CSSRule.cssText assignment a no-op.
    fn set_css_text(&self, _text: &str) {}
    fn parent_rule(&self) -> Result<Option<Handle<CSSRule>>, JsError> {
        let (sheet, parent) = match keyframe_record(self) {
            Ok(keyframe) => (keyframe.sheet, keyframe.keyframe.parent_rule()),
            Err(_) => {
                let rule = rule_record(self)?;
                (rule.sheet, rule.rule.parent_rule())
            }
        };
        Ok(parent.map(|parent| {
            let key = sheet.rule_key(self.context(), parent);
            Handle::from_bits(encode_host(self.slot(), key))
        }))
    }
    fn parent_style_sheet(&self) -> Result<Option<Handle<CSSStyleSheet>>, JsError> {
        let (sheet, linked) = match keyframe_record(self) {
            Ok(keyframe) => {
                let linked = keyframe.keyframe.has_parent_style_sheet();
                (keyframe.sheet, linked)
            }
            Err(_) => {
                let rule = rule_record(self)?;
                let linked = rule.rule.has_parent_style_sheet();
                (rule.sheet, linked)
            }
        };
        Ok(linked.then(|| {
            let key = sheet.sheet_key(self.context());
            Handle::from_bits(encode_host(self.slot(), key))
        }))
    }
    fn r#type(&self) -> u16 {
        if keyframe_record(self).is_ok() {
            return legacy_rule_type(CssRuleType::Keyframe);
        }
        rule_record(self).map_or(0, |rule| legacy_rule_type(rule.rule.rule_type()))
    }
}

impl CSSGroupingRuleOps for HostObject {
    fn css_rules(&self) -> Result<Handle<CSSRuleList>, JsError> {
        let rule = rule_record(self)?;
        let key = rule.sheet.list_key(self.context(), Some(rule.rule));
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }

    fn insert_rule(&self, text: &str, index: Option<u32>) -> Result<u32, JsError> {
        let rule = rule_record(self)?;
        let index = index.unwrap_or(0);
        rule.sheet
            .sheet
            .borrow_mut()
            .insert_nested_rule(&rule.rule, text, index as usize)
            .map_err(css_error)?;
        Ok(index)
    }

    fn delete_rule(&self, index: u32) -> Result<(), JsError> {
        let rule = rule_record(self)?;
        rule.sheet
            .sheet
            .borrow_mut()
            .delete_nested_rule(&rule.rule, index as usize)
            .map_err(css_error)?;
        Ok(())
    }
}

impl CSSStyleRuleOps for HostObject {
    fn selector_text(&self) -> Result<String, JsError> {
        rule_record(self)?
            .rule
            .selector_text()
            .ok_or_else(|| JsError::type_error("the receiver is not a CSSStyleRule"))
    }
    fn set_selector_text(&self, value: &str) {
        if let Ok(rule) = rule_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .set_rule_selector_text(&rule.rule, value);
        }
    }
    fn style(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        let rule = rule_record(self)?;
        if !rule.rule.has_style() {
            return Err(wrong_kind("CSSStyleRule"));
        }
        let key = rule.sheet.style_key(self.context(), rule.rule);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

impl CSSConditionRuleOps for HostObject {
    fn condition_text(&self) -> Result<String, JsError> {
        rule_record(self)?
            .rule
            .condition_text()
            .ok_or_else(|| wrong_kind("CSSConditionRule"))
    }
}

impl CSSMediaRuleOps for HostObject {
    fn media(&self) -> Result<Handle<MediaList>, JsError> {
        let rule = rule_record(self)?;
        if rule.rule.rule_type() != CssRuleType::Media {
            return Err(wrong_kind("CSSMediaRule"));
        }
        let key = rule.sheet.media_key(self.context(), rule.rule);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

impl MediaListOps for HostObject {
    fn media_text(&self) -> Result<String, JsError> {
        media_record(self)?
            .rule
            .media_text()
            .ok_or_else(|| wrong_kind("MediaList"))
    }
    fn set_media_text(&self, value: &str) {
        if let Ok(rule) = media_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .set_rule_media_text(&rule.rule, value);
        }
    }
    fn length(&self) -> u32 {
        media_record(self)
            .ok()
            .and_then(|rule| rule.rule.media_queries())
            .map_or(0, |queries| {
                u32::try_from(queries.len()).unwrap_or(u32::MAX)
            })
    }
    fn item(&self, index: u32) -> Result<Option<String>, JsError> {
        Ok(media_record(self)?
            .rule
            .media_queries()
            .and_then(|mut queries| {
                let index = index as usize;
                (index < queries.len()).then(|| queries.swap_remove(index))
            }))
    }
    fn append_medium(&self, medium: &str) -> Result<(), JsError> {
        let rule = media_record(self)?;
        rule.sheet
            .sheet
            .borrow_mut()
            .append_rule_medium(&rule.rule, medium)
            .map_err(css_error)?;
        Ok(())
    }
    fn delete_medium(&self, medium: &str) -> Result<(), JsError> {
        let rule = media_record(self)?;
        rule.sheet
            .sheet
            .borrow_mut()
            .delete_rule_medium(&rule.rule, medium)
            .map_err(css_error)?;
        Ok(())
    }
    fn to_string(&self) -> Result<String, JsError> {
        <HostObject as MediaListOps>::media_text(self)
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<HostObject as MediaListOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <HostObject as MediaListOps>::length(self))
    }
}

impl CSSSupportsRuleOps for HostObject {
    fn matches(&self) -> bool {
        rule_record(self)
            .ok()
            .and_then(|rule| rule.rule.supports_matches())
            .unwrap_or(false)
    }
}

impl CSSLayerBlockRuleOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        rule_record(self)?
            .rule
            .layer_name()
            .ok_or_else(|| wrong_kind("CSSLayerBlockRule"))
    }
}

impl CSSLayerStatementRuleOps for HostObject {
    fn name_list(&self) -> Result<Vec<String>, JsError> {
        rule_record(self)?
            .rule
            .layer_names()
            .ok_or_else(|| wrong_kind("CSSLayerStatementRule"))
    }
}

impl CSSScopeRuleOps for HostObject {
    fn start(&self) -> Result<Option<String>, JsError> {
        let bounds = rule_record(self)?.rule.scope_bounds();
        Ok(bounds.ok_or_else(|| wrong_kind("CSSScopeRule"))?.0)
    }
    fn end(&self) -> Result<Option<String>, JsError> {
        let bounds = rule_record(self)?.rule.scope_bounds();
        Ok(bounds.ok_or_else(|| wrong_kind("CSSScopeRule"))?.1)
    }
}

impl CSSNamespaceRuleOps for HostObject {
    fn namespace_uri(&self) -> Result<String, JsError> {
        let namespace = rule_record(self)?.rule.namespace();
        Ok(namespace.ok_or_else(|| wrong_kind("CSSNamespaceRule"))?.1)
    }
    fn prefix(&self) -> Result<String, JsError> {
        let namespace = rule_record(self)?.rule.namespace();
        Ok(namespace.ok_or_else(|| wrong_kind("CSSNamespaceRule"))?.0)
    }
}

impl CSSPropertyRuleOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(property_registration(self)?.name)
    }
    fn syntax(&self) -> Result<String, JsError> {
        // The descriptor is a string, and the attribute is what that string
        // says: `"<length>"` in the rule is `<length>` here.
        let syntax = property_registration(self)?.syntax.unwrap_or_default();
        Ok(
            match syntax
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
            {
                Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
                None => syntax,
            },
        )
    }
    fn inherits(&self) -> bool {
        property_registration(self)
            .is_ok_and(|registration| registration.inherits.as_deref() == Some("true"))
    }
    fn initial_value(&self) -> Result<Option<String>, JsError> {
        Ok(property_registration(self)?.initial_value)
    }
}

fn property_registration(host: &HostObject) -> Result<tenta_css::CssPropertyRegistration, JsError> {
    rule_record(host)?
        .rule
        .property_registration()
        .ok_or_else(|| wrong_kind("CSSPropertyRule"))
}

/// The record of a rule that is an `@keyframes` rule.
fn keyframes_record(host: &HostObject) -> Result<CssRuleRecord, JsError> {
    let rule = rule_record(host)?;
    if rule.rule.rule_type() != CssRuleType::Keyframes {
        return Err(wrong_kind("CSSKeyframesRule"));
    }
    Ok(rule)
}

impl CSSKeyframesRuleOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        rule_record(self)?
            .rule
            .keyframes_name()
            .ok_or_else(|| wrong_kind("CSSKeyframesRule"))
    }
    fn set_name(&self, value: &str) {
        if let Ok(rule) = keyframes_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .set_keyframes_name(&rule.rule, value);
        }
    }
    fn css_rules(&self) -> Result<Handle<CSSRuleList>, JsError> {
        let rule = keyframes_record(self)?;
        let key = rule.sheet.list_key(self.context(), Some(rule.rule));
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
    fn length(&self) -> u32 {
        keyframes_record(self)
            .ok()
            .and_then(|rule| rule.rule.keyframe_count())
            .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
    }
    fn append_rule(&self, text: &str) {
        if let Ok(rule) = keyframes_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .append_keyframe(&rule.rule, text);
        }
    }
    fn delete_rule(&self, select: &str) {
        if let Ok(rule) = keyframes_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .delete_keyframe(&rule.rule, select);
        }
    }
    fn find_rule(&self, select: &str) -> Result<Option<Handle<CSSKeyframeRule>>, JsError> {
        let rule = keyframes_record(self)?;
        Ok(rule.rule.find_keyframe(select).map(|keyframe| {
            let key = rule.sheet.keyframe_key(self.context(), keyframe);
            Handle::from_bits(encode_host(self.slot(), key))
        }))
    }
    fn indexed_getter(&self, index: u32) -> Result<Handle<CSSKeyframeRule>, JsError> {
        let rule = keyframes_record(self)?;
        let keyframe = rule
            .rule
            .keyframe_at(index as usize)
            .ok_or_else(|| JsError::type_error("the rule has no keyframe at this index"))?;
        let key = rule.sheet.keyframe_key(self.context(), keyframe);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<HostObject as CSSKeyframesRuleOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <HostObject as CSSKeyframesRuleOps>::length(self))
    }
}

impl CSSKeyframeRuleOps for HostObject {
    fn key_text(&self) -> Result<String, JsError> {
        Ok(keyframe_record(self)?.keyframe.key_text())
    }
    fn set_key_text(&self, value: &str) -> Result<(), JsError> {
        let keyframe = keyframe_record(self)?;
        keyframe
            .sheet
            .sheet
            .borrow_mut()
            .set_keyframe_key_text(&keyframe.keyframe, value)
            .map_err(css_error)?;
        Ok(())
    }
    fn style(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        let keyframe = keyframe_record(self)?;
        let key = keyframe
            .sheet
            .keyframe_style_key(self.context(), keyframe.keyframe);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

impl CSSFontFaceRuleOps for HostObject {
    fn style(&self) -> Result<Handle<crate::bindings::CSSStyleDeclaration>, JsError> {
        let rule = rule_record(self)?;
        if rule.rule.rule_type() != CssRuleType::FontFace {
            return Err(wrong_kind("CSSFontFaceRule"));
        }
        let key = rule.sheet.style_key(self.context(), rule.rule);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

impl CSSFontFeatureValuesRuleOps for HostObject {
    fn font_family(&self) -> Result<String, JsError> {
        rule_record(self)?
            .rule
            .font_feature_family()
            .ok_or_else(|| wrong_kind("CSSFontFeatureValuesRule"))
    }
}

impl CSSNestedDeclarationsOps for HostObject {
    fn style(&self) -> Result<Handle<CSSStyleProperties>, JsError> {
        <HostObject as CSSStyleRuleOps>::style(self)
    }
}

// Interface objects with no instances yet; see their IDL.
impl CSSImportRuleOps for HostObject {}
impl CSSPageRuleOps for HostObject {}

#[cfg(feature = "v8")]
impl crate::bindings::style_properties::StyleDeclarationHost for HostObject {
    fn is_style_declaration(&self) -> bool {
        matches!(self, HostObject::Style { .. })
            || style_record(self).is_some()
            || super::computed_style::computed_record(self).is_some()
    }
}

impl CSSStylePropertiesOps for HostObject {
    fn css_float(&self) -> Result<String, JsError> {
        <HostObject as CSSStyleDeclarationOps>::get_property_value(self, "float")
    }
    fn set_css_float(&self, value: &str) {
        let _ = <HostObject as CSSStyleDeclarationOps>::set_property(self, "float", value, None);
    }
}

impl CSSStyleDeclarationOps for HostObject {
    fn css_text(&self) -> Result<String, JsError> {
        // CSSOM: the text of a computed declaration is empty.
        if super::computed_style::computed_record(self).is_some() {
            return Ok(String::new());
        }
        if let Some(style) = style_record(self) {
            return Ok(style.snapshot().css_text);
        }
        Ok(self.style_declarations().snapshot().css_text)
    }
    fn set_css_text(&self, value: &str) {
        if super::computed_style::computed_record(self).is_some() {
            return;
        }
        if let Some(style) = style_record(self) {
            style.set_css_text(value);
            return;
        }
        let (doc, _) = self.anchor();
        let base = {
            let doc = doc.borrow();
            doc.base_url()
                .or(doc.url())
                .cloned()
                .unwrap_or_else(about_blank)
        };
        self.set_style_declarations(&CssDeclarationBlock::parse(value, &base));
    }
    fn length(&self) -> u32 {
        if let Some(record) = super::computed_style::computed_record(self) {
            return self.computed_length(&record);
        }
        if let Some(style) = style_record(self) {
            return style.snapshot().properties.len() as u32;
        }
        self.style_declarations().len() as u32
    }
    fn item(&self, index: u32) -> Result<String, JsError> {
        if let Some(record) = super::computed_style::computed_record(self) {
            return Ok(self.computed_item(&record, index));
        }
        if let Some(style) = style_record(self) {
            return Ok(style
                .snapshot()
                .properties
                .get(index as usize)
                .map(|property| property.name.clone())
                .unwrap_or_default());
        }
        Ok(self.style_declarations().item(index as usize))
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<HostObject as CSSStyleDeclarationOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <HostObject as CSSStyleDeclarationOps>::length(self))
    }
    fn get_property_value(&self, property: &str) -> Result<String, JsError> {
        if let Some(record) = super::computed_style::computed_record(self) {
            return Ok(self.computed_value_of(&record, property));
        }
        if let Some(style) = style_record(self) {
            return Ok(style.property_value(property));
        }
        Ok(self.style_declarations().property_value(property))
    }
    fn get_property_priority(&self, property: &str) -> Result<String, JsError> {
        if super::computed_style::computed_record(self).is_some() {
            return Ok(String::new());
        }
        let important = if let Some(style) = style_record(self) {
            style.is_important(property)
        } else {
            self.style_declarations().is_important(property)
        };
        Ok(if important { "important" } else { "" }.to_owned())
    }
    fn set_property(
        &self,
        property: &str,
        value: &str,
        priority: Option<&str>,
    ) -> Result<(), JsError> {
        if let Some(record) = super::computed_style::computed_record(self) {
            return Err(self.computed_read_only_for(&record));
        }
        if let Some(style) = style_record(self) {
            return style
                .set_property(property, value, priority.unwrap_or_default())
                .map_err(css_error);
        }
        let mut declarations = self.style_declarations();
        if declarations.set_property(property, value, priority.unwrap_or_default()) {
            self.set_style_declarations(&declarations);
        }
        Ok(())
    }
    fn remove_property(&self, property: &str) -> Result<String, JsError> {
        if let Some(record) = super::computed_style::computed_record(self) {
            return Err(self.computed_read_only_for(&record));
        }
        if let Some(style) = style_record(self) {
            return style.remove_property(property).map_err(css_error);
        }
        let mut declarations = self.style_declarations();
        let before = declarations.len();
        let removed = declarations.remove_property(property);
        if declarations.len() != before {
            self.set_style_declarations(&declarations);
        }
        Ok(removed)
    }
    fn parent_rule(&self) -> Result<Option<Handle<CSSRule>>, JsError> {
        let Some(style) = style_record(self) else {
            return Ok(None);
        };
        let key = style.parent_key(self.context());
        Ok(Some(Handle::from_bits(encode_host(self.slot(), key))))
    }
}
