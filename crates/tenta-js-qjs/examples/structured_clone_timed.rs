//! Run with `cargo run --release -p tenta-js-qjs --example structured_clone_timed_qjs`.

use std::error::Error;

#[path = "../../../bench/workloads/structured_clone_timed.rs"]
mod structured_clone_timed;

fn main() -> Result<(), Box<dyn Error>> {
    let engine = tenta_js_qjs::Engine::new().map_err(structured_clone_timed::script_error)?;
    structured_clone_timed::run("quickjs", engine)
}
