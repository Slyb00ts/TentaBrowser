// Derived from html5ever's `src/tokenizer/char_ref/mod.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The character reference sub-tokenizer.
//!
//! Named references are matched against a sorted static table
//! ([`super::entities::NAMED_ENTITIES`]) by narrowing a candidate range one
//! byte at a time, which needs no allocation and finds the longest match the
//! spec asks for. The first byte is looked up in an index instead of searched,
//! because that first step would otherwise binary-search the whole table and
//! dominate the cost of every character reference (see [`first_byte_ranges`]).

use std::borrow::Cow::Borrowed;
use std::char::from_u32;
use std::sync::OnceLock;

use crate::input::Input;
use crate::tokenizer::Tokenizer;
use crate::tokenizer::entities::{MAX_ENTITY_NAME_LEN, NAMED_ENTITIES};
use crate::tokenizer::interface::TokenSink;

/// Replacements for numeric references in the C1 range, per the
/// "numeric character reference end state" table.
static C1_REPLACEMENTS: [Option<char>; 32] = [
    Some('\u{20ac}'),
    None,
    Some('\u{201a}'),
    Some('\u{0192}'),
    Some('\u{201e}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02c6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    None,
    Some('\u{017d}'),
    None,
    None,
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201c}'),
    Some('\u{201d}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02dc}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203a}'),
    Some('\u{0153}'),
    None,
    Some('\u{017e}'),
    Some('\u{0178}'),
];

/// The characters a reference expanded to.
pub(super) struct CharRef {
    pub(super) chars: [char; 2],
    pub(super) num_chars: u8,
}

impl CharRef {
    const EMPTY: CharRef = CharRef {
        chars: ['\0', '\0'],
        num_chars: 0,
    };
}

/// Whether the sub-tokenizer made progress, needs input, or finished.
pub(super) enum Status {
    Stuck,
    Progress,
    Done(CharRef),
}

#[derive(Debug)]
enum State {
    Begin,
    Octothorpe,
    Numeric(u32),
    NumericSemicolon,
    Named,
    BogusName,
}

/// State of one in-progress character reference.
pub(super) struct CharRefTokenizer {
    state: State,
    is_consumed_in_attribute: bool,
    num: u32,
    num_too_big: bool,
    seen_digit: bool,
    hex_marker: Option<char>,
    name_buf: String,
    /// Candidate range in [`NAMED_ENTITIES`] that still shares `name_buf` as a
    /// prefix.
    range: (usize, usize),
    name_match: Option<(u32, u32)>,
    name_len: usize,
}

/// The byte of `name` at `index`, or `None` past the end. `None` sorts first,
/// which matches the table's byte order.
#[inline]
fn byte_at(name: &str, index: usize) -> Option<u8> {
    name.as_bytes().get(index).copied()
}

/// For each ASCII byte, the half-open range of [`NAMED_ENTITIES`] whose names
/// start with it.
///
/// Narrowing by binary search costs two `partition_point`s over the candidate
/// range, and at depth 0 that range is the whole table: about twenty scattered
/// reads through fifty kilobytes of names before a single character of the
/// reference has been consumed. Entity names are ASCII and the table is
/// sorted, so one linear pass at first use replaces that step with an array
/// read. The pass is O(table) once per process; the table is static, so the
/// result is too.
fn first_byte_ranges() -> &'static [(u32, u32); 128] {
    static RANGES: OnceLock<[(u32, u32); 128]> = OnceLock::new();
    RANGES.get_or_init(|| {
        let mut ranges = [(0u32, 0u32); 128];
        for (index, (name, _, _)) in NAMED_ENTITIES.iter().enumerate() {
            let Some(&byte) = name.as_bytes().first() else {
                continue;
            };
            debug_assert!(byte.is_ascii(), "entity names are ASCII");
            let slot = &mut ranges[byte as usize];
            if slot.0 == slot.1 {
                *slot = (index as u32, index as u32 + 1);
            } else {
                debug_assert_eq!(
                    slot.1, index as u32,
                    "the entity table must be sorted by name"
                );
                slot.1 = index as u32 + 1;
            }
        }
        ranges
    })
}

impl CharRefTokenizer {
    pub(super) fn new(is_consumed_in_attribute: bool) -> CharRefTokenizer {
        CharRefTokenizer {
            state: State::Begin,
            is_consumed_in_attribute,
            num: 0,
            num_too_big: false,
            seen_digit: false,
            hex_marker: None,
            name_buf: String::new(),
            range: (0, NAMED_ENTITIES.len()),
            name_match: None,
            name_len: 0,
        }
    }

    /// How many bytes before the cursor the parser must keep so this
    /// sub-tokenizer can still un-consume them across a chunk boundary.
    pub(super) fn rewind_reserve(&self) -> usize {
        match self.state {
            State::Begin => 0,
            State::Octothorpe => 1,
            State::Numeric(_) | State::NumericSemicolon => {
                1 + usize::from(self.hex_marker.is_some())
            }
            // Bounded by the longest entity name plus the one character that
            // ended the match.
            State::Named | State::BogusName => self.name_buf.len().max(MAX_ENTITY_NAME_LEN + 1),
        }
    }

