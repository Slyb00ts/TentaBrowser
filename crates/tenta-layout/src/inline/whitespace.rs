//! White-space processing for inline formatting contexts.
//!
//! The items of a context are turned into chunks of processed text, each of
//! which becomes one paragraph for the shaper, and into the entries that sit
//! between chunks: inline box edges, atomic inlines and break opportunities.
//! Every forced break is an entry of its own, so a chunk never contains a
//! segment break.

use std::borrow::Cow;
use std::ops::Range;

use tenta_core::NodeId;
use tenta_dom::Document;

use super::style::{BoxInfo, Handling, Infos};
use crate::box_tree::{BoxId, InlineFormattingContext, InlineItem, TextSource};
use crate::text::transform;

/// How a run of processed bytes maps back to the source text.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MapChunk {
    proc_start: u32,
    proc_len: u32,
    src_start: u32,
    src_len: u32,
}

impl MapChunk {
    fn identity(&self) -> bool {
        self.proc_len == self.src_len
    }
}

/// Offsets of processed text in the source text, as pieces that are either a
/// verbatim copy or an arbitrary rewrite of some source bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SrcMap {
    chunks: Vec<MapChunk>,
    len: u32,
}

impl SrcMap {
    /// Appends `proc_len` processed bytes that stand for `src_len` bytes at
    /// `src_start`.
    fn push(&mut self, proc_len: u32, src_start: u32, src_len: u32) {
        if proc_len == src_len
            && let Some(last) = self.chunks.last_mut()
            && last.identity()
            && last.src_start + last.src_len == src_start
        {
            last.proc_len += proc_len;
            last.src_len += src_len;
        } else {
            self.chunks.push(MapChunk {
                proc_start: self.len,
                proc_len,
                src_start,
                src_len,
            });
        }
        self.len += proc_len;
    }

    /// Drops the last processed byte.
    fn pop_byte(&mut self) {
        let Some(last) = self.chunks.last_mut() else {
            return;
        };
        self.len -= 1;
        if last.proc_len == 1 {
            self.chunks.pop();
        } else {
            last.proc_len -= 1;
            if last.identity() {
                last.src_len -= 1;
            }
        }
    }

    /// The source offset where processed offset `at` begins.
    pub(super) fn start(&self, at: usize) -> u32 {
        let at = at as u32;
        let index = self
            .chunks
            .partition_point(|c| c.proc_start + c.proc_len <= at);
        match self.chunks.get(index) {
            Some(c) if c.identity() => c.src_start + (at - c.proc_start),
            Some(c) => c.src_start,
            None => self.chunks.last().map_or(0, |c| c.src_start + c.src_len),
        }
    }

    /// The source offset where processed offset `at` ends a range.
    pub(super) fn end(&self, at: usize) -> u32 {
        let at = at as u32;
        let index = self
            .chunks
            .partition_point(|c| c.proc_start + c.proc_len < at);
        match self.chunks.get(index) {
            Some(c) if c.identity() => c.src_start + (at - c.proc_start).min(c.src_len),
            Some(c) => c.src_start + if at == c.proc_start { 0 } else { c.src_len },
            None => self.chunks.last().map_or(0, |c| c.src_start + c.src_len),
        }
    }

    /// The map of the processed bytes `from..to`.
    fn slice(&self, from: usize, to: usize) -> SrcMap {
        let (from, to) = (from as u32, to as u32);
        let mut out = SrcMap::default();
        for c in &self.chunks {
            let lo = from.max(c.proc_start);
            let hi = to.min(c.proc_start + c.proc_len);
            if lo >= hi {
                continue;
            }
            if c.identity() {
                out.push(hi - lo, c.src_start + (lo - c.proc_start), hi - lo);
            } else {
                out.push(hi - lo, c.src_start, c.src_len);
            }
        }
        out
    }

    /// Replaces the map by one that spans the same source bytes with `len`
    /// processed bytes, for rewrites that change the length.
    fn coarse(&self, len: usize) -> SrcMap {
        let (Some(first), Some(last)) = (self.chunks.first(), self.chunks.last()) else {
            return SrcMap::default();
        };
        let src_len = last.src_start + last.src_len - first.src_start;
        SrcMap {
            chunks: vec![MapChunk {
                proc_start: 0,
                proc_len: len as u32,
                src_start: first.src_start,
                src_len,
            }],
            len: len as u32,
        }
    }
}

/// A stretch of a chunk's text that one box styles.
#[derive(Clone, Debug)]
pub(super) struct SpanRec {
    pub(super) range: Range<usize>,
    pub(super) box_id: BoxId,
    /// The DOM text node, or `NodeId::NONE` for generated text.
    pub(super) node: NodeId,
    pub(super) map: SrcMap,
    /// A preserved space of `break-spaces`, which must not hang.
    pub(super) lone_space: bool,
}

/// An inline box edge that sits between pieces of text.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum Event {
    Open { box_id: BoxId, resumes: bool },
    Close { box_id: BoxId, interrupted: bool },
}

