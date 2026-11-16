//! Layout answers for script (CSS1): the numbers `getBoundingClientRect`,
//! `offsetWidth`, `clientHeight`, `scrollHeight`, `getComputedStyle` and
//! `elementFromPoint` report.
//!
//! The page asks through [`RenderSession`], which brings style and layout up to
//! date first (they do nothing while the document and viewport are unchanged),
//! so a script that reads geometry between two mutations sees the layout of the
//! document as it is. Nothing scrolls, so viewport and document coordinates are
//! the same and `scrollTop`/`scrollLeft` are 0.
//!
//! Values follow CSSOM View. They are not rounded here except where the
//! standard says an integer is returned, so the caller maps them onto the
//! `DOMRect` and `long` members.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_css::cascade::PseudoKind;
use tenta_dom::Document;
use tenta_layout::{FragId, FragmentTree, OverflowKind, PositionKind, Rect};

use crate::geometry::{hit_test, node_boxes, own_fragments};
use crate::session::RenderSession;

/// `offset*`, `client*` and `scroll*` of one element, in CSS pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementMetrics {
    /// `offsetTop`.
    pub offset_top: f64,
    /// `offsetLeft`.
    pub offset_left: f64,
    /// `offsetWidth`.
    pub offset_width: f64,
    /// `offsetHeight`.
    pub offset_height: f64,
    /// `offsetParent`, when the element has one.
    pub offset_parent: Option<NodeId>,
    /// `clientTop`.
    pub client_top: f64,
    /// `clientLeft`.
    pub client_left: f64,
    /// `clientWidth`.
    pub client_width: f64,
    /// `clientHeight`.
    pub client_height: f64,
    /// `scrollWidth`.
    pub scroll_width: f64,
    /// `scrollHeight`.
    pub scroll_height: f64,
}

/// What the second argument of `getComputedStyle` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoArgument {
    /// No pseudo-element: the element itself.
    None,
    /// A pseudo-element this engine resolves.
    Element(PseudoKind),
    /// A string that names none: the declaration is empty.
    Unknown,
}

/// Parses the pseudo-element argument of `getComputedStyle`.
pub fn parse_pseudo(text: Option<&str>) -> PseudoArgument {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return PseudoArgument::None;
    };
    // The single-colon spellings of the CSS 2 pseudo-elements are still valid.
    let Some(name) = text.strip_prefix("::").or_else(|| text.strip_prefix(':')) else {
        return PseudoArgument::Unknown;
    };
    match name.to_ascii_lowercase().as_str() {
        "before" => PseudoArgument::Element(PseudoKind::Before),
        "after" => PseudoArgument::Element(PseudoKind::After),
        "marker" if text.starts_with("::") => PseudoArgument::Element(PseudoKind::Marker),
        "first-line" => PseudoArgument::Element(PseudoKind::FirstLine),
        "first-letter" => PseudoArgument::Element(PseudoKind::FirstLetter),
        _ => PseudoArgument::Unknown,
    }
}

impl RenderSession {
    /// The border box of every fragment `node` produced, as `[x, y, width,
    /// height]` in viewport coordinates, transforms applied. Empty for a node
    /// with no box.
    pub fn element_rects(&mut self, doc: &Document, node: NodeId) -> Vec<[f64; 4]> {
        if !doc.is_element(node) || !doc.is_connected(node) || doc.document_element().is_none() {
            return Vec::new();
        }
        let tree = self.layout(doc);
        node_boxes(tree, node)
            .iter()
            .map(|boxes| {
                let (x, y, width, height) = boxes.border.bounds();
                [x, y, width, height]
            })
            .collect()
    }

