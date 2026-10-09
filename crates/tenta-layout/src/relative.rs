//! `position: relative` offsets.

use style::values::computed::{Inset, Length};

use crate::box_tree::{BoxId, BoxTree};
use crate::constraint::own_direction;
use crate::types::Direction;

/// An inset in CSS pixels, or `None` for `auto` (and for anchor functions,
/// which have no meaning without anchor positioning).
pub(crate) fn resolve(inset: &Inset, basis: Option<f32>) -> Option<f32> {
    match inset {
        Inset::LengthPercentage(lp) if lp.has_percentage() => {
            basis.map(|b| lp.resolve(Length::new(b)).px())
        }
        Inset::LengthPercentage(lp) => Some(lp.resolve(Length::new(0.0)).px()),
        _ => None,
    }
}

/// The `(dx, dy)` offset `position: relative` applies to box `id`, resolving
/// percentages against `containing`, the containing block's content width and
/// its height when that is definite (a percentage of an indefinite height is
/// `auto`). `(0, 0)` for boxes that are not relatively positioned.
///
/// When both insets of an axis are set the over-constrained one loses: `right`
/// and `bottom` give way to `left` and `top`, except that `direction: rtl`
/// makes `left` give way to `right` (CSS Positioned Layout 3, 3.2.1). The
/// direction is the containing block's, which is the parent box's.
pub(crate) fn relative_offset_in(
    tree: &BoxTree,
    id: BoxId,
    containing: (f32, Option<f32>),
) -> (f32, f32) {
    let b = tree.get(id);
    if b.paint.position != crate::types::PositionKind::Relative {
        return (0.0, 0.0);
    }
    let pos = b.style.get_position();
    let left = resolve(&pos.left, Some(containing.0));
    let right = resolve(&pos.right, Some(containing.0));
    let top = resolve(&pos.top, containing.1);
    let bottom = resolve(&pos.bottom, containing.1);
    let direction = match b.parent {
        BoxId::NONE => own_direction(&b.style),
        parent => own_direction(&tree.get(parent).style),
    };
    let dx = match (left, right, direction) {
        (Some(l), Some(_), Direction::Ltr) => l,
        (Some(_), Some(r), Direction::Rtl) => -r,
        (Some(l), None, _) => l,
        (None, Some(r), _) => -r,
        (None, None, _) => 0.0,
    };
    let dy = match (top, bottom) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        (None, None) => 0.0,
    };
    (dx, dy)
}
