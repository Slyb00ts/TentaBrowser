//! Images in a render session: which ones a document asks for, what has been
//! loaded, and the answers layout and paint need about them.
//!
//! The session never fetches. [`RenderSession::plan_images`] lists what the
//! document wants (every `<img>` source, `srcset`/`sizes`/`<picture>` choice
//! included, and every `background-image` URL of a laid-out box), the page
//! fetches the bytes through its own client, [`decode_image`] turns them into
//! pixels off the page thread, and `install_image` or `fail_image` hands the
//! outcome back. An image counts as loaded or failed once; installing or
//! failing one invalidates the box tree, because a broken `<img>` with `alt`
//! text is laid out as text.
//!
//! [`RenderSession::plan_images`]: crate::RenderSession::plan_images

mod data_url;
mod element;
mod media;
mod select;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::Document;
use tenta_layout::{
    FragId, FragmentTree, ImageInfo as LayoutImages, ImageKey, ImageState, IntrinsicSize, Rect,
};
use tenta_paint::{
    DecodeControl, DecodeError, DecodeLimits, DecodeParams, DecodedImage, DrawableImage,
    ImageBindings, ImageStore, StoreError, StoreKey, SvgNatural, decode_svg, decode_with_control,
};
use url::Url;

pub use data_url::{DataUrl, decode_data_url};
pub use element::{ElementImage, ImageStatus};
pub(crate) use media::MediaEnv;

/// How far outside the viewport a `loading=lazy` image or a background still
/// loads, in CSS pixels: Chromium's distance for a fast connection.
const LAZY_MARGIN: f32 = 1250.0;

/// Longest side, in device pixels, a decoded raster is kept at.
const MAX_STORED_HEIGHT: u32 = 16_384;

/// Which part of the page needs its images.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LazyRegion {
    /// The viewport and a margin around it: `loading=lazy` images and
    /// backgrounds farther away are left alone.
    Viewport,
    /// The whole document, for a full-page capture.
    All,
}

/// What a want is for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WantOrigin {
    /// An `<img>` (or `<input type=image>`) element.
    Element,
    /// A CSS `url()`.
    Style,
}

/// An image the document needs and the session does not have.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageWant {
    /// Absolute URL to fetch (`http`, `https`, `data` or `file`).
    pub url: Url,
    /// The key to install or fail it under.
    pub key: ImageKey,
    /// What asked for it.
    pub origin: WantOrigin,
    /// `Some` asks for an image already fetched to be decoded again from the
    /// retained body, at the size the laid-out boxes need (see [`Rerender`]).
    /// A body that was not retained, or a decode that fails, leaves the first
    /// decode in place.
    pub rerender: Option<Rerender>,
}

/// How to decode a retained body again.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Rerender {
    /// Render an SVG at this many bitmap pixels per CSS pixel of its natural
    /// size ([`DecodeSettings::device_scale`]).
    Svg {
        /// Bitmap pixels per CSS pixel.
        scale: f32,
    },
    /// Decode a raster shrunk to at most this many pixels wide
    /// ([`DecodeSettings::max_size`]); a raster is never enlarged past its
    /// source.
    Raster {
        /// Widest bitmap, in device pixels.
        max_width: u32,
    },
}

/// How a fetched body becomes pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DecodeSettings {
    /// Device pixels per CSS pixel; an SVG is rendered at this scale.
    pub device_scale: f32,
    /// Largest stored size in device pixels. A raster wider than this is
    /// shrunk while it decodes; its natural size is unchanged.
    pub max_size: (u32, u32),
    /// Pixel and memory bounds of one decode.
    pub limits: DecodeLimits,
}

impl DecodeSettings {
    fn params(&self) -> DecodeParams {
        DecodeParams {
            max_size: Some(self.max_size),
        }
    }
}

/// The images a document needs, in the order to fetch them.
#[derive(Clone, Debug, PartialEq)]
pub struct ImagePlan {
    /// Wanted images not yet loaded or failed, `<img>` elements first in
    /// document order, then CSS images in paint order, each URL once.
    pub wants: Vec<ImageWant>,
    /// How to decode what arrives.
    pub decode: DecodeSettings,
}

/// What kind of image a body is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// PNG, JPEG, WebP, GIF, BMP or ICO, found by content.
    Raster,
    /// SVG, which a browser takes on its declared type only.
    Svg,
}

impl ImageKind {
    /// SVG when the declared `Content-Type` (or `data:` media type) says so;
    /// everything else is sniffed.
    pub fn of(content_type: Option<&str>) -> Self {
        let essence = content_type
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase());
        if essence.as_deref() == Some("image/svg+xml") {
            Self::Svg
        } else {
            Self::Raster
        }
    }
}

