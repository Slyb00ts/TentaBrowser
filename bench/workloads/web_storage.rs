//! Shared real-engine Storage benchmark with untimed result verification.

use std::cell::RefCell;
use std::error::Error;
use std::fmt::Write as _;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::PageEngine;
use tenta_js::storage::{ContextStorage, PageStorage};

const CALLS: usize = 100_000;
const WARMUP_CALLS: usize = 1_000;
const WORKLOAD: &str = include_str!("web_storage.js");

fn evaluate<E: PageEngine>(engine: &mut E, source: &str) -> Result<String, io::Error> {
    let result = engine
        .evaluate(source, Duration::from_secs(30))
        .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))?;
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!("native failure at {boundary}")));
    }
    Ok(result)
}

fn require(actual: &str, expected: &str) -> Result<(), io::Error> {
    if actual != expected {
        return Err(io::Error::other(format!(
            "Storage workload returned {actual:?}; expected {expected:?}"
        )));
    }
    Ok(())
}

pub fn run<E: PageEngine>(
    backend: &str,
    make_engine: impl FnOnce(SharedDocument, PageStorage) -> Result<E, io::Error>,
) -> Result<(), Box<dyn Error>> {
    let document = Rc::new(RefCell::new(Document::with_url(url::Url::parse(
        "https://storage.test/",
    )?)));
    let context = ContextStorage::new(1024 * 1024);
    let mut engine = make_engine(document, context.new_page())?;
    evaluate(&mut engine, WORKLOAD)?;
    let mut output = format!(
        "{{\"scenario\":\"S-storage-calls\",\"backend\":\"{backend}\",\"keys\":128,\"calls_per_operation\":{CALLS},\"warmup_calls_per_operation\":{WARMUP_CALLS}"
    );
    for (name, function, wrote) in [
        ("get_item", "storageGetItem", false),
        ("named_get", "storageNamedGet", false),
        ("set_item", "storageSetItem", true),
        ("named_set", "storageNamedSet", true),
        ("own_keys", "storageOwnKeys", false),
        ("symbol_create_delete", "storageSymbolCreateDelete", false),
    ] {
        evaluate(&mut engine, "resetStorage()")?;
        require(
            &evaluate(&mut engine, &format!("{function}({WARMUP_CALLS})"))?,
            &WARMUP_CALLS.to_string(),
        )?;
        require(
            &evaluate(
                &mut engine,
                &format!("verifyStorage({WARMUP_CALLS},{wrote})"),
            )?,
            "ok",
        )?;
        evaluate(&mut engine, "resetStorage()")?;
        let invocation = format!("{function}({CALLS})");
        let started = Instant::now();
        let result = evaluate(&mut engine, &invocation)?;
        let elapsed_ns = started.elapsed().as_nanos();
        require(&result, &CALLS.to_string())?;
        require(
            &evaluate(&mut engine, &format!("verifyStorage({CALLS},{wrote})"))?,
            "ok",
        )?;
        write!(
            output,
            ",\"{name}_elapsed_ns\":{elapsed_ns},\"{name}_ns_per_call\":{}",
            elapsed_ns as f64 / CALLS as f64
        )?;
    }
    println!("{output}}}");
    Ok(())
}
