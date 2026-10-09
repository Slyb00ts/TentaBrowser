//! Inline formatting context layout: line boxes, text and inline boxes.

mod align;
mod atomic;
mod bidi;
mod ellipsis;
mod lines;
mod valign;
mod whitespace;

use crate::box_tree::{BoxTree, InlineFormattingContext, LayoutCx};
use crate::fragment::FragmentBuilder;
use crate::space::{ConstraintSpace, FloatContext, InlineResult};

/// Lays out `ifc` into line fragments within `space`, avoiding the floats in
/// `floats`.
///
/// Font metrics (`line-height: normal`, the strut and the quirks-mode strut
/// rule through `cx.quirks`) are read through `cx.fonts`, never from the text
/// engine directly.
pub fn layout_inline(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
    space: &ConstraintSpace,
    floats: &mut dyn FloatContext,
    out: &mut FragmentBuilder,
) -> InlineResult {
    lines::build_lines(cx, tree, ifc, space, floats, out)
}

/// The `(min-content, max-content)` widths of the inline content in `ifc`.
pub fn intrinsic_inline_content_sizes(
    cx: &mut LayoutCx,
    tree: &BoxTree,
    ifc: &InlineFormattingContext,
) -> (f32, f32) {
    let _ = (cx, tree, ifc);
    todo!("L3a")
}
