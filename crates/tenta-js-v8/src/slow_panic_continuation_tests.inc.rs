// Native panic must stop the current slow V8 callback before page code resumes.

#[test]
fn generated_getter_panic_stops_pure_javascript_in_the_same_task() {
    assert_native_panic_stops_pure_javascript("document.body", "get_document_body");
}

#[test]
fn handwritten_fetch_panic_stops_pure_javascript_in_the_same_task() {
    assert_native_panic_stops_pure_javascript("fetch('https://example.test/')", "fetch");
}

fn assert_native_panic_stops_pure_javascript(operation: &str, boundary: &str) {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(Rc::clone(&document));
    engine
        .evaluate(
            "globalThis.nativePanicProgress = \
             { after: false, caught: false, finalized: false };",
        )
        .expect("publish a plain progress object before the injected panic");

    // Retain the exact plain object before poisoning the page. Looking it
    // up on Window afterward could itself enter a poisoned interceptor.
    let progress = {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let key = v8::String::new(scope, "nativePanicProgress").unwrap();
        let value = context.global(scope).get(scope, key.into()).unwrap();
        let object = v8::Local::<v8::Object>::try_from(value).unwrap();
        v8::Global::new(scope, object)
    };

    let source = format!(
        "try {{ {operation}; nativePanicProgress.after = true; }} \
         catch (_) {{ nativePanicProgress.caught = true; }} \
         finally {{ nativePanicProgress.finalized = true; }}"
    );
    let failure = {
        // Existing embedding violation injects a real RefCell panic in
        // DocumentOps::body or the handwritten fetch URL path.
        let _borrow = document.borrow_mut();
        engine
            .evaluate(&source)
            .expect_err("native panic discards the page")
    };
    assert_eq!(engine.native_failure(), Some(boundary));
    assert!(failure.message.contains(boundary));
    assert!(failure.message.contains("must be discarded"));

    // Read own data properties of the saved ordinary object through the
    // embedder API. A later evaluate would rightly refuse the poisoned
    // engine and could not reveal same-script continuation.
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let progress = v8::Local::new(scope, &progress);
        for field in ["after", "caught", "finalized"] {
            let key = v8::String::new(scope, field).unwrap();
            let value = progress
                .get(scope, key.into())
                .expect("plain own data field");
            assert!(value.is_false(), "{boundary}: page ran {field} after panic");
        }
    }
    assert!(engine.evaluate("1").is_err());
}
