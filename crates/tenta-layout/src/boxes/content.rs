//! Generated content, counters and list markers.

use style::counter_style::CounterStyle;
use style::properties::ComputedValues;
use style::values::generics::counters::{GenericContent, GenericContentItem};
use style::values::specified::list::Quotes;
use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_css::cascade::Style;

use super::builder::Builder;
use super::counters::format_counter;
use super::paint::paint_style;
use crate::box_tree::{BoxFlags, BoxId, BoxKind, LayoutBox};
use crate::types::{DecorationPaint, MarkerKind};

const LIST_ITEM: &str = "list-item";

/// Opening and closing quotes `quotes: auto` falls back to, by nesting level.
const AUTO_QUOTES: [(&str, &str); 2] = [("\u{201C}", "\u{201D}"), ("\u{2018}", "\u{2019}")];

fn counter_text(value: i32, style: &CounterStyle) -> String {
    match style {
        CounterStyle::Name(name) => format_counter(value, &name.0),
        CounterStyle::None => String::new(),
        CounterStyle::String(text) => text.to_string(),
        CounterStyle::Symbols { .. } => value.to_string(),
    }
}

impl Builder<'_, '_> {
    /// Applies `counter-reset` and `counter-increment` of one box, plus the
    /// implicit list-item counting HTML defines for `ol`, `ul`, `menu` and `li`.
    pub(super) fn apply_counters(&mut self, node: NodeId, style: &ComputedValues, generated: bool) {
        let doc = self.cx.doc;
        let scope = self.depth;
        let html = !generated && doc.namespace(node) == Some(atoms::NS_HTML);
        let tag = if html { doc.tag(node) } else { None };

        if tag == Some(atoms::OL) {
            let reversed = doc.has_attribute(node, atoms::REVERSED);
            let step = if reversed { -1 } else { 1 };
            let start = doc
                .attribute(node, atoms::START)
                .and_then(|v| v.trim().parse::<i32>().ok())
                .unwrap_or_else(|| {
                    if reversed {
                        doc.children(node)
                            .filter(|&c| doc.tag(c) == Some(atoms::LI))
                            .count() as i32
                    } else {
                        1
                    }
                });
            self.counters.reset(LIST_ITEM, start - step, scope, step);
        } else if tag == Some(atoms::UL) || tag == Some(atoms::MENU) {
            self.counters.reset(LIST_ITEM, 0, scope, 1);
        }

        let counters = style.get_counters();
        for pair in counters.counter_reset.iter() {
            self.counters.reset(&pair.name.0, pair.value, scope, 1);
        }
        let mut explicit_list_item = false;
        for pair in counters.counter_increment.iter() {
            explicit_list_item |= &*pair.name.0 == LIST_ITEM;
            self.counters.increment(&pair.name.0, pair.value, scope);
        }
        if style.get_box().display.is_list_item() && !explicit_list_item {
            let value = (tag == Some(atoms::LI))
                .then(|| doc.attribute(node, atoms::VALUE))
                .flatten()
                .and_then(|v| v.trim().parse::<i32>().ok());
            match value {
                Some(v) => self.counters.set(LIST_ITEM, v, scope),
                None => {
                    let step = self.counters.step(LIST_ITEM);
                    self.counters.increment(LIST_ITEM, step, scope);
                }
            }
        }
    }

    /// The text of a `::before` or `::after` box with style `style`, or `None`
    /// when the pseudo-element generates nothing.
    pub(super) fn generated_content(&mut self, node: NodeId, style: &Style) -> Option<String> {
        if style.get_box().display.is_none() {
            return None;
        }
        self.apply_counters(node, style, true);
        self.content_text(node, style)
    }

