//! Font registration, family resolution and font metrics.
//!
//! The default font database is built only from the bundled files: no system
//! font is enumerated and no file is read, so every machine resolves a family
//! list to the same face. [`FontSystem::new_with_system_fallback`] (feature
//! `system-fonts`, D-036) adds the platform database for scripts the bundled
//! fonts lack; family lists still resolve to bundled faces first.

#[cfg(feature = "embedded-fonts")]
mod families;
#[cfg(feature = "embedded-fonts")]
mod metrics;
#[cfg(all(test, feature = "embedded-fonts"))]
mod tests;

use std::fmt;
#[cfg(feature = "embedded-fonts")]
use std::sync::{Arc, OnceLock};

#[cfg(feature = "embedded-fonts")]
use parley::fontique::{
    Blob, Collection, CollectionOptions, FallbackKey, FamilyId, Script, SourceCache,
};
#[cfg(feature = "embedded-fonts")]
use parley::{FontContext, LayoutContext};
#[cfg(feature = "embedded-fonts")]
use tenta_css::cascade::FontMetrics;

#[cfg(feature = "embedded-fonts")]
use crate::embedded_fonts::EMBEDDED_FONTS;
#[cfg(feature = "embedded-fonts")]
use families::{Family, GENERICS, resolve_list};

#[cfg(feature = "embedded-fonts")]
/// Scripts whose missing glyphs fall back to DejaVu Sans, the only bundled face
/// beyond Latin. Common and Inherited carry punctuation and symbols.
const FALLBACK_SCRIPTS: [[u8; 4]; 9] = [
    *b"Zyyy", *b"Zinh", *b"Latn", *b"Grek", *b"Cyrl", *b"Arab", *b"Hebr", *b"Armn", *b"Geor",
];

#[cfg(feature = "embedded-fonts")]
/// Vertical metrics of one face at one size, following Blink's `normal`
/// line-height: ascent and descent are rounded individually.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct VerticalMetrics {
    /// Distance from the baseline to the top of the line box, rounded.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line box, rounded and
    /// positive.
    pub descent: f32,
    /// The font's recommended extra spacing between lines, unrounded.
    pub line_gap: f32,
}

#[cfg(feature = "embedded-fonts")]
impl VerticalMetrics {
    /// The `line-height: normal` used value.
    pub fn line_spacing(&self) -> f32 {
        self.ascent + self.descent + self.line_gap.round()
    }
}

#[cfg(feature = "embedded-fonts")]
/// The primary face a family list, weight and style resolve to, and what must
/// be synthesised on top of it. Glyphs the face lacks are not covered here:
/// per-glyph fallback is parley's job, over the list from
/// [`FontSystem::family_stack`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    family: Family,
    bold_face: bool,
    synthetic_bold: bool,
    synthetic_oblique: bool,
}

#[cfg(feature = "embedded-fonts")]
impl ResolvedFont {
    /// Resolves the first family of `family_list` that the database knows.
    ///
    /// Weights above 500 select the bold face; a family without one is
    /// emboldened only from 600 up, so 500 stays regular. No italic face is
    /// bundled, so italic is always an oblique skew of the upright face.
    pub fn resolve(family_list: &str, weight: u16, italic: bool) -> Self {
        let family = resolve_list(family_list)[0];
        let has_bold_face = family.face(true).file_name != family.face(false).file_name;
        Self {
            family,
            bold_face: has_bold_face && weight > 500,
            synthetic_bold: !has_bold_face && weight >= 600,
            synthetic_oblique: italic,
        }
    }

    /// The family name registered with the parley collection.
    pub fn family_name(&self) -> &'static str {
        self.family.name()
    }

    /// The complete font file this resolves to.
    pub fn bytes(&self) -> &'static [u8] {
        self.family.face(self.bold_face).bytes
    }

    /// The weight to give parley instead of the CSS weight. Parley emboldens
    /// whenever the requested weight exceeds the face's, which would embolden a
    /// regular-only face at 500; passing the resolved weight keeps parley's
    /// synthesis identical to [`Self::synthetic_bold`].
    pub fn parley_weight(&self) -> f32 {
        if self.bold_face || self.synthetic_bold {
            700.0
        } else {
            400.0
        }
    }

    /// Whether the glyphs must be emboldened because no bold face exists.
    pub fn synthetic_bold(&self) -> bool {
        self.synthetic_bold
    }

    /// Whether the glyphs must be skewed to fake an italic.
    pub fn synthetic_oblique(&self) -> bool {
        self.synthetic_oblique
    }
}

/// Owns the font database and shaping contexts of one page thread. Layout
/// reaches fonts only through this type.
pub struct FontSystem {
    #[cfg(feature = "embedded-fonts")]
    system_fallback: bool,
    #[cfg(feature = "embedded-fonts")]
    font_cx: FontContext,
    #[cfg(feature = "embedded-fonts")]
    layout_cx: LayoutContext,
}

impl fmt::Debug for FontSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontSystem").finish_non_exhaustive()
    }
}

