//! Constructed CSS stylesheets and live style-rule wrappers backed by Stylo.
//!
//! Only CSSStyleRule trees cross the JS boundary in this package. Stylo
//! parsing rejects other concrete rule kinds before mutation; no generic
//! CSSRule wrapper stands in for a different rule type.

pub mod graph;

use std::cell::RefCell;
use std::rc::Rc;

use tenta_css::{CssRuleHandle, CssStyleSheet, CssomError};

use super::*;
use crate::bindings::{
    CSSGroupingRuleOps, CSSRule, CSSRuleList, CSSRuleListOps, CSSRuleOps, CSSStyleProperties,
    CSSStylePropertiesOps, CSSStyleRuleOps, CSSStyleSheet, CSSStyleSheetConstructor,
    CSSStyleSheetOps, StyleSheetOps,
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

#[derive(Default)]
struct CssKeys {
    sheet: Option<HostKey>,
    top_list: Option<HostKey>,
    rule_lists: Vec<(CssRuleHandle, HostKey)>,
    rules: Vec<(CssRuleHandle, HostKey)>,
    styles: Vec<(CssRuleHandle, HostKey)>,
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
        keys.rule_lists.retain(|(_, existing)| *existing != key);
        keys.rules.retain(|(_, existing)| *existing != key);
        keys.styles.retain(|(_, existing)| *existing != key);
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
        let found = {
            let keys = self.keys.borrow();
            match &parent {
                Some(parent) => keys
                    .rule_lists
                    .iter()
                    .find(|(rule, _)| rule.same_rule(parent))
                    .map(|(_, key)| *key),
                None => keys.top_list,
            }
        };
        if let Some(key) = found
            && ctx.host_records.borrow().get(key).is_some()
        {
            return key;
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::CssRuleList(CssRuleListRecord {
                sheet: Rc::clone(self),
                parent: parent.clone(),
            }));
        let mut keys = self.keys.borrow_mut();
        if let Some(parent) = parent {
            keys.rule_lists.push((parent, key));
        } else {
            keys.top_list = Some(key);
        }
        key
    }

    fn rule_key(self: &Rc<Self>, ctx: &SharedContext, rule: CssRuleHandle) -> HostKey {
        let found = self
            .keys
            .borrow()
            .rules
            .iter()
            .find(|(candidate, _)| candidate.same_rule(&rule))
            .map(|(_, key)| *key);
        if let Some(key) = found
            && ctx.host_records.borrow().get(key).is_some()
        {
            return key;
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::CssStyleRule(CssRuleRecord {
                sheet: Rc::clone(self),
                rule: rule.clone(),
            }));
        self.keys.borrow_mut().rules.push((rule, key));
        key
    }

    fn style_key(self: &Rc<Self>, ctx: &SharedContext, rule: CssRuleHandle) -> HostKey {
        let found = self
            .keys
            .borrow()
            .styles
            .iter()
            .find(|(candidate, _)| candidate.same_rule(&rule))
            .map(|(_, key)| *key);
        if let Some(key) = found
            && ctx.host_records.borrow().get(key).is_some()
        {
            return key;
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::CssRuleStyle(CssRuleRecord {
                sheet: Rc::clone(self),
                rule: rule.clone(),
            }));
        self.keys.borrow_mut().styles.push((rule, key));
        key
    }
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

pub(super) fn record_state(record: &HostRecord) -> Option<Rc<CssSheetState>> {
    match record {
        HostRecord::CssSheet(sheet) => Some(Rc::clone(sheet)),
        HostRecord::CssRuleList(list) => Some(Rc::clone(&list.sheet)),
        HostRecord::CssStyleRule(rule) | HostRecord::CssRuleStyle(rule) => {
            Some(Rc::clone(&rule.sheet))
        }
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
    let Some(HostRecord::CssStyleRule(rule)) = records.get(*key) else {
        return Err(JsError::type_error("the rule is gone"));
    };
    Ok(rule.clone())
}

fn style_record(host: &HostObject) -> Option<CssRuleRecord> {
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
        CssomError::UnsupportedRuleContext
        | CssomError::UnsupportedRuleType
        | CssomError::DepthLimit => "NotSupportedError",
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
            .insert_supported_style_rule(rule, index as usize)
            .map_err(css_error)?;
        Ok(index)
    }
    fn delete_rule(&self, index: u32) -> Result<(), JsError> {
        let sheet = sheet_record(self)?;
        sheet.check_readable()?;
        // All sheet-visible rules passed the style-only preflight. The native
        // API still handles the index and hierarchy checks.
        sheet
            .sheet
            .borrow_mut()
            .delete_rule(index as usize)
            .map_err(css_error)?;
        Ok(())
    }
    fn replace_sync(&self, text: &str) -> Result<(), JsError> {
        let sheet = sheet_record(self)?;
        sheet.check_readable()?;
        sheet
            .sheet
            .borrow_mut()
            .replace_sync_supported_style_rules(text)
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
            Some(parent) => parent.child_count(),
            None => sheet.sheet.borrow().rule_count(),
        };
        u32::try_from(len).unwrap_or(u32::MAX)
    }
    fn item(&self, index: u32) -> Result<Option<Handle<CSSRule>>, JsError> {
        let (sheet, parent) = list_record(self)?;
        let rule = match parent {
            Some(parent) => parent.child_at(index as usize),
            None => sheet.sheet.borrow().rule_at(index as usize),
        };
        let Some(rule) = rule else { return Ok(None) };
        if rule.rule_type() != tenta_css::CssRuleType::Style {
            return Err(JsError::dom_exception(
                "NotSupportedError",
                "this CSS rule kind is outside the supported CSSStyleRule subset",
            ));
        }
        let key = sheet.rule_key(self.context(), rule);
        Ok(Some(Handle::from_bits(encode_host(self.slot(), key))))
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
        rule_record(self)?.rule.css_text().map_err(css_error)
    }
    // CSSOM explicitly makes CSSRule.cssText assignment a no-op.
    fn set_css_text(&self, _text: &str) {}
    fn parent_rule(&self) -> Result<Option<Handle<CSSRule>>, JsError> {
        let rule = rule_record(self)?;
        let Some(parent) = rule.rule.parent_rule() else {
            return Ok(None);
        };
        let key = rule.sheet.rule_key(self.context(), parent);
        Ok(Some(Handle::from_bits(encode_host(self.slot(), key))))
    }
    fn parent_style_sheet(&self) -> Result<Option<Handle<CSSStyleSheet>>, JsError> {
        let rule = rule_record(self)?;
        if !rule.rule.has_parent_style_sheet() {
            return Ok(None);
        }
        let key = rule.sheet.sheet_key(self.context());
        Ok(Some(Handle::from_bits(encode_host(self.slot(), key))))
    }
    fn r#type(&self) -> u16 {
        1
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
            .insert_supported_nested_style_rule(&rule.rule, text, index as usize)
            .map_err(css_error)?;
        Ok(index)
    }

    fn delete_rule(&self, index: u32) -> Result<(), JsError> {
        let rule = rule_record(self)?;
        rule.sheet
            .sheet
            .borrow_mut()
            .delete_supported_nested_style_rule(&rule.rule, index as usize)
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
        let key = rule.sheet.style_key(self.context(), rule.rule);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

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
        if let Some(rule) = style_record(self) {
            return Ok(rule
                .rule
                .style_declarations()
                .ok_or_else(|| JsError::type_error("the style rule is gone"))?
                .css_text);
        }
        Ok(self.style_declarations().snapshot().css_text)
    }
    fn set_css_text(&self, value: &str) {
        if super::computed_style::computed_record(self).is_some() {
            return;
        }
        if let Some(rule) = style_record(self) {
            let _ = rule
                .sheet
                .sheet
                .borrow_mut()
                .set_rule_style_css_text(&rule.rule, value);
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
        if let Some(rule) = style_record(self) {
            return rule
                .rule
                .style_declarations()
                .map_or(0, |s| s.properties.len() as u32);
        }
        self.style_declarations().len() as u32
    }
    fn item(&self, index: u32) -> Result<String, JsError> {
        if let Some(record) = super::computed_style::computed_record(self) {
            return Ok(self.computed_item(&record, index));
        }
        if let Some(rule) = style_record(self) {
            return Ok(rule
                .rule
                .style_declarations()
                .and_then(|s| s.properties.get(index as usize).map(|p| p.name.clone()))
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
        if let Some(rule) = style_record(self) {
            return Ok(rule.rule.style_property_value(property).unwrap_or_default());
        }
        Ok(self.style_declarations().property_value(property))
    }
    fn get_property_priority(&self, property: &str) -> Result<String, JsError> {
        if super::computed_style::computed_record(self).is_some() {
            return Ok(String::new());
        }
        let important = if let Some(rule) = style_record(self) {
            rule.rule
                .style_property_is_important(property)
                .unwrap_or(false)
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
        if let Some(rule) = style_record(self) {
            rule.sheet
                .sheet
                .borrow_mut()
                .set_rule_style_property(&rule.rule, property, value, priority.unwrap_or_default())
                .map_err(css_error)?;
            return Ok(());
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
        if let Some(rule) = style_record(self) {
            let (value, _) = rule
                .sheet
                .sheet
                .borrow_mut()
                .remove_rule_style_property(&rule.rule, property)
                .map_err(css_error)?;
            return Ok(value);
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
        let Some(rule) = style_record(self) else {
            return Ok(None);
        };
        let key = rule.sheet.rule_key(self.context(), rule.rule);
        Ok(Some(Handle::from_bits(encode_host(self.slot(), key))))
    }
}