    /// `offset*`, `client*` and `scroll*` of `node`; `None` when it has no box.
    pub fn element_metrics(&mut self, doc: &Document, node: NodeId) -> Option<ElementMetrics> {
        if !doc.is_element(node) || !doc.is_connected(node) || doc.document_element().is_none() {
            return None;
        }
        self.layout(doc);
        let inline = self.is_inline_box(doc, node);
        let quirks = self.styles.env().quirks;
        let tree = self.frags.as_ref()?;
        let fragments = own_fragments(tree, node);
        let first = *fragments.first()?;
        let fragment = tree.get(first);
        // The root reports the viewport in standards mode, and the body does in
        // quirks mode, where the root is an ordinary box.
        let viewport_element = if quirks {
            doc.body() == Some(node)
        } else {
            doc.document_element() == Some(node)
        };
        let is_body = doc.body() == Some(node);

        // offset*: the union of the border boxes, ignoring transforms.
        let union = fragments
            .iter()
            .map(|&id| tree.absolute_border_box(id))
            .reduce(union_of)?;
        let first_box = tree.absolute_border_box(first);
        let (offset_parent, parent_box) = offset_parent_of(doc, tree, node);
        // Against a static `<body>` the offsets are taken from the root's border
        // box, not from the document origin or the body (Chromium's rule; the
        // body itself reports 0).
        let root_origin = || {
            let root = tree.get(tree.root);
            (root.rect.x, root.rect.y)
        };
        let origin = match (parent_box, offset_parent) {
            (Some(parent), _) => (parent.x, parent.y),
            (None, Some(_)) => root_origin(),
            (None, None) => (0.0, 0.0),
        };
        let mut metrics = ElementMetrics {
            offset_top: if is_body {
                0.0
            } else {
                f64::from(first_box.y - origin.1).round()
            },
            offset_left: if is_body {
                0.0
            } else {
                f64::from(first_box.x - origin.0).round()
            },
            offset_width: f64::from(union.w).round(),
            offset_height: f64::from(union.h).round(),
            offset_parent,
            ..ElementMetrics::default()
        };

        // client*: the padding box, zero for an inline box; the root and a quirks
        // body report the viewport.
        let viewport = tree.viewport;
        if viewport_element {
            metrics.client_top = f64::from(fragment.border.top).round();
            metrics.client_left = f64::from(fragment.border.left).round();
            metrics.client_width = f64::from(viewport.w).round();
            metrics.client_height = f64::from(viewport.h).round();
        } else if !inline {
            metrics.client_top = f64::from(fragment.border.top).round();
            metrics.client_left = f64::from(fragment.border.left).round();
            // Chromium measures a table's client box from its border box.
            let is_table = doc.tag(node) == Some(atoms::TABLE);
            let (width, height) = if is_table {
                (fragment.rect.w, fragment.rect.h)
            } else {
                (
                    fragment.rect.w - fragment.border.left - fragment.border.right,
                    fragment.rect.h - fragment.border.top - fragment.border.bottom,
                )
            };
            metrics.client_width = f64::from(width).round();
            metrics.client_height = f64::from(height).round();
        }

        // scroll*: the scrollable overflow area, at least the client box.
        if viewport_element {
            metrics.scroll_width = metrics
                .client_width
                .max(f64::from(tree.scroll_extent.right()).round());
            metrics.scroll_height = metrics
                .client_height
                .max(f64::from(tree.scroll_extent.bottom()).round());
        } else if !inline {
            // The overflow rectangle starts as the border box, which is not part
            // of the scrollable overflow; it only counts where content reaches
            // past it.
            let beyond = |edge: f32, border_box: f32, border: f32| {
                if edge > border_box {
                    edge - border
                } else {
                    0.0
                }
            };
            let overflow_right = beyond(
                fragment.overflow.right(),
                fragment.rect.w,
                fragment.border.left,
            );
            let overflow_bottom = beyond(
                fragment.overflow.bottom(),
                fragment.rect.h,
                fragment.border.top,
            );
            // A scroll container's end padding is part of what scrolls once
            // the content reaches past the padding box.
            let scroll_container = !matches!(fragment.paint.overflow_x, OverflowKind::Visible)
                || !matches!(fragment.paint.overflow_y, OverflowKind::Visible);
            let end_padding = |reaches: bool, padding: f32| {
                if scroll_container && reaches {
                    padding
                } else {
                    0.0
                }
            };
            // The padding box, which is also what a table's scroll size starts
            // from although its client box is the border box.
            let padding_box_width =
                f64::from(fragment.rect.w - fragment.border.left - fragment.border.right).round();
            let padding_box_height =
                f64::from(fragment.rect.h - fragment.border.top - fragment.border.bottom).round();
            let right = overflow_right
                + end_padding(
                    f64::from(overflow_right) > padding_box_width,
                    fragment.padding.right,
                );
            let bottom = overflow_bottom
                + end_padding(
                    f64::from(overflow_bottom) > padding_box_height,
                    fragment.padding.bottom,
                );
            metrics.scroll_width = padding_box_width.max(f64::from(right).round());
            metrics.scroll_height = padding_box_height.max(f64::from(bottom).round());
        }
        Some(metrics)
    }

