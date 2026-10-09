//! Run with `cargo run --release -p tenta-js-qjs --example storage_calls_qjs`.

use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js_qjs::Engine;

#[path = "../../../bench/workloads/web_storage.rs"]
mod web_storage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    web_storage::run("quickjs", |document, page| {
        Engine::with_document_on_clock_and_storage(
            document,
            Clock::virtual_at(MonoTime::ZERO),
            None,
            tenta_net::CookieJar::new(),
            page,
        )
        .map_err(|error| std::io::Error::other(format!("{:?}: {}", error.kind, error.message)))
    })
}
