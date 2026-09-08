//! The tokenizer's view of the decoded input.
//!
//! The parser hands the tokenizer one `&str` per chunk and the tokenizer
//! consumes it with a cursor. Text tokens are slices of that `&str`, so no
//! copy happens between the decoder and the tree sink
//! (`docs/spec/02-architecture.md` §7). The cursor can move backwards, which
//! is how the character-reference sub-tokenizer un-consumes a name it could
//! not match.

/// A chunk of decoded input with a cursor.
#[derive(Debug)]
pub(crate) struct Input<'a> {
    text: &'a str,
    pos: usize,
    at_eof: bool,
}

impl<'a> Input<'a> {
    /// A new cursor over `text`, starting at `pos` bytes (the prefix a
    /// previous chunk retained so the cursor can still move backwards).
    pub(crate) fn new(text: &'a str, pos: usize, at_eof: bool) -> Input<'a> {
        debug_assert!(text.is_char_boundary(pos));
        Input { text, pos, at_eof }
    }

    /// The whole chunk, independent of the cursor borrow.
    #[inline]
    pub(crate) fn text(&self) -> &'a str {
        self.text
    }

    /// Byte offset of the cursor.
    #[inline]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Whether this is the last chunk of the document.
    #[inline]
    pub(crate) fn at_eof(&self) -> bool {
        self.at_eof
    }

    /// The not-yet-consumed remainder.
    #[inline]
    pub(crate) fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    /// The next character without consuming it.
    #[inline]
    pub(crate) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Consumes and returns the next character.
    #[inline]
    pub(crate) fn next_char(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Consumes `len` bytes, which must end on a character boundary.
    #[inline]
    pub(crate) fn advance(&mut self, len: usize) {
        self.pos += len;
        debug_assert!(self.text.is_char_boundary(self.pos));
    }

    /// Moves the cursor back `len` bytes, un-consuming text that was consumed
    /// verbatim (no newline normalisation), which is the only case where the
    /// exact bytes are known.
    #[inline]
    pub(crate) fn rewind(&mut self, len: usize) {
        debug_assert!(self.pos >= len);
        self.pos -= len;
        debug_assert!(self.text.is_char_boundary(self.pos));
    }
}
