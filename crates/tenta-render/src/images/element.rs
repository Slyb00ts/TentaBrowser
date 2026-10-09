//! What an image element asks for and what the session knows about it, with no
//! layout involved: the page loads images when they enter the document, long
//! before anything is rendered (`docs/dev/p3-plan.md`, IM4).

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_layout::ImageKey;
use url::Url;

use super::{ImageTable, MediaEnv, image_source, is_image_element, is_lazy, key_of, scaled};
use crate::RenderSession;

/// The image an element selected, from its attributes alone.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementImage {
    /// Absolute URL to fetch.
    pub url: Url,
    /// The key the session stores the image under.
    pub key: ImageKey,
    /// Image pixels per CSS pixel of the chosen candidate.
    pub density: f32,
    /// `loading=lazy`.
    pub lazy: bool,
    /// Whether the element fires `load` and `error`: `<img>` and
    /// `<input type=image>` do, a `<video poster>` does not.
    pub fires_events: bool,
}

/// What the session knows about one image key.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageStatus {
    /// Neither loaded nor failed.
    Unknown,
    /// Decoded; the density-corrected natural size in CSS pixels, rounded.
    Loaded {
        /// Natural width.
        width: u32,
        /// Natural height.
        height: u32,
    },
    /// Could not be loaded or decoded.
    Failed,
}

impl ImageTable {
    fn status(&self, key: &ImageKey, density: f32) -> ImageStatus {
        if let Some(loaded) = self.loaded.get(key) {
            let natural = scaled(loaded.natural, density);
            let round = |side: Option<f32>| side.map_or(0, |v| v.max(0.0).round() as u32);
            return ImageStatus::Loaded {
                width: round(natural.width),
                height: round(natural.height),
            };
        }
        if self.failed.contains_key(key) {
            return ImageStatus::Failed;
        }
        ImageStatus::Unknown
    }
}

impl RenderSession {
    /// The image `node` selects for this session's viewport, or `None` when
    /// it is not an image element or has no usable source. `base` resolves
    /// relative URLs.
    pub fn element_image(&self, doc: &Document, node: NodeId, base: &Url) -> Option<ElementImage> {
        if !is_image_element(doc, node) {
            return None;
        }
        let viewport = self.config.viewport;
        let env = MediaEnv {
            width: viewport.width as f32,
            height: viewport.height as f32,
            dpr: viewport.device_pixel_ratio,
            dark: self.config.dark,
        };
        let source = image_source(doc, node, base, &env)?;
        Some(ElementImage {
            key: key_of(&source.url),
            url: source.url,
            density: source.density,
            lazy: is_lazy(doc, node),
            fires_events: doc.tag(node) != Some(tenta_core::atom::atoms::VIDEO),
        })
    }

    /// Whether `key` is loaded or failed, and how big a loaded image is at
    /// `density` image pixels per CSS pixel.
    pub fn image_status(&self, key: &ImageKey, density: f32) -> ImageStatus {
        self.images.status(key, density)
    }
}
