//! Presentational hints synthesized from legacy HTML attributes.

use selectors::matching::VisitedHandlingMode;
use selectors::sink::Push;
use style::applicable_declarations::ApplicableDeclarationBlock;

use super::Slot;

/// Pushes the declarations that `bgcolor`, `align`, `width` and the other legacy
/// attributes of the element in `slot` stand for.
pub(crate) fn synthesize<V>(slot: &Slot<'_>, visited: VisitedHandlingMode, hints: &mut V)
where
    V: Push<ApplicableDeclarationBlock>,
{
    let _ = (slot, visited, hints);
    todo!("C1b")
}
