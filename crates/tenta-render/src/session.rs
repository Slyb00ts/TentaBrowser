//! The render session: style, layout and paint behind one owner.

use std::time::Instant;

use tenta_core::NodeId;
use tenta_css::cascade::{StyleDiff, StyleEngine, StyleEnv, StyleSource};
use tenta_dom::Document;
use tenta_layout::{BoxTree, FontSystem, FragmentTree, ImageKey, LayoutCx, Rect, layout};
use tenta_paint::{
    Color, DecodeControl, DisplayList, PaintContext, PngProfile, RasterOptions, StoreError, build,
    encode_jpeg, encode_png, rasterize,
};
use url::Url;

use crate::images::{
    DecodeSettings, Decoded, ImageKind, ImagePlan, ImageStats, ImageTable, LazyRegion, MediaEnv,
    Rerender, decode_data_url, decode_image, decode_settings,
};
use crate::screenshot::{
    Capture, ImageFormat, RenderError, Screenshot, ScreenshotOptions, StageTimes,
};
use crate::viewport::{FontPolicy, RenderConfig, Viewport};

/// Longest side of a capture in device pixels, the limit Chromium applies to
/// a texture.
const MAX_DIMENSION: u32 = 16_384;

/// Largest capture in device pixels (256 MiB of RGBA).
const MAX_PIXELS: u64 = 1 << 26;

/// What a layout was computed for.
#[derive(Copy, Clone, PartialEq)]
struct LayoutStamp {
    generation: u64,
    viewport: Viewport,
    /// The image table's epoch: a loaded or failed image changes sizes and
    /// can change which boxes exist.
    images: u64,
}

/// The display list of the layout with this epoch.
struct RetainedList {
    epoch: u64,
    list: DisplayList,
}

/// Units of work a session has done since it was created. They only grow, so
/// a test or a benchmark tells that a call did nothing by comparing before and
/// after.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkCounters {
    /// Times the author sheets were replaced.
    pub source_sets: u64,
    /// Times the style traversal ran.
    pub style_traversals: u64,
    /// Times the box tree and fragment tree were produced.
    pub layouts: u64,
    /// Times a display list was built.
    pub display_lists: u64,
}

/// Where and how large a capture is.
struct Area {
    /// Top-left in CSS pixels, a whole number of device pixels.
    origin: (f32, f32),
    width: u32,
    height: u32,
    clamped: bool,
}

/// Everything derived from one document: the style engine, font system, box
/// tree and fragment tree, with the stamp that says when they are current.
///
/// A session borrows the document for the duration of a call and keeps no
/// reference to it in between. It is single-threaded; the page keeps exactly
/// one per page, on the page thread. Its state is visible to the sibling
/// modules of this crate, so geometry queries read it without editing this file.
///
/// Layout and paint are tracked separately: a geometry query brings only the
/// fragment tree up to date, and the display list is built on the first
/// capture after each layout and reused by the captures that follow.
pub struct RenderSession {
    pub(crate) config: RenderConfig,
    pub(crate) styles: StyleEngine,
    pub(crate) fonts: FontSystem,
    pub(crate) tree: Option<BoxTree>,
    pub(crate) frags: Option<FragmentTree>,
    /// Document generation and viewport the fragment tree was laid out for.
    laid_out: Option<LayoutStamp>,
    /// Incremented by every layout, to tie the display list to its fragments.
    epoch: u64,
    display: Option<RetainedList>,
    /// Style changes a style-only flush ([`Self::flush_styles`]) found and the
    /// next layout has not consumed yet.
    pending_diff: StyleDiff,
    pub(crate) images: ImageTable,
    work: WorkCounters,
    #[cfg(test)]
    fail_next_layout: bool,
    #[cfg(test)]
    fail_next_flush: bool,
}