/// Processed text shaped as one paragraph.
#[derive(Clone, Debug, Default)]
pub(super) struct Chunk {
    pub(super) text: String,
    pub(super) spans: Vec<SpanRec>,
    /// Box edges inside the text: `(byte offset, event)` in source order.
    pub(super) events: Vec<(usize, Event)>,
}

/// One element of the processed inline content, in logical order.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Chunk(usize),
    Event(Event),
    Atomic(BoxId),
    /// A preserved tab: the box that styles it, its text node and its source
    /// bytes.
    Tab(BoxId, NodeId, Range<u32>),
    Forced,
    /// A wrap opportunity without a glyph.
    Soft,
}

/// The white-space processed content of one inline formatting context.
#[derive(Debug, Default)]
pub(super) struct Prepared {
    pub(super) chunks: Vec<Chunk>,
    pub(super) entries: Vec<Entry>,
}

/// Reads the boxes `ifc` mentions into `infos`.
pub(super) fn read_infos(
    cx: &mut crate::box_tree::LayoutCx,
    tree: &crate::box_tree::BoxTree,
    ifc: &InlineFormattingContext,
    infos: &mut Infos,
) {
    infos.ensure(cx, tree, ifc.root);
    for item in &ifc.items {
        match item {
            InlineItem::Text { style_box, .. } => infos.ensure(cx, tree, *style_box),
            InlineItem::OpenInline { box_id, .. }
            | InlineItem::CloseInline { box_id, .. }
            | InlineItem::Atomic { box_id } => infos.ensure(cx, tree, *box_id),
            InlineItem::ForcedBreak { .. }
            | InlineItem::SoftBreak
            | InlineItem::Float { .. }
            | InlineItem::AbsPos { .. } => {}
        }
    }
}

/// Applies CSS Text 3 section 4 to the items of `ifc`.
pub(super) fn prepare(doc: &Document, infos: &Infos, ifc: &InlineFormattingContext) -> Prepared {
    let mut b = Builder::default();
    for item in &ifc.items {
        match item {
            InlineItem::Text { source, style_box } => {
                let info = infos.get(*style_box);
                match source {
                    TextSource::Node { node, range } => {
                        if let Some(data) = doc.text_data(*node)
                            && let Some(slice) = data.get(range.start as usize..range.end as usize)
                        {
                            b.text_item(info, *style_box, *node, slice, range.start);
                        }
                    }
                    TextSource::Owned(text) => {
                        b.text_item(info, *style_box, NodeId::NONE, text, 0);
                    }
                }
            }
            InlineItem::OpenInline { box_id, resumes } => b.event(Event::Open {
                box_id: *box_id,
                resumes: *resumes,
            }),
            InlineItem::CloseInline {
                box_id,
                interrupted,
            } => b.event(Event::Close {
                box_id: *box_id,
                interrupted: *interrupted,
            }),
            InlineItem::Atomic { box_id } => {
                b.end_chunk();
                b.out.entries.push(Entry::Atomic(*box_id));
                b.prev_space = false;
                b.in_word = false;
            }
            InlineItem::ForcedBreak { .. } => b.forced(&mut Seg::default()),
            InlineItem::SoftBreak => {
                b.end_chunk();
                b.out.entries.push(Entry::Soft);
            }
            InlineItem::Float { .. } | InlineItem::AbsPos { .. } => {}
        }
    }
    b.trim_trailing(&mut Seg::default());
    b.end_chunk();
    b.out
}

/// The most bytes shaped as one paragraph.
const CHUNK_LIMIT: usize = 24 * 1024;

/// Processed text of one item that has not been added to the chunk yet.
#[derive(Default)]
struct Seg {
    text: String,
    map: SrcMap,
}

struct Builder {
    out: Prepared,
    cur: Chunk,
    /// The previous character was a collapsible space, so the next one is
    /// removed; also true at the start of a line.
    prev_space: bool,
    /// The last character written is a collapsed space that a line end would
    /// remove.
    trailing: bool,
    /// The text so far ends inside a word, for `capitalize`.
    in_word: bool,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            out: Prepared::default(),
            cur: Chunk::default(),
            prev_space: true,
            trailing: false,
            in_word: false,
        }
    }
}

fn is_segment_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r')
}

impl Builder {
    fn event(&mut self, event: Event) {
        if self.cur.text.is_empty() {
            self.out.entries.push(Entry::Event(event));
        } else {
            self.cur.events.push((self.cur.text.len(), event));
        }
    }

    fn end_chunk(&mut self) {
        self.trailing = false;
        let chunk = std::mem::take(&mut self.cur);
        if chunk.text.is_empty() {
            self.out
                .entries
                .extend(chunk.events.into_iter().map(|(_, e)| Entry::Event(e)));
        } else {
            self.out.entries.push(Entry::Chunk(self.out.chunks.len()));
            self.out.chunks.push(chunk);
        }
    }

