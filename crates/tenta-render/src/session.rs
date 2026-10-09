//! The render session: style, layout and paint behind one owner.

use tenta_css::cascade::{StyleDiff, StyleEngine, StyleEnv, StyleSource};
use tenta_dom::Document;
use tenta_layout::{BoxTree, FontSystem, FragmentTree, LayoutCx, NoImages, layout};

use crate::screenshot::{RenderError, Screenshot, ScreenshotOptions};
use crate::viewport::{RenderConfig, Viewport};

/// Everything derived from one document: the style engine, font system, box
/// tree and fragment tree, with the stamp that says when they are current.
///
/// A session borrows the document for the duration of a call and keeps no
/// reference to it in between. It is single-threaded; the page keeps exactly
/// one per page, on the page thread. Its state is visible to the sibling
/// modules of this crate, so geometry queries read it without editing this file.
pub struct RenderSession {
    pub(crate) config: RenderConfig,
    pub(crate) styles: StyleEngine,
    pub(crate) fonts: FontSystem,
    pub(crate) tree: Option<BoxTree>,
    pub(crate) frags: Option<FragmentTree>,
    /// Document generation and viewport the fragment tree was laid out for.
    pub(crate) laid_out: Option<(u64, Viewport)>,
}

impl RenderSession {
    /// A session for `cfg` with no document and no style sources.
    pub fn new(cfg: RenderConfig) -> Self {
        let fonts = FontSystem::new_embedded();
        let styles = StyleEngine::new(style_env(&cfg, false), fonts.metrics());
        Self {
            config: cfg,
            styles,
            fonts,
            tree: None,
            frags: None,
            laid_out: None,
        }
    }

    /// Changes the viewport and invalidates layout.
    pub fn set_viewport(&mut self, v: Viewport) {
        self.config.viewport = v;
        self.styles
            .set_env(style_env(&self.config, self.styles.env().quirks));
    }

    /// Replaces the author stylesheets; the page calls this only when the
    /// sheet list changed.
    pub fn set_style_sources(&mut self, s: Vec<StyleSource>) {
        self.styles.set_sources(s);
    }

    /// Flushes style and lays the document out, without painting. Does no
    /// work while the document generation and the viewport are unchanged.
    pub fn layout(&mut self, doc: &Document) -> &FragmentTree {
        let quirks = StyleEnv::quirks_of(doc);
        if self.styles.env().quirks != quirks {
            self.styles.set_env(style_env(&self.config, quirks));
        }
        let diff = self.styles.flush(doc);
        let stamp = (doc.mutation_generation(), self.config.viewport);
        if diff != StyleDiff::None || self.tree.is_none() || self.laid_out != Some(stamp) {
            let viewport = (
                self.config.viewport.width as f32,
                self.config.viewport.height as f32,
            );
            let mut cx = LayoutCx {
                doc,
                styles: &self.styles,
                fonts: &mut self.fonts,
                images: &NoImages,
                viewport,
                quirks,
            };
            let tree = BoxTree::build(&cx);
            let frags = layout(&mut cx, &tree);
            self.tree = Some(tree);
            self.frags = Some(frags);
            self.laid_out = Some(stamp);
        }
        self.frags
            .as_ref()
            .expect("layout stored a fragment tree above")
    }

    /// Captures the document as an encoded image.
    pub fn screenshot(
        &mut self,
        doc: &Document,
        opts: &ScreenshotOptions,
    ) -> Result<Screenshot, RenderError> {
        let _ = (doc, opts);
        todo!("R2")
    }
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