impl RenderSession {
    /// A session for `cfg` with no document and no style sources.
    pub fn new(cfg: RenderConfig) -> Self {
        let fonts = match cfg.fonts {
            #[cfg(feature = "system-fonts")]
            FontPolicy::SystemFallback => FontSystem::new_with_system_fallback(),
            _ => FontSystem::new_embedded(),
        };
        let styles = StyleEngine::new(style_env(&cfg, false), fonts.metrics());
        Self {
            config: cfg,
            styles,
            fonts,
            tree: None,
            frags: None,
            laid_out: None,
            epoch: 0,
            display: None,
            pending_diff: StyleDiff::None,
            images: ImageTable::new(),
            work: WorkCounters::default(),
            #[cfg(test)]
            fail_next_layout: false,
            #[cfg(test)]
            fail_next_flush: false,
        }
    }

    /// Changes the viewport, including its device pixel ratio, and invalidates
    /// style and layout. Setting the current viewport again does nothing.
    pub fn set_viewport(&mut self, v: Viewport) {
        self.config.viewport = v;
    }

    /// Replaces the author stylesheets. Sheets equal to the current ones are
    /// ignored, because installing them would restyle the whole document.
    pub fn set_style_sources(&mut self, s: Vec<StyleSource>) {
        if same_sources(self.styles.sources(), &s) {
            return;
        }
        self.styles.set_sources(s);
        self.work.source_sets += 1;
    }

    /// Lists the images `doc` needs and the session lacks, laid out as far as
    /// the images known so far allow. Call it again after installing images:
    /// a background image is only found once its box has a fragment, and
    /// `loading=lazy` elements are judged by where layout put them.
    ///
    /// `base` resolves relative image URLs (the document's base URL).
    pub fn plan_images(&mut self, doc: &Document, base: &Url, region: LazyRegion) -> ImagePlan {
        let viewport = self.config.viewport;
        let env = self.media_env();
        self.images.refresh_nodes(doc, base, &env);
        self.refresh_layout(doc);
        let wants = self.images.wants(
            self.frags.as_ref(),
            region,
            (viewport.width as f32, viewport.height as f32),
            viewport.device_pixel_ratio,
        );
        ImagePlan {
            wants,
            decode: self.decode_settings(),
        }
    }

    /// How fetched bodies are decoded for this session's viewport.
    pub fn decode_settings(&self) -> DecodeSettings {
        let viewport = self.config.viewport;
        decode_settings(viewport.width, viewport.device_pixel_ratio)
    }

    /// Stores a decoded image under `key` and relays out what it sizes.
    /// Fails when the image store cannot hold it, which the caller reports as
    /// that image failing.
    pub fn install_image(&mut self, key: ImageKey, decoded: Decoded) -> Result<(), StoreError> {
        let settings = self.decode_settings();
        self.images.install(key, decoded, &settings)
    }

    /// Records that `key` could not be loaded, so every element that wanted it
    /// is a broken image.
    pub fn fail_image(&mut self, key: ImageKey, reason: impl Into<String>) {
        self.images.fail(key, reason.into());
    }

    /// Why `key` failed, if it did.
    pub fn image_failure(&self, key: &ImageKey) -> Option<&str> {
        self.images.failure(key)
    }

    /// Why the image `node` shows failed, if it did.
    #[doc(hidden)]
    pub fn image_failure_for(&self, _doc: &Document, node: NodeId) -> Option<&str> {
        self.images.failure_of_node(node)
    }

    /// What the session holds, for tests and the memory budget.
    pub fn image_stats(&self) -> ImageStats {
        self.images.stats()
    }

