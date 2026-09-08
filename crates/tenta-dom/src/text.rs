//! Immutable text rope chunks (`docs/spec/02-architecture.md` §7,
//! `docs/spec/04-dom-and-css.md` §2).
//!
//! Text nodes reference `(chunk, range)` in sealed `Arc<TextChunk>`s of 4-8 KB.
//! The parser appends into an open buffer; when the buffer reaches
//! [`CHUNK_TARGET`] it is sealed into an `Arc` and the text nodes created
//! while it was open are patched to point at it. Because chunks are immutable
//! once sealed, mutating a text node writes a fresh copy into the open buffer
//! and releases the old chunk reference: copy-on-write at chunk granularity,
//! so an external JavaScript string holding an `Arc` clone can never observe
//! its bytes change.
//!
//! ## Growing text nodes
//!
//! Source text reaches a node in pieces: `\r\n` normalisation flushes a
//! segment per CR, every character reference splits the run around it, every
//! CDATA section and every `feed()` boundary splits it again. Appending to the
//! open buffer is free only while the node's bytes are the tail of that
//! buffer; once the buffer is sealed the node's bytes live in an immutable
//! chunk and the next append would have to copy the whole node forward, which
//! makes a single long text node quadratic in its own length.
//!
//! So a node whose bytes are no longer at the tail is **promoted**: it gets an
//! owned, growable buffer of its own ([`TextRope::promote`]) and every later
//! append to it is a `String::push_str`. A node is promoted at most once, so
//! the bytes of a document are copied at most twice in total no matter how
//! many pieces they arrive in.

use std::sync::{Arc, OnceLock};

use tenta_core::NodeId;

/// Seal the open buffer once it reaches this many bytes.
pub const CHUNK_TARGET: usize = 4096;
/// Never let the open buffer grow past this before sealing, except for a
/// single run that is larger than the whole budget.
pub const CHUNK_MAX: usize = 8192;

const _: () = assert!(
    CHUNK_TARGET <= CHUNK_MAX,
    "the seal threshold must fit the chunk budget"
);

/// A sealed run of document text.
#[derive(Debug)]
pub struct TextChunk {
    data: Box<str>,
    is_latin1: bool,
}

impl TextChunk {
    /// The chunk's bytes.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.data
    }

    /// Whether every scalar value in the chunk is U+0000..=U+00FF, so a
    /// one-byte external string can be built without transcoding.
    #[inline]
    pub fn is_latin1(&self) -> bool {
        self.is_latin1
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The shared zero-length chunk used by empty and freed text nodes.
    pub fn empty() -> Arc<TextChunk> {
        static EMPTY: OnceLock<Arc<TextChunk>> = OnceLock::new();
        EMPTY
            .get_or_init(|| {
                Arc::new(TextChunk {
                    data: String::new().into_boxed_str(),
                    is_latin1: true,
                })
            })
            .clone()
    }

    /// The sentinel every text node points at while its bytes still live in
    /// the open buffer. Identity is checked with [`Arc::ptr_eq`].
    pub(crate) fn open_sentinel() -> Arc<TextChunk> {
        static OPEN: OnceLock<Arc<TextChunk>> = OnceLock::new();
        OPEN.get_or_init(|| {
            Arc::new(TextChunk {
                data: String::new().into_boxed_str(),
                is_latin1: true,
            })
        })
        .clone()
    }

    /// The sentinel a promoted text node points at while its bytes live in
    /// its own growable buffer; the node's `range.start` is the buffer index.
    pub(crate) fn owned_sentinel() -> Arc<TextChunk> {
        static OWNED: OnceLock<Arc<TextChunk>> = OnceLock::new();
        OWNED
            .get_or_init(|| {
                Arc::new(TextChunk {
                    data: String::new().into_boxed_str(),
                    is_latin1: true,
                })
            })
            .clone()
    }
}

/// True when every scalar value of `s` fits in one byte.
fn latin1(s: &str) -> bool {
    s.is_ascii() || s.chars().all(|c| (c as u32) <= 0xFF)
}

/// One promoted text node's own buffer.
#[derive(Default)]
struct OwnedText {
    data: String,
    is_latin1: bool,
}

/// The document's rope: one open buffer, the text nodes that point into it,
/// and the owned buffers of promoted nodes. Sealed chunks are owned by the
/// text nodes themselves.
#[derive(Default)]
pub struct TextRope {
    open: String,
    open_is_latin1: bool,
    /// Text nodes whose range refers to `open`; patched when it is sealed.
    pending: Vec<NodeId>,
    /// Buffers of promoted nodes, addressed by their `range.start`.
    owned: Vec<OwnedText>,
    free_owned: Vec<u32>,
    bytes_written: usize,
    sealed_chunks: usize,
}