    /// The element under a point of the viewport, `None` outside it.
    pub fn element_at(&mut self, doc: &Document, x: f64, y: f64) -> Option<NodeId> {
        if doc.document_element().is_none() || !x.is_finite() || !y.is_finite() {
            return None;
        }
        let viewport = self.config.viewport;
        if x < 0.0 || y < 0.0 || x >= f64::from(viewport.width) || y >= f64::from(viewport.height) {
            return None;
        }
        let tree = self.layout(doc);
        hit_test(tree, doc, x as f32, y as f32)
    }

    /// The resolved value of `property` of `node` (or of its pseudo-element), as
    /// `getComputedStyle` reports it: the computed value, except where CSSOM
    /// makes it the used value and the element has a box (`width`, `height`,
    /// the margins, the paddings, `top`/`right`/`bottom`/`left` of a positioned
    /// box, `transform-origin`).
    ///
    /// `None` for an unknown or unsupported property, and for a node with no
    /// style (not an element, or detached).
    pub fn resolved_value(
        &mut self,
        doc: &Document,
        node: NodeId,
        pseudo: Option<PseudoKind>,
        property: &str,
    ) -> Option<String> {
        if doc.document_element().is_none() || !doc.is_element(node) {
            return None;
        }
        let property = property.to_ascii_lowercase();
        // Only a used value needs a layout; every other property is the computed
        // value, for which the styles are enough.
        let used = pseudo.is_none() && depends_on_layout(&property);
        if used {
            self.layout(doc);
        } else {
            self.flush_styles(doc);
        }
        let value = self
            .styles
            .computed_value_in(doc, node, pseudo, &property)
            .ok()??;
        if !used {
            return Some(value);
        }
        Some(self.use_layout(doc, node, &property, value))
    }

    /// Whether `node`'s principal box is a non-replaced inline box.
    fn is_inline_box(&self, doc: &Document, node: NodeId) -> bool {
        let display = self
            .styles
            .computed_value_in(doc, node, None, "display")
            .ok()
            .flatten();
        if display.as_deref() != Some("inline") {
            return false;
        }
        // A replaced element is `display: inline` too, and has a padding box.
        !matches!(
            doc.tag(node),
            Some(tag) if tag == atoms::IMG || tag == atoms::INPUT || tag == atoms::BUTTON
                || tag == atoms::SELECT || tag == atoms::TEXTAREA
        )
    }

    /// Replaces the computed `value` of `property` by its used value when CSSOM
    /// says the resolved value is the used one and the element has a box.
    fn use_layout(&self, doc: &Document, node: NodeId, property: &str, value: String) -> String {
        let used_property = depends_on_layout(property);
        if !used_property {
            return value;
        }
        let Some(tree) = self.frags.as_ref() else {
            return value;
        };
        let Some(&first) = own_fragments(tree, node).first() else {
            return trim_origin_depth(property, value);
        };
        let display = self
            .styles
            .computed_value_in(doc, node, None, "display")
            .ok()
            .flatten()
            .unwrap_or_default();
        if display == "none" || display == "contents" {
            return trim_origin_depth(property, value);
        }
        let fragment = tree.get(first);
        let inline = self.is_inline_box(doc, node);
        let px = |number: f32| format!("{}px", css_number(f64::from(number)));
        match property {
            "width" | "height" | "inline-size" | "block-size" => {
                if inline {
                    return value;
                }
                let horizontal = matches!(property, "width" | "inline-size");
                let border_box = self
                    .styles
                    .computed_value_in(doc, node, None, "box-sizing")
                    .ok()
                    .flatten()
                    .is_some_and(|sizing| sizing == "border-box");
                let (size, extra) = if horizontal {
                    (
                        fragment.rect.w,
                        fragment.border.left
                            + fragment.border.right
                            + fragment.padding.left
                            + fragment.padding.right,
                    )
                } else {
                    (
                        fragment.rect.h,
                        fragment.border.top
                            + fragment.border.bottom
                            + fragment.padding.top
                            + fragment.padding.bottom,
                    )
                };
                px(if border_box {
                    size
                } else {
                    (size - extra).max(0.0)
                })
            }
            // Chromium reports a fixed margin or padding as written and the used
            // value of an `auto` or percentage one.
            _ if value.ends_with("px")
                && (property.starts_with("margin-") || property.starts_with("padding-")) =>
            {
                value
            }
            "margin-left" => px(fragment.margin.left),
            "margin-right" => px(fragment.margin.right),
            "margin-top" if !inline => px(fragment.margin.top),
            "margin-bottom" if !inline => px(fragment.margin.bottom),
            "padding-left" => px(fragment.padding.left),
            "padding-right" => px(fragment.padding.right),
            "padding-top" => px(fragment.padding.top),
            "padding-bottom" => px(fragment.padding.bottom),
            // `min-width: auto` is the content-based minimum only for a flex or
            // grid item; anywhere else it is 0.
            "min-width" | "min-height" if value == "auto" => {
                let in_flex_or_grid = doc
                    .ancestors(node)
                    .next()
                    .and_then(|parent| {
                        self.styles
                            .computed_value_in(doc, parent, None, "display")
                            .ok()
                            .flatten()
                    })
                    .is_some_and(|display| display.contains("flex") || display.contains("grid"));
                if in_flex_or_grid {
                    value
                } else {
                    "0px".to_owned()
                }
            }
            "top" | "right" | "bottom" | "left" => {
                let opposite = match property {
                    "top" => "bottom",
                    "bottom" => "top",
                    "left" => "right",
                    _ => "left",
                };
                let opposite = self
                    .styles
                    .computed_value_in(doc, node, None, opposite)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                positioned_offset(tree, first, property, &value, &opposite).unwrap_or(value)
            }
            "transform-origin" | "perspective-origin" => {
                // An inline box does not transform: its percentages are of nothing.
                let size = if inline {
                    Rect::new(0.0, 0.0, 0.0, 0.0)
                } else {
                    fragment.rect
                };
                origin_in_pixels(&value, size).unwrap_or(value)
            }
            _ => value,
        }
    }
}