    /// Loads every image `doc` references through a `data:` URL and fails the
    /// rest, for callers that have no network (fixtures and tests). Returns
    /// the number of images installed.
    pub fn load_inline_images(&mut self, doc: &Document, base: &Url) -> usize {
        let mut installed = 0;
        // Laying out with the first images can reveal CSS images of boxes
        // that were empty before, so plan until nothing new is wanted.
        for _ in 0..4 {
            let plan = self.plan_images(doc, base, LazyRegion::All);
            if plan.wants.is_empty() {
                break;
            }
            for want in plan.wants {
                let mut settings = plan.decode;
                match want.rerender {
                    Some(Rerender::Svg { scale }) => settings.device_scale = scale,
                    Some(Rerender::Raster { max_width }) => {
                        settings.max_size.0 = max_width.max(1);
                    }
                    None => {}
                }
                let outcome = if want.url.scheme() == "data" {
                    decode_data_url(want.url.as_str(), 64 << 20)
                        .map_err(|e| e.to_string())
                        .and_then(|data| {
                            let kind = ImageKind::of(data.mime.as_deref());
                            decode_image(&data.bytes, kind, &settings, &DecodeControl::new())
                                .map_err(|e| e.to_string())
                        })
                } else {
                    Err("no network is available to this session".to_owned())
                };
                let again = want.rerender.is_some();
                match outcome {
                    Ok(decoded) => match self.install_image(want.key.clone(), decoded) {
                        Ok(()) => installed += usize::from(!again),
                        Err(_) if again => {}
                        Err(error) => {
                            self.fail_image(
                                want.key,
                                format!("image memory budget exceeded: {error}"),
                            );
                        }
                    },
                    // A failed second decode leaves the first in place.
                    Err(_) if again => {}
                    Err(reason) => self.fail_image(want.key, reason),
                }
            }
        }
        installed
    }

    fn media_env(&self) -> MediaEnv {
        let viewport = self.config.viewport;
        MediaEnv {
            width: viewport.width as f32,
            height: viewport.height as f32,
            dpr: viewport.device_pixel_ratio,
            dark: self.config.dark,
        }
    }

    /// What the session has done so far.
    pub fn counters(&self) -> WorkCounters {
        WorkCounters {
            style_traversals: self.styles.traversal_count(),
            ..self.work
        }
    }

    /// Brings the computed styles up to date without laying anything out: what
    /// `getComputedStyle` of a property that does not depend on layout needs.
    /// The change it found is kept for the next layout, which must still see it.
    pub fn flush_styles(&mut self, doc: &Document) {
        let env = style_env(&self.config, self.styles.env().quirks);
        if *self.styles.env() != env {
            self.styles.set_env(env);
        }
        let diff = self.styles.flush(doc);
        self.pending_diff = merge_diff(
            std::mem::replace(&mut self.pending_diff, StyleDiff::None),
            diff,
        );
    }

    /// Flushes style and lays the document out, without painting. Does no
    /// work while the document generation and the viewport are unchanged.
    pub fn layout(&mut self, doc: &Document) -> &FragmentTree {
        self.refresh_layout(doc);
        self.frags
            .as_ref()
            .expect("refresh_layout stored a fragment tree")
    }

    /// Captures the document as an encoded image.
    ///
    /// The output has `CSS pixels x device pixel ratio x scale` device pixels
    /// per side: the ratio belongs to the viewport and reaches style (media
    /// queries), while `scale` only enlarges the raster.
    ///
    /// `FullPage` lays out once at the configured viewport and captures the
    /// whole scrollable extent, as Chromium does with `captureBeyondViewport`:
    /// `vh` units are unchanged and fixed-position boxes are painted once, at
    /// their place in the first screen.
    pub fn screenshot(
        &mut self,
        doc: &Document,
        opts: &ScreenshotOptions,
    ) -> Result<Screenshot, RenderError> {
        match opts.format {
            ImageFormat::Png => {}
            ImageFormat::Jpeg { quality } if (1..=100).contains(&quality) => {}
            ImageFormat::Jpeg { .. } => {
                return Err(RenderError::InvalidOption("JPEG quality is 1 to 100"));
            }
            ImageFormat::Webp => return Err(RenderError::UnsupportedFormat(opts.format)),
        }
        let clip_scale = opts.scale.unwrap_or(1.0);
        if !(clip_scale.is_finite() && clip_scale > 0.0) {
            return Err(RenderError::InvalidOption(
                "scale must be a finite positive number",
            ));
        }
        if doc.document_element().is_none() {
            return Err(RenderError::NoRoot);
        }

        let scale = self.config.viewport.device_pixel_ratio * clip_scale;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(RenderError::InvalidOption(
                "the device pixel ratio times the scale must be a finite positive number",
            ));
        }

