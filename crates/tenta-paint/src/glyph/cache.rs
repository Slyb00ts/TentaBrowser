//! The glyph mask cache.
//!
//! A mask is the 8-bit coverage of one glyph rasterised at a whole device
//! pixel with a quarter-pixel horizontal offset. Masks are cached rather than
//! paths because a text-heavy page repeats a few hundred (glyph, size)
//! pairs thousands of times, and re-filling the outline costs far more than
//! the per-pixel blend that a cached mask needs (see the `glyph_throughput`
//! test). The cache is per thread, so rendering threads never contend, and a
//! mask is a pure function of its key, so a hit and a miss draw identical
//! bytes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Weak};

use tenta_layout::{FontBlob, FontRef};
use tiny_skia::{Paint, Pixmap, Stroke, Transform};

use super::outline::{NoOutline, Outliner};

/// Bytes of masks one generation may hold before the generations rotate; a
/// page rarely needs more than a few hundred distinct glyphs.
const BYTE_BUDGET: usize = 8 << 20;

/// Misses between sweeps for fonts that no one else holds any more.
const SWEEP_INTERVAL: u64 = 4096;

/// Faux-bold expansion as a fraction of the em, as FreeType's
/// `FT_Outline_Embolden` is driven by Skia's scaler on Linux.
const EMBOLDEN_EM_FRACTION: f32 = 1.0 / 24.0;

/// Quarter-pixel steps per pixel.
pub(super) const SUBPIXEL_STEPS: i32 = 4;

/// A face as the cache knows it, so glyph keys need no font clone or hash.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub(super) struct FontId(u32);

#[derive(PartialEq, Eq, Hash)]
struct FontKey {
    blob: usize,
    index: u32,
    coords: Arc<[i16]>,
}

struct FontEntry {
    id: FontId,
    // A weak handle never keeps font bytes alive, yet it keeps the blob's
    // allocation reserved, so its address (the identity in `FontKey`) cannot
    // be reused by another font while the entry exists.
    blob: Weak<FontBlob>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    font: FontId,
    glyph: u32,
    size_bits: u32,
    quarter: u8,
    bold: bool,
    skew_bits: u32,
}

/// A rasterised glyph.
pub(super) struct Mask {
    /// Offset of the first column from the glyph origin's whole pixel.
    pub(super) left: i32,
    /// Offset of the first row from the baseline row.
    pub(super) top: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) alpha: Vec<u8>,
}

/// What a lookup yields.
pub(super) enum Cached {
    /// Coverage, empty for a glyph with nothing to paint.
    Drawn(Mask),
    /// The glyph cannot be rendered.
    Undrawn,
}

/// Heap and table cost of one entry beyond its mask bytes: the key, the
/// shared pointer with its counts, the value and the table's control byte.
const ENTRY_OVERHEAD: usize =
    size_of::<Key>() + size_of::<Rc<Cached>>() + 2 * size_of::<usize>() + size_of::<Cached>() + 1;

struct GlyphCache {
    fonts: HashMap<FontKey, FontEntry>,
    next_font: u32,
    // Two generations: a hit in `previous` moves the entry to `current`, and
    // a full `current` replaces `previous`, so a working set that fits in one
    // generation survives rotation while stale glyphs age out together.
    current: HashMap<Key, Rc<Cached>>,
    previous: HashMap<Key, Rc<Cached>>,
    bytes: usize,
    budget: usize,
    hits: u64,
    misses: u64,
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self {
            fonts: HashMap::new(),
            next_font: 0,
            current: HashMap::new(),
            previous: HashMap::new(),
            bytes: 0,
            budget: BYTE_BUDGET,
            hits: 0,
            misses: 0,
        }
    }
}

impl GlyphCache {
    /// Forgets fonts nobody else holds and the glyphs rasterised from them.
    fn sweep(&mut self) {
        let before = self.fonts.len();
        self.fonts.retain(|_, font| font.blob.strong_count() > 0);
        if self.fonts.len() == before {
            return;
        }
        let live: HashSet<u32> = self.fonts.values().map(|font| font.id.0).collect();
        self.current.retain(|key, _| live.contains(&key.font.0));
        self.previous.retain(|key, _| live.contains(&key.font.0));
        self.bytes = self.current.values().map(|cached| cost(cached)).sum();
    }

    fn rotate(&mut self) {
        self.sweep();
        self.previous = std::mem::take(&mut self.current);
        self.bytes = 0;
    }
}

fn cost(cached: &Cached) -> usize {
    ENTRY_OVERHEAD
        + match cached {
            Cached::Drawn(mask) => mask.alpha.capacity(),
            Cached::Undrawn => 0,
        }
}

thread_local! {
    static CACHE: RefCell<GlyphCache> = RefCell::new(GlyphCache::default());
}

/// Hit and miss counts of this thread's cache.
#[cfg(test)]
pub(super) fn stats() -> (u64, u64) {
    CACHE.with(|cache| {
        let cache = cache.borrow();
        (cache.hits, cache.misses)
    })
}

/// Entries in the current and previous generation, and fonts known.
#[cfg(test)]
pub(super) fn sizes() -> (usize, usize, usize) {
    CACHE.with(|cache| {
        let cache = cache.borrow();
        (cache.current.len(), cache.previous.len(), cache.fonts.len())
    })
}

