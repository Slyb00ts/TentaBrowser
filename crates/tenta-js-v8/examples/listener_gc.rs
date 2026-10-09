//! Real V8-backed listener GC benchmark.
//!
//! Run with `cargo run --release -p tenta-js-v8 --example listener_gc`.

use std::error::Error;

use tenta_js_v8::Engine;

#[path = "../../../bench/workloads/listener_gc.rs"]
mod listener_gc;

fn main() -> Result<(), Box<dyn Error>> {
    listener_gc::run("v8", |document| Ok(Engine::with_document(document)))
}