fn union_of(a: Rect, b: Rect) -> Rect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = a.right().max(b.right());
    let bottom = a.bottom().max(b.bottom());
    Rect::new(left, top, right - left, bottom - top)
}

/// CSSOM View's `offsetParent`, with the padding box (absolute coordinates) of
/// the element it returns. `(None, None)` when the element has none; the
/// `<body>` answers with no padding box because offsets against it are taken
/// from the document origin.
fn offset_parent_of(
    doc: &Document,
    tree: &FragmentTree,
    node: NodeId,
) -> (Option<NodeId>, Option<Rect>) {
    if doc.document_element() == Some(node) || doc.body() == Some(node) {
        return (None, None);
    }
    let Some(&first) = own_fragments(tree, node).first() else {
        return (None, None);
    };
    let position = tree.get(first).paint.position;
    if position == PositionKind::Fixed {
        return (None, None);
    }
    for ancestor in doc.ancestors(node) {
        if !doc.is_element(ancestor) {
            continue;
        }
        let Some(&id) = own_fragments(tree, ancestor).first() else {
            continue;
        };
        let fragment = tree.get(id);
        if doc.body() == Some(ancestor) && fragment.paint.position == PositionKind::Static {
            return (Some(ancestor), None);
        }
        let table_part = matches!(
            doc.tag(ancestor),
            Some(tag) if tag == atoms::TD || tag == atoms::TH || tag == atoms::TABLE
        );
        if fragment.paint.position != PositionKind::Static
            || (position == PositionKind::Static && table_part)
        {
            let border_box = tree.absolute_border_box(id);
            let padding_box = Rect::new(
                border_box.x + fragment.border.left,
                border_box.y + fragment.border.top,
                border_box.w - fragment.border.left - fragment.border.right,
                border_box.h - fragment.border.top - fragment.border.bottom,
            );
            return (Some(ancestor), Some(padding_box));
        }
    }
    (None, None)
}

