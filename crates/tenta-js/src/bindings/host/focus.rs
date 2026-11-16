//! Focus: `HTMLElement.focus()` / `blur()`, `tabIndex` and the focus events
//! (HTML Standard §6.6).
//!
//! The document owns one focused area (`Document::focused_element`); this module
//! decides what can hold focus and fires the four events around a change in the
//! order browsers use: `blur`, `focusout` on the element that loses focus, then
//! `focus`, `focusin` on the one that gains it. While the loser's events run,
//! `document.activeElement` already answers the fallback, as it does in browsers.
//!
//! Not modelled: iframes (no browsing context to move focus into), shadow
//! trees, the sequential focus navigation order and the window's own focus
//! events.

use super::*;
use crate::bindings::FocusOptions;

/// The HTML "rules for parsing integers", enough for `tabindex`: optional
/// leading whitespace, an optional sign, then digits. Anything after the digits
/// is ignored; no digits is `None`.
fn parse_tabindex(value: &str) -> Option<i32> {
    let value = value.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let (negative, digits) = match value.as_bytes().first()? {
        b'-' => (true, &value[1..]),
        b'+' => (false, &value[1..]),
        _ => (false, value),
    };
    let end = digits
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    let magnitude: i64 = digits[..end].parse().ok()?;
    let signed = if negative { -magnitude } else { magnitude };
    i32::try_from(signed).ok()
}

fn is_html(doc: &Document, id: NodeId) -> bool {
    doc.namespace(id) == Some(atoms::NS_HTML)
}

/// Whether the element is a focusable area by default, ignoring `tabindex`.
fn focusable_by_default(doc: &Document, id: NodeId) -> bool {
    let Some(tag) = doc.tag(id) else {
        return false;
    };
    if !is_html(doc, id) {
        return false;
    }
    if tag == atoms::A || tag == atoms::AREA {
        return doc.has_attribute(id, atoms::HREF);
    }
    if tag == atoms::INPUT {
        return !doc
            .attribute(id, atoms::TYPE)
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("hidden"));
    }
    if tag == atoms::BUTTON || tag == atoms::SELECT || tag == atoms::TEXTAREA {
        return true;
    }
    doc.attribute(id, atoms::CONTENTEDITABLE)
        .is_some_and(|value| {
            let value = value.trim();
            value.is_empty()
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("plaintext-only")
        })
}

/// Whether `id` can hold focus: it is a connected, rendered element that is
/// either focusable by default or carries a parsable `tabindex`, and is not a
/// disabled form control. "Rendered" is approximated by the `hidden` attribute
/// on the element or an ancestor, the only way an element is hidden before
/// author styles are considered.
pub fn is_focusable_area(doc: &Document, id: NodeId) -> bool {
    if !doc.is_live(id) || !doc.is_element(id) || !doc.is_connected(id) {
        return false;
    }
    if tenta_dom::selectors::is_disabled(doc, id) {
        return false;
    }
    // `hidden` stands in for "not rendered" when there is no layout to ask;
    // `inert` removes the whole subtree from focus.
    let excluded = std::iter::once(id).chain(doc.ancestors(id)).any(|node| {
        doc.is_element(node)
            && (doc.has_attribute(node, atoms::HIDDEN)
                || doc.attribute_str(node, "inert").is_some())
    });
    if excluded {
        return false;
    }
    doc.attribute(id, atoms::TABINDEX)
        .and_then(parse_tabindex)
        .is_some()
        || focusable_by_default(doc, id)
}

/// Whether layout gives `id` a box that can show focus: no `display: none` on
/// it or an ancestor and no `visibility: hidden` on it. Without a layout to ask
/// (a page below the render tier) every element counts as rendered.
fn is_rendered(ctx: &SharedContext, id: NodeId) -> bool {
    let Some(script) = ctx.script.borrow().clone() else {
        return true;
    };
    let chain: Vec<NodeId> = {
        let doc = ctx.doc.borrow();
        // An `<area>` is `display: none` and still focusable: its image map
        // draws it.
        if doc.tag(id) == Some(atoms::AREA) {
            return true;
        }
        std::iter::once(id)
            .chain(doc.ancestors(id))
            .filter(|node| doc.is_element(*node))
            .collect()
    };
    for node in chain {
        match script.computed_style_value(node, "display") {
            None => return true,
            Some(value) if value == "none" => return false,
            Some(_) => {}
        }
    }
    !matches!(
        script.computed_style_value(id, "visibility").as_deref(),
        Some("hidden" | "collapse")
    )
}

/// Fires the `blur` and `focusout` an element owes for having left the
/// document while focused. Call after script regains control from a binding
/// call, with no borrow held.
pub fn fire_lost_focus(ctx: &SharedContext) -> Result<(), JsError> {
    let lost = match ctx.doc.try_borrow_mut() {
        Ok(mut doc) => doc.take_lost_focus(),
        Err(_) => return Ok(()),
    };
    let Some(target) = lost else {
        return Ok(());
    };
    if ctx.doc.borrow().resolve(target).is_err() {
        return Ok(());
    }
    fire(ctx, target, "blur", false, None)?;
    fire(ctx, target, "focusout", true, None)
}