        let mut times = StageTimes::default();
        let started = Instant::now();
        let area = self.capture_area(doc, opts.capture, f64::from(scale))?;
        times.layout = started.elapsed();

        let started = Instant::now();
        let built = self.ensure_display_list();
        if built {
            times.display_list = started.elapsed();
        }

        let started = Instant::now();
        let raster = RasterOptions {
            width: area.width,
            height: area.height,
            scale,
            origin: area.origin,
            background: (!opts.omit_background).then_some(Color::WHITE),
        };
        let list = &self
            .display
            .as_ref()
            .expect("the list was just ensured")
            .list;
        let output = rasterize(list, &raster, &self.images.store)?;
        times.raster = started.elapsed();

        let started = Instant::now();
        let bytes = match opts.format {
            ImageFormat::Jpeg { quality } => encode_jpeg(&output.pixmap, quality),
            _ => encode_png(&output.pixmap, PngProfile::Fast),
        };
        times.encode = started.elapsed();

        Ok(Screenshot {
            bytes,
            format: opts.format,
            width: area.width,
            height: area.height,
            stats: output.stats,
            clamped: area.clamped,
            times,
        })
    }

    /// Brings the fragment tree up to date for the configured viewport.
    ///
    /// Whatever a panic could leave half-done is cleared before the work
    /// starts, so after one the next call rebuilds everything.
    fn refresh_layout(&mut self, doc: &Document) {
        let viewport = self.config.viewport;
        let previous = self.laid_out.take();
        let old_tree = self.tree.take();
        let old_frags = self.frags.take();
        let env = style_env(&self.config, self.styles.env().quirks);
        if *self.styles.env() != env {
            self.styles.set_env(env);
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_flush) {
            panic!("injected flush failure");
        }
        let diff = merge_diff(
            std::mem::replace(&mut self.pending_diff, StyleDiff::None),
            self.styles.flush(doc),
        );
        let stamp = LayoutStamp {
            generation: doc.mutation_generation(),
            viewport,
            images: self.images.epoch,
        };
        // A box tree built before an image changed state may hold the wrong
        // kind of box (a broken image with `alt` is text), so it is rebuilt.
        let old_tree = old_tree.filter(|_| previous.is_some_and(|p| p.images == stamp.images));
        if diff == StyleDiff::None && old_tree.is_some() && previous == Some(stamp) {
            self.laid_out = previous;
            self.tree = old_tree;
            self.frags = old_frags;
            return;
        }
        let mut cx = LayoutCx {
            doc,
            styles: &self.styles,
            fonts: &mut self.fonts,
            images: &self.images,
            viewport: (viewport.width as f32, viewport.height as f32),
            quirks: self.styles.env().quirks,
        };
        let tree = match old_tree {
            Some(mut tree) => {
                tree.update(&cx, &diff);
                tree
            }
            None => BoxTree::build(&cx),
        };
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_layout) {
            panic!("injected layout failure");
        }
        self.frags = Some(layout(&mut cx, &tree));
        self.tree = Some(tree);
        self.laid_out = Some(stamp);
        self.epoch += 1;
        self.work.layouts += 1;
    }

    /// Builds the display list unless the retained one belongs to the current
    /// layout. Returns whether it built one.
    fn ensure_display_list(&mut self) -> bool {
        if self.display.as_ref().is_some_and(|d| d.epoch == self.epoch) {
            return false;
        }
        let frags = self
            .frags
            .as_ref()
            .expect("a capture area is computed after layout");
        let list = build(
            frags,
            &PaintContext {
                images: &self.images.store,
                bindings: &self.images,
            },
        );
        // Every loaded image stays resident, so none is evicted before it is drawn.
        self.images.pin_loaded();
        self.display = Some(RetainedList {
            epoch: self.epoch,
            list,
        });
        self.work.display_lists += 1;
        true
    }

    /// Lays out as the capture needs and returns the pixels it covers.
    fn capture_area(
        &mut self,
        doc: &Document,
        capture: Capture,
        scale: f64,
    ) -> Result<Area, RenderError> {
        let viewport = self.config.viewport;
        match capture {
            Capture::FullPage => {
                self.refresh_layout(doc);
                Ok(self.full_page_area(scale))
            }
            Capture::Viewport => {
                self.refresh_layout(doc);
                let rect = Rect::new(0.0, 0.0, viewport.width as f32, viewport.height as f32);
                widen(rect, scale)
            }
            Capture::Clip(rect) => {
                let finite = [rect.x, rect.y, rect.w, rect.h]
                    .iter()
                    .all(|v| v.is_finite());
                if !finite || rect.w <= 0.0 || rect.h <= 0.0 {
                    return Err(RenderError::InvalidOption(
                        "a clip must be a finite rectangle with a positive area",
                    ));
                }
                self.refresh_layout(doc);
                widen(rect, scale)
            }
            Capture::Element(node) => {
                self.refresh_layout(doc);
                let rect = self
                    .border_box_union(node)
                    .ok_or(RenderError::NoBox(node))?;
                widen(rect, scale)
            }
        }
    }

    /// The union of the document-space border boxes of `node`'s fragments.
    fn border_box_union(&self, node: NodeId) -> Option<Rect> {
        let frags = self.frags.as_ref()?;
        crate::geometry::own_fragments(frags, node)
            .into_iter()
            .map(|id| frags.absolute_border_box(id))
            .reduce(|a, b| a.union(&b))
    }

    /// The whole scrollable extent of the current layout, cut at the size
    /// limits.
    fn full_page_area(&self, scale: f64) -> Area {
        let extent = self
            .frags
            .as_ref()
            .expect("layout stored a fragment tree")
            .scroll_extent;
        let width = device_extent(f64::from(extent.right()), scale).min(MAX_DIMENSION);
        let tallest = MAX_DIMENSION.min((MAX_PIXELS / u64::from(width.max(1))) as u32);
        let wanted_height = device_extent(f64::from(extent.bottom()), scale);
        let height = wanted_height.min(tallest);
        Area {
            origin: (0.0, 0.0),
            width,
            height,
            clamped: height < wanted_height
                || width < device_extent(f64::from(extent.right()), scale),
        }
    }
}