/// A decoded image and its natural size.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    /// Premultiplied pixels.
    pub image: DecodedImage,
    /// The size the image reports, in its own pixels for a raster and in CSS
    /// pixels for an SVG; a ratio alone when the SVG gives no size.
    pub natural: IntrinsicSize,
    /// An SVG, rendered at the settings' scale rather than decoded: it can be
    /// rendered again, larger, when a box turns out to show it bigger.
    pub vector: bool,
    /// For an SVG, the bitmap pixels per CSS pixel it was rendered at.
    pub scale: f32,
}

/// Decodes `bytes` under `settings`, stopping early when `control` says so.
/// Pure and `Send`: the page runs it on a blocking thread.
pub fn decode_image(
    bytes: &[u8],
    kind: ImageKind,
    settings: &DecodeSettings,
    control: &DecodeControl,
) -> Result<Decoded, DecodeError> {
    match kind {
        ImageKind::Raster => {
            let image = decode_with_control(bytes, &settings.params(), &settings.limits, control)?;
            let (w, h) = (image.source_width as f32, image.source_height as f32);
            Ok(Decoded {
                natural: IntrinsicSize {
                    width: Some(w),
                    height: Some(h),
                    ratio: (h > 0.0).then(|| w / h),
                },
                image,
                vector: false,
                scale: 1.0,
            })
        }
        ImageKind::Svg => {
            let (image, natural) =
                decode_svg(bytes, settings.device_scale, &settings.limits, control)?;
            Ok(Decoded {
                natural: svg_natural(natural),
                image,
                vector: true,
                scale: settings.device_scale,
            })
        }
    }
}

fn svg_natural(natural: SvgNatural) -> IntrinsicSize {
    IntrinsicSize {
        width: natural.width,
        height: natural.height,
        ratio: natural.ratio,
    }
}

/// What the session knows about one `<img>`-like element.
#[derive(Clone, Debug, PartialEq)]
struct NodeImage {
    /// The chosen source; `None` for an element with no usable one.
    key: Option<ImageKey>,
    url: Option<Url>,
    /// Image pixels per CSS pixel of the chosen candidate.
    density: f32,
    lazy: bool,
    /// The ratio of the `width` and `height` attributes, which a missing or
    /// broken image keeps (the attributes map to `aspect-ratio: auto w / h`).
    attr_ratio: Option<f32>,
}

#[derive(Copy, Clone, Debug)]
struct Loaded {
    id: tenta_paint::ImageId,
    natural: IntrinsicSize,
    /// For an SVG: the bitmap's size in CSS pixels of the document, which is
    /// its size in device pixels over the scale it was rendered at.
    vector: Option<(f32, f32)>,
    /// The bitmap's size in device pixels.
    bitmap: (u32, u32),
    /// Times a new decode has been asked for; an image is asked for at most
    /// twice, so a size that cannot be met does not loop.
    asks: u8,
}

/// Most decodes asked for again per image.
const MAX_ASKS: u8 = 2;

/// A raster is decoded again smaller when its bitmap is this much wider than
/// the box, and larger when the box is this much wider than the bitmap.
const SHRINK_FACTOR: f32 = 1.5;
const GROW_FACTOR: f32 = 1.25;

/// Bitmaps under this many bytes are not worth decoding again smaller.
const MIN_SHRINK_BYTES: u64 = 256 * 1024;

/// An SVG is rendered again only when the box is this much wider than the
/// bitmap, and never beyond this many device pixels a side.
const RERENDER_FACTOR: f32 = 1.15;
const MAX_RERENDER_SIDE: f32 = 4096.0;

/// Counts for tests and diagnostics.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageStats {
    /// Images installed.
    pub loaded: usize,
    /// Images that failed.
    pub failed: usize,
    /// Bytes the store holds.
    pub store_bytes: usize,
    /// Loaded images whose pixels the store has since evicted. They keep
    /// their box and paint nothing.
    pub evicted: usize,
}

/// The images of a session: the store and what points into it.
#[derive(Default)]
pub(crate) struct ImageTable {
    pub(crate) store: ImageStore,
    nodes: HashMap<NodeId, NodeImage>,
    /// Elements in document order, for planning.
    order: Vec<NodeId>,
    loaded: HashMap<ImageKey, Loaded>,
    failed: HashMap<ImageKey, String>,
    /// Bumped whenever an answer layout or paint gives could change.
    pub(crate) epoch: u64,
}