    /// A forced break: the collapsible space before it goes, the next line
    /// starts afresh.
    fn forced(&mut self, seg: &mut Seg) {
        self.trim_trailing(seg);
        self.end_chunk();
        self.out.entries.push(Entry::Forced);
        self.prev_space = true;
        self.in_word = false;
    }

    /// Removes a collapsed space at the very end of the text written so far.
    fn trim_trailing(&mut self, seg: &mut Seg) {
        if !self.trailing {
            return;
        }
        self.trailing = false;
        if seg.text.ends_with(' ') {
            seg.text.pop();
            seg.map.pop_byte();
        } else if seg.text.is_empty()
            && self.cur.text.ends_with(' ')
            && let Some(last) = self.cur.spans.last_mut()
        {
            self.cur.text.pop();
            last.range.end -= 1;
            last.map.pop_byte();
            if last.range.is_empty() {
                self.cur.spans.pop();
            }
            let len = self.cur.text.len();
            for (offset, _) in &mut self.cur.events {
                *offset = (*offset).min(len);
            }
        }
    }

    fn flush(&mut self, seg: &mut Seg, info: &BoxInfo, box_id: BoxId, node: NodeId) {
        if seg.text.is_empty() {
            return;
        }
        let seg = std::mem::take(seg);
        let (text, map) = match transform::apply(&seg.text, info.case, &mut self.in_word) {
            Cow::Borrowed(_) => (seg.text, seg.map),
            Cow::Owned(changed) if changed.len() == seg.text.len() => (changed, seg.map),
            Cow::Owned(changed) => {
                let map = seg.map.coarse(changed.len());
                (changed, map)
            }
        };
        let mut text = text.as_str();
        let mut map = map;
        // The shaper's text indices are 16 bits wide, so a very long run is
        // handed over in parts that no wrap opportunity separates.
        while text.len() > CHUNK_LIMIT {
            let mut cut = CHUNK_LIMIT;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            // Cut after the last space when there is one, so that the wrap
            // opportunity after it is not lost; inside a word nothing is.
            let at_space = text[..cut].rfind(' ').map(|i| i + 1);
            cut = at_space.unwrap_or(cut);
            self.push_span(&text[..cut], map.slice(0, cut), box_id, node, false);
            self.end_chunk();
            if at_space.is_some() {
                self.out.entries.push(Entry::Soft);
            }
            map = map.slice(cut, text.len());
            text = &text[cut..];
        }
        self.push_span(text, map, box_id, node, false);
    }

    fn push_span(&mut self, text: &str, map: SrcMap, box_id: BoxId, node: NodeId, lone: bool) {
        if !self.cur.text.is_empty() && self.cur.text.len() + text.len() > CHUNK_LIMIT {
            let after_space = self.cur.text.ends_with(' ');
            self.end_chunk();
            if after_space {
                self.out.entries.push(Entry::Soft);
            }
        }
        let start = self.cur.text.len();
        self.cur.text.push_str(text);
        self.cur.spans.push(SpanRec {
            range: start..start + text.len(),
            box_id,
            node,
            map,
            lone_space: lone,
        });
    }

    fn text_item(&mut self, info: &BoxInfo, box_id: BoxId, node: NodeId, src: &str, base: u32) {
        let mut seg = Seg::default();
        let mut chars = src.char_indices().peekable();
        while let Some((pos, ch)) = chars.next() {
            let at = base + pos as u32;
            let len = ch.len_utf8() as u32;
            let preserves_breaks = info.collapse != Handling::Collapse;
            if is_segment_break(ch) {
                if ch == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
                    continue;
                }
                if preserves_breaks {
                    self.flush(&mut seg, info, box_id, node);
                    self.forced(&mut seg);
                    continue;
                }
            }
            match ch {
                ' ' | '\t' | '\n' | '\r' if info.collapse.collapses_spaces() => {
                    if !self.prev_space {
                        seg.text.push(' ');
                        seg.map.push(1, at, len);
                        self.prev_space = true;
                        self.trailing = true;
                    }
                }
                '\t' => {
                    self.flush(&mut seg, info, box_id, node);
                    self.end_chunk();
                    self.out
                        .entries
                        .push(Entry::Tab(box_id, node, at..at + len));
                    self.prev_space = false;
                    self.in_word = false;
                }
                ' ' if info.collapse == Handling::BreakSpaces => {
                    self.flush(&mut seg, info, box_id, node);
                    self.end_chunk();
                    let mut map = SrcMap::default();
                    map.push(1, at, len);
                    self.push_span(" ", map, box_id, node, true);
                    self.end_chunk();
                    self.out.entries.push(Entry::Soft);
                    self.prev_space = false;
                    self.trailing = false;
                }
                _ => {
                    seg.text.push(ch);
                    seg.map.push(len, at, len);
                    self.prev_space = false;
                    self.trailing = false;
                }
            }
        }
        self.flush(&mut seg, info, box_id, node);
    }
}
