//! List-item layout and marker placement.
//!
//! An outside marker hangs off the start edge of its list item's border box,
//! aligned to the baseline of the item's first line, the way Blink lays out
//! `::marker`. Bullets are boxes of their own (the fragment rectangle is the
//! shape paint fills); counter strings are text and keep their glyphs in a
//! child fragment.

use smallvec::SmallVec;
use tenta_core::NodeId;

use crate::box_tree::{BoxId, BoxKind, BoxTree, LayoutCx};
use crate::constraint::own_direction;
use crate::fragment::{FragId, Fragment, FragmentBuilder, FragmentKind};
use crate::inline::layout_inline;
use crate::space::{AvailableSize, ConstraintSpace, NoFloats};
use crate::types::{Direction, MarkerKind, Rect};

/// Blink's gap between a bullet's box and the list item (`kCMarkerPaddingPx`).
const BULLET_GAP: f32 = 7.0;

/// What a marker needs to know about its list item once the item's content is
/// laid out.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ItemGeometry {
    /// Border-box width of the item, where an RTL marker hangs off.
    pub(crate) width: f32,
    /// Top of the item's content box, from its border-box top.
    pub(crate) content_top: f32,
    /// Baseline of the item's first line from its border-box top; `None` when
    /// the item has no line, then the strut of the item stands in.
    pub(crate) first_baseline: Option<f32>,
}

/// Lays out the outside marker box `marker` of a list item and returns its
/// fragment, positioned relative to the list item's border box. The marker
/// hangs off the inline-start edge: left of the item in LTR, right of it in RTL.
///
/// The fragment belongs to no DOM node: the marker box shares the item's node,
/// and a geometry query for the item must find the item's own box first.
pub(crate) fn layout_marker(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    marker: BoxId,
    item: ItemGeometry,
    out: &mut FragmentBuilder,
) -> FragId {
    let m = tree.get(marker);
    let BoxKind::Marker(kind) = m.kind else {
        unreachable!("a list item's marker box has a marker kind");
    };
    let baseline = item
        .first_baseline
        .unwrap_or_else(|| item.content_top + strut_baseline(cx, &tree.get(m.parent).style));
    let rtl = own_direction(&m.style) == Direction::Rtl;
    let (rect, children) = match (kind, &m.ifc) {
        (MarkerKind::Text, Some(ifc)) => {
            let mut space =
                ConstraintSpace::new(AvailableSize::MaxContent, AvailableSize::MaxContent);
            space.direction = own_direction(&m.style);
            let inline = layout_inline(cx, tree, ifc, &space, &mut NoFloats, out);
            let rect = Rect::new(
                if rtl {
                    item.width
                } else {
                    -inline.inline_extent
                },
                baseline - inline.first_baseline.unwrap_or(0.0),
                inline.inline_extent,
                inline.block_size,
            );
            let mut texts: SmallVec<[FragId; 4]> = SmallVec::new();
            for &line in &inline.lines {
                let origin = out.get(line).rect;
                let kids = out.get(line).children.clone();
                for kid in kids {
                    let text = out.get_mut(kid);
                    text.node = NodeId::NONE;
                    text.rect.x += origin.x;
                    text.rect.y += origin.y;
                    texts.push(kid);
                }
            }
            (rect, texts)
        }
        _ => {
            let ascent = ascent_of(cx, &m.style).round() as i32;
            let symbol_width = ascent * 2 / 3 + 1;
            let size = (symbol_width / 2) as f32;
            // The bullet sits one pixel inside its marker box on the side that
            // faces the text.
            let x = if rtl {
                item.width + BULLET_GAP + (symbol_width - 1) as f32 - size
            } else {
                1.0 - BULLET_GAP - symbol_width as f32
            };
            let y = baseline - ascent as f32 + (3 * (ascent - ascent * 2 / 3) / 2) as f32;
            (Rect::new(x, y, size, size), SmallVec::new())
        }
    };
    let mut fragment = Fragment::new(
        NodeId::NONE,
        FragmentKind::Marker(kind),
        tree.paint_style_for(marker, rect),
        rect,
    );
    fragment.overflow = Rect::new(0.0, 0.0, rect.w, rect.h);
    fragment.children = children;
    out.push(fragment)
}

