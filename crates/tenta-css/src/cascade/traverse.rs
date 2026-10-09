//! The sequential Stylo traversal and the `DomTraversal` implementation.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use style::Atom;
use style::animation::DocumentAnimationSet;
use style::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
    StyleSystemOptions,
};
use style::dom::{TElement, TNode};
use style::driver::traverse_dom;
use style::selector_parser::SnapshotMap;
use style::shared_lock::{SharedRwLockReadGuard, StylesheetGuards};
use style::sharing::StyleSharingCache;
use style::stylist::Stylist;
use style::thread_state::{self, ThreadState};
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::traversal_flags::TraversalFlags;

use super::dom::{Handle, NodeHandle};

/// No paint worklets are ever registered.
pub(super) struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

/// Recomputes the style of every element it visits.
struct RecalcStyle<'a> {
    shared: SharedStyleContext<'a>,
}

impl<'a, 'd> DomTraversal<Handle<'d>> for RecalcStyle<'a> {
    fn process_preorder<F>(
        &self,
        data: &PerLevelTraversalData,
        context: &mut StyleContext<Handle<'d>>,
        node: NodeHandle<'d>,
        note_child: F,
    ) where
        F: FnMut(NodeHandle<'d>),
    {
        let Some(element) = node.as_element() else {
            return;
        };
        // SAFETY: the traversal is sequential and holds no other borrow of this
        // element's data, which is what `ensure_data` requires.
        let mut element_data = unsafe { element.ensure_data() };
        recalc_style_at(self, data, context, element, &mut element_data, note_child);
        // SAFETY: as above; nothing reads the dirty-descendants flag concurrently.
        unsafe { element.unset_dirty_descendants() };
    }

    fn process_postorder(&self, _: &mut StyleContext<Handle<'d>>, _: NodeHandle<'d>) {}

    // Stylo's bottom-up step counts finished children through
    // `TElement::did_process_child`, which the adapter does not implement.
    fn needs_postorder_traversal() -> bool {
        false
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.shared
    }
}

/// Holds `ThreadState::LAYOUT` for its lifetime, so the state is left again on
/// unwind. Stylo reads it in debug assertions only, but they fire without it.
pub(super) struct LayoutThreadState;

impl LayoutThreadState {
    pub(super) fn enter() -> Self {
        thread_state::enter(ThreadState::LAYOUT);
        Self
    }
}

impl Drop for LayoutThreadState {
    fn drop(&mut self) {
        thread_state::exit(ThreadState::LAYOUT);
    }
}

/// The context both the flush's traversal and on-demand resolution run in:
/// no animations, no snapshots, no visited styles, no paint worklets.
pub(super) fn shared_context<'a>(
    stylist: &'a Stylist,
    guard: &'a SharedRwLockReadGuard<'a>,
    snapshots: &'a SnapshotMap,
) -> SharedStyleContext<'a> {
    SharedStyleContext {
        stylist,
        visited_styles_enabled: false,
        options: StyleSystemOptions::default(),
        guards: StylesheetGuards::same(guard),
        current_time_for_animations: 0.0,
        traversal_flags: TraversalFlags::empty(),
        snapshot_map: snapshots,
        animations: DocumentAnimationSet::default(),
        registered_speculative_painters: &NoPainters,
    }
}

/// Computes the style of `root` and everything below it that is not under a
/// `display: none` element, on the calling thread.
///
/// Stylo's asserts are panics, so the traversal runs under `catch_unwind`. The
/// thread-local style-sharing cache is emptied before the panic is handed
/// back: the next traversal's constructor asserts it is empty, and it keeps
/// elements of this flush as untyped words.
///
/// `root` must have no element data, or Stylo treats its subtree as current.
pub(super) fn restyle(
    stylist: &Stylist,
    guard: &SharedRwLockReadGuard<'_>,
    root: Handle<'_>,
) -> Result<(), Box<dyn Any + Send>> {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _state = LayoutThreadState::enter();
        let snapshots = SnapshotMap::new();
        let shared = shared_context(stylist, guard, &snapshots);
        let traversal = RecalcStyle { shared };
        let token = RecalcStyle::pre_traverse(root, traversal.shared_context());
        if token.should_traverse() {
            // No thread pool: the whole tree is styled on this thread.
            traverse_dom(&traversal, token, None);
        }
    }));
    if outcome.is_err() {
        StyleSharingCache::<Handle<'static>>::new().clear();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_traversal_has_no_postorder_step() {
        assert!(
            !<RecalcStyle<'static> as DomTraversal<Handle<'static>>>::needs_postorder_traversal()
        );
    }

    #[test]
    fn the_layout_state_is_left_on_unwind() {
        let outcome = catch_unwind(|| {
            let _state = LayoutThreadState::enter();
            assert!(thread_state::get().contains(ThreadState::LAYOUT));
            panic!("unwind through the guard");
        });
        assert!(outcome.is_err());
        assert!(!thread_state::get().contains(ThreadState::LAYOUT));
    }
}
