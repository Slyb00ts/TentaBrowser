//! A shaped paragraph as flat arrays, and the cache that keeps it.
//!
//! Inline layout asks for the same text again and again: min-content,
//! max-content, every re-layout at another width, and the same sentence on
//! many pages. Shaping is most of the cost and does not depend on the width,
//! so a paragraph is shaped once into one unbroken line, with the wrap
//! opportunities the line breaker found recorded beside it. Pieces, widths and
//! glyph slices for any line are then read off the arrays without further
//! allocation.
//!
//! The cache is per thread, because the font system that owns the shaping
//! contexts is. It evicts the least recently used entry, and its answers never
//! depend on iteration order.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use super::style::Paragraph;
use crate::font::VerticalMetrics;
use crate::types::{FontRef, Glyph};

/// The face a run of glyphs was shaped in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Face {
    pub(crate) font: FontRef,
    pub(crate) size: f32,
    pub(crate) synthetic_bold: bool,
    pub(crate) skew: f32,
}

/// Clusters of one span in one font.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChunkRun {
    /// Index of the span in the paragraph whose style the run uses.
    pub(crate) span: u32,
    pub(crate) rtl: bool,
    pub(crate) face: Face,
    /// Vertical metrics of the run's font at its size.
    pub(crate) metrics: VerticalMetrics,
}

/// One cluster of the unbroken line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChunkCluster {
    /// Byte range of the cluster in the paragraph text.
    pub(crate) text: Range<u32>,
    /// Distance from the line's left edge to the cluster's left edge.
    pub(crate) x: f32,
    pub(crate) advance: f32,
    /// Glyphs of the cluster, as indices into [`ShapedChunk::glyphs`].
    pub(crate) glyphs: Range<u32>,
    /// Index into [`ShapedChunk::runs`].
    pub(crate) run: u32,
}

/// A wrap opportunity: a line may end after the byte `end`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Break {
    pub(crate) end: u32,
    /// The opportunity lies inside a word and exists only because the style
    /// asks for `overflow-wrap: anywhere`.
    pub(crate) inside_word: bool,
}

/// A paragraph shaped as one line.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShapedChunk {
    pub(crate) runs: Vec<ChunkRun>,
    /// Clusters in visual order, left to right.
    pub(crate) clusters: Vec<ChunkCluster>,
    /// Glyphs in visual order; `x` is measured from the line's left edge and
    /// `y` is positive downward from the baseline.
    pub(crate) glyphs: Vec<Glyph>,
    /// Wrap opportunities in logical order, without the end of the text.
    pub(crate) breaks: Vec<Break>,
    /// Width of the line.
    pub(crate) width: f32,
    pub(crate) all_rtl: bool,
    pub(crate) any_rtl: bool,
}

impl ShapedChunk {
    /// The distance from the left edge of the line to the left edge of cluster
    /// `index`, or to the end of the line when `index` is past the last one.
    pub(crate) fn x_at(&self, index: usize) -> f32 {
        self.clusters.get(index).map_or(self.width, |c| c.x)
    }

    /// The clusters of the byte range `start..end`, as indices into
    /// [`ShapedChunk::clusters`]. The range is exact when the paragraph is
    /// shaped in one direction; for mixed text it covers the clusters of the
    /// range and whatever lies visually between them.
    pub(crate) fn cluster_range(&self, start: u32, end: u32) -> Range<usize> {
        if !self.any_rtl {
            let first = self.clusters.partition_point(|c| c.text.start < start);
            let last = self.clusters.partition_point(|c| c.text.start < end);
            return first..last;
        }
        if self.all_rtl {
            let first = self.clusters.partition_point(|c| c.text.start >= end);
            let last = self.clusters.partition_point(|c| c.text.start >= start);
            return first..last;
        }
        let mut inside = self
            .clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.text.start >= start && c.text.start < end)
            .map(|(i, _)| i);
        match (inside.next(), inside.next_back()) {
            (Some(first), Some(last)) => first..last + 1,
            (Some(only), None) => only..only + 1,
            _ => 0..0,
        }
    }

    /// The width of a cluster range from [`ShapedChunk::cluster_range`].
    pub(crate) fn width_of(&self, clusters: &Range<usize>) -> f32 {
        if clusters.is_empty() {
            0.0
        } else {
            self.x_at(clusters.end) - self.x_at(clusters.start)
        }
    }

    /// The runs that the clusters of a cluster range belong to.
    pub(crate) fn runs_of(&self, clusters: &Range<usize>) -> Range<usize> {
        // Runs are numbered in the order their clusters are laid out.
        match (
            self.clusters.get(clusters.start),
            clusters
                .end
                .checked_sub(1)
                .and_then(|i| self.clusters.get(i)),
        ) {
            (Some(first), Some(last)) => first.run as usize..last.run as usize + 1,
            _ => 0..0,
        }
    }

    fn cost(&self) -> usize {
        size_of::<ShapedChunk>()
            + self.runs.len() * size_of::<ChunkRun>()
            + self.clusters.len() * size_of::<ChunkCluster>()
            + self.glyphs.len() * size_of::<Glyph>()
            + self.breaks.len() * size_of::<Break>()
    }
}

