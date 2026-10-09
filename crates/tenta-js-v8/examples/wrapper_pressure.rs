//! Real V8 wrapper ownership and admission workload.

use std::error::Error;
use tenta_js_v8::Engine;

#[path = "../../../bench/workloads/wrapper_pressure.rs"]
mod wrapper_pressure;

fn main() -> Result<(), Box<dyn Error>> {
    wrapper_pressure::run("v8", |document| Ok(Engine::with_document(document)))
}