#[cfg(feature = "embedded-fonts")]
mod metrics {
    use style::properties::ComputedValues;
    use style::values::computed::font::{
        FontFamilyNameSyntax, FontStyle, GenericFontFamily, LineHeight, SingleFontFamily,
    };

    use crate::box_tree::LayoutCx;
    use crate::font::VerticalMetrics;

    fn family_list(style: &ComputedValues) -> String {
        let mut out = String::new();
        for family in style.get_font().font_family.families.iter() {
            if !out.is_empty() {
                out.push_str(", ");
            }
            match family {
                SingleFontFamily::FamilyName(name) => {
                    let text = name.name.to_string();
                    if name.syntax == FontFamilyNameSyntax::Quoted {
                        out.push('"');
                        for ch in text.chars() {
                            if matches!(ch, '"' | '\\') {
                                out.push('\\');
                            }
                            out.push(ch);
                        }
                        out.push('"');
                    } else {
                        out.push_str(&text);
                    }
                }
                SingleFontFamily::Generic(generic) => out.push_str(match generic {
                    GenericFontFamily::Serif => "serif",
                    GenericFontFamily::SansSerif => "sans-serif",
                    GenericFontFamily::Monospace => "monospace",
                    GenericFontFamily::Cursive => "cursive",
                    GenericFontFamily::Fantasy => "fantasy",
                    GenericFontFamily::SystemUi => "system-ui",
                    GenericFontFamily::None => continue,
                }),
            }
        }
        out
    }

    fn vertical_metrics(cx: &LayoutCx, style: &ComputedValues) -> (VerticalMetrics, f32) {
        let font = style.get_font();
        let size = font.font_size.computed_size().px();
        let weight = font.font_weight.value().clamp(1.0, 1000.0).round() as u16;
        let italic = font.font_style != FontStyle::NORMAL;
        let metrics = cx
            .fonts
            .resolve(&family_list(style), weight, italic)
            .vertical_metrics(size);
        (metrics, size)
    }

    /// The rounded ascent of the primary font of `style`.
    pub(super) fn ascent_of(cx: &LayoutCx, style: &ComputedValues) -> f32 {
        vertical_metrics(cx, style).0.ascent
    }

    /// The height of a line holding only the strut of `style`.
    pub(crate) fn strut_height(cx: &LayoutCx, style: &ComputedValues) -> f32 {
        let (metrics, size) = vertical_metrics(cx, style);
        match style.get_font().line_height {
            LineHeight::Normal => metrics.line_spacing(),
            LineHeight::Number(n) => (n.0 * size * 64.0).floor() / 64.0,
            LineHeight::Length(l) => (l.0.px() * 64.0).floor() / 64.0,
        }
    }

    /// The distance from the top of a line box to its baseline when the line
    /// holds only the strut of `style`; half-leading is rounded down as in
    /// Blink.
    pub(super) fn strut_baseline(cx: &LayoutCx, style: &ComputedValues) -> f32 {
        let (metrics, _) = vertical_metrics(cx, style);
        let used = strut_height(cx, style);
        let above = ((used - (metrics.ascent + metrics.descent)) / 2.0).floor();
        metrics.ascent + above
    }
}

#[cfg(feature = "embedded-fonts")]
pub(crate) use metrics::strut_height;
#[cfg(feature = "embedded-fonts")]
use metrics::{ascent_of, strut_baseline};

/// Without the bundled fonts nothing can be measured.
#[cfg(not(feature = "embedded-fonts"))]
fn ascent_of(_: &LayoutCx, _: &style::properties::ComputedValues) -> f32 {
    0.0
}

#[cfg(not(feature = "embedded-fonts"))]
pub(crate) fn strut_height(_: &LayoutCx, _: &style::properties::ComputedValues) -> f32 {
    0.0
}

#[cfg(not(feature = "embedded-fonts"))]
fn strut_baseline(_: &LayoutCx, _: &style::properties::ComputedValues) -> f32 {
    0.0
}
