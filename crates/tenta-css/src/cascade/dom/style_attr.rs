//! Parsed `style=""` blocks, keyed by node and attribute revision.

use servo_arc::ArcBorrow;
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::Locked;

use super::Slot;

/// The element's parsed `style` attribute, parsed with the engine's lock and
/// cached in the slot's store entry. `None` when the element has no `style`
/// attribute or it is empty.
pub(crate) fn style_attribute<'a>(
    slot: &'a Slot<'a>,
) -> Option<ArcBorrow<'a, Locked<PropertyDeclarationBlock>>> {
    let _ = slot;
    todo!("C1b")
}