/// The collection with every bundled font registered, built once per process.
/// Page threads clone it; the clone shares the font blobs.
#[cfg(feature = "embedded-fonts")]
fn embedded_font_context() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();
    TEMPLATE.get_or_init(|| build_context(false))
}

#[cfg(feature = "embedded-fonts")]
fn build_context(system_fonts: bool) -> FontContext {
    {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts,
        });
        for font in EMBEDDED_FONTS {
            collection.register_fonts(Blob::new(Arc::new(font.bytes)), None);
        }
        let id = |family: Family, collection: &mut Collection| -> Option<FamilyId> {
            collection.family_id(family.name())
        };
        for (_, generic, family) in GENERICS {
            let ids = id(family, &mut collection);
            collection.set_generic_families(generic, ids.into_iter());
        }
        for script in FALLBACK_SCRIPTS {
            let ids = id(Family::DejaVu, &mut collection);
            let key = FallbackKey::new(Script::from_bytes(script), None);
            collection.set_fallbacks(key, ids.into_iter());
        }
        FontContext {
            collection,
            source_cache: SourceCache::default(),
        }
    }
}

/// Like [`embedded_font_context`], plus the platform font database. Bundled
/// families keep their names and generics; only the fallback for scripts the
/// bundled fonts do not cover (Han, Kana, Hangul, Thai, ...) reaches system
/// faces, because DejaVu stays the fallback of [`FALLBACK_SCRIPTS`].
#[cfg(feature = "system-fonts")]
fn system_font_context() -> &'static FontContext {
    static TEMPLATE: OnceLock<FontContext> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let mut cx = build_context(true);
        // Enumerate once, here, so page threads never race the platform scan.
        let _ = cx.collection.family_names().count();
        cx
    })
}

impl FontSystem {
    /// A system over the bundled fonts plus the platform font database for
    /// scripts the bundled fonts lack. Output depends on the installed fonts, so
    /// golden tests and the render corpus use [`Self::new_embedded`].
    #[cfg(feature = "system-fonts")]
    pub fn new_with_system_fallback() -> Self {
        Self {
            system_fallback: true,
            font_cx: system_font_context().clone(),
            layout_cx: LayoutContext::new(),
        }
    }

    /// A system over the bundled fonts. Never enumerates system fonts.
    #[cfg(feature = "embedded-fonts")]
    pub fn new_embedded() -> Self {
        Self {
            system_fallback: false,
            font_cx: embedded_font_context().clone(),
            layout_cx: LayoutContext::new(),
        }
    }

    /// A system with Liberation Sans alone, claiming `system_fallback`'s cache
    /// scope. Next to [`Self::new_embedded`] it shapes Georgian differently on
    /// any machine, which is what the cache-scope test needs.
    #[cfg(test)]
    pub(crate) fn liberation_sans_only(system_fallback: bool) -> Self {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let sans = EMBEDDED_FONTS
            .iter()
            .find(|font| font.file_name == "LiberationSans-Regular.ttf")
            .expect("Liberation Sans is bundled");
        collection.register_fonts(Blob::new(Arc::new(sans.bytes)), None);
        Self {
            system_fallback,
            font_cx: FontContext {
                collection,
                source_cache: SourceCache::default(),
            },
            layout_cx: LayoutContext::new(),
        }
    }

    /// Which font database this system shapes against: shaped text is cached per
    /// thread and entries of different databases must not mix.
    #[cfg(feature = "embedded-fonts")]
    pub(crate) fn cache_scope(&self) -> u8 {
        u8::from(self.system_fallback)
    }

    /// The `ex`/`ch` metrics source the style engine needs.
    #[cfg(feature = "embedded-fonts")]
    pub fn metrics(&self) -> Arc<dyn FontMetrics> {
        Arc::new(metrics::EmbeddedMetrics)
    }

    /// The parley contexts, borrowed together because shaping needs both.
    #[cfg(feature = "embedded-fonts")]
    pub fn parley_contexts(&mut self) -> (&mut FontContext, &mut LayoutContext) {
        (&mut self.font_cx, &mut self.layout_cx)
    }

    /// The face `family_list`, `weight` and `italic` resolve to.
    #[cfg(feature = "embedded-fonts")]
    pub fn resolve(&self, family_list: &str, weight: u16, italic: bool) -> ResolvedFont {
        ResolvedFont::resolve(family_list, weight, italic)
    }

    /// The family names to hand parley for `family_list`, in list order. Names
    /// the database does not know are dropped, aliases are replaced by the
    /// bundled family they stand for, and DejaVu Sans ends the list so glyphs
    /// the listed families lack still resolve.
    #[cfg(feature = "embedded-fonts")]
    pub fn family_stack(&self, family_list: &str) -> Vec<&'static str> {
        let mut stack = resolve_list(family_list);
        if !stack.contains(&Family::DejaVu) {
            stack.push(Family::DejaVu);
        }
        stack.into_iter().map(Family::name).collect()
    }
}
