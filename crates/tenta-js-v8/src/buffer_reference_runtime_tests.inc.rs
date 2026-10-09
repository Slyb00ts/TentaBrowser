// A real DOM world's Admission must stop an in-progress native buffer copy.

thread_local! {
    static BUFFER_COPY_PREFIX: RefCell<Option<(u8, u8)>> = const { RefCell::new(None) };
}

fn copy_buffer_inside_nested_boundary<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        // A direct nested boundary may observe the stop first, but must leave
        // its pending marker for this registered outer callback's final tail.
        let copied = tenta_js::bindings::v8_callback_boundary(
            scope,
            "nested native buffer copy",
            false,
            |scope| {
                tenta_js::bindings::v8_checked_begin_stored_value_lease(scope)
                    .expect("an admitted nested callback can own a buffer lease");
                let outcome = (|| {
                    let source = tenta_js::bindings::buffer_source_ref_from_v8(scope, args.get(0))?;
                    let info = tenta_js::bindings::v8_buffer_source_info(scope, source)?;
                    assert_eq!(info.byte_length, 2 * 64 * 1024);
                    let mut bytes = vec![0; info.byte_length];
                    let result =
                        tenta_js::bindings::v8_copy_buffer_source_into(scope, source, &mut bytes);
                    BUFFER_COPY_PREFIX.with(|slot| {
                        *slot.borrow_mut() = Some((bytes[0], bytes[64 * 1024]));
                    });
                    result.map(|_| ())
                })();
                tenta_js::bindings::v8_end_stored_value_lease(scope);
                outcome.is_ok()
            },
        );
        rv.set_bool(copied);
    };
    // SAFETY: this FunctionTemplate is installed as an allocating slow callback.
    unsafe {
        tenta_js::bindings::v8_registered_slow_callback_boundary(
            scope,
            "native buffer copy runtime control",
            (),
            call,
        )
    };
}

fn buffer_copy_engine(
    native_memory: tenta_core::NativeMemoryBudget,
) -> (Engine, Rc<world::cycles::Admission>) {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document_on_clock_storage_and_memory(
        document,
        Clock::virtual_at(MonoTime::ZERO),
        None,
        CookieJar::new(),
        PageStorage::isolated(),
        native_memory,
    );
    let dom = engine.dom.as_ref().expect("DOM world");
    let admission = Rc::new(world::cycles::Admission::new(
        Rc::clone(&dom.state),
        dom.ctx.clone(),
        Arc::clone(&engine.termination),
    ));
    install_v8_callback_admission(&mut engine.isolate, admission.clone());
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let template =
            v8::FunctionTemplate::builder(copy_buffer_inside_nested_boundary).build(scope);
        let function = template.get_function(scope).expect("slow function");
        let key = v8::String::new(scope, "nativeBufferCopy").expect("function name");
        let global = context.global(scope);
        assert_eq!(global.set(scope, key.into(), function.into()), Some(true));
    }
    (engine, admission)
}

fn saved_buffer_progress(engine: &mut Engine) -> v8::Global<v8::Object> {
    engine
        .evaluate("globalThis.bufferProgress = {after:false,caught:false,finalized:false};")
        .expect("publish a plain progress object before termination");
    let _entry = engine.entered();
    v8::scope!(let handle_scope, &mut engine.isolate);
    let context = v8::Local::new(handle_scope, &engine.context);
    let mut context_scope = v8::ContextScope::new(handle_scope, context);
    v8::scope!(let scope, &mut context_scope);
    let key = v8::String::new(scope, "bufferProgress").expect("progress key");
    let value = context
        .global(scope)
        .get(scope, key.into())
        .expect("progress");
    let object = v8::Local::<v8::Object>::try_from(value).expect("plain object");
    v8::Global::new(scope, object)
}

fn assert_buffer_progress_stopped(engine: &mut Engine, progress: &v8::Global<v8::Object>) {
    let _entry = engine.entered();
    v8::scope!(let handle_scope, &mut engine.isolate);
    let context = v8::Local::new(handle_scope, &engine.context);
    let mut context_scope = v8::ContextScope::new(handle_scope, context);
    v8::scope!(let scope, &mut context_scope);
    let progress = v8::Local::new(scope, progress);
    for field in ["after", "caught", "finalized"] {
        let key = v8::String::new(scope, field).expect("field name");
        let value = progress.get(scope, key.into()).expect("plain own field");
        assert!(value.is_false(), "page ran {field} after native hard stop");
    }
}