    /// Narrows the candidate range by one character. Returns false when no
    /// entity can still match.
    fn narrow(&mut self, c: char) -> bool {
        if !c.is_ascii() {
            self.range = (0, 0);
            return false;
        }
        let depth = self.name_buf.len();
        if depth == 0 {
            let (lo, hi) = first_byte_ranges()[c as usize];
            self.range = (lo as usize, hi as usize);
            return lo < hi;
        }
        let key = Some(c as u8);
        let (start, end) = self.range;
        let window = &NAMED_ENTITIES[start..end];
        let lo = start + window.partition_point(|e| byte_at(e.0, depth) < key);
        let hi = start + window.partition_point(|e| byte_at(e.0, depth) <= key);
        self.range = (lo, hi);
        lo < hi
    }

    fn finish_one(&mut self, c: char) -> Status {
        Status::Done(CharRef {
            chars: [c, '\0'],
            num_chars: 1,
        })
    }

    pub(super) fn step<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> Status {
        match self.state {
            State::Begin => self.do_begin(tokenizer, input),
            State::Octothorpe => self.do_octothorpe(tokenizer, input),
            State::Numeric(base) => self.do_numeric(tokenizer, input, sink, base),
            State::NumericSemicolon => self.do_numeric_semicolon(tokenizer, input, sink),
            State::Named => self.do_named(tokenizer, input, sink),
            State::BogusName => self.do_bogus_name(tokenizer, input, sink),
        }
    }

    fn do_begin(&mut self, tokenizer: &mut Tokenizer, input: &mut Input<'_>) -> Status {
        match tokenizer.peek(input) {
            Some('a'..='z' | 'A'..='Z' | '0'..='9') => {
                self.state = State::Named;
                Status::Progress
            }
            Some('#') => {
                tokenizer.discard_char(input);
                self.state = State::Octothorpe;
                Status::Progress
            }
            Some(_) => Status::Done(CharRef::EMPTY),
            None => Status::Stuck,
        }
    }

    fn do_octothorpe(&mut self, tokenizer: &mut Tokenizer, input: &mut Input<'_>) -> Status {
        match tokenizer.peek(input) {
            Some(c @ ('x' | 'X')) => {
                tokenizer.discard_char(input);
                self.hex_marker = Some(c);
                self.state = State::Numeric(16);
            }
            Some(_) => {
                self.hex_marker = None;
                self.state = State::Numeric(10);
            }
            None => return Status::Stuck,
        }
        Status::Progress
    }

