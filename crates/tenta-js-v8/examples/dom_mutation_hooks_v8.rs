//! Run with `cargo run --release -p tenta-js-v8 --example dom_mutation_hooks_v8`.

use tenta_js_v8::Engine;

#[path = "../../../bench/workloads/dom_mutation_hooks.rs"]
mod dom_mutation_hooks;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dom_mutation_hooks::run("v8", |document| Ok(Engine::with_document(document)))
}