/// `tabIndex` as its getter reports it: the parsed `tabindex` attribute, or 0
/// for an element that is focusable by default and -1 for every other.
pub fn tab_index(doc: &Document, id: NodeId) -> i32 {
    doc.attribute(id, atoms::TABINDEX)
        .and_then(parse_tabindex)
        .unwrap_or_else(|| if focusable_by_default(doc, id) { 0 } else { -1 })
}

/// Fires one focus-family event at `target`. UA-initiated, so trusted.
fn fire(
    ctx: &SharedContext,
    target: NodeRef,
    type_: &str,
    bubbles: bool,
    related: Option<NodeRef>,
) -> Result<(), JsError> {
    let mut record = EventRecord::new(
        type_.to_owned(),
        EventKind::Focus {
            view: (ctx.slot == 0).then_some(encode_window()),
            detail: 0,
            which: 0,
            related_target: related.map(|node| encode_node(ctx.slot, node)),
            related_value: None,
        },
        ctx.time_stamp(),
    );
    record.bubbles = bubbles;
    record.composed = true;
    record.flags.initialized = true;
    let key = ctx.events.borrow_mut().insert(record);
    let result = dispatch(ctx, target, key, true);
    let orphaned = ctx
        .events
        .borrow()
        .get(key)
        .is_some_and(|record| !record.has_wrapper);
    if orphaned {
        ctx.events.borrow_mut().remove(key);
    }
    result.map(|_| ())
}

/// The focusing steps for `id`. Does nothing when `id` is not a focusable area
/// or already holds focus.
pub fn focus_element(ctx: &SharedContext, id: NodeId, visible: bool) -> Result<(), JsError> {
    if !is_rendered(ctx, id) {
        return Ok(());
    }
    let (previous, target) = {
        let doc = ctx.doc.borrow();
        if !is_focusable_area(&doc, id) {
            return Ok(());
        }
        let previous = doc.focused_element().map(|node| doc.node_ref(node));
        (previous, doc.node_ref(id))
    };
    if previous == Some(target) {
        return Ok(());
    }
    if let Some(previous) = previous {
        ctx.doc.borrow_mut().set_focused_element(None, false);
        fire(ctx, previous, "blur", false, Some(target))?;
        fire(ctx, previous, "focusout", true, Some(target))?;
        // A listener moved focus itself: that change wins.
        let still_focusable = {
            let doc = ctx.doc.borrow();
            doc.focused_element().is_none()
                && doc
                    .resolve(target)
                    .is_ok_and(|node| is_focusable_area(&doc, node))
        };
        if !still_focusable {
            return Ok(());
        }
    }
    ctx.doc.borrow_mut().set_focused_element(Some(id), visible);
    fire(ctx, target, "focus", false, previous)?;
    fire(ctx, target, "focusin", true, previous)
}

/// The unfocusing steps for `id`: when it holds focus, focus returns to the
/// viewport.
pub fn blur_element(ctx: &SharedContext, id: NodeId) -> Result<(), JsError> {
    let target = {
        let doc = ctx.doc.borrow();
        if doc.focused_element() != Some(id) {
            return Ok(());
        }
        doc.node_ref(id)
    };
    ctx.doc.borrow_mut().set_focused_element(None, false);
    fire(ctx, target, "blur", false, None)?;
    fire(ctx, target, "focusout", true, None)
}

/// Processes the document's `autofocus` candidates: the first element in tree
/// order that has the attribute and is a focusable area receives focus, unless
/// something already holds it. The standard queues candidates as elements are
/// inserted and flushes them at the first rendering opportunity; this runs once
/// when the document's `DOMContentLoaded` has fired, which is the same moment
/// for a parsed page and differs for an element a script inserts later.
pub fn flush_autofocus(ctx: &SharedContext) -> Result<(), JsError> {
    let candidate = {
        let doc = ctx.doc.borrow();
        if doc.focused_element().is_some() {
            return Ok(());
        }
        let root = doc.document_node();
        doc.descendants(root).find(|node| {
            doc.is_element(*node)
                && is_html(&doc, *node)
                && doc.attribute_str(*node, "autofocus").is_some()
                && is_focusable_area(&doc, *node)
        })
    };
    match candidate {
        Some(node) => focus_element(ctx, node, true),
        None => Ok(()),
    }
}

impl HostObject {
    pub(super) fn focus_steps(&self, options: Option<FocusOptions>) -> Result<(), JsError> {
        let (_, id) = self.anchor();
        let visible = options
            .and_then(|options| options.focus_visible)
            .unwrap_or(true);
        focus_element(self.context(), id, visible)
    }

    pub(super) fn unfocus_steps(&self) -> Result<(), JsError> {
        let (_, id) = self.anchor();
        blur_element(self.context(), id)
    }

    pub(super) fn tab_index_value(&self) -> i32 {
        let (doc, id) = self.anchor();
        tab_index(&doc.borrow(), id)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tabindex;

    #[test]
    fn tabindex_follows_the_rules_for_parsing_integers() {
        assert_eq!(parse_tabindex("0"), Some(0));
        assert_eq!(parse_tabindex("  -1"), Some(-1));
        assert_eq!(parse_tabindex("+3x"), Some(3));
        assert_eq!(parse_tabindex(""), None);
        assert_eq!(parse_tabindex("abc"), None);
        assert_eq!(parse_tabindex("-"), None);
        assert_eq!(parse_tabindex("99999999999"), None);
    }
}