/// The used value of an inset property of a positioned box, or `None` when the
/// computed value stands (a static box). `opposite` is the computed value of the
/// property on the other side of the same axis.
fn positioned_offset(
    tree: &FragmentTree,
    id: FragId,
    property: &str,
    computed: &str,
    opposite: &str,
) -> Option<String> {
    let fragment = tree.get(id);
    let px = |number: f32| format!("{}px", css_number(f64::from(number)));
    match fragment.paint.position {
        PositionKind::Static => None,
        PositionKind::Relative | PositionKind::Sticky => {
            // A relative box is offset from its normal-flow place by the
            // computed inset; `auto` on both sides of an axis is 0, and on one
            // side it is the negative of the other.
            let parent = tree.parent(id).map(|parent| tree.get(parent).rect);
            let along = match property {
                "left" | "right" => parent.map_or(0.0, |p| p.w),
                _ => parent.map_or(0.0, |p| p.h),
            };
            let resolve = |text: &str| -> Option<f32> {
                if let Some(number) = text.strip_suffix("px") {
                    number.parse().ok()
                } else if let Some(percent) = text.strip_suffix('%') {
                    percent.parse::<f32>().ok().map(|p| p * along / 100.0)
                } else {
                    None
                }
            };
            Some(px(resolve(computed)
                .or_else(|| resolve(opposite).map(|other| -other))
                .unwrap_or(0.0)))
        }
        PositionKind::Absolute | PositionKind::Fixed => {
            let block = tree.containing_block_of(id)?;
            let block_fragment = tree.get(block);
            let block_box = tree.absolute_border_box(block);
            let padding_box = Rect::new(
                block_box.x + block_fragment.border.left,
                block_box.y + block_fragment.border.top,
                block_box.w - block_fragment.border.left - block_fragment.border.right,
                block_box.h - block_fragment.border.top - block_fragment.border.bottom,
            );
            let border = tree.absolute_border_box(id);
            let margin = fragment.margin;
            let margin_box = Rect::new(
                border.x - margin.left,
                border.y - margin.top,
                border.w + margin.left + margin.right,
                border.h + margin.top + margin.bottom,
            );
            Some(px(match property {
                "left" => margin_box.x - padding_box.x,
                "top" => margin_box.y - padding_box.y,
                "right" => padding_box.right() - margin_box.right(),
                _ => padding_box.bottom() - margin_box.bottom(),
            }))
        }
    }
}

/// `transform-origin` / `perspective-origin` with percentages resolved against
/// the border box.
fn origin_in_pixels(value: &str, border_box: Rect) -> Option<String> {
    let mut parts = value.split_whitespace();
    let x = parts.next()?;
    let y = parts.next()?;
    let z = parts.next();
    let resolve = |text: &str, size: f32| -> Option<f64> {
        if let Some(percent) = text.strip_suffix('%') {
            percent
                .parse::<f64>()
                .ok()
                .map(|p| p * f64::from(size) / 100.0)
        } else {
            text.strip_suffix("px")?.parse().ok()
        }
    };
    let mut text = format!(
        "{}px {}px",
        css_number(resolve(x, border_box.w)?),
        css_number(resolve(y, border_box.h)?)
    );
    if let Some(z) = z.filter(|z| *z != "0px") {
        text.push(' ');
        text.push_str(z);
    }
    Some(text)
}

/// Chromium leaves the third component of an origin out while it is 0.
fn trim_origin_depth(property: &str, value: String) -> String {
    if matches!(property, "transform-origin" | "perspective-origin")
        && let Some(trimmed) = value.strip_suffix(" 0px")
        && trimmed.split_whitespace().count() == 2
    {
        return trimmed.to_owned();
    }
    value
}

/// A number the way Chromium serialises one: at most six significant digits,
/// no trailing zeros, no exponent.
pub fn css_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).clamp(0, 12) as usize;
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" { "0".to_owned() } else { text }
}

/// Whether the resolved value of `property` can differ from its computed value
/// once the element has a box.
fn depends_on_layout(property: &str) -> bool {
    matches!(
        property,
        "width"
            | "height"
            | "inline-size"
            | "block-size"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "min-width"
            | "min-height"
            | "transform-origin"
            | "perspective-origin"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_serialised_with_six_significant_digits() {
        assert_eq!(css_number(100.0), "100");
        assert_eq!(css_number(100.0 / 3.0), "33.3333");
        assert_eq!(css_number(1000.0 / 3.0), "333.333");
        assert_eq!(css_number(0.5), "0.5");
        assert_eq!(css_number(-12.25), "-12.25");
        assert_eq!(css_number(1234567.0), "1234567");
        assert_eq!(css_number(0.000123456789), "0.000123457");
    }

    #[test]
    fn pseudo_elements_are_parsed_in_both_spellings() {
        use PseudoArgument::{Element, Unknown};
        assert_eq!(parse_pseudo(None), PseudoArgument::None);
        assert_eq!(parse_pseudo(Some("")), PseudoArgument::None);
        assert_eq!(parse_pseudo(Some("::before")), Element(PseudoKind::Before));
        assert_eq!(parse_pseudo(Some(":after")), Element(PseudoKind::After));
        assert_eq!(parse_pseudo(Some("::marker")), Element(PseudoKind::Marker));
        assert_eq!(parse_pseudo(Some(":marker")), Unknown);
        assert_eq!(parse_pseudo(Some("before")), Unknown);
        assert_eq!(parse_pseudo(Some("::selection")), Unknown);
    }
}
