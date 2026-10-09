// Inspect plain page progress without running another script after hard failure.
fn crypto_budget_progress(engine: &mut Engine) -> v8::Global<v8::Object> {
    engine
        .evaluate("globalThis.cryptoBudgetProgress = {after:false,caught:false,finalized:false};")
        .expect("publish a plain progress object before the native request");
    let _entry = engine.entered();
    v8::scope!(let handle_scope, &mut engine.isolate);
    let context = v8::Local::new(handle_scope, &engine.context);
    let mut context_scope = v8::ContextScope::new(handle_scope, context);
    v8::scope!(let scope, &mut context_scope);
    let key = v8::String::new(scope, "cryptoBudgetProgress").expect("progress key");
    let value = context
        .global(scope)
        .get(scope, key.into())
        .expect("progress object");
    let object = v8::Local::<v8::Object>::try_from(value).expect("plain object");
    v8::Global::new(scope, object)
}

fn assert_crypto_budget_progress_stopped(engine: &mut Engine, progress: &v8::Global<v8::Object>) {
    let _entry = engine.entered();
    v8::scope!(let handle_scope, &mut engine.isolate);
    let context = v8::Local::new(handle_scope, &engine.context);
    let mut context_scope = v8::ContextScope::new(handle_scope, context);
    v8::scope!(let scope, &mut context_scope);
    let progress = v8::Local::new(scope, progress);
    for field in ["after", "caught", "finalized"] {
        let key = v8::String::new(scope, field).expect("field name");
        let value = progress
            .get(scope, key.into())
            .expect("plain own data field");
        assert!(
            value.is_false(),
            "page entered {field} after native Web Crypto refusal"
        );
    }
}

#[test]
fn web_crypto_budget_stop_never_enters_plain_js_catch_or_finally() {
    use tenta_core::{
        BudgetKind, NativeMemoryBudget, NativeMemoryComponent, NativeMemoryFailure,
        NativeMemoryFailureCause,
    };

    let document = Rc::new(RefCell::new(Document::new()));
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 4);
    let mut engine = Engine::with_document_on_clock_storage_and_memory(
        document,
        Clock::virtual_at(MonoTime::ZERO),
        None,
        CookieJar::new(),
        PageStorage::isolated(),
        budget.clone(),
    );
    let progress = crypto_budget_progress(&mut engine);
    let error = tenta_js::engine::PageEngine::evaluate(
        &mut engine,
        "try { crypto.getRandomValues(new Uint8Array(8));
               cryptoBudgetProgress.after = true; }
         catch (_) { cryptoBudgetProgress.caught = true; }
         finally { cryptoBudgetProgress.finalized = true; }",
        std::time::Duration::from_secs(5),
    )
    .expect_err("an eight-byte native request exceeds the four-byte crypto cap");
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert!(error.message.contains("web crypto"), "{error:?}");
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

    assert_crypto_budget_progress_stopped(&mut engine, &progress);
    assert!(
        engine.evaluate("1 + 1").is_err(),
        "the failed page stays stopped"
    );
}
