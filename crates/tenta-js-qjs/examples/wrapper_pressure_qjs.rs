//! Real QuickJS wrapper ownership and admission workload.

use std::error::Error;
use std::io;
use tenta_js_qjs::Engine;

#[path = "../../../bench/workloads/wrapper_pressure.rs"]
mod wrapper_pressure;

fn main() -> Result<(), Box<dyn Error>> {
    wrapper_pressure::run("quickjs", |document| {
        Engine::with_document(document)
            .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))
    })
}