impl ImageTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Reads the source of every image element of `doc`. Returns whether the
    /// answers changed.
    pub(crate) fn refresh_nodes(&mut self, doc: &Document, base: &Url, env: &MediaEnv) {
        let mut nodes = HashMap::new();
        let mut order = Vec::new();
        for node in doc.descendants(doc.document_node()) {
            if !is_image_element(doc, node) {
                continue;
            }
            let source = image_source(doc, node, base, env);
            let lazy = is_lazy(doc, node);
            let dimension = |name| {
                doc.attribute(node, name)
                    .and_then(parse_dimension)
                    .filter(|v| *v > 0.0)
            };
            let attr_ratio = dimension(atoms::WIDTH)
                .zip(dimension(atoms::HEIGHT))
                .map(|(w, h)| w / h);
            let key = source.as_ref().map(|s| key_of(&s.url));
            nodes.insert(
                node,
                NodeImage {
                    key,
                    url: source.as_ref().map(|s| s.url.clone()),
                    density: source.map_or(1.0, |s| s.density),
                    lazy,
                    attr_ratio,
                },
            );
            order.push(node);
        }
        if nodes != self.nodes {
            self.epoch += 1;
        }
        self.nodes = nodes;
        self.order = order;
    }

    /// Wanted images that are neither loaded nor failed.
    pub(crate) fn wants(
        &mut self,
        frags: Option<&FragmentTree>,
        region: LazyRegion,
        viewport: (f32, f32),
        dpr: f32,
    ) -> Vec<ImageWant> {
        let near = |rect: Rect| match region {
            LazyRegion::All => true,
            LazyRegion::Viewport => {
                rect.right() > -LAZY_MARGIN
                    && rect.x < viewport.0 + LAZY_MARGIN
                    && rect.bottom() > -LAZY_MARGIN
                    && rect.y < viewport.1 + LAZY_MARGIN
            }
        };
        let mut seen = std::collections::HashSet::new();
        let mut wants = Vec::new();
        for node in &self.order {
            let image = &self.nodes[node];
            let (Some(key), Some(url)) = (&image.key, &image.url) else {
                continue;
            };
            if self.loaded.contains_key(key) || self.failed.contains_key(key) {
                continue;
            }
            if image.lazy
                && let Some(frags) = frags
                && let Some(rect) = frags
                    .fragments_of(*node)
                    .iter()
                    .map(|&f| frags.absolute_border_box(f))
                    .reduce(|a, b| a.union(&b))
                && !near(rect)
            {
                continue;
            }
            if seen.insert(key.clone()) {
                wants.push(ImageWant {
                    url: url.clone(),
                    key: key.clone(),
                    origin: WantOrigin::Element,
                    rerender: None,
                });
            }
        }
        let Some(frags) = frags else {
            return wants;
        };
        self.rerenders(frags, dpr, &mut wants);
        for index in 0..frags.len() {
            let id = FragId(index as u32);
            let frag = frags.get(id);
            for layer in &frag.paint.background {
                let tenta_layout::BackgroundContent::Image(key) = &layer.content else {
                    continue;
                };
                if self.loaded.contains_key(key)
                    || self.failed.contains_key(key)
                    || !near(frags.absolute_border_box(id))
                    || !seen.insert(key.clone())
                {
                    continue;
                }
                let Ok(url) = Url::parse(&key.0) else {
                    continue;
                };
                wants.push(ImageWant {
                    url,
                    key: key.clone(),
                    origin: WantOrigin::Style,
                    rerender: None,
                });
            }
        }
        wants
    }

    /// Wants for images whose bitmap is the wrong size for the boxes that show
    /// it: an SVG narrower than its box, a raster much larger than it (so
    /// the pixels fit the budget) or much smaller than it (after a larger
    /// viewport or device ratio).
    fn rerenders(&mut self, frags: &FragmentTree, dpr: f32, wants: &mut Vec<ImageWant>) {
        // The widest content box each loaded image is shown in.
        let mut widest: Vec<(ImageKey, Url, f32)> = Vec::new();
        for node in &self.order {
            let image = &self.nodes[node];
            let (Some(key), Some(url)) = (&image.key, &image.url) else {
                continue;
            };
            let width = frags
                .fragments_of(*node)
                .iter()
                .map(|&f| {
                    let frag = frags.get(f);
                    (frag.rect.w - frag.border.horizontal() - frag.padding.horizontal()).max(0.0)
                })
                .fold(0.0_f32, f32::max);
            match widest.iter_mut().find(|(k, ..)| k == key) {
                Some(entry) => entry.2 = entry.2.max(width),
                None => widest.push((key.clone(), url.clone(), width)),
            }
        }
        for (key, url, width) in widest {
            let Some(loaded) = self.loaded.get_mut(&key) else {
                continue;
            };
            let need = (width * dpr).ceil();
            if loaded.asks >= MAX_ASKS || need < 1.0 {
                continue;
            }
            let have = loaded.bitmap.0 as f32;
            let rerender = if let Some((base_w, _)) = loaded.vector {
                (base_w > 0.0 && need > have * RERENDER_FACTOR && need <= MAX_RERENDER_SIDE).then(
                    || Rerender::Svg {
                        scale: need / base_w,
                    },
                )
            } else {
                let source = loaded.natural.width.unwrap_or(have);
                let target = need.min(source);
                let bytes = u64::from(loaded.bitmap.0) * u64::from(loaded.bitmap.1) * 4;
                let smaller = have > target * SHRINK_FACTOR && bytes >= MIN_SHRINK_BYTES;
                let larger = target > have * GROW_FACTOR && have < source;
                (smaller || larger).then_some(Rerender::Raster {
                    max_width: target as u32,
                })
            };
            if let Some(rerender) = rerender {
                loaded.asks += 1;
                wants.push(ImageWant {
                    url,
                    key,
                    origin: WantOrigin::Element,
                    rerender: Some(rerender),
                });
            }
        }
    }

    /// Pins every loaded image, so that none a capture needs is evicted
    /// before it is drawn. The store refuses an insert that would take the
    /// pinned set past its hard ceiling, which is how an image that cannot fit
    /// is reported instead of painted blank.
    pub(crate) fn pin_loaded(&mut self) {
        let ids: Vec<_> = self.loaded.values().map(|l| l.id).collect();
        self.store.set_pinned(ids);
    }

    pub(crate) fn install(
        &mut self,
        key: ImageKey,
        decoded: Decoded,
        _settings: &DecodeSettings,
    ) -> Result<(), StoreError> {
        let (width, height) = (decoded.image.width, decoded.image.height);
        // Each decode of one URL is its own entry, told apart by its size.
        let params = DecodeParams {
            max_size: Some((width, height)),
        };
        let scale = if decoded.scale > 0.0 {
            decoded.scale
        } else {
            1.0
        };
        let vector = decoded
            .vector
            .then(|| (width as f32 / scale, height as f32 / scale));
        // The image being replaced does not count against the ceiling.
        let others: Vec<_> = self
            .loaded
            .iter()
            .filter(|(k, _)| **k != key)
            .map(|(_, l)| l.id)
            .collect();
        self.store.set_pinned(others);
        let store_key = StoreKey {
            url: key.clone(),
            params,
        };
        let inserted = self.store.insert(store_key, decoded.image, 1.0);
        let id = match inserted {
            Ok(id) => id,
            Err(error) => {
                self.pin_loaded();
                return Err(error);
            }
        };
        self.failed.remove(&key);
        let asks = self.loaded.get(&key).map_or(0, |l| l.asks);
        self.loaded.insert(
            key,
            Loaded {
                id,
                natural: decoded.natural,
                vector,
                bitmap: (width, height),
                asks,
            },
        );
        self.pin_loaded();
        // Renderings this one replaced are of no use to anyone.
        self.store.trim(0);
        self.epoch += 1;
        Ok(())
    }

    pub(crate) fn fail(&mut self, key: ImageKey, reason: String) {
        self.loaded.remove(&key);
        self.failed.insert(key, reason);
        self.epoch += 1;
    }

    pub(crate) fn stats(&self) -> ImageStats {
        ImageStats {
            loaded: self.loaded.len(),
            failed: self.failed.len(),
            store_bytes: self.store.bytes(),
            evicted: self
                .loaded
                .values()
                .filter(|l| self.store.get(l.id).is_none())
                .count(),
        }
    }

    pub(crate) fn failure_of_node(&self, node: NodeId) -> Option<&str> {
        let key = self.nodes.get(&node)?.key.as_ref()?;
        self.failure(key)
    }

    pub(crate) fn failure(&self, key: &ImageKey) -> Option<&str> {
        self.failed.get(key).map(String::as_str)
    }

    /// The loaded image, if the store still holds its pixels.
    fn resident(&self, key: &ImageKey) -> Option<Loaded> {
        self.loaded
            .get(key)
            .copied()
            .filter(|l| self.store.get(l.id).is_some())
    }
}

