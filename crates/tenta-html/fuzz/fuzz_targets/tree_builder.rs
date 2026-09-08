#![no_main]

#[path = "../../tests/common/tree.rs"]
mod tree;

use libfuzzer_sys::fuzz_target;
use tenta_html::{MAX_DOM_DEPTH, Parser, ParserOptions};
use tree::Tree;

fn parse(bytes: &[u8], scripting_enabled: bool) -> Tree {
    let options = ParserOptions {
        scripting_enabled,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed(bytes);
    parser.finish()
}

fuzz_target!(|data: &[u8]| {
    let tree = parse(data, true);
    // The depth cap is a hard invariant: no input may build a deeper tree.
    assert!(tree.depth_of_deepest_element() <= MAX_DOM_DEPTH);

    // Chunking must not change the tree.
    let options = ParserOptions::default();
    let mut chunked = Parser::new(Tree::new(), options);
    for chunk in data.chunks(7) {
        chunked.feed(chunk);
    }
    let chunked = chunked.finish();
    assert_eq!(
        tree::serialize(&tree, 0, true),
        tree::serialize(&chunked, 0, true)
    );

    let _ = parse(data, false);
});
