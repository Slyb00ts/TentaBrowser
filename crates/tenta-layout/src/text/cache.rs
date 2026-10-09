//! Shaped paragraphs kept between calls.
//!
//! Layout asks about the same paragraph several times: min-content, max-content
//! and the real width. Shaping dominates the cost and does not depend on the
//! width, so the shaped layout is kept and only the line breaking is redone.
//!
//! The cache is per thread because the font system that owns the shaping
//! contexts is, and it never holds output that depends on iteration order: an
//! entry is found by comparing its full key, and eviction is oldest first.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};

use parley::Layout;

use super::style::{Paragraph, TextStyle};
use crate::types::Direction;

/// The brush parley carries per cluster. It holds the index of the span the
/// cluster belongs to, which is how styles are found again after shaping.
pub(super) type Brush = [u8; 4];

/// The most entries kept.
const MAX_ENTRIES: usize = 128;
/// The estimated heap bytes kept across entries (about 320 KB of text).
const MAX_BYTES: usize = 32 << 20;
/// Heap bytes a shaped layout holds per byte of its text. Measured with a
/// counting allocator on English text in Liberation Sans: 129x at 75 bytes,
/// 98x at 741 and 100x at 5 KB, including the key. The layout's tables are
/// private to parley, so the cost is estimated from the text length.
const BYTES_PER_TEXT_BYTE: usize = 100;

/// What decides a shaped layout. Two paragraphs with equal keys shape alike.
struct Key {
    text: Box<str>,
    direction: Direction,
    /// Whether `overflow-wrap: break-word` was shaped as `normal`.
    ignore_break_word: bool,
    spans: Vec<(usize, TextStyle)>,
}

impl Key {
    fn matches(&self, paragraph: &Paragraph<'_>, ignore_break_word: bool) -> bool {
        *self.text == *paragraph.text
            && self.direction == paragraph.direction
            && self.ignore_break_word == ignore_break_word
            && self.spans.len() == paragraph.spans.len()
            && self
                .spans
                .iter()
                .zip(paragraph.spans)
                .all(|((end, style), span)| *end == span.range.end && style == span.style)
    }
}

fn hash_of(paragraph: &Paragraph<'_>, ignore_break_word: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    paragraph.text.hash(&mut hasher);
    (paragraph.direction as u8).hash(&mut hasher);
    ignore_break_word.hash(&mut hasher);
    for span in paragraph.spans {
        span.range.end.hash(&mut hasher);
        let style = span.style;
        style.family_list.hash(&mut hasher);
        style.size.to_bits().hash(&mut hasher);
        style.weight.hash(&mut hasher);
        style.italic.hash(&mut hasher);
        style.letter_spacing.to_bits().hash(&mut hasher);
        style.word_spacing.to_bits().hash(&mut hasher);
        style.features.len().hash(&mut hasher);
        style.lang.hash(&mut hasher);
        (style.white_space as u8).hash(&mut hasher);
        (style.line_break as u8).hash(&mut hasher);
        (style.word_break as u8).hash(&mut hasher);
        (style.overflow_wrap as u8).hash(&mut hasher);
    }
    hasher.finish()
}

struct Entry {
    hash: u64,
    cost: usize,
    key: Key,
    layout: Layout<Brush>,
}

/// Counters of the calling thread's cache.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct CacheStats {
    /// Paragraphs found already shaped.
    pub(crate) hits: u64,
    /// Paragraphs that had to be shaped.
    pub(crate) misses: u64,
    /// Paragraphs currently kept.
    pub(crate) entries: usize,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    cost: usize,
    stats: CacheStats,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// The hit and miss counts of the calling thread.
pub(crate) fn stats() -> CacheStats {
    CACHE.with_borrow(|cache| CacheStats {
        entries: cache.entries.len(),
        ..cache.stats
    })
}

/// Whether equal keys can be recognised: a NaN size or spacing never equals
/// itself, so such a paragraph would only fill the cache with unreachable entries.
fn cacheable(paragraph: &Paragraph<'_>) -> bool {
    paragraph.spans.iter().all(|span| {
        let style = span.style;
        style.size.is_finite() && style.letter_spacing.is_finite() && style.word_spacing.is_finite()
    })
}

/// Runs `use_layout` on the shaped layout of `paragraph`, shaping it with
/// `shape` first when no entry matches.
///
/// `use_layout` must not call back into the cache.
pub(super) fn with_layout<R>(
    paragraph: &Paragraph<'_>,
    ignore_break_word: bool,
    shape: impl FnOnce() -> Layout<Brush>,
    use_layout: impl FnOnce(&mut Layout<Brush>) -> R,
) -> R {
    if !cacheable(paragraph) {
        return use_layout(&mut shape());
    }
    let hash = hash_of(paragraph, ignore_break_word);
    CACHE.with_borrow_mut(|cache| {
        let found = cache.entries.iter().position(|entry| {
            entry.hash == hash && entry.key.matches(paragraph, ignore_break_word)
        });
        let index = match found {
            Some(index) => {
                cache.stats.hits += 1;
                index
            }
            None => {
                cache.stats.misses += 1;
                let key = Key {
                    text: paragraph.text.into(),
                    direction: paragraph.direction,
                    ignore_break_word,
                    spans: paragraph
                        .spans
                        .iter()
                        .map(|span| (span.range.end, span.style.clone()))
                        .collect(),
                };
                let cost = key.text.len() * BYTES_PER_TEXT_BYTE;
                cache.cost += cost;
                cache.entries.push_back(Entry {
                    hash,
                    cost,
                    key,
                    layout: shape(),
                });
                while cache.entries.len() > MAX_ENTRIES
                    || (cache.cost > MAX_BYTES && cache.entries.len() > 1)
                {
                    if let Some(evicted) = cache.entries.pop_front() {
                        cache.cost -= evicted.cost;
                    }
                }
                cache.entries.len() - 1
            }
        };
        use_layout(&mut cache.entries[index].layout)
    })
}
