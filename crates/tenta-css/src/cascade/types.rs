//! Public value types of the cascade.

use std::ops::{BitOr, BitOrAssign};

use servo_arc::Arc;
use style::properties::ComputedValues;
use tenta_core::NodeId;
use url::Url;

/// An element's computed style, shared between elements that match the same
/// rules.
pub type Style = Arc<ComputedValues>;

/// The environment style is computed for. A change invalidates every style.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleEnv {
    /// Layout viewport in CSS pixels; viewport units and media queries read it.
    pub viewport: (f32, f32),
    /// Device pixel ratio (`resolution` media features).
    pub device_pixel_ratio: f32,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
    /// `prefers-reduced-motion: reduce`.
    pub reduced_motion: bool,
    /// Media type is `print` instead of `screen`.
    pub print: bool,
    /// The document is in quirks mode; selects the quirks user-agent rules.
    pub quirks: bool,
}

impl Default for StyleEnv {
    fn default() -> Self {
        Self {
            viewport: (800.0, 600.0),
            device_pixel_ratio: 1.0,
            dark: false,
            reduced_motion: false,
            print: false,
            quirks: false,
        }
    }
}

impl StyleEnv {
    /// True when `doc` is in full quirks mode, the value of
    /// [`StyleEnv::quirks`] for that document.
    pub fn quirks_of(doc: &tenta_dom::Document) -> bool {
        doc.quirks_mode() == tenta_html::QuirksMode::Quirks
    }
}

/// One author stylesheet: its text and where it came from. Produced by the page
/// from `<style>` and `<link>` elements, parsed by the engine.
#[derive(Clone, Debug)]
pub struct StyleSource {
    /// The `<style>` or `<link>` element; its position in the sources vector
    /// is the document order.
    pub owner: Option<NodeId>,
    /// Base URL for `url()` and `@import`.
    pub url: Url,
    /// Stylesheet text.
    pub text: String,
    /// `media` attribute of the owner.
    pub media: Option<String>,
    /// `disabled` state of the owner.
    pub disabled: bool,
}

/// A pseudo-element whose style the engine resolves.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PseudoKind {
    /// `::before`.
    Before,
    /// `::after`.
    After,
    /// `::marker`.
    Marker,
    /// `::first-line`.
    FirstLine,
    /// `::first-letter`.
    FirstLetter,
}

/// What a style change requires downstream.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct ChangeHint(u8);

impl ChangeHint {
    /// Nothing to redo.
    pub const NONE: ChangeHint = ChangeHint(0);
    /// Paint again.
    pub const REPAINT: ChangeHint = ChangeHint(1);
    /// Lay out again.
    pub const RELAYOUT: ChangeHint = ChangeHint(2);
    /// Rebuild the element's boxes.
    pub const RECONSTRUCT_BOX: ChangeHint = ChangeHint(4);

    /// True when every bit of `other` is set.
    pub fn contains(self, other: ChangeHint) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when no bit is set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for ChangeHint {
    type Output = ChangeHint;

    fn bitor(self, rhs: ChangeHint) -> ChangeHint {
        ChangeHint(self.0 | rhs.0)
    }
}

impl BitOrAssign for ChangeHint {
    fn bitor_assign(&mut self, rhs: ChangeHint) {
        self.0 |= rhs.0;
    }
}

/// The result of a flush.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleDiff {
    /// Nothing changed since the previous flush.
    None,
    /// Every style may have changed. M1 reports this for any change.
    Full,
    /// Only these elements changed. Declared for the incremental path; the
    /// engine does not produce it until Stylo snapshots and restyle hints are
    /// wired through the style store.
    Partial(Vec<(NodeId, ChangeHint)>),
}

/// Measurements of one font at one size, in CSS pixels: what Stylo's own
/// `FontMetrics` carries for `ex`, `ch`, `cap`, `ic`, `font-size-adjust` and
/// `math-depth`. `None` makes Stylo use its fallback ratio.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FontMetricsData {
    /// x-height.
    pub x_height: Option<f32>,
    /// Advance of the digit zero (`ch`).
    pub zero_advance: Option<f32>,
    /// Cap height.
    pub cap_height: Option<f32>,
    /// Advance of the water ideograph (`ic`).
    pub ic_width: Option<f32>,
    /// Ascent; always available.
    pub ascent: f32,
    /// Scale factor for `math-depth: 1`.
    pub script_percent_scale_down: Option<f32>,
    /// Scale factor for `math-depth: 2`.
    pub script_script_percent_scale_down: Option<f32>,
}

/// The font measurements the style system queries. Implemented by the layout
/// crate's font system and handed to Stylo's `FontMetricsProvider`.
pub trait FontMetrics: Send + Sync {
    /// Measurements of the face `family_list`, `weight` and `italic` resolve
    /// to, at `size_px`.
    fn metrics(
        &self,
        family_list: &str,
        weight: u16,
        italic: bool,
        size_px: f32,
    ) -> FontMetricsData;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_hints_combine_like_flags() {
        let mut hint = ChangeHint::REPAINT | ChangeHint::RELAYOUT;
        assert!(hint.contains(ChangeHint::REPAINT));
        assert!(!hint.contains(ChangeHint::RECONSTRUCT_BOX));
        hint |= ChangeHint::RECONSTRUCT_BOX;
        assert!(hint.contains(ChangeHint::RELAYOUT | ChangeHint::RECONSTRUCT_BOX));
        assert!(ChangeHint::NONE.is_empty());
        assert!(!hint.is_empty());
    }

    #[test]
    fn default_env_is_the_screen_light_viewport() {
        let env = StyleEnv::default();
        assert_eq!(env.viewport, (800.0, 600.0));
        assert!(!env.dark && !env.print && !env.quirks);
    }
}
