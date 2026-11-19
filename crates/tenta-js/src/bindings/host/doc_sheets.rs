//! `document.styleSheets`, `HTMLLinkElement.sheet` and `HTMLStyleElement.sheet`:
//! the CSSOM view of the sheets the document's elements own (CSS1).
//!
//! A `<style>` element owns a sheet made of its text, and a `<link>` one made of
//! the text the page fetched for it (asked of the [`StyleHost`]); a link whose
//! sheet has not loaded, or failed, has none. The sheet is the same
//! `CSSStyleSheet` constructed sheets are, so `cssRules.length`, `ownerNode`,
//! `href` and the style rules read back. The sheet object of an element is
//! stable while its text is the same (`link.sheet === document.styleSheets[0]`);
//! when a `<style>`'s text changes it gets a new one, as HTML says.
//!
//! What script does to a sheet reaches layout: the sheet object is handed to
//! the [`StyleHost`] when it is made, and the page reads its rules from it
//! once script has changed them (S1). An `@import` is not among a sheet's
//! `cssRules`: the sheet is parsed without a loader, which leaves it out.

use std::collections::HashMap;

use tenta_css::CssStyleSheet;
use url::Url;

use super::cssom::{CssSheetState, SheetOwner};
use super::*;
use crate::bindings::{CSSStyleSheet, StyleSheetListOps};

/// What a list of owners was made for: the document generation and the style
/// host's epoch.
type OwnersKey = (u64, u64);

/// The sheet objects made so far, by owner element, with a fingerprint of the
/// text each was made from.
#[derive(Default)]
pub struct DocSheets {
    entries: HashMap<NodeRef, (u64, Rc<CssSheetState>)>,
    /// The owners last listed, with the document generation and the style
    /// host's epoch they were listed for.
    owners: Option<(OwnersKey, Rc<Vec<NodeId>>)>,
    /// `adoptedStyleSheets` of the document (`None`) and of each shadow root
    /// that has any. The list is what keeps an adopted sheet alive when
    /// script holds it nowhere else.
    adopted: HashMap<Option<NodeRef>, Vec<Rc<CssSheetState>>>,
}

/// What an element's sheet is made of.
struct SheetText {
    /// A link's resolved URL.
    href: Option<Url>,
    base: Url,
    text: Rc<str>,
    title: Option<String>,
    opaque: bool,
}

fn is_css_type(doc: &Document, node: NodeId) -> bool {
    doc.attribute(node, atoms::TYPE)
        .map(str::trim)
        .is_none_or(|value| value.is_empty() || value.eq_ignore_ascii_case("text/css"))
}

fn about_blank() -> Url {
    Url::parse("about:blank").expect("a constant URL")
}

impl HostObject {
    /// The text the sheet of `node` is made of, when it has one.
    fn sheet_text(&self, node: NodeId) -> Option<SheetText> {
        let doc = self.context().doc.try_borrow().ok()?;
        if !doc.is_composed_connected(node) || doc.namespace(node)? != atoms::NS_HTML {
            return None;
        }
        let tag = doc.tag(node)?;
        let title = doc.attribute(node, atoms::TITLE).map(str::to_owned);
        let document_base = doc
            .base_url()
            .or(doc.url())
            .cloned()
            .unwrap_or_else(about_blank);
        if tag == atoms::STYLE {
            if !is_css_type(&doc, node) {
                return None;
            }
            let text: String = doc
                .children(node)
                .filter_map(|child| doc.text_data(child))
                .collect();
            return Some(SheetText {
                href: None,
                base: document_base,
                text: text.into(),
                title,
                opaque: false,
            });
        }
        if tag != atoms::LINK
            || !doc
                .attribute(node, atoms::REL)
                .unwrap_or_default()
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
            || !is_css_type(&doc, node)
        {
            return None;
        }
        drop(doc);
        let host = self.context().style_host.borrow().clone()?;
        let linked = host.linked_sheet(node)?;
        Some(SheetText {
            href: Some(linked.url.clone()),
            base: linked.url,
            text: linked.text,
            title,
            opaque: linked.opaque,
        })
    }

    /// The `CSSStyleSheet` of the element `node`, made on first use.
    fn sheet_of(&self, node: NodeId) -> Option<Handle<CSSStyleSheet>> {
        let source = self.sheet_text(node)?;
        let ctx = self.context();
        let print = crate::stylesheets::source_fingerprint(source.href.as_ref(), &source.text);
        let reference = ctx.doc.try_borrow().ok()?.node_ref(node);
        let existing = ctx
            .doc_sheets
            .borrow()
            .entries
            .get(&reference)
            .filter(|(known, _)| *known == print)
            .map(|(_, state)| Rc::clone(state));
        let state = match existing {
            Some(state) => state,
            None => {
                let state = CssSheetState::owned(
                    CssStyleSheet::parse(&source.text, source.base),
                    SheetOwner {
                        node: reference,
                        href: source.href.map(|url| url.to_string()),
                        title: source.title,
                        opaque: source.opaque,
                    },
                );
                let mut sheets = ctx.doc_sheets.borrow_mut();
                if sheets.entries.len() >= 64 {
                    let doc = ctx.doc.borrow();
                    sheets.entries.retain(|node, _| doc.resolve(*node).is_ok());
                }
                sheets.entries.insert(reference, (print, Rc::clone(&state)));
                drop(sheets);
                // From here on the page applies what script does to it.
                if let Some(host) = ctx.style_host.borrow().clone() {
                    host.element_sheet(
                        reference,
                        print,
                        Rc::clone(&state) as Rc<dyn crate::stylesheets::ScriptSheet>,
                    );
                }
                state
            }
        };
        let key = state.sheet_key(ctx);
        Some(Handle::from_bits(encode_host(self.slot(), key)))
    }

