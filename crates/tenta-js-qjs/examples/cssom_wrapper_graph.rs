//! Run with `cargo run --release -p tenta-js-qjs --example cssom_wrapper_graph_qjs`.

use std::io;
use tenta_js_qjs::Engine;

#[path = "../../../bench/workloads/cssom_wrapper_graph.rs"]
mod cssom_wrapper_graph;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cssom_wrapper_graph::run("quickjs", |document| {
        Engine::with_document(document)
            .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))
    })
}
