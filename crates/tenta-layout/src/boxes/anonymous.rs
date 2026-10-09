//! Computed styles for boxes that have no element of their own.

use style::properties::style_structs::Font;
use style::properties::{ComputedValues, ComputedValuesInner};
use style::values::specified::box_::Display;
use tenta_css::cascade::Style;

/// Builds inherited-only styles for anonymous boxes: every inherited
/// property comes from the parent, every other one is the CSS initial value,
/// so a wrapper never repeats its parent's padding, border or background.
pub(super) struct AnonymousStyles {
    initial: Style,
    block: servo_arc::Arc<style::properties::style_structs::Box>,
}

impl AnonymousStyles {
    pub(super) fn new() -> Self {
        let initial = ComputedValues::initial_values_with_font_override(Font::initial_values());
        let mut block = (*initial.clone_box()).clone();
        block.set_display(Display::Block);
        Self {
            initial,
            block: servo_arc::Arc::new(block),
        }
    }

    /// The style every element-less document falls back to.
    pub(super) fn initial(&self) -> Style {
        self.initial.clone()
    }

    /// An anonymous `display: block` (or `inline` when `block` is false)
    /// style inheriting from `parent`.
    pub(super) fn derive(&self, parent: &ComputedValues, block: bool) -> Style {
        let initial: &ComputedValuesInner = &self.initial;
        let parent_inner: &ComputedValuesInner = parent;
        let box_ = if block {
            self.block.clone()
        } else {
            initial.clone_box()
        };
        ComputedValues::new(
            None,
            parent.custom_properties().clone(),
            Default::default(),
            parent.writing_mode,
            parent.effective_zoom,
            parent.flags.inherited(),
            None,
            None,
            initial.clone_background(),
            initial.clone_border(),
            box_,
            initial.clone_column(),
            initial.clone_counters(),
            initial.clone_effects(),
            parent_inner.clone_font(),
            parent_inner.clone_inherited_box(),
            parent_inner.clone_inherited_svg(),
            parent_inner.clone_inherited_table(),
            parent_inner.clone_inherited_text(),
            parent_inner.clone_inherited_ui(),
            parent_inner.clone_list(),
            initial.clone_margin(),
            initial.clone_outline(),
            initial.clone_padding(),
            initial.clone_position(),
            initial.clone_svg(),
            initial.clone_table(),
            initial.clone_text(),
            initial.clone_ui(),
        )
    }
}