    /// Resolves the item list of `content` to text; `None` for `normal` and
    /// `none`.
    fn content_text(&mut self, node: NodeId, style: &Style) -> Option<String> {
        let GenericContent::Items(items) = &style.get_counters().content else {
            return None;
        };
        let doc = self.cx.doc;
        let visible = &items.items[..items.alt_start.min(items.items.len())];
        let mut out = String::new();
        for item in visible {
            match item {
                GenericContentItem::String(s) => out.push_str(s),
                GenericContentItem::Counter(name, counter_style) => {
                    out.push_str(&counter_text(self.counters.value(&name.0), counter_style));
                }
                GenericContentItem::Counters(name, separator, counter_style) => {
                    let parts: Vec<String> = self
                        .counters
                        .all(&name.0)
                        .map(|v| counter_text(v, counter_style))
                        .collect();
                    out.push_str(&parts.join(separator));
                }
                GenericContentItem::OpenQuote => {
                    out.push_str(&self.quote(style, true));
                    self.quote_depth += 1;
                }
                GenericContentItem::CloseQuote => {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                    out.push_str(&self.quote(style, false));
                }
                GenericContentItem::NoOpenQuote => self.quote_depth += 1,
                GenericContentItem::NoCloseQuote => {
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                }
                GenericContentItem::Attr(attr) => match doc.attribute_str(node, &attr.attribute) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&attr.fallback),
                },
                // Generated images arrive with image support.
                GenericContentItem::Image(_) => {}
            }
        }
        Some(out)
    }

    fn quote(&self, style: &ComputedValues, open: bool) -> String {
        let depth = self.quote_depth;
        match &style.get_list().quotes {
            Quotes::Auto => {
                let (o, c) = AUTO_QUOTES[depth.min(AUTO_QUOTES.len() - 1)];
                (if open { o } else { c }).to_owned()
            }
            Quotes::QuoteList(list) => list
                .0
                .get(depth.min(list.0.len().saturating_sub(1)))
                .map(|pair| if open { &pair.opening } else { &pair.closing })
                .map_or_else(String::new, |s| s.to_string()),
        }
    }

    /// Creates the marker box of a list item that has just been entered.
    /// `::marker { content }` replaces the default marker and, as in CSS Lists
    /// 3, produces one even when `list-style-type` is `none`.
    pub(super) fn push_marker(&mut self, node: NodeId, item: BoxId, item_style: &Style) {
        let marker_style = self.marker_style(node, item_style);
        let content = match marker_style.as_ref().map(|s| &s.get_counters().content) {
            Some(GenericContent::None) => return,
            Some(GenericContent::Items(_)) => {
                let style = marker_style.clone().expect("matched Some above");
                self.content_text(node, &style)
            }
            _ => None,
        };
        let default = self.tree.get(item).paint.list_marker;
        let kind = match (&content, default) {
            (Some(_), _) => MarkerKind::Text,
            (None, Some(m)) => m.kind,
            (None, None) => return,
        };
        let inside = matches!(
            item_style.get_list().list_style_position,
            style::computed_values::list_style_position::T::Inside
        );
        let text = match content {
            Some(text) => Some(text),
            None => (kind == MarkerKind::Text).then(|| self.marker_text(item_style)),
        };
        let style = marker_style.unwrap_or_else(|| self.anon.derive(item_style, false));
        let (paint, paint_flags) = paint_style(&style, &DecorationPaint::default());
        let mut flags = BoxFlags::GENERATED | paint_flags;
        if inside {
            flags |= BoxFlags::INLINE_LEVEL;
        }
        let parent = if inside { BoxId::NONE } else { item };
        let mut layout_box = LayoutBox::new(
            node,
            parent,
            BoxKind::Marker(kind),
            style,
            std::sync::Arc::new(paint),
        );
        layout_box.flags = flags;
        let id = self.push_marker_box(layout_box, text.as_deref(), inside);
        if !inside {
            self.tree.get_mut(item).kind = BoxKind::ListItem { marker: Some(id) };
        }
    }

    fn marker_text(&self, item_style: &ComputedValues) -> String {
        let value = self.counters.value(LIST_ITEM);
        match &item_style.get_list().list_style_type.0 {
            CounterStyle::String(text) => text.to_string(),
            CounterStyle::Name(name) => format!("{}. ", format_counter(value, &name.0)),
            _ => format!("{value}. "),
        }
    }
}