/// Whether `node` is an element that shows an image from a URL: an `<img>`, an
/// `<input type=image>`, or a `<video>` with a `poster`.
pub fn is_image_element(doc: &Document, node: NodeId) -> bool {
    if doc.namespace(node) != Some(atoms::NS_HTML) {
        return false;
    }
    match doc.tag(node) {
        Some(tag) if tag == atoms::IMG => true,
        // A video's poster is its image until it plays.
        Some(tag) if tag == atoms::VIDEO => doc.attribute_str(node, "poster").is_some(),
        Some(tag) if tag == atoms::INPUT => doc
            .attribute(node, atoms::TYPE)
            .is_some_and(|t| t.eq_ignore_ascii_case("image")),
        _ => false,
    }
}

/// The source an image element selects for `env`, or `None` when it has none
/// that resolves.
pub(crate) fn image_source(
    doc: &Document,
    node: NodeId,
    base: &Url,
    env: &MediaEnv,
) -> Option<select::Source> {
    if doc.tag(node) == Some(atoms::IMG) {
        return select::select(doc, node, base, env);
    }
    let value = if doc.tag(node) == Some(atoms::VIDEO) {
        doc.attribute_str(node, "poster")
    } else {
        doc.attribute(node, atoms::SRC)
    };
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| base.join(s).ok())
        .map(|url| select::Source { url, density: 1.0 })
}

