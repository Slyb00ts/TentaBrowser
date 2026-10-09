//! Run with `cargo run --release -p tenta-js-v8 --example storage_calls`.

use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js_v8::Engine;

#[path = "../../../bench/workloads/web_storage.rs"]
mod web_storage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    web_storage::run("v8", |document, page| {
        Ok(Engine::with_document_on_clock_and_storage(
            document,
            Clock::virtual_at(MonoTime::ZERO),
            None,
            tenta_net::CookieJar::new(),
            page,
        ))
    })
}
