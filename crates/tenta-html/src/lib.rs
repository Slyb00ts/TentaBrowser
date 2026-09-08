//! HTML tokenizer and tree builder with preload scanner and encoding sniffing; XML parser.
//!
//! `docs/spec/04-dom-and-css.md` §3. The parsers write into any
//! [`sink::TreeSink`]; `tenta-dom` provides the arena implementation.
//!
//! The tokenizer and tree builder are derived from html5ever's state machines
//! (MIT/Apache-2.0; see `THIRD_PARTY_NOTICES.md`) with the changes the spec
//! calls for: `memchr`-based scanning of the text states, text delivered as
//! `&str` slices of the input rather than owned buffers, direct construction
//! through the sink with Blink's DOM depth cap, WHATWG encoding sniffing, and
//! a preload scanner that runs ahead of the tree builder.

pub mod encoding;
mod input;
mod names;
mod parser;
mod preload;
mod scan;
pub mod sink;
mod tokenizer;
mod tree_builder;
pub mod xml;

pub use encoding::{Confidence, Decoder, Sniffed, sniff};
pub use parser::{ParseStatus, Parser, ParserOptions};
pub use preload::{
    PreloadAttrs, PreloadHint, PreloadKind, PreloadScanner, choose_srcset_candidate,
};
pub use scan::ScanMode;
pub use sink::{Attribute, ElementFlags, NodeOrText, ParseControl, QualName, QuirksMode, TreeSink};
pub use tree_builder::MAX_DOM_DEPTH;

/// Re-export of the `encoding_rs` encoding type used across the public API.
pub use encoding_rs::Encoding;