fn run_buffer_copy_until_second_poll(
    engine: &mut Engine,
    admission: &world::cycles::Admission,
) -> (JsError, v8::Global<v8::Object>) {
    BUFFER_COPY_PREFIX.with(|slot| *slot.borrow_mut() = None);
    let progress = saved_buffer_progress(engine);
    let values_before = engine.stored_value_count();
    let error = engine
        .evaluate(
            "try { nativeBufferCopy(new Uint8Array(131072).fill(7)); \
             bufferProgress.after = true; } \
             catch (_) { bufferProgress.caught = true; } \
             finally { bufferProgress.finalized = true; }",
        )
        .expect_err("hard stop must discard the current script");
    assert_eq!(
        admission.buffer_poll_count(),
        2,
        "native copy reached its second chunk"
    );
    BUFFER_COPY_PREFIX.with(|slot| {
        assert_eq!(
            *slot.borrow(),
            Some((7, 0)),
            "first chunk copied; later chunk was untouched",
        );
    });
    assert_eq!(
        engine.stored_value_count(),
        values_before,
        "temporary root released"
    );
    assert_eq!(
        engine.native_failure(),
        None,
        "hard stop is not a native panic"
    );
    assert_buffer_progress_stopped(engine, &progress);
    (error, progress)
}

#[test]
fn production_admission_watchdog_stops_nested_native_copy_and_recovers() {
    let memory = tenta_core::NativeMemoryBudget::with_limits(32 * 1024, 4096, 4096, 4096);
    let (mut engine, admission) = buffer_copy_engine(memory.clone());
    admission.fail_on_buffer_poll(2, world::cycles::BufferPollFault::Watchdog);
    let (error, progress) = run_buffer_copy_until_second_poll(&mut engine, &admission);
    assert!(error.message.contains("terminated"), "{error}");
    assert_eq!(memory.first_failure(), None);
    assert_eq!(engine.evaluate("1 + 1").expect("next task recovers"), "2");
    BUFFER_COPY_PREFIX.with(|slot| *slot.borrow_mut() = None);
    let values_before = engine.stored_value_count();
    assert_eq!(
        engine
            .evaluate("nativeBufferCopy(new Uint8Array(131072).fill(7))")
            .expect("the next native callback must recover too"),
        "true",
    );
    BUFFER_COPY_PREFIX.with(|slot| {
        assert_eq!(*slot.borrow(), Some((7, 7)), "both chunks copied");
    });
    assert_eq!(engine.stored_value_count(), values_before);
    assert_eq!(engine.native_failure(), None);
    assert_buffer_progress_stopped(&mut engine, &progress);
}

#[test]
fn production_admission_memory_failure_stops_nested_native_copy_and_stays_typed() {
    let memory = tenta_core::NativeMemoryBudget::with_limits(32 * 1024, 4096, 4096, 4096);
    let (mut engine, admission) = buffer_copy_engine(memory.clone());
    admission.fail_on_buffer_poll(2, world::cycles::BufferPollFault::PageMemory);
    let (error, progress) = run_buffer_copy_until_second_poll(&mut engine, &admission);
    assert!(error.message.contains("structured clone"), "{error}");
    assert_eq!(
        memory.first_failure(),
        Some(tenta_core::NativeMemoryFailure {
            kind: tenta_core::BudgetKind::PageMemory,
            component: tenta_core::NativeMemoryComponent::StructuredClone,
            cause: tenta_core::NativeMemoryFailureCause::Limit,
        }),
    );
    assert_eq!(
        memory.used(tenta_core::NativeMemoryComponent::StructuredClone),
        0,
        "a rejected reservation retains no charge",
    );
    assert!(
        engine.evaluate("1 + 1").is_err(),
        "sticky budget discards later tasks"
    );
    assert_buffer_progress_stopped(&mut engine, &progress);
}
