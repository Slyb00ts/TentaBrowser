//! Run with `cargo run --release -p tenta-js-v8 --example structured_clone_timed`.

use std::error::Error;

#[path = "../../../bench/workloads/structured_clone_timed.rs"]
mod structured_clone_timed;

fn main() -> Result<(), Box<dyn Error>> {
    structured_clone_timed::run("v8", tenta_js_v8::Engine::new())
}