/// The most entries kept.
const MAX_ENTRIES: usize = if cfg!(test) { 64 } else { 16_384 };
/// The estimated heap bytes kept across entries.
const MAX_BYTES: usize = 48 << 20;

/// What decides a shaped paragraph, as two independent 64-bit digests of its
/// direction, span ends and every style field, plus the text itself.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
struct Digest(u64, u64);

fn digest(paragraph: &Paragraph<'_>, fonts: u8) -> Digest {
    let mut halves = [DefaultHasher::new(), DefaultHasher::new()];
    for (seed, hasher) in halves.iter_mut().enumerate() {
        seed.hash(hasher);
        fonts.hash(hasher);
        paragraph.text.hash(hasher);
        (paragraph.direction as u8).hash(hasher);
        for span in paragraph.spans {
            span.range.end.hash(hasher);
            let style = span.style;
            style.family_list.hash(hasher);
            style.size.to_bits().hash(hasher);
            style.weight.hash(hasher);
            style.italic.hash(hasher);
            style.letter_spacing.to_bits().hash(hasher);
            style.word_spacing.to_bits().hash(hasher);
            for feature in &style.features {
                feature.tag.hash(hasher);
                feature.value.hash(hasher);
            }
            style.lang.hash(hasher);
            (style.white_space as u8).hash(hasher);
            (style.line_break as u8).hash(hasher);
            (style.word_break as u8).hash(hasher);
            (style.overflow_wrap as u8).hash(hasher);
        }
    }
    Digest(halves[0].finish(), halves[1].finish())
}

struct Entry {
    text: Box<str>,
    chunk: Rc<ShapedChunk>,
    cost: usize,
    used: u64,
}

/// Counters of the calling thread's cache.
#[cfg(test)]
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct CacheStats {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) entries: usize,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Digest, Entry>,
    cost: usize,
    clock: u64,
    #[cfg(test)]
    stats: CacheStats,
}

impl Cache {
    /// Drops the least recently used entries until a quarter of the room is
    /// free again, so that evicting does not run on every insertion.
    fn evict(&mut self) {
        if self.entries.len() <= MAX_ENTRIES && self.cost <= MAX_BYTES {
            return;
        }
        let mut by_use: Vec<(u64, Digest)> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        by_use.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in by_use {
            if self.entries.len() <= MAX_ENTRIES * 3 / 4 && self.cost <= MAX_BYTES * 3 / 4 {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.cost -= entry.cost;
            }
        }
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// The hit and miss counts of the calling thread.
#[cfg(test)]
pub(crate) fn stats() -> CacheStats {
    CACHE.with_borrow(|cache| CacheStats {
        entries: cache.entries.len(),
        ..cache.stats
    })
}

/// Whether equal keys can be recognised: a NaN size or spacing never equals
/// itself, so such a paragraph would only fill the cache with unreachable
/// entries.
fn cacheable(paragraph: &Paragraph<'_>) -> bool {
    paragraph.spans.iter().all(|span| {
        let style = span.style;
        style.size.is_finite() && style.letter_spacing.is_finite() && style.word_spacing.is_finite()
    })
}

/// The shaped chunk of `paragraph`, from the cache or from `shape`.
///
/// `fonts` is [`FontSystem::cache_scope`]: chunks shaped against different font
/// databases must not share entries.
pub(super) fn cached(
    paragraph: &Paragraph<'_>,
    fonts: u8,
    shape: impl FnOnce() -> ShapedChunk,
) -> Rc<ShapedChunk> {
    if !cacheable(paragraph) {
        return Rc::new(shape());
    }
    let key = digest(paragraph, fonts);
    if let Some(found) = CACHE.with_borrow_mut(|cache| {
        cache.clock += 1;
        let clock = cache.clock;
        let hit = cache
            .entries
            .get_mut(&key)
            .filter(|entry| *entry.text == *paragraph.text)
            .map(|entry| {
                entry.used = clock;
                entry.chunk.clone()
            });
        #[cfg(test)]
        if hit.is_some() {
            cache.stats.hits += 1;
        }
        hit
    }) {
        return found;
    }
    let chunk = Rc::new(shape());
    CACHE.with_borrow_mut(|cache| {
        #[cfg(test)]
        {
            cache.stats.misses += 1;
        }
        let cost = chunk.cost() + paragraph.text.len();
        let used = cache.clock;
        if let Some(old) = cache.entries.insert(
            key,
            Entry {
                text: paragraph.text.into(),
                chunk: chunk.clone(),
                cost,
                used,
            },
        ) {
            cache.cost -= old.cost;
        }
        cache.cost += cost;
        cache.evict();
    });
    chunk
}
