//! Run with `cargo run --release -p tenta-js-qjs --example dom_mutation_hooks_qjs`.

use std::io;
use tenta_js_qjs::Engine;

#[path = "../../../bench/workloads/dom_mutation_hooks.rs"]
mod dom_mutation_hooks;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dom_mutation_hooks::run("quickjs", |document| {
        Engine::with_document(document)
            .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))
    })
}
