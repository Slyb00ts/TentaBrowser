//! Inline formatting context layout: line boxes, text and inline boxes.

// Everything below shapes text, which needs the bundled fonts.
#[cfg(feature = "embedded-fonts")]
mod align;
pub(crate) mod atomic;
#[cfg(feature = "embedded-fonts")]
mod atoms;
#[cfg(feature = "embedded-fonts")]
mod bidi;
#[cfg(feature = "embedded-fonts")]
mod ellipsis;
#[cfg(feature = "embedded-fonts")]
mod lines;
#[cfg(feature = "embedded-fonts")]
mod style;
#[cfg(feature = "embedded-fonts")]
mod valign;
#[cfg(feature = "embedded-fonts")]
mod whitespace;

#[cfg(all(test, feature = "embedded-fonts"))]
mod tests;

use crate::box_tree::{BoxTree, InlineFormattingContext, LayoutCx};
use crate::fragment::FragmentBuilder;
use crate::space::{ConstraintSpace, FloatContext, InlineResult};

/// Lays out `ifc` into line fragments within `space`, avoiding the floats in
/// `floats`.
///
/// Font metrics (`line-height: normal`, the strut and the quirks-mode strut
/// rule through `cx.quirks`) are read through `cx.fonts`, never from the text
/// engine directly. Without the bundled fonts no text can be shaped and the
/// context has no lines.
pub fn layout_inline(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
    space: &ConstraintSpace,
    floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
) -> InlineResult {
    #[cfg(feature = "embedded-fonts")]
    {
        lines::build_lines(cx, tree, ifc, space, floats, out)
    }
    #[cfg(not(feature = "embedded-fonts"))]
    {
        let _ = (cx, tree, ifc, space, floats, out);
        InlineResult {
            lines: Default::default(),
            block_size: 0.0,
            first_baseline: None,
            last_baseline: None,
            overflow: Default::default(),
            inline_extent: 0.0,
        }
    }
}

/// The `(min-content, max-content)` widths of the inline content in `ifc`.
///
/// Min-content is the widest piece between wrap opportunities, max-content the
/// widest line when only forced breaks end lines. Percentage paddings and
/// margins count as zero.
pub fn intrinsic_inline_content_sizes(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
) -> (f32, f32) {
    #[cfg(feature = "embedded-fonts")]
    {
        lines::intrinsic_sizes(cx, tree, ifc)
    }
    #[cfg(not(feature = "embedded-fonts"))]
    {
        let _ = (cx, tree, ifc);
        (0.0, 0.0)
    }
}
