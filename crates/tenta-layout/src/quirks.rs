//! Quirks-mode layout rules for block flow.
//!
//! Two rules live here: `html` and `body` with an `auto` height fill the
//! viewport, and a percentage height whose containing block is `auto` tall
//! resolves against the nearest ancestor that has a height.
//!
//! The margin-collapsing quirk for the first and last child of `body` and
//! table cells is not implemented: it discards only margins that came from the
//! user-agent sheet, and computed values do not record where a margin came from.

use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::values::computed::Size;

use crate::block_sizes::frame_of;
use crate::box_tree::{BoxFlags, BoxId, BoxKind, BoxTree, LayoutCx};
use crate::space::ConstraintSpace;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Role {
    Root,
    Body,
}

/// Whether `id` is the `html` or `body` box that fills the viewport: quirks
/// mode, in flow, a block container, and a height of `auto`.
fn filling_role(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> Option<Role> {
    if !cx.quirks {
        return None;
    }
    let b = tree.get(id);
    let block = matches!(b.kind, BoxKind::Block | BoxKind::FlowRoot);
    if !block
        || b.flags.contains(BoxFlags::OUT_OF_FLOW)
        || !matches!(b.style.get_position().height, Size::Auto)
    {
        return None;
    }
    let root = cx.doc.document_element();
    if b.node.is_some() && Some(b.node) == root {
        Some(Role::Root)
    } else if tree.canvas_source(cx.doc) == Some(id) {
        Some(Role::Body)
    } else {
        None
    }
}

/// The content-box height of an `html` or `body` box that fills the viewport.
/// `html` takes the viewport height less its own margins; `body` takes the
/// content height of `html` less its own margins. Padding and border sit
/// inside that border-box height.
fn fill_content_height(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> Option<f32> {
    let role = filling_role(cx, tree, id)?;
    let (width, height) = cx.viewport;
    let (available, basis) = match role {
        Role::Root => (height, width),
        Role::Body => {
            let root = tree.get(id).parent;
            let root_frame = frame_of(tree, root, width);
            let available = definite_content_height(cx, tree, root).unwrap_or(height);
            let basis = width
                - root_frame.margin.horizontal()
                - root_frame.padding.horizontal()
                - root_frame.border.horizontal();
            (available, basis.max(0.0))
        }
    };
    let frame = frame_of(tree, id, basis);
    let edges = frame.margin.vertical() + frame.padding.vertical() + frame.border.vertical();
    Some((available - edges).max(0.0))
}

/// The content-box height of `id` when it does not depend on its content: the
/// viewport fill, a length, or a percentage of a definite height above it.
/// Percentage padding resolves against the viewport width here, which is exact
/// only for children of `body`.
fn definite_content_height(cx: &LayoutCx, tree: &BoxTree, id: BoxId) -> Option<f32> {
    if let Some(height) = fill_content_height(cx, tree, id) {
        return Some(height);
    }
    let b = tree.get(id);
    let Size::LengthPercentage(lp) = &b.style.get_position().height else {
        return None;
    };
    let specified = if lp.0.has_percentage() {
        let base = height_through_auto_ancestors(cx, tree, b.parent)?;
        lp.0.resolve(style::values::computed::Length::new(base))
            .px()
    } else {
        lp.0.to_length()?.px()
    };
    let frame = frame_of(tree, id, cx.viewport.0);
    Some(match b.style.get_position().box_sizing {
        BoxSizing::BorderBox => {
            (specified - frame.padding.vertical() - frame.border.vertical()).max(0.0)
        }
        BoxSizing::ContentBox => specified,
    })
}

/// The percentage base the quirk finds: the first box at or above `start`
/// that has a definite height, skipping boxes whose height is `auto`.
fn height_through_auto_ancestors(cx: &LayoutCx, tree: &BoxTree, start: BoxId) -> Option<f32> {
    let mut current = start;
    while current != BoxId::NONE {
        if let Some(height) = definite_content_height(cx, tree, current) {
            return Some(height);
        }
        if !matches!(tree.get(current).style.get_position().height, Size::Auto) {
            return None;
        }
        current = tree.get(current).parent;
    }
    None
}

/// The height percentages of box `id`'s `height` resolve against (and of its
/// limits, for boxes that are not replaced): the containing block's definite
/// height, or in quirks mode that of the nearest ancestor with one.
pub(crate) fn percentage_height_base(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
) -> Option<f32> {
    space.percentage_base.1.or_else(|| {
        if cx.quirks {
            height_through_auto_ancestors(cx, tree, tree.get(id).parent)
        } else {
            None
        }
    })
}

/// The height the percentages of `min-height` and `max-height` of box `id`
/// resolve against. Same as [`percentage_height_base`], except that a replaced
/// box only sees a definite height of its own parent: Chrome does not pass its
/// limits through auto-height ancestors.
pub(crate) fn percentage_limit_base(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    space: &ConstraintSpace,
) -> Option<f32> {
    let b = tree.get(id);
    if !cx.quirks || !matches!(b.kind, BoxKind::Replaced(_)) {
        return percentage_height_base(cx, tree, id, space);
    }
    space.percentage_base.1.or_else(|| {
        (b.parent != BoxId::NONE)
            .then(|| definite_content_height(cx, tree, b.parent))
            .flatten()
    })
}

/// Applies the quirks-mode rule that `html` and `body` fill the viewport
/// height. Returns `content_height` unchanged outside quirks mode and for
/// other boxes. Content taller than the viewport keeps its height.
pub(crate) fn fill_viewport_height(
    cx: &LayoutCx,
    tree: &BoxTree,
    id: BoxId,
    content_height: f32,
) -> f32 {
    fill_content_height(cx, tree, id).map_or(content_height, |fill| content_height.max(fill))
}