/// Where a freshly appended run of text landed.
#[derive(Clone, Copy, Debug)]
pub struct TextSpan {
    pub start: u32,
    pub len: u32,
}

impl TextRope {
    pub fn new() -> Self {
        Self {
            open: String::with_capacity(CHUNK_TARGET),
            open_is_latin1: true,
            ..Default::default()
        }
    }

    /// Bytes currently in the open buffer.
    #[inline]
    pub fn open_len(&self) -> usize {
        self.open.len()
    }

    #[inline]
    pub fn open_str(&self) -> &str {
        &self.open
    }

    /// Number of chunks sealed so far.
    #[inline]
    pub fn sealed_chunks(&self) -> usize {
        self.sealed_chunks
    }

    /// Every byte ever written into the rope, including the copy a promotion
    /// makes. Diagnostic only: it never shrinks and is not a memory figure
    /// (`docs/spec/04-dom-and-css.md` §8 counts live bytes).
    #[inline]
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Bytes held by the buffers of promoted text nodes.
    pub fn owned_bytes(&self) -> usize {
        self.owned.iter().map(|buffer| buffer.data.len()).sum()
    }

    /// Number of promoted text nodes.
    #[inline]
    pub fn owned_buffers(&self) -> usize {
        self.owned.len() - self.free_owned.len()
    }

    /// Appends `s` to the open buffer, returning its span.
    pub fn append(&mut self, s: &str) -> TextSpan {
        let start = self.open.len() as u32;
        self.open.push_str(s);
        self.bytes_written += s.len();
        if self.open_is_latin1 && !latin1(s) {
            self.open_is_latin1 = false;
        }
        TextSpan {
            start,
            len: s.len() as u32,
        }
    }

    /// Gives a text node its own growable buffer, seeded with `existing`.
    /// The returned span addresses the buffer, not the open one.
    pub fn promote(&mut self, existing: &str, added: &str) -> TextSpan {
        let mut data = String::with_capacity((existing.len() + added.len()).next_power_of_two());
        data.push_str(existing);
        data.push_str(added);
        self.bytes_written += data.len();
        let is_latin1 = latin1(&data);
        let len = data.len() as u32;
        let buffer = OwnedText { data, is_latin1 };
        let index = match self.free_owned.pop() {
            Some(index) => {
                self.owned[index as usize] = buffer;
                index
            }
            None => {
                let index = u32::try_from(self.owned.len()).expect("owned text table overflow");
                self.owned.push(buffer);
                index
            }
        };
        TextSpan { start: index, len }
    }

    /// Appends to a promoted node's own buffer, returning its new length.
    pub fn extend_owned(&mut self, index: u32, text: &str) -> u32 {
        let buffer = &mut self.owned[index as usize];
        buffer.data.push_str(text);
        if buffer.is_latin1 && !latin1(text) {
            buffer.is_latin1 = false;
        }
        self.bytes_written += text.len();
        u32::try_from(buffer.data.len()).expect("text node longer than 4 GiB")
    }

    /// Replaces a promoted node's own buffer with `text`.
    pub fn set_owned(&mut self, index: u32, text: &str) -> u32 {
        let buffer = &mut self.owned[index as usize];
        buffer.data.clear();
        buffer.data.push_str(text);
        buffer.is_latin1 = latin1(text);
        self.bytes_written += text.len();
        u32::try_from(buffer.data.len()).expect("text node longer than 4 GiB")
    }

    /// The bytes of a promoted node.
    #[inline]
    pub fn owned_str(&self, index: u32) -> &str {
        &self.owned[index as usize].data
    }

    /// Whether a promoted node's bytes are all Latin-1.
    #[inline]
    pub fn owned_is_latin1(&self, index: u32) -> bool {
        self.owned[index as usize].is_latin1
    }

    /// Seals a promoted node's buffer into an immutable chunk, releasing the
    /// buffer. The node must be re-pointed at the returned chunk.
    pub fn seal_owned(&mut self, index: u32) -> Arc<TextChunk> {
        let buffer = std::mem::take(&mut self.owned[index as usize]);
        self.free_owned.push(index);
        Arc::new(TextChunk {
            data: buffer.data.into_boxed_str(),
            is_latin1: buffer.is_latin1,
        })
    }

    /// Releases a promoted node's buffer when the node is freed or rewritten.
    pub fn release_owned(&mut self, index: u32) {
        self.owned[index as usize] = OwnedText::default();
        self.free_owned.push(index);
    }