/// The smallest whole-pixel area that covers `rect` once scaled, checked
/// against the capture limits.
fn widen(rect: Rect, scale: f64) -> Result<Area, RenderError> {
    let left = (f64::from(rect.x) * scale).floor();
    let top = (f64::from(rect.y) * scale).floor();
    let right = (f64::from(rect.x + rect.w) * scale).ceil();
    let bottom = (f64::from(rect.y + rect.h) * scale).ceil();
    // Float-to-integer casts saturate, which keeps absurd sizes reportable.
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    if width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(RenderError::TooLarge { width, height });
    }
    Ok(Area {
        origin: ((left / scale) as f32, (top / scale) as f32),
        width,
        height,
        clamped: false,
    })
}

fn device_extent(css: f64, scale: f64) -> u32 {
    (css * scale).ceil() as u32
}

fn style_env(cfg: &RenderConfig, quirks: bool) -> StyleEnv {
    StyleEnv {
        viewport: (cfg.viewport.width as f32, cfg.viewport.height as f32),
        device_pixel_ratio: cfg.viewport.device_pixel_ratio,
        dark: cfg.dark,
        reduced_motion: false,
        print: false,
        quirks,
    }
}

fn same_sources(a: &[StyleSource], b: &[StyleSource]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.owner == y.owner
                && x.url == y.url
                && x.text == y.text
                && x.media == y.media
                && x.disabled == y.disabled
                && x.imports.len() == y.imports.len()
                && x.imports
                    .iter()
                    .zip(&y.imports)
                    .all(|(p, q)| p.url == q.url && p.final_url == q.final_url && p.text == q.text)
        })
}

