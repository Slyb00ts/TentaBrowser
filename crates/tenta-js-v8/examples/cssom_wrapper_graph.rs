//! Run with `cargo run --release -p tenta-js-v8 --example cssom_wrapper_graph`.

use tenta_js_v8::Engine;

#[path = "../../../bench/workloads/cssom_wrapper_graph.rs"]
mod cssom_wrapper_graph;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cssom_wrapper_graph::run("v8", |document| Ok(Engine::with_document(document)))
}