    /// The tree whose `adoptedStyleSheets` this object has: `None` for the
    /// document, and the shadow root for a `ShadowRoot`.
    fn adopting_scope(&self) -> Option<NodeRef> {
        let (doc, id) = self.anchor();
        let doc = doc.borrow();
        (id != doc.document_node()).then(|| doc.node_ref(id))
    }

    /// `adoptedStyleSheets` of the anchored document or shadow root.
    pub(super) fn adopted_style_sheets(&self) -> Result<Vec<Handle<CSSStyleSheet>>, JsError> {
        let ctx = self.context();
        let scope = self.adopting_scope();
        let sheets = ctx
            .doc_sheets
            .borrow()
            .adopted
            .get(&scope)
            .cloned()
            .unwrap_or_default();
        Ok(sheets
            .iter()
            .map(|state| Handle::from_bits(encode_host(self.slot(), state.sheet_key(ctx))))
            .collect())
    }

    /// Assigns `adoptedStyleSheets`. Nothing changes when any of the sheets
    /// cannot be adopted here.
    pub(super) fn set_adopted_style_sheets(
        &self,
        sheets: Vec<Handle<CSSStyleSheet>>,
    ) -> Result<(), JsError> {
        let ctx = self.context();
        let mut adopted = Vec::with_capacity(sheets.len());
        for handle in sheets {
            let decoded = decode(handle.bits());
            // A sheet is adopted by the document it was constructed in: one
            // of another document is a record of another slot.
            let state = (decoded.tag == TAG_HOST && decoded.slot == self.slot())
                .then(|| match ctx.host_records.borrow().get(decoded.host_key()) {
                    Some(HostRecord::CssSheet(state)) => Some(Rc::clone(state)),
                    _ => None,
                })
                .flatten()
                .filter(|state| state.is_constructed());
            let Some(state) = state else {
                return Err(JsError::dom_exception(
                    "NotAllowedError",
                    "Can't adopt a stylesheet that was not constructed in this document",
                ));
            };
            adopted.push(state);
        }
        let scope = self.adopting_scope();
        if let Some(host) = ctx.style_host.borrow().clone() {
            host.adopted_sheets(
                scope,
                adopted
                    .iter()
                    .map(|state| Rc::clone(state) as Rc<dyn crate::stylesheets::ScriptSheet>)
                    .collect(),
            );
        }
        let mut sheets = ctx.doc_sheets.borrow_mut();
        if adopted.is_empty() {
            sheets.adopted.remove(&scope);
        } else {
            sheets.adopted.insert(scope, adopted);
        }
        Ok(())
    }

    /// `link.sheet` and `style.sheet` of the anchored element.
    pub(super) fn element_sheet(&self) -> Option<Handle<CSSStyleSheet>> {
        self.sheet_of(self.anchor().1)
    }

    /// The elements that own a sheet right now, in tree order.
    fn sheet_owners(&self) -> Rc<Vec<NodeId>> {
        let Ok(doc) = self.context().doc.try_borrow() else {
            return Rc::default();
        };
        let epoch = self
            .context()
            .style_host
            .borrow()
            .as_ref()
            .map_or(0, |host| host.epoch());
        let key = (doc.mutation_generation(), epoch);
        if let Some((known, owners)) = &self.context().doc_sheets.borrow().owners
            && *known == key
        {
            return Rc::clone(owners);
        }
        let candidates: Vec<NodeId> = doc
            .descendants(doc.document_node())
            .filter(|node| {
                matches!(doc.tag(*node), Some(tag) if tag == atoms::STYLE || tag == atoms::LINK)
            })
            .collect();
        drop(doc);
        let owners: Rc<Vec<NodeId>> = Rc::new(
            candidates
                .into_iter()
                .filter(|node| self.sheet_text(*node).is_some())
                .collect(),
        );
        self.context().doc_sheets.borrow_mut().owners = Some((key, Rc::clone(&owners)));
        owners
    }

    /// `document.styleSheets`, the same object on every read.
    pub(super) fn document_style_sheets(
        &self,
    ) -> Result<Handle<crate::bindings::StyleSheetList>, JsError> {
        let ctx = self.context();
        if let Some(key) = *ctx.style_sheet_list_key.borrow()
            && matches!(
                ctx.host_records.borrow().get(key),
                Some(HostRecord::StyleSheetList)
            )
        {
            return Ok(Handle::from_bits(encode_host(self.slot(), key)));
        }
        let key = ctx
            .host_records
            .borrow_mut()
            .insert(HostRecord::StyleSheetList);
        *ctx.style_sheet_list_key.borrow_mut() = Some(key);
        Ok(Handle::from_bits(encode_host(self.slot(), key)))
    }
}

impl StyleSheetListOps for HostObject {
    fn length(&self) -> u32 {
        u32::try_from(self.sheet_owners().len()).unwrap_or(u32::MAX)
    }

    fn item(&self, index: u32) -> Result<Option<Handle<CSSStyleSheet>>, JsError> {
        let owners = self.sheet_owners();
        Ok(owners
            .get(index as usize)
            .and_then(|node| self.sheet_of(*node)))
    }

    fn supported_property_indices(&self) -> Result<Vec<u32>, JsError> {
        Ok((0..<Self as StyleSheetListOps>::length(self)).collect())
    }

    fn is_supported_property_index(&self, index: u32) -> Result<bool, JsError> {
        Ok(index < <Self as StyleSheetListOps>::length(self))
    }
}