/// Empties this thread's cache and sets the generation budget.
#[cfg(test)]
pub(super) fn reset(budget: usize) {
    CACHE.with(|cache| {
        *cache.borrow_mut() = GlyphCache {
            budget,
            ..GlyphCache::default()
        };
    });
}

/// Font size to the extra width a faux-bold outline gains.
pub(super) fn embolden_width(size: f32) -> f32 {
    size * EMBOLDEN_EM_FRACTION
}

/// The cache's id for `font`, assigned on first sight. One call per run keeps
/// the font's hash off the per-glyph path.
pub(super) fn font_id(font: &FontRef) -> FontId {
    let key = FontKey {
        blob: Arc::as_ptr(&font.data) as usize,
        index: font.index,
        coords: Arc::clone(&font.coords),
    };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.fonts.get(&key) {
            return entry.id;
        }
        let id = FontId(cache.next_font);
        cache.next_font += 1;
        let blob = Arc::downgrade(&font.data);
        cache.fonts.insert(key, FontEntry { id, blob });
        id
    })
}

pub(super) struct Request<'a> {
    pub(super) font: &'a FontRef,
    pub(super) font_id: FontId,
    pub(super) glyph: u32,
    /// Device pixels per em.
    pub(super) size: f32,
    pub(super) quarter: u8,
    pub(super) bold: bool,
    pub(super) skew: f32,
}

/// The mask for `request`, rasterising it on a miss.
pub(super) fn lookup<'a>(
    request: &Request<'a>,
    outliner: &mut Option<Option<Outliner<'a>>>,
) -> Rc<Cached> {
    let key = Key {
        font: request.font_id,
        glyph: request.glyph,
        size_bits: request.size.to_bits(),
        quarter: request.quarter,
        bold: request.bold,
        skew_bits: request.skew.to_bits(),
    };
    let found = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let mut found = cache.current.get(&key).cloned();
        if found.is_none()
            && let Some(old) = cache.previous.remove(&key)
        {
            cache.bytes += cost(&old);
            cache.current.insert(key.clone(), Rc::clone(&old));
            found = Some(old);
        }
        if found.is_some() {
            cache.hits += 1;
        } else {
            cache.misses += 1;
        }
        found
    });
    if let Some(found) = found {
        return found;
    }
    let outliner = outliner.get_or_insert_with(|| Outliner::new(request.font));
    let built = Rc::new(match outliner {
        Some(outliner) => rasterise(outliner, request),
        None => Cached::Undrawn,
    });
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.misses % SWEEP_INTERVAL == 0 {
            cache.sweep();
        }
        if cache.bytes + cost(&built) > cache.budget {
            cache.rotate();
        }
        cache.bytes += cost(&built);
        cache.current.insert(key, Rc::clone(&built));
    });
    built
}

fn rasterise(outliner: &Outliner<'_>, request: &Request) -> Cached {
    let path = match outliner.path(request.glyph, request.size, request.skew) {
        Ok(Some(path)) => path,
        Ok(None) => return empty(),
        Err(NoOutline) => return Cached::Undrawn,
    };
    let grow = if request.bold {
        embolden_width(request.size)
    } else {
        0.0
    };
    let offset = f32::from(request.quarter) / SUBPIXEL_STEPS as f32;
    let bounds = path.bounds();
    // The widened outline grows by grow / 2 each way and is then shifted by
    // grow / 2 right and up, which FreeType does to keep the left and bottom
    // edges where the regular glyph has them.
    let pad = grow + 1.0;
    let left = (bounds.left() + offset - pad).floor();
    let top = (bounds.top() - pad).floor();
    let right = (bounds.right() + offset + pad).ceil();
    let bottom = (bounds.bottom() + pad).ceil();
    let (width, height) = (right - left, bottom - top);
    if !(width.is_finite() && height.is_finite()) || width > 4096.0 || height > 4096.0 {
        return Cached::Undrawn;
    }
    let transform = Transform::from_translate(offset - left + grow / 2.0, -top - grow / 2.0);
    let Some(alpha) = coverage(
        &path,
        transform,
        (width as u32, height as u32),
        request.bold.then_some(grow),
    ) else {
        return empty();
    };
    Cached::Drawn(Mask {
        left: left as i32,
        top: top as i32,
        width: width as u32,
        height: height as u32,
        alpha,
    })
}

fn empty() -> Cached {
    Cached::Drawn(Mask {
        left: 0,
        top: 0,
        width: 0,
        height: 0,
        alpha: Vec::new(),
    })
}

/// Anti-aliased coverage of `path` under `transform` on a `size` grid. With
/// `embolden`, the outline also grows by that width: the stroke goes to its
/// own pixmap and the two coverages combine by maximum, because compositing
/// the stroke over the fill would darken the edges both cover.
pub(super) fn coverage(
    path: &tiny_skia::Path,
    transform: Transform,
    size: (u32, u32),
    embolden: Option<f32>,
) -> Option<Vec<u8>> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let mut filled = Pixmap::new(size.0, size.1)?;
    filled.fill_path(path, &paint, tiny_skia::FillRule::Winding, transform, None);
    let mut alpha: Vec<u8> = filled
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| px[3])
        .collect();
    if let Some(width) = embolden {
        let mut stroked = Pixmap::new(size.0, size.1)?;
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        stroked.stroke_path(path, &paint, &stroke, transform, None);
        for (a, px) in alpha.iter_mut().zip(stroked.data().as_chunks::<4>().0) {
            *a = (*a).max(px[3]);
        }
    }
    Some(alpha)
}
