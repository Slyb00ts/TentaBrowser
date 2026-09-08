#![no_main]

#[path = "../../tests/common/tree.rs"]
mod tree;

use libfuzzer_sys::fuzz_target;
use tenta_html::xml::{Parser, XmlOptions};
use tree::Tree;

fuzz_target!(|data: &[u8]| {
    let mut parser = Parser::new(Tree::new(), XmlOptions::default());
    parser.feed(data);
    // Malformed input must produce an error, never a panic.
    let _ = parser.finish();
});