/// Whether the element asks to be loaded lazily.
pub(crate) fn is_lazy(doc: &Document, node: NodeId) -> bool {
    doc.attribute(node, atoms::LOADING)
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("lazy"))
}

/// The key of a URL: the URL without its fragment.
pub(crate) fn key_of(url: &Url) -> ImageKey {
    let mut url = url.clone();
    url.set_fragment(None);
    ImageKey(url.as_str().into())
}

/// An HTML dimension attribute: a non-negative number, ignoring a trailing
/// `px`; percentages and anything else are not dimensions.
fn parse_dimension(value: &str) -> Option<f32> {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, rest) = value.split_at(end);
    if !(rest.is_empty() || rest.eq_ignore_ascii_case("px")) {
        return None;
    }
    number.parse().ok()
}

/// A node's natural size with its source density applied.
fn scaled(natural: IntrinsicSize, density: f32) -> IntrinsicSize {
    let density = if density.is_finite() && density > 0.0 {
        density
    } else {
        1.0
    };
    IntrinsicSize {
        width: natural.width.map(|w| w / density),
        height: natural.height.map(|h| h / density),
        ratio: natural.ratio,
    }
}

impl LayoutImages for ImageTable {
    fn intrinsic(&self, node: NodeId) -> Option<IntrinsicSize> {
        let image = self.nodes.get(&node)?;
        // The size outlives the pixels: an image the store evicted keeps its
        // box, so layout does not move when memory runs short.
        if let Some(loaded) = image.key.as_ref().and_then(|key| self.loaded.get(key)) {
            return Some(scaled(loaded.natural, image.density));
        }
        image.attr_ratio.map(|ratio| IntrinsicSize {
            width: None,
            height: None,
            ratio: Some(ratio),
        })
    }

    fn state(&self, node: NodeId) -> ImageState {
        let Some(image) = self.nodes.get(&node) else {
            return ImageState::Pending;
        };
        match &image.key {
            None => ImageState::Broken,
            // Loaded, even when the store has since evicted the pixels.
            Some(key) if self.loaded.contains_key(key) => ImageState::Loaded,
            Some(key) if self.failed.contains_key(key) => ImageState::Broken,
            Some(_) => ImageState::Pending,
        }
    }
}

impl ImageBindings for ImageTable {
    fn node_image(&self, node: NodeId) -> Option<DrawableImage> {
        let image = self.nodes.get(&node)?;
        let loaded = self.resident(image.key.as_ref()?)?;
        Some(DrawableImage {
            id: loaded.id,
            natural: scaled(loaded.natural, image.density),
        })
    }

    fn url_image(&self, key: &ImageKey) -> Option<DrawableImage> {
        let loaded = self.resident(key)?;
        Some(DrawableImage {
            id: loaded.id,
            natural: loaded.natural,
        })
    }

    fn node_broken(&self, node: NodeId) -> bool {
        self.state(node) == ImageState::Broken
    }
}

/// The decode settings for a viewport.
pub(crate) fn decode_settings(viewport_width: u32, dpr: f32) -> DecodeSettings {
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    let width = ((viewport_width as f32 * dpr * 1.5).ceil() as u32).clamp(512, 8192);
    DecodeSettings {
        device_scale: dpr,
        max_size: (width, MAX_STORED_HEIGHT),
        limits: DecodeLimits::default(),
    }
}
