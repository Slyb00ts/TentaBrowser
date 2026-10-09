//! The display-list builder: fragment tree to [`DisplayList`].

mod background;
mod border;
mod clip;
mod effects;
mod marker;
mod replaced;
mod shadow;
mod stacking;
mod text;

use tenta_core::NodeId;
use tenta_layout::{FragmentTree, ImageKey, IntrinsicSize};

use crate::dl::DisplayList;
use crate::image::{ImageId, ImageStore};

/// An image the builder can draw: its handle in the store and its natural size
/// in CSS pixels (after the density of its `srcset` candidate).
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DrawableImage {
    /// Handle in the [`ImageStore`].
    pub id: ImageId,
    /// Natural width, height and ratio; the ratio alone for an image without a
    /// size.
    pub natural: IntrinsicSize,
}

/// Which stored image a node or a CSS URL shows. The render session implements
/// it over the images the page has loaded.
pub trait ImageBindings {
    /// The image an `<img>`-like node shows, once loaded.
    fn node_image(&self, node: NodeId) -> Option<DrawableImage>;
    /// The image behind a CSS `url()`, once loaded.
    fn url_image(&self, key: &ImageKey) -> Option<DrawableImage>;
    /// Whether the node's image failed to load or decode, so its empty box is
    /// drawn as a broken image.
    fn node_broken(&self, _node: NodeId) -> bool {
        false
    }
}

/// Bindings that know no image.
#[derive(Copy, Clone, Default, Debug)]
pub struct NoBindings;

impl ImageBindings for NoBindings {
    fn node_image(&self, _node: NodeId) -> Option<DrawableImage> {
        None
    }

    fn url_image(&self, _key: &ImageKey) -> Option<DrawableImage> {
        None
    }
}

/// What the builder reads besides the fragments.
#[derive(Copy, Clone)]
pub struct PaintContext<'a> {
    /// Decoded images, for `Item::Image`.
    pub images: &'a ImageStore,
    /// Which stored image each node and URL shows.
    pub bindings: &'a dyn ImageBindings,
}

impl<'a> PaintContext<'a> {
    /// A context for a page without images.
    pub fn new(images: &'a ImageStore) -> Self {
        Self {
            images,
            bindings: &NoBindings,
        }
    }
}

/// Builds the display list for `frags` in CSS 2.1 Appendix E painting order.
///
/// Every `Push*` item in the result is closed by its `Pop*`.
pub fn build(frags: &FragmentTree, cx: &PaintContext) -> DisplayList {
    let (unsupported, approximated) = effects::count_unsupported(frags);
    DisplayList {
        items: stacking::paint(frags, cx),
        bounds: frags.scroll_extent,
        unsupported,
        approximated,
    }
}

#[cfg(test)]
mod tests;
