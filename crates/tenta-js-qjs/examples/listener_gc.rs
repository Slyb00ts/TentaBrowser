//! Real QuickJS-backed listener GC benchmark.
//!
//! Run with `cargo run --release -p tenta-js-qjs --example listener_gc_qjs`.

use std::error::Error;
use std::io;

use tenta_js_qjs::Engine;

#[path = "../../../bench/workloads/listener_gc.rs"]
mod listener_gc;

fn main() -> Result<(), Box<dyn Error>> {
    listener_gc::run("quickjs", |document| {
        Engine::with_document(document)
            .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))
    })
}