    /// Registers `node` as pointing into the open buffer. Entries are not
    /// removed when a node is freed; [`TextRope::seal`] hands the list back
    /// and the caller re-checks each node before patching it.
    pub fn register_pending(&mut self, node: NodeId) {
        self.pending.push(node);
    }

    /// Whether the open buffer has grown enough that it should be sealed.
    #[inline]
    pub fn should_seal(&self) -> bool {
        self.open.len() >= CHUNK_TARGET
    }

    /// Seals the open buffer into an `Arc<TextChunk>` and returns it with the
    /// nodes that must be re-pointed at it. Returns `None` when the buffer is
    /// empty.
    #[must_use]
    pub fn seal(&mut self) -> Option<(Arc<TextChunk>, Vec<NodeId>)> {
        if self.open.is_empty() {
            self.pending.clear();
            return None;
        }
        let data = std::mem::replace(&mut self.open, String::with_capacity(CHUNK_TARGET));
        let is_latin1 = std::mem::replace(&mut self.open_is_latin1, true);
        self.sealed_chunks += 1;
        let chunk = Arc::new(TextChunk {
            data: data.into_boxed_str(),
            is_latin1,
        });
        let pending = std::mem::take(&mut self.pending);
        Some((chunk, pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_buffer_seals_into_an_immutable_chunk() {
        let mut rope = TextRope::new();
        let span = rope.append("hello ");
        assert_eq!((span.start, span.len), (0, 6));
        rope.register_pending(NodeId(7));
        let span = rope.append("wörld");
        assert_eq!(span.start, 6);
        let (chunk, pending) = rope.seal().expect("non-empty");
        assert_eq!(chunk.as_str(), "hello wörld");
        assert!(chunk.is_latin1());
        assert_eq!(pending, vec![NodeId(7)]);
        assert_eq!(rope.open_len(), 0);
        assert!(rope.seal().is_none());
    }

    #[test]
    fn non_latin1_text_clears_the_flag() {
        let mut rope = TextRope::new();
        rope.append("ok ");
        rope.append("日本語");
        let (chunk, _) = rope.seal().expect("non-empty");
        assert!(!chunk.is_latin1());
        assert_eq!(chunk.len(), "ok 日本語".len());
    }

    #[test]
    fn seal_threshold_follows_the_chunk_budget() {
        let mut rope = TextRope::new();
        assert!(!rope.should_seal());
        rope.append(&"a".repeat(CHUNK_TARGET - 1));
        assert!(!rope.should_seal());
        rope.append("a");
        assert!(rope.should_seal());
    }

    #[test]
    fn empty_chunk_is_shared() {
        let a = TextChunk::empty();
        let b = TextChunk::empty();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.is_empty());
        assert!(!Arc::ptr_eq(
            &TextChunk::empty(),
            &TextChunk::open_sentinel()
        ));
        assert!(!Arc::ptr_eq(
            &TextChunk::owned_sentinel(),
            &TextChunk::open_sentinel()
        ));
    }

    #[test]
    fn a_promoted_node_grows_in_place_and_seals_into_one_chunk() {
        let mut rope = TextRope::new();
        let span = rope.promote("start", "-more");
        assert_eq!(rope.owned_str(span.start), "start-more");
        assert_eq!(span.len as usize, "start-more".len());
        assert_eq!(rope.owned_buffers(), 1);

        // U+00E9 still fits Latin-1, so the flag survives the append.
        let len = rope.extend_owned(span.start, "-é");
        assert_eq!(rope.owned_str(span.start), "start-more-é");
        assert_eq!(len as usize, "start-more-é".len());
        assert!(rope.owned_is_latin1(span.start));
        assert_eq!(rope.extend_owned(span.start, "日"), 16);
        assert!(!rope.owned_is_latin1(span.start));

        let chunk = rope.seal_owned(span.start);
        assert_eq!(chunk.as_str(), "start-more-é日");
        assert_eq!(rope.owned_buffers(), 0);
        // The slot is reused rather than grown.
        let again = rope.promote("x", "y");
        assert_eq!(again.start, span.start);
        assert_eq!(rope.owned_str(again.start), "xy");
    }

    #[test]
    fn bytes_written_counts_copies_and_is_not_a_memory_figure() {
        let mut rope = TextRope::new();
        rope.append("abcd");
        assert_eq!(rope.bytes_written(), 4);
        let span = rope.promote("abcd", "e");
        assert_eq!(rope.bytes_written(), 9, "the promotion copied 5 bytes");
        assert_eq!(rope.owned_bytes(), 5);
        rope.release_owned(span.start);
        assert_eq!(rope.owned_bytes(), 0);
        assert_eq!(rope.bytes_written(), 9, "the counter never shrinks");
    }
}
