//! XML parsing for `application/xhtml+xml`, standalone SVG documents and
//! `text/xml` (`docs/spec/04-dom-and-css.md` §3.5).
//!
//! The parser is namespace-aware and writes into the same [`TreeSink`] as the
//! HTML parser. It is not a validating parser: an external DTD is never
//! fetched and an internal subset is skipped, so only the five predefined
//! entities and character references are recognised. Everything else that XML
//! calls a well-formedness constraint is enforced, and the first violation
//! ends the parse with an [`XmlError`] carrying its position.
//!
//! Unlike the HTML parser, the tree is built when [`Parser::finish`] is
//! called: an XML document is only usable once it is complete (a
//! well-formedness error invalidates everything), so `feed` accumulates.

mod parse;

use encoding_rs::Encoding;

use crate::encoding::{Decoder, PRESCAN_LIMIT, sniff_xml};
use crate::sink::TreeSink;

pub use parse::{MAX_XML_DEPTH, XmlError};

/// Options for an XML parse.
#[derive(Debug, Default)]
pub struct XmlOptions {
    /// Encoding from the transport layer (an HTTP `charset` parameter), which
    /// wins over the XML declaration.
    pub encoding: Option<&'static Encoding>,
}

/// A namespace-aware XML parser.
pub struct Parser<S: TreeSink> {
    sink: S,
    text: String,
    bytes: Vec<u8>,
    decoder: Option<Decoder>,
    encoding: Option<&'static Encoding>,
}

impl<S: TreeSink> Parser<S> {
    /// A parser writing into `sink`.
    pub fn new(sink: S, options: XmlOptions) -> Parser<S> {
        Parser {
            sink,
            text: String::new(),
            bytes: Vec::new(),
            decoder: options.encoding.map(Decoder::new),
            encoding: options.encoding,
        }
    }

    /// The encoding in use, once it is known.
    pub fn encoding(&self) -> Option<&'static Encoding> {
        self.encoding
    }

    /// Feeds encoded bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        match &mut self.decoder {
            Some(decoder) => {
                let (owned, borrowed) = decoder.decode(bytes, false);
                self.text.push_str(owned);
                self.text.push_str(borrowed);
            }
            None => {
                self.bytes.extend_from_slice(bytes);
                if self.bytes.len() >= PRESCAN_LIMIT {
                    self.start_decoding();
                }
            }
        }
    }

    /// Feeds already decoded text.
    pub fn feed_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Parses what was fed and returns the tree, or the first
    /// well-formedness error.
    pub fn finish(mut self) -> Result<S::Output, XmlError> {
        if self.decoder.is_none() && !self.bytes.is_empty() {
            self.start_decoding();
        }
        if let Some(decoder) = &mut self.decoder {
            let (owned, borrowed) = decoder.decode(&[], true);
            self.text.push_str(owned);
            self.text.push_str(borrowed);
        }
        parse::parse(&self.text, &mut self.sink)?;
        Ok(self.sink.finish())
    }

    fn start_decoding(&mut self) {
        let buffered = std::mem::take(&mut self.bytes);
        let sniffed = sniff_xml(&buffered);
        self.encoding = Some(sniffed.encoding);
        let mut decoder = Decoder::new(sniffed.encoding);
        let (owned, borrowed) = decoder.decode(&buffered[sniffed.bom_length..], false);
        self.text.push_str(owned);
        self.text.push_str(borrowed);
        self.decoder = Some(decoder);
    }
}
