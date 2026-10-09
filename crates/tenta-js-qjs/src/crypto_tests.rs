//! Crypto wrapper identity under a real QuickJS collection.

use super::Engine;

#[test]
fn window_crypto_is_strongly_owned_across_gc() {
    let mut engine = Engine::new().expect("QuickJS runtime");
    assert_eq!(
        engine
            .evaluate("crypto.marker = 73; crypto === window.crypto")
            .expect("initial Crypto wrapper"),
        "true",
    );
    // No separate JavaScript global retains Crypto during this collection.
    engine.runtime.run_gc();
    assert_eq!(
        engine
            .evaluate("window.crypto.marker")
            .expect("retained expando"),
        "73",
    );
    assert_eq!(
        engine
            .evaluate("crypto === window.crypto")
            .expect("same canonical Crypto wrapper"),
        "true",
    );
}

#[test]
fn final_native_write_watchdog_request_stops_page_then_recovers() {
    use tenta_js::engine::ScriptErrorKind;

    let mut engine = Engine::new().expect("QuickJS runtime");
    engine
        .state
        .interrupt_after_crypto_write
        .replace(Some(engine.interrupt.clone()));
    let error = engine
        .evaluate(
            "globalThis.cryptoProgress = {after: false, caught: false, finalized: false};
             try { crypto.getRandomValues(new Uint8Array(256)); cryptoProgress.after = true; }
             catch (_) { cryptoProgress.caught = true; }
             finally { cryptoProgress.finalized = true; }",
        )
        .expect_err("the real watchdog request after the native write stops script");
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert!(
        engine.state.interrupt_after_crypto_write.borrow().is_none(),
        "the request was injected after the actual write"
    );
    assert_eq!(engine.state.world.native_failure.get(), None);
    assert!(
        engine
            .state
            .world
            .host
            .native_memory
            .first_failure()
            .is_none()
    );
    // Inspect the saved context before a new script task resets the watchdog.
    // These plain own-data writes require no further native callback admission.
    engine.context.with(|ctx| {
        let progress: rquickjs::Object = ctx
            .globals()
            .get("cryptoProgress")
            .expect("the page created its progress object first");
        for name in ["after", "caught", "finalized"] {
            assert!(
                !progress.get::<_, bool>(name).expect("plain boolean flag"),
                "page script entered {name} after the fatal request"
            );
        }
    });
    assert_eq!(
        engine
            .evaluate("crypto.getRandomValues(new Uint8Array(8)).byteLength")
            .expect("a later task resets the watchdog and enters Crypto again"),
        "8"
    );
}

#[test]
fn web_crypto_budget_stop_never_enters_plain_js_catch_or_finally() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use tenta_core::{
        BudgetKind, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryFailure,
        NativeMemoryFailureCause,
    };
    use tenta_js::engine::ScriptErrorKind;
    use tenta_js::event_loop::{Clock, MonoTime};
    use tenta_js::storage::PageStorage;

    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 4);
    let mut engine = Engine::with_document_on_clock_storage_and_memory(
        document,
        Clock::virtual_at(MonoTime::ZERO),
        None,
        tenta_net::CookieJar::new(),
        PageStorage::isolated(),
        budget.clone(),
    )
    .expect("bounded QuickJS engine");

    let error = engine
        .evaluate(
            "globalThis.cryptoBudgetProgress = {after: false, caught: false, finalized: false};
             try { crypto.getRandomValues(new Uint8Array(8));
                   cryptoBudgetProgress.after = true; }
             catch (_) { cryptoBudgetProgress.caught = true; }
             finally { cryptoBudgetProgress.finalized = true; }",
        )
        .expect_err("an eight-byte native request exceeds the four-byte crypto cap");
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert_eq!(error.message, "page memory budget exceeded");
    assert_eq!(
        budget.first_failure(),
        Some(NativeMemoryFailure {
            kind: BudgetKind::PageMemory,
            component: NativeMemoryComponent::WebCrypto,
            cause: NativeMemoryFailureCause::Limit,
        })
    );
    assert_eq!(budget.used(NativeMemoryComponent::WebCrypto), 0);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(
        engine.native_failure(),
        None,
        "quota refusal is not a native panic"
    );

    // Inspect the already-created plain object directly. A second evaluation
    // would be refused by the sticky budget and could mask JS continuation.
    engine.context.with(|ctx| {
        let progress: rquickjs::Object = ctx
            .globals()
            .get("cryptoBudgetProgress")
            .expect("progress object was created before the native request");
        for field in ["after", "caught", "finalized"] {
            assert!(
                !progress
                    .get::<_, bool>(field)
                    .expect("own boolean data field"),
                "page entered {field} after native Web Crypto refusal"
            );
        }
    });
    assert!(
        engine.evaluate("1 + 1").is_err(),
        "the failed page stays stopped"
    );
}