    fn do_numeric<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
        base: u32,
    ) -> Status {
        let Some(c) = tokenizer.peek(input) else {
            return Status::Stuck;
        };
        match c.to_digit(base) {
            Some(n) => {
                tokenizer.discard_char(input);
                self.num = self.num.wrapping_mul(base);
                if self.num > 0x10_FFFF {
                    self.num_too_big = true;
                }
                self.num = self.num.wrapping_add(n);
                self.seen_digit = true;
                Status::Progress
            }
            None if !self.seen_digit => self.unconsume_numeric(tokenizer, input, sink),
            None => {
                self.state = State::NumericSemicolon;
                Status::Progress
            }
        }
    }

    fn do_numeric_semicolon<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> Status {
        match tokenizer.peek(input) {
            Some(';') => tokenizer.discard_char(input),
            Some(_) => tokenizer.emit_error(
                sink,
                Borrowed("Semicolon missing after numeric character reference"),
            ),
            None => return Status::Stuck,
        }
        self.finish_numeric(tokenizer, sink)
    }

    fn unconsume_numeric<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> Status {
        let mut back = 1;
        if self.hex_marker.is_some() {
            back += 1;
        }
        input.rewind(back);
        tokenizer.emit_error(sink, Borrowed("Numeric character reference without digits"));
        Status::Done(CharRef::EMPTY)
    }

    fn finish_numeric<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        sink: &mut Sink,
    ) -> Status {
        fn conv(n: u32) -> char {
            from_u32(n).expect("invalid char missed by error handling cases")
        }

        let (c, error) = match self.num {
            n if n > 0x10_FFFF || self.num_too_big => ('\u{fffd}', true),
            0x00 | 0xD800..=0xDFFF => ('\u{fffd}', true),
            0x80..=0x9F => match C1_REPLACEMENTS[(self.num - 0x80) as usize] {
                Some(c) => (c, true),
                None => (conv(self.num), true),
            },
            0x01..=0x08 | 0x0B | 0x0D..=0x1F | 0x7F | 0xFDD0..=0xFDEF => (conv(self.num), true),
            n if (n & 0xFFFE) == 0xFFFE => (conv(n), true),
            n => (conv(n), false),
        };

        if error {
            tokenizer.emit_error(sink, Borrowed("Invalid numeric character reference"));
        }
        self.finish_one(c)
    }

    fn do_named<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> Status {
        // peek + discard skips newline normalisation, which is what makes
        // un-consuming by byte length correct.
        let Some(c) = tokenizer.peek(input) else {
            return Status::Stuck;
        };
        tokenizer.discard_char(input);
        let still_matching = self.narrow(c);
        self.name_buf.push(c);
        if !still_matching {
            return self.finish_named(tokenizer, input, sink, Some(c));
        }
        let (lo, _) = self.range;
        let candidate = NAMED_ENTITIES[lo];
        if candidate.0.len() == self.name_buf.len() {
            self.name_match = Some((candidate.1, candidate.2));
            self.name_len = self.name_buf.len();
        }
        Status::Progress
    }

    fn emit_name_error<Sink: TokenSink>(&mut self, tokenizer: &mut Tokenizer, sink: &mut Sink) {
        tokenizer.emit_error(sink, Borrowed("Invalid character reference"));
    }

    fn unconsume_name(&mut self, input: &mut Input<'_>) {
        input.rewind(self.name_buf.len());
        self.name_buf.clear();
    }

    fn finish_named<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
        end_char: Option<char>,
    ) -> Status {
        match self.name_match {
            None => {
                match end_char {
                    Some(c) if c.is_ascii_alphanumeric() => {
                        // Keep consuming to see whether a semicolon follows,
                        // which decides whether this is a parse error.
                        self.state = State::BogusName;
                        return Status::Progress;
                    }
                    // `&;` on its own is not a parse error.
                    Some(';') if self.name_buf.len() > 1 => self.emit_name_error(tokenizer, sink),
                    _ => (),
                }
                self.unconsume_name(input);
                Status::Done(CharRef::EMPTY)
            }

            Some((c1, c2)) => {
                let name_len = self.name_len;
                debug_assert!(name_len > 0);
                let last_matched = self.name_buf[name_len - 1..].chars().next().unwrap();
                let next_after = if name_len == self.name_buf.len() {
                    None
                } else {
                    self.name_buf[name_len..].chars().next()
                };

                // Historical rule: inside an attribute value a reference
                // without a trailing semicolon that is followed by `=` or an
                // alphanumeric is not expanded.
                let unconsume_all = match (self.is_consumed_in_attribute, last_matched, next_after)
                {
                    (_, ';', _) => false,
                    (true, _, Some('=')) => true,
                    (true, _, Some(c)) if c.is_ascii_alphanumeric() => true,
                    _ => {
                        tokenizer.emit_error(
                            sink,
                            Borrowed("Character reference does not end with semicolon"),
                        );
                        false
                    }
                };

                if unconsume_all {
                    self.unconsume_name(input);
                    Status::Done(CharRef::EMPTY)
                } else {
                    input.rewind(self.name_buf.len() - name_len);
                    tokenizer.ignore_lf = false;
                    Status::Done(CharRef {
                        chars: [from_u32(c1).unwrap(), from_u32(c2).unwrap_or('\0')],
                        num_chars: if c2 == 0 { 1 } else { 2 },
                    })
                }
            }
        }
    }

    fn do_bogus_name<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> Status {
        let Some(c) = tokenizer.peek(input) else {
            return Status::Stuck;
        };
        tokenizer.discard_char(input);
        self.name_buf.push(c);
        match c {
            _ if c.is_ascii_alphanumeric() => return Status::Progress,
            ';' => self.emit_name_error(tokenizer, sink),
            _ => (),
        }
        self.unconsume_name(input);
        Status::Done(CharRef::EMPTY)
    }

    pub(super) fn end_of_file<Sink: TokenSink>(
        &mut self,
        tokenizer: &mut Tokenizer,
        input: &mut Input<'_>,
        sink: &mut Sink,
    ) -> CharRef {
        loop {
            let status = match self.state {
                State::Begin => Status::Done(CharRef::EMPTY),
                State::Numeric(_) if !self.seen_digit => {
                    self.unconsume_numeric(tokenizer, input, sink)
                }
                State::Numeric(_) | State::NumericSemicolon => {
                    tokenizer.emit_error(sink, Borrowed("EOF in numeric character reference"));
                    self.finish_numeric(tokenizer, sink)
                }
                State::Named => self.finish_named(tokenizer, input, sink, None),
                State::BogusName => {
                    self.unconsume_name(input);
                    Status::Done(CharRef::EMPTY)
                }
                State::Octothorpe => {
                    input.rewind(1);
                    tokenizer.emit_error(sink, Borrowed("EOF after '#' in character reference"));
                    Status::Done(CharRef::EMPTY)
                }
            };

            match status {
                Status::Done(char_ref) => return char_ref,
                Status::Stuck => return CharRef::EMPTY,
                Status::Progress => {}
            }
        }
    }
}