/// Two flush results in order: anything but `None` survives, `Full` dominates.
fn merge_diff(earlier: StyleDiff, later: StyleDiff) -> StyleDiff {
    match (earlier, later) {
        (StyleDiff::None, other) | (other, StyleDiff::None) => other,
        (StyleDiff::Partial(mut a), StyleDiff::Partial(b)) => {
            a.extend(b);
            StyleDiff::Partial(a)
        }
        _ => StyleDiff::Full,
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use tenta_core::atom::{Atom, atoms};
    use tenta_dom::Mutation;
    use tenta_html::{Parser, ParserOptions, QualName};

    use super::*;

    fn parse(html: &str) -> Document {
        let mut parser = Parser::new(Document::new(), ParserOptions::default());
        parser.feed_last(html.as_bytes());
        parser.finish()
    }

    fn corner_pixel(shot: &Screenshot) -> [u8; 4] {
        let mut reader = png::Decoder::new(std::io::Cursor::new(&shot.bytes))
            .read_info()
            .expect("valid PNG");
        let mut rgba = vec![0; reader.output_buffer_size().expect("fits")];
        reader.next_frame(&mut rgba).expect("valid frame");
        rgba[..4].try_into().expect("four channels")
    }

    #[test]
    fn a_panic_during_flush_or_layout_forces_a_full_rebuild() {
        for flush in [false, true] {
            panic_recovery(flush);
        }
    }

    fn panic_recovery(flush: bool) {
        let mut doc = parse(
            "<!doctype html><body style='margin:0'><div id=d style='width:8px;height:8px;\
             background:#f00'></div>",
        );
        let mut session = RenderSession::new(RenderConfig::default());
        let opts = ScreenshotOptions::default();

        if flush {
            session.fail_next_flush = true;
        } else {
            session.fail_next_layout = true;
        }
        let result = catch_unwind(AssertUnwindSafe(|| session.screenshot(&doc, &opts)));
        assert!(result.is_err(), "the injected failure propagates");
        let shot = session.screenshot(&doc, &opts).expect("recovers");
        assert_eq!(corner_pixel(&shot), [255, 0, 0, 255]);
        assert_eq!(session.counters().layouts, 1);

        let element = doc.get_element_by_id("d").expect("the div");
        doc.mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, Atom::new("style")),
            value: "width:8px;height:8px;background:#00f".to_owned(),
        })
        .expect("set attribute");
        if flush {
            session.fail_next_flush = true;
        } else {
            session.fail_next_layout = true;
        }
        let result = catch_unwind(AssertUnwindSafe(|| session.screenshot(&doc, &opts)));
        assert!(result.is_err());
        let shot = session.screenshot(&doc, &opts).expect("recovers");
        assert_eq!(
            corner_pixel(&shot),
            [0, 0, 255, 255],
            "the mutation is not hidden behind the stale tree"
        );
        assert_eq!(session.counters().layouts, 2);
    }

    #[test]
    fn widening_covers_fractional_rects_and_keeps_device_alignment() {
        let area = widen(Rect::new(10.25, 4.5, 20.0, 10.0), 2.0).unwrap();
        assert_eq!((area.width, area.height), (41, 20));
        assert_eq!(area.origin, (10.0, 4.5));
    }

    #[test]
    fn widening_rejects_sizes_over_the_limits() {
        let tall = widen(Rect::new(0.0, 0.0, 10.0, 20_000.0), 1.0);
        assert_eq!(
            tall.err(),
            Some(RenderError::TooLarge {
                width: 10,
                height: 20_000
            })
        );
        let many = widen(Rect::new(0.0, 0.0, 10_000.0, 10_000.0), 1.0);
        assert!(matches!(many, Err(RenderError::TooLarge { .. })));
        let absurd = widen(Rect::new(0.0, 0.0, 1e30, 1.0), 1.0);
        assert!(matches!(absurd, Err(RenderError::TooLarge { .. })));
    }
}
