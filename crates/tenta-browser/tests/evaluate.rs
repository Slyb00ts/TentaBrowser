//! `Page::evaluate` and its relatives on a real page, once per compiled
//! backend: values and handles, awaited promises, console and exception events,
//! bindings (`docs/spec/11-library-api.md` §1).

#![cfg(any(feature = "v8", feature = "quickjs"))]

use std::future::Future;
use std::time::Duration;

use tenta_browser::{
    BrowserContext, CallArgument, Completion, ConsoleCall, ContextOptions, Engine, EvalOptions,
    ExceptionEvent, Goto, JsEngine, Page, PageEvent, PageEvents, PageOptions, PropertyQuery,
    RemoteKind, RemoteObject, RemoteValue, Settle, World,
};
use tenta_core::{Budgets, Error, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(20);

const INDEX: &str = "<!doctype html><html><head><title>t</title></head>\
    <body><div id=\"m\" class=\"a b\">before</div></body></html>";

fn backends() -> Vec<(&'static str, JsEngine)> {
    Vec::from([
        #[cfg(feature = "v8")]
        ("v8", JsEngine::V8),
        #[cfg(feature = "quickjs")]
        ("quickjs", JsEngine::QuickJs),
    ])
}

struct Rig {
    name: &'static str,
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new(
        name: &'static str,
        js: JsEngine,
        budgets: Budgets,
        files: &[(&str, &str)],
    ) -> Rig {
        let temp = tenta_testutil::temp_dir("evaluate");
        temp.write("index.html", INDEX);
        temp.write(
            "other.html",
            "<!doctype html><body><p id=\"o\">other</p></body>",
        );
        for (relative, contents) in files {
            temp.write(relative, contents);
        }
        let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
            .await
            .expect("fixture server");
        let engine = Engine::builder()
            .js(js)
            .max_tier(Tier::Js)
            .io_threads(1)
            .page_threads(1)
            .budgets(budgets)
            .ephemeral(true)
            .build()
            .expect("engine");
        let context = engine
            .new_context(ContextOptions {
                allow_private_network: true,
                ..ContextOptions::default()
            })
            .expect("context");
        Rig {
            name,
            server,
            context,
            engine,
            _files: temp,
        }
    }

    async fn page(&self) -> Page {
        self.context
            .new_page(PageOptions::default())
            .await
            .expect("page")
    }

    /// A page that has loaded `path`.
    async fn loaded(&self, path: &str) -> Page {
        let page = self.page().await;
        self.load(&page, path).await;
        page
    }

    async fn load(&self, page: &Page, path: &str) {
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
    }

    async fn finish(self, page: Page) {
        page.close().await.expect("close");
        self.context.close();
        self.engine.close();
    }
}

async fn each_backend<F, Fut>(scenario: F)
where
    F: Fn(Rig) -> Fut,
    Fut: Future<Output = ()>,
{
    each_backend_with(Budgets::default(), &[], scenario).await;
}

async fn each_backend_with<F, Fut>(budgets: Budgets, files: &[(&str, &str)], scenario: F)
where
    F: Fn(Rig) -> Fut,
    Fut: Future<Output = ()>,
{
    for (name, js) in backends() {
        scenario(Rig::new(name, js, budgets.clone(), files).await).await;
    }
}

fn by_value() -> EvalOptions {
    EvalOptions {
        return_by_value: true,
        ..EvalOptions::default()
    }
}

async fn value(page: &Page, expression: &str) -> RemoteValue {
    page.evaluate_with(expression, by_value())
        .await
        .expect("evaluation ran")
        .into_value::<RemoteValue>()
        .unwrap_or_else(|error| panic!("{expression} threw: {error}"))
}

fn number(n: f64) -> RemoteValue {
    RemoteValue::Number(n)
}

fn string(s: &str) -> RemoteValue {
    RemoteValue::String(s.to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primitives_and_special_numbers_come_back_by_value() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        assert_eq!(
            value(&page, "undefined").await,
            RemoteValue::Undefined,
            "{name}"
        );
        assert_eq!(value(&page, "null").await, RemoteValue::Null, "{name}");
        assert_eq!(
            value(&page, "1 < 2").await,
            RemoteValue::Bool(true),
            "{name}"
        );
        assert_eq!(value(&page, "6 * 7").await, number(42.0), "{name}");
        assert_eq!(value(&page, "'a' + 1").await, string("a1"), "{name}");
        assert_eq!(value(&page, "document.title").await, string("t"), "{name}");
        assert_eq!(
            value(&page, "123456789012345678901234567890n").await,
            RemoteValue::BigInt("123456789012345678901234567890".to_owned()),
            "{name}"
        );

        let nan = value(&page, "NaN").await;
        assert!(
            matches!(nan, RemoteValue::Number(n) if n.is_nan()),
            "{name}"
        );
        assert_eq!(nan.unserializable().as_deref(), Some("NaN"), "{name}");
        let negative_zero = value(&page, "-0").await;
        assert!(
            matches!(negative_zero, RemoteValue::Number(n) if n == 0.0 && n.is_sign_negative()),
            "{name}"
        );
        assert_eq!(
            negative_zero.unserializable().as_deref(),
            Some("-0"),
            "{name}"
        );
        assert_eq!(
            value(&page, "Infinity").await.unserializable().as_deref(),
            Some("Infinity"),
            "{name}"
        );
        assert_eq!(
            value(&page, "-Infinity").await.unserializable().as_deref(),
            Some("-Infinity"),
            "{name}"
        );
        assert_eq!(
            value(&page, "12n").await.unserializable().as_deref(),
            Some("12n"),
            "{name}"
        );

        // Primitives are values even when the caller did not ask for them.
        let plain = page
            .evaluate("'x'")
            .await
            .expect("ran")
            .into_value::<String>();
        assert_eq!(plain.expect("a string"), "x", "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn containers_dates_and_regexps_are_copied_by_value() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let copied = value(
            &page,
            "({ a: 1, b: [1, undefined, Infinity, 'z'], c: { d: null }, e: new Date(86400000), \
               f: /a+/gi, g: function () {}, h: undefined })",
        )
        .await;
        assert_eq!(
            copied,
            RemoteValue::Object(vec![
                ("a".into(), number(1.0)),
                (
                    "b".into(),
                    RemoteValue::Array(vec![
                        number(1.0),
                        RemoteValue::Undefined,
                        number(f64::INFINITY),
                        string("z"),
                    ])
                ),
                (
                    "c".into(),
                    RemoteValue::Object(vec![("d".into(), RemoteValue::Null)])
                ),
                ("e".into(), RemoteValue::Date(Some(86_400_000.0))),
                (
                    "f".into(),
                    RemoteValue::RegExp {
                        source: "a+".into(),
                        flags: "gi".into()
                    }
                ),
                ("h".into(), RemoteValue::Undefined),
            ]),
            "{name}: functions are dropped, undefined is kept"
        );
        rig.finish(page).await;
    })
    .await;
}

fn previewed(group: &str) -> EvalOptions {
    EvalOptions {
        generate_preview: true,
        object_group: Some(group.to_owned()),
        ..EvalOptions::default()
    }
}

async fn handle(page: &Page, expression: &str, options: EvalOptions) -> RemoteObject {
    page.evaluate_with(expression, options)
        .await
        .expect("evaluation ran")
        .into_handle()
        .unwrap_or_else(|error| panic!("{expression} is not an object: {error}"))
}

fn awaiting() -> EvalOptions {
    EvalOptions {
        await_promise: true,
        return_by_value: true,
        ..EvalOptions::default()
    }
}

fn console_calls(events: &mut PageEvents) -> Vec<ConsoleCall> {
    events
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            PageEvent::Console(call) => Some(call),
            _ => None,
        })
        .collect()
}

fn exception_events(events: &mut PageEvents) -> Vec<ExceptionEvent> {
    events
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            PageEvent::Exception(event) => Some(event),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn objects_come_back_as_handles_with_a_property_preview() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;

        let object = handle(
            &page,
            "({ a: 1, b: 'x', c: [1, 2], d: { z: 1 } })",
            previewed("g"),
        )
        .await;
        assert_eq!(object.kind, RemoteKind::Object, "{name}");
        assert_eq!(object.class_name, "Object", "{name}");
        assert_eq!(object.description, "Object", "{name}");
        assert_eq!(object.subtype, None, "{name}");
        let preview = object.preview.expect("a preview was requested");
        assert!(!preview.overflow, "{name}");
        let summary: Vec<_> = preview
            .properties
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str(), p.value.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("a", "number", Some("1")),
                ("b", "string", Some("x")),
                ("c", "object", Some("Array(2)")),
                ("d", "object", Some("Object")),
            ],
            "{name}"
        );

        let wide = handle(
            &page,
            "({ a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7 })",
            previewed("g"),
        )
        .await;
        let preview = wide.preview.expect("a preview");
        assert!(
            preview.overflow,
            "{name}: more members than the preview lists"
        );
        assert_eq!(preview.properties.len(), 5, "{name}");

        let plain = handle(&page, "({ a: 1 })", EvalOptions::default()).await;
        assert!(plain.preview.is_none(), "{name}: previews are opt-in");

        let array = handle(&page, "[1, 2, 3]", EvalOptions::default()).await;
        assert_eq!(array.subtype.as_deref(), Some("array"), "{name}");
        assert_eq!(array.description, "Array(3)", "{name}");

        let function = handle(
            &page,
            "(function named(a) { return a; })",
            EvalOptions::default(),
        )
        .await;
        assert_eq!(function.kind, RemoteKind::Function, "{name}");
        assert!(
            function.description.contains("named"),
            "{name}: {}",
            function.description
        );

        let node = handle(
            &page,
            "document.getElementById('m')",
            EvalOptions::default(),
        )
        .await;
        assert_eq!(node.subtype.as_deref(), Some("node"), "{name}");
        assert_eq!(node.class_name, "HTMLDivElement", "{name}");
        assert_eq!(node.description, "div#m.a.b", "{name}");

        let error = handle(&page, "new RangeError('r')", EvalOptions::default()).await;
        assert_eq!(error.subtype.as_deref(), Some("error"), "{name}");
        assert_eq!(error.class_name, "RangeError", "{name}");
        assert!(
            error.description.contains("RangeError: r"),
            "{name}: {}",
            error.description
        );

        let promise = handle(&page, "Promise.resolve(1)", EvalOptions::default()).await;
        assert_eq!(promise.subtype.as_deref(), Some("promise"), "{name}");

        let symbol = handle(&page, "Symbol('s')", EvalOptions::default()).await;
        assert_eq!(symbol.kind, RemoteKind::Symbol, "{name}");
        assert_eq!(symbol.description, "Symbol(s)", "{name}");

        let ids: std::collections::HashSet<_> = [&object.id, &wide.id, &plain.id, &array.id]
            .into_iter()
            .collect();
        assert_eq!(ids.len(), 4, "{name}: every handle has its own id");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_function_on_binds_this_and_takes_values_and_handles() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let receiver = handle(&page, "({ n: 40 })", EvalOptions::default()).await;
        let other = handle(&page, "({ m: 100 })", EvalOptions::default()).await;

        let sum = page
            .call_function_on(
                Some(&receiver),
                "function (x, other) { return this.n + x + other.m; }",
                vec![
                    CallArgument::Value(number(2.0)),
                    CallArgument::Handle(other.id.clone()),
                ],
                by_value(),
            )
            .await
            .expect("ran")
            .into_value::<f64>()
            .expect("a number");
        assert_eq!(sum, 142.0, "{name}");

        // Special values survive the trip in.
        let echoed = page
            .call_function_on(
                None,
                "(a, b, c) => [typeof this, 1 / a, b, Object.is(c, undefined)]",
                vec![
                    CallArgument::Value(number(-0.0)),
                    CallArgument::Value(RemoteValue::BigInt("7".into())),
                    CallArgument::Value(RemoteValue::Undefined),
                ],
                by_value(),
            )
            .await
            .expect("ran")
            .into_value::<RemoteValue>()
            .expect("a value");
        assert_eq!(
            echoed,
            RemoteValue::Array(vec![
                string("object"),
                number(f64::NEG_INFINITY),
                RemoteValue::BigInt("7".into()),
                RemoteValue::Bool(true),
            ]),
            "{name}: -0, a bigint and undefined arrive intact, and `this` is the global"
        );

        // A returned object is a handle again, in the group asked for.
        let made = page
            .call_function_on(
                Some(&receiver),
                "function () { return { twice: this.n * 2 }; }",
                Vec::new(),
                previewed("made"),
            )
            .await
            .expect("ran")
            .into_handle()
            .expect("an object");
        let preview = made.preview.expect("a preview");
        assert_eq!(preview.properties[0].value.as_deref(), Some("80"), "{name}");

        // An async function can be awaited.
        let done = page
            .call_function_on(
                None,
                "async function (ms) { await new Promise((r) => setTimeout(r, ms)); return 'done'; }",
                vec![CallArgument::Value(number(10.0))],
                awaiting(),
            )
            .await
            .expect("ran")
            .into_value::<String>()
            .expect("a string");
        assert_eq!(done, "done", "{name}");

        let bad = page
            .call_function_on(None, "42", Vec::new(), by_value())
            .await
            .expect("ran");
        assert!(bad.exception().is_some(), "{name}: 42 is not a function");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_promise_settled_by_a_timer_is_awaited_without_blocking_the_page() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;

        // Not awaited: the promise itself is the result.
        let promise = handle(
            &page,
            "new Promise((r) => setTimeout(() => r(1), 5))",
            EvalOptions::default(),
        )
        .await;
        assert_eq!(promise.subtype.as_deref(), Some("promise"), "{name}");

        let late = page
            .evaluate_with(
                "new Promise((r) => setTimeout(() => r({ ok: true }), 20))",
                awaiting(),
            )
            .await
            .expect("ran")
            .into_value::<RemoteValue>()
            .expect("settled");
        assert_eq!(
            late,
            RemoteValue::Object(vec![("ok".into(), RemoteValue::Bool(true))]),
            "{name}"
        );

        // The page thread stayed free: a second evaluation runs while the first
        // is still waiting for its timer.
        let slow = {
            let page = page.clone();
            tokio::spawn(async move {
                page.evaluate_with(
                    "new Promise((r) => setTimeout(() => r('slow'), 60))",
                    awaiting(),
                )
                .await
            })
        };
        assert_eq!(value(&page, "1 + 1").await, number(2.0), "{name}");
        let slow = slow
            .await
            .expect("task")
            .expect("ran")
            .into_value::<String>();
        assert_eq!(slow.expect("settled"), "slow", "{name}");

        // Already settled, and not a promise at all.
        let settled = page.evaluate_with("Promise.resolve(5)", awaiting()).await;
        assert_eq!(
            settled.expect("ran").into_value::<f64>().expect("5"),
            5.0,
            "{name}"
        );
        let plain = page.evaluate_with("7", awaiting()).await;
        assert_eq!(
            plain.expect("ran").into_value::<f64>().expect("7"),
            7.0,
            "{name}"
        );

        // A promise that never settles ends at the caller's deadline.
        let never = page
            .evaluate_with(
                "new Promise(() => {})",
                EvalOptions {
                    timeout: Some(Duration::from_millis(150)),
                    ..awaiting()
                },
            )
            .await;
        assert!(
            matches!(never, Err(Error::Timeout { .. })),
            "{name}: {never:?}"
        );
        assert_eq!(
            value(&page, "2 + 2").await,
            number(4.0),
            "{name}: the page is unharmed"
        );
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejection_or_a_throw_carries_exception_details() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let mut events = page.events();

        let rejected = page
            .evaluate_with("Promise.reject(new Error('boom'))", awaiting())
            .await
            .expect("ran");
        let details = rejected.exception().expect("the promise rejected").clone();
        assert_eq!(details.message, "Error: boom", "{name}");
        assert!(
            details.stack.as_deref().is_some_and(|s| s.contains("boom")),
            "{name}"
        );
        let RemoteValue::Handle(error) = &details.exception else {
            panic!("{name}: an error object stays in the realm");
        };
        assert_eq!(error.class_name, "Error", "{name}");
        assert_eq!(error.subtype.as_deref(), Some("error"), "{name}");
        match rejected.into_value::<String>() {
            Err(Error::Script { message, .. }) => assert_eq!(message, "Error: boom", "{name}"),
            other => panic!("{name}: {other:?}"),
        }

        let primitive = page
            .evaluate_with("Promise.reject('plain')", awaiting())
            .await
            .expect("ran");
        assert_eq!(
            primitive.exception().expect("rejected").exception,
            string("plain"),
            "{name}"
        );

        let thrown = page
            .evaluate("(function f() { throw new TypeError('bad'); })()")
            .await
            .expect("ran");
        let details = thrown.exception().expect("it threw");
        assert_eq!(details.message, "TypeError: bad", "{name}");
        assert_eq!(details.text, "Uncaught", "{name}");
        assert_eq!(
            details.line,
            Some(0),
            "{name}: the throw is on the first line"
        );
        assert!(details.column.is_some(), "{name}");
        assert!(!details.frames.is_empty(), "{name}");

        let syntax = page.evaluate("1 +").await.expect("ran");
        assert!(
            syntax
                .exception()
                .expect("it threw")
                .message
                .contains("SyntaxError"),
            "{name}: {syntax:?}"
        );

        // Handling it is not reporting it: the host's own evaluations and a
        // promise somebody awaits raise no page-level exception events.
        assert!(exception_events(&mut events).is_empty(), "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handle_from_before_a_navigation_is_refused() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let old = handle(&page, "({ a: 1 })", EvalOptions::default()).await;
        assert_eq!(page.live_remote_objects().await.expect("count"), 1, "{name}");

        rig.load(&page, "/other.html").await;

        let stale = |error: Error| {
            matches!(&error, Error::Other { code, .. } if code == "execution_context_destroyed")
        };
        let call = page
            .call_function_on(Some(&old), "function () { return 1; }", Vec::new(), by_value())
            .await;
        assert!(call.is_err_and(stale), "{name}");
        let as_argument = page
            .call_function_on(
                None,
                "function (o) { return 1; }",
                vec![CallArgument::Handle(old.id.clone())],
                by_value(),
            )
            .await;
        assert!(as_argument.is_err_and(stale), "{name}");
        assert_eq!(page.live_remote_objects().await.expect("count"), 0, "{name}: a new realm");
        // The new document answers normally.
        assert_eq!(value(&page, "document.getElementById('o').textContent").await, string("other"), "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_ends_the_wait_for_a_promise() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let mut events = page.events();
        let waiting = {
            let page = page.clone();
            tokio::spawn(async move {
                page.evaluate_with(
                    "console.log('armed'), new Promise(() => {})",
                    EvalOptions {
                        timeout: Some(PATIENCE),
                        ..awaiting()
                    },
                )
                .await
            })
        };
        loop {
            match events.next().await {
                Some(PageEvent::Console(call)) if call.kind == "log" => break,
                Some(_) => {}
                None => panic!("{name}: the stream ended"),
            }
        }
        rig.load(&page, "/other.html").await;
        let outcome = waiting.await.expect("task");
        assert!(
            matches!(&outcome, Err(Error::Other { code, .. }) if code == "execution_context_destroyed"),
            "{name}: {outcome:?}"
        );
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn releasing_an_object_or_a_group_frees_it() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let in_group = |group: &str| EvalOptions {
            object_group: Some(group.to_owned()),
            ..EvalOptions::default()
        };
        let first = handle(&page, "({ n: 1 })", in_group("one")).await;
        let second = handle(&page, "({ n: 2 })", in_group("one")).await;
        let _other = handle(&page, "({ n: 3 })", in_group("two")).await;
        let _loose = handle(&page, "({ n: 4 })", EvalOptions::default()).await;
        assert_eq!(
            page.live_remote_objects().await.expect("count"),
            4,
            "{name}"
        );

        assert!(
            page.release_object(&first.id).await.expect("release"),
            "{name}"
        );
        assert!(
            !page.release_object(&first.id).await.expect("release"),
            "{name}: already gone"
        );
        assert_eq!(
            page.live_remote_objects().await.expect("count"),
            3,
            "{name}"
        );

        page.release_object_group("one")
            .await
            .expect("release group");
        assert_eq!(
            page.live_remote_objects().await.expect("count"),
            2,
            "{name}"
        );
        let released = page
            .call_function_on(
                Some(&second),
                "function () { return this.n; }",
                Vec::new(),
                by_value(),
            )
            .await;
        assert!(
            matches!(released, Err(Error::InvalidArgument { .. })),
            "{name}: {released:?}"
        );

        page.release_object_group("nothing")
            .await
            .expect("an unknown group is empty");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_calls_become_events_with_their_arguments_and_stack() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let mut events = page.events();

        page.evaluate(
            "console.log('a', 1, { x: 1 }, [1, 2], undefined, NaN, 10n); \
             console.warn('w'); console.error(new Error('e')); \
             console.assert(false, 'nope'); console.assert(true, 'quiet'); \
             console.table([{ a: 1 }]); console.group('g'); console.info('in'); \
             console.groupEnd(); console.debug('d'); console.count(); console.dir({ k: 1 })",
        )
        .await
        .expect("ran");

        let calls = console_calls(&mut events);
        let kinds: Vec<_> = calls.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "log", "warning", "error", "assert", "table", "startGroup", "info", "endGroup",
                "debug", "count", "dir"
            ],
            "{name}"
        );

        let first = &calls[0];
        assert_eq!(first.args.len(), 7, "{name}");
        assert_eq!(first.args[0], string("a"), "{name}");
        assert_eq!(first.args[1], number(1.0), "{name}");
        let RemoteValue::Handle(object) = &first.args[2] else {
            panic!("{name}: an object argument stays in the realm");
        };
        assert_eq!(object.preview.as_ref().expect("console previews").properties[0].name, "x", "{name}");
        assert!(matches!(&first.args[3], RemoteValue::Handle(a) if a.subtype.as_deref() == Some("array")), "{name}");
        assert_eq!(first.args[4], RemoteValue::Undefined, "{name}");
        assert!(matches!(first.args[5], RemoteValue::Number(n) if n.is_nan()), "{name}");
        assert_eq!(first.args[6], RemoteValue::BigInt("10".into()), "{name}");
        assert!(!first.stack.is_empty(), "{name}: a stack is captured");
        assert!(first.stack[0].line.is_some(), "{name}");
        assert!(first.timestamp_ms > 1.0e12, "{name}: a wall-clock timestamp");

        assert_eq!(calls[1].args, [string("w")], "{name}");
        assert!(matches!(&calls[2].args[0], RemoteValue::Handle(e) if e.subtype.as_deref() == Some("error")), "{name}");
        assert_eq!(calls[3].args, [string("nope")], "{name}");
        assert_eq!(calls[9].args, [string("default: 1")], "{name}");
        assert!(calls.iter().all(|c| c.context == first.context), "{name}");

        // Console objects are held until released with their group.
        let held = page.live_remote_objects().await.expect("count");
        assert!(held > 0, "{name}");
        page.release_object_group("console").await.expect("release");
        assert_eq!(page.live_remote_objects().await.expect("count"), 0, "{name}");

        // Nobody listening, nothing described.
        drop(events);
        page.evaluate("console.log({ unobserved: true })").await.expect("ran");
        assert_eq!(page.live_remote_objects().await.expect("count"), 0, "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncaught_exceptions_and_unhandled_rejections_become_events() {
    let page_source = "<!doctype html><body>\
        <script>throw new Error('sync top-level');</script>\
        <script>setTimeout(function () { throw new Error('from timer'); }, 1);</script>\
        <script>Promise.reject(new Error('unhandled'));</script>\
        <script>Promise.reject(new Error('quiet')).catch(function () {});</script>\
        </body>";
    each_backend_with(Budgets::default(), &[("throws.html", page_source)], |rig| async move {
        let page = rig.page().await;
        let name = rig.name;
        let mut events = page.events();
        rig.load(&page, "/throws.html").await;
        page.wait_for_settle(Settle::NetworkIdle, PATIENCE)
            .await
            .expect("settle");

        let raised = exception_events(&mut events);
        let describe = |event: &ExceptionEvent| {
            (
                event.details.message.clone(),
                event.details.text.clone(),
                event.unhandled_rejection,
            )
        };
        let seen: Vec<_> = raised.iter().map(describe).collect();
        assert!(
            seen.contains(&("Error: sync top-level".into(), "Uncaught".into(), false)),
            "{name}: {seen:?}"
        );
        assert!(
            seen.contains(&("Error: from timer".into(), "Uncaught".into(), false)),
            "{name}: {seen:?}"
        );
        assert!(
            seen.contains(&("Error: unhandled".into(), "Uncaught (in promise)".into(), true)),
            "{name}: {seen:?}"
        );
        assert!(
            !seen.iter().any(|(message, ..)| message.contains("quiet")),
            "{name}: a rejection with a handler is not reported: {seen:?}"
        );
        let timer = raised
            .iter()
            .find(|event| event.details.message == "Error: from timer")
            .expect("reported");
        assert!(matches!(&timer.details.exception, RemoteValue::Handle(e) if e.class_name == "Error"), "{name}");
        assert!(timer.details.stack.is_some(), "{name}");
        assert!(timer.context > 0, "{name}");

        // A listener that throws is reported, and the dispatch carries on.
        page.evaluate(
            "window.addEventListener('probe', function () { throw new Error('from listener'); }); \
             window.addEventListener('probe', function () { window.second = true; }); \
             window.dispatchEvent(new Event('probe'))",
        )
        .await
        .expect("ran");
        let listener = exception_events(&mut events);
        assert!(
            listener.iter().any(|e| e.details.message == "Error: from listener"),
            "{name}: {listener:?}"
        );
        assert_eq!(value(&page, "window.second").await, RemoteValue::Bool(true), "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binding_reaches_the_host_from_page_script_and_survives_navigation() {
    let page_source = "<!doctype html><body><script>\
        report('hello');\
        try { report(1); } catch (e) { window.bindingError = e.name; }\
        </script></body>";
    each_backend_with(
        Budgets::default(),
        &[("binding.html", page_source)],
        |rig| async move {
            let page = rig.page().await;
            let name = rig.name;
            page.add_binding("report")
                .await
                .expect("registered before any engine");
            assert!(page.add_binding("not valid").await.is_err(), "{name}");
            let mut events = page.events();
            rig.load(&page, "/binding.html").await;

            let calls: Vec<_> = events
                .drain()
                .into_iter()
                .filter_map(|event| match event {
                    PageEvent::BindingCalled(call) => Some(call),
                    _ => None,
                })
                .collect();
            assert_eq!(calls.len(), 1, "{name}: {calls:?}");
            assert_eq!(
                (calls[0].name.as_str(), calls[0].payload.as_str()),
                ("report", "hello"),
                "{name}"
            );
            assert!(calls[0].context > 0, "{name}");
            assert_eq!(
                value(&page, "window.bindingError").await,
                string("TypeError"),
                "{name}"
            );

            // A document loaded later has it from its first script on.
            rig.load(&page, "/other.html").await;
            assert_eq!(
                value(&page, "typeof report").await,
                string("function"),
                "{name}"
            );
            page.evaluate("report('from the host')").await.expect("ran");
            assert!(
                events.drain().iter().any(|event| matches!(
                    event,
                    PageEvent::BindingCalled(call) if call.payload == "from the host"
                )),
                "{name}"
            );

            page.remove_binding("report").await.expect("removed");
            assert_eq!(
                value(&page, "typeof report").await,
                string("undefined"),
                "{name}"
            );
            rig.finish(page).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_evaluation_ends_at_the_entry_deadline() {
    let budgets = Budgets::default().evaluate_time(Duration::from_millis(300));
    each_backend_with(budgets, &[], |rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let mut events = page.events();
        let started = std::time::Instant::now();
        let outcome = page.evaluate("while (true) {}").await;
        assert!(
            matches!(&outcome, Err(Error::Other { code, .. }) if code == "script_terminated"),
            "{name}: {outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{name}: bounded by the deadline"
        );
        assert!(
            events
                .drain()
                .iter()
                .any(|e| matches!(e, PageEvent::ScriptTerminated { .. })),
            "{name}"
        );
        assert_eq!(
            value(&page, "1 + 1").await,
            number(2.0),
            "{name}: the realm survives"
        );
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn values_that_cannot_be_copied_and_worlds_that_do_not_exist_are_refused() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        for expression in [
            "(() => { const o = {}; o.self = o; return o; })()",
            "(function () {})",
            "Symbol('s')",
        ] {
            let outcome = page.evaluate_with(expression, by_value()).await;
            assert!(
                matches!(&outcome, Err(Error::Other { code, .. }) if code == "not_serializable"),
                "{name}: {expression}: {outcome:?}"
            );
        }
        let isolated = page
            .evaluate_with(
                "1",
                EvalOptions {
                    world: World::Isolated("utility".into()),
                    ..EvalOptions::default()
                },
            )
            .await;
        assert!(
            matches!(isolated, Err(Error::Unsupported { .. })),
            "{name}: {isolated:?}"
        );
        let completion = page.evaluate("1").await.expect("ran").into_completion();
        assert!(matches!(completion, Completion::Value(_)), "{name}");
        rig.finish(page).await;
    })
    .await;
}

/// A page whose first script is `prelude`: the page is hostile before the host
/// asks it anything.
fn hostile_page(prelude: &str) -> String {
    format!("<!doctype html><body><script>{prelude}</script></body>")
}

fn first_console(events: &mut PageEvents) -> ConsoleCall {
    console_calls(events)
        .into_iter()
        .next()
        .expect("a console event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_adds_to_json_to_the_prototypes_cannot_break_evaluation() {
    let page_source = hostile_page(
        "Object.prototype.toJSON = function () { return 5; }; \
         Array.prototype.toJSON = function () { return 6; }; \
         String.prototype.toJSON = function () { return 7; };",
    );
    each_backend_with(
        Budgets::default(),
        &[("hostile.html", page_source.as_str())],
        |rig| async move {
            let page = rig.page().await;
            let name = rig.name;
            let mut events = page.events();
            page.add_binding("report").await.expect("binding");
            rig.load(&page, "/hostile.html").await;

            assert_eq!(value(&page, "1 + 1").await, number(2.0), "{name}");
            assert_eq!(
                value(&page, "({ a: [1, 'two'] })").await,
                RemoteValue::Object(vec![(
                    "a".into(),
                    RemoteValue::Array(vec![number(1.0), string("two")])
                )]),
                "{name}"
            );
            let object = handle(&page, "({ a: 1 })", previewed("g")).await;
            assert_eq!(
                object.preview.expect("preview").properties.len(),
                1,
                "{name}"
            );
            let thrown = page.evaluate("throw new Error('x')").await.expect("ran");
            assert_eq!(
                thrown.exception().expect("threw").message,
                "Error: x",
                "{name}"
            );

            page.evaluate("console.log({ a: 1 }, 'text'); report('payload')")
                .await
                .expect("ran");
            let seen = events.drain();
            assert!(
                seen.iter()
                    .any(|e| matches!(e, PageEvent::Console(c) if c.args.len() == 2)),
                "{name}: console payloads survive: {seen:?}"
            );
            assert!(
                seen.iter()
                    .any(|e| matches!(e, PageEvent::BindingCalled(b) if b.payload == "payload")),
                "{name}: binding payloads survive: {seen:?}"
            );
            rig.finish(page).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_breaks_object_create_cannot_break_object_groups() {
    let page_source = hostile_page(
        "Object.create = function () { throw new Error('no create'); }; \
         Object.defineProperty(Object.prototype, 'value', { get() { throw new Error('proto value'); }, configurable: true });",
    );
    each_backend_with(
        Budgets::default(),
        &[("hostile.html", page_source.as_str())],
        |rig| async move {
            let page = rig.loaded("/hostile.html").await;
            let name = rig.name;
            let mut events = page.events();
            let grouped = handle(&page, "({ a: 1 })", previewed("g")).await;
            let again = page
                .call_function_on(
                    Some(&grouped),
                    "function () { return [this.a]; }",
                    Vec::new(),
                    previewed("g"),
                )
                .await
                .expect("ran")
                .into_handle()
                .expect("an array");
            assert_eq!(again.subtype.as_deref(), Some("array"), "{name}");
            page.evaluate("console.log({ b: 2 })").await.expect("ran");
            assert_eq!(console_calls(&mut events).len(), 1, "{name}");
            page.release_object_group("g").await.expect("release");
            assert_eq!(
                page.live_remote_objects().await.expect("count"),
                1,
                "{name}: only the console object"
            );
            rig.finish(page).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxies_are_opaque_handles_whose_traps_never_run() {
    let page_source = hostile_page(
        "window.ran = 0; \
         const trap = function () { window.ran++; throw new Error('trap'); }; \
         window.throwing = new Proxy({}, { get: trap, ownKeys: trap, getPrototypeOf: trap, \
           getOwnPropertyDescriptor: trap, has: trap }); \
         window.hanging = new Proxy({}, { getPrototypeOf() { for (;;); }, ownKeys() { for (;;); }, \
           get() { for (;;); } }); \
         window.revoked = (() => { const r = Proxy.revocable({}, {}); r.revoke(); return r.proxy; })(); \
         window.proxied = new Proxy([], { get: trap, getPrototypeOf: trap, ownKeys: trap }); \
         window.holder = { inner: window.hanging, list: [window.throwing] };",
    );
    each_backend_with(
        Budgets::default().evaluate_time(Duration::from_secs(5)),
        &[("hostile.html", page_source.as_str())],
        |rig| async move {
            let page = rig.loaded("/hostile.html").await;
            let name = rig.name;
            let mut events = page.events();
            for expression in ["throwing", "hanging", "revoked", "proxied"] {
                let proxy = handle(&page, expression, previewed("p")).await;
                assert_eq!(proxy.subtype.as_deref(), Some("proxy"), "{name}: {expression}");
                assert!(proxy.preview.is_none(), "{name}: {expression}");
                let by_value = page.evaluate_with(expression, by_value()).await;
                assert!(
                    matches!(&by_value, Err(Error::Other { code, .. }) if code == "not_serializable"),
                    "{name}: {expression}: {by_value:?}"
                );
            }
            // Reached through an ordinary object, a proxy is still not entered.
            let holder = handle(&page, "holder", previewed("p")).await;
            let preview = holder.preview.expect("preview");
            let inner = preview.properties.iter().find(|p| p.name == "inner").expect("inner");
            assert_eq!(inner.subtype.as_deref(), Some("proxy"), "{name}");

            page.evaluate("console.log(throwing, hanging, revoked, proxied, holder)")
                .await
                .expect("ran");
            let call = first_console(&mut events);
            assert_eq!(call.args.len(), 5, "{name}");
            assert!(
                call.args[..4]
                    .iter()
                    .all(|a| matches!(a, RemoteValue::Handle(h) if h.subtype.as_deref() == Some("proxy"))),
                "{name}: {:?}",
                call.args
            );
            assert_eq!(value(&page, "window.ran").await, number(0.0), "{name}: no trap ran");
            rig.finish(page).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_cannot_redirect_how_promises_are_recognised_or_awaited() {
    let page_source = hostile_page(
        "Object.defineProperty(Promise, Symbol.hasInstance, { value: () => false }); \
         Object.defineProperty(Error, Symbol.hasInstance, { value() { throw new Error('hasInstance'); } }); \
         const nativeThen = Promise.prototype.then; \
         Promise.prototype.then = function () { throw new Error('patched then'); }; \
         window.nativeThen = nativeThen; \
         window.Throwing = class extends Promise { then() { throw new Error('subclass then'); } }; \
         window.Species = class extends Promise { static get [Symbol.species]() { throw new Error('species'); } };",
    );
    each_backend_with(
        Budgets::default(),
        &[("hostile.html", page_source.as_str())],
        |rig| async move {
            let page = rig.loaded("/hostile.html").await;
            let name = rig.name;
            let settled = page.evaluate_with("Promise.resolve(3)", awaiting()).await;
            assert_eq!(
                settled.expect("ran").into_value::<f64>().expect("3"),
                3.0,
                "{name}"
            );
            let throwing = page
                .evaluate_with("new Throwing((resolve) => resolve(4))", awaiting())
                .await;
            assert_eq!(
                throwing.expect("ran").into_value::<f64>().expect("4"),
                4.0,
                "{name}"
            );
            let error = handle(&page, "new Error('e')", EvalOptions::default()).await;
            assert_eq!(error.subtype.as_deref(), Some("error"), "{name}");

            // A promise whose species lookup throws cannot be reacted to
            // directly; it is adopted instead, and the page's own `then`, which
            // throws here, decides the outcome: a rejection, not a broken
            // evaluation.
            let species = page
                .evaluate_with("new Species((resolve) => resolve(5))", awaiting())
                .await
                .expect("ran");
            assert!(
                species
                    .exception()
                    .is_some_and(|d| d.message.contains("then")),
                "{name}: {species:?}"
            );
            rig.finish(page).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spoofed_string_tags_do_not_change_what_a_value_is() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        for tag in ["Date", "RegExp", "Map", "Promise", "Error"] {
            let expression = format!("({{ [Symbol.toStringTag]: '{tag}', a: 1 }})");
            assert_eq!(
                value(&page, &expression).await,
                RemoteValue::Object(vec![("a".into(), number(1.0))]),
                "{name}: {tag} by value"
            );
            let described = handle(&page, &expression, previewed("t")).await;
            assert_eq!(described.subtype, None, "{name}: {tag}");
            // Not a promise, so awaiting it returns it at once.
            let awaited = page
                .evaluate_with(&expression, awaiting())
                .await
                .expect("ran");
            assert!(awaited.exception().is_none(), "{name}: {tag}");
        }
        let throwing = handle(
            &page,
            "({ get [Symbol.toStringTag]() { throw new Error('tag'); } })",
            previewed("t"),
        )
        .await;
        assert_eq!(throwing.class_name, "Object", "{name}");
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stacks_the_host_sees_or_the_page_creates_hold_no_inspector_frames() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let mut events = page.events();
        let internal = |text: &str| {
            [
                "tenta",
                "dispatch",
                "inspector",
                "finish",
                "prepare",
                "emitConsole",
            ]
            .iter()
            .any(|marker| text.contains(marker))
        };

        page.evaluate("(function logs() { console.log('from a function'); })()")
            .await
            .expect("ran");
        let call = first_console(&mut events);
        assert!(!call.stack.is_empty(), "{name}");
        assert!(
            call.stack
                .iter()
                .all(|f| !internal(&f.function_name) && !internal(&f.url)),
            "{name}: {:?}",
            call.stack
        );

        let created = page
            .evaluate_with("new Error('made here').stack", by_value())
            .await
            .expect("ran")
            .into_value::<String>()
            .expect("a string");
        assert!(!internal(&created), "{name}: {created}");

        let thrown = page
            .evaluate("(function boom() { throw new Error('boom'); })()")
            .await
            .expect("ran");
        let details = thrown.exception().expect("threw");
        assert!(
            !internal(details.stack.as_deref().unwrap_or("")),
            "{name}: {:?}",
            details.stack
        );
        assert!(
            details
                .frames
                .iter()
                .all(|f| !internal(&f.function_name) && !internal(&f.url)),
            "{name}: {:?}",
            details.frames
        );
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn properties_are_listed_without_running_page_code_and_realms_are_numbered() {
    each_backend(|rig| async move {
        let page = rig.loaded("/index.html").await;
        let name = rig.name;
        let realm = page.execution_context().await.expect("a realm");
        assert_eq!(
            page.execution_context().await.expect("again"),
            realm,
            "{name}"
        );

        let object = handle(
            &page,
            "({ plain: 1, nested: { x: 2 }, get lazy() { globalThis.ran = true; return 3; } })",
            previewed("props"),
        )
        .await;
        assert!(
            object.id.starts_with(&format!("{realm}.")),
            "{name}: {}",
            object.id
        );

        let own = page
            .get_properties(
                &object.id,
                PropertyQuery {
                    own_only: true,
                    ..PropertyQuery::default()
                },
            )
            .await
            .expect("properties");
        let names: Vec<&str> = own.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["plain", "nested", "lazy", "__proto__"], "{name}");
        assert_eq!(own[0].value, Some(RemoteValue::Number(1.0)), "{name}");
        assert_eq!(own[0].writable, Some(true), "{name}");
        assert!(
            own[1]
                .value
                .as_ref()
                .unwrap()
                .clone()
                .into_handle()
                .is_some(),
            "{name}"
        );
        assert!(
            own[2].getter.is_some() && own[2].value.is_none(),
            "{name}: an accessor"
        );
        assert!(own.iter().all(|p| p.is_own), "{name}");
        assert_eq!(
            value(&page, "typeof globalThis.ran").await,
            RemoteValue::String("undefined".into()),
            "{name}: the getter did not run"
        );

        let accessors = page
            .get_properties(
                &object.id,
                PropertyQuery {
                    accessors_only: true,
                    ..PropertyQuery::default()
                },
            )
            .await
            .expect("accessors");
        assert!(
            accessors
                .iter()
                .all(|p| p.getter.is_some() || p.setter.is_some()),
            "{name}"
        );
        assert!(accessors.iter().any(|p| p.name == "lazy"), "{name}");

        // The members belong to the group of the object they came from.
        page.release_object_group("props").await.expect("release");
        let released = page
            .get_properties(&object.id, PropertyQuery::default())
            .await;
        assert!(released.is_err(), "{name}: the object is gone");

        // A navigation is a new realm with a new number.
        rig.load(&page, "/other.html").await;
        assert_ne!(
            page.execution_context().await.expect("new realm"),
            realm,
            "{name}"
        );
        rig.finish(page).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_page_runs_script_in_a_blank_document() {
    each_backend(|rig| async move {
        let name = rig.name;
        let page = rig.page().await;
        assert_eq!(
            value(&page, "1 + 1").await,
            RemoteValue::Number(2.0),
            "{name}"
        );
        assert_eq!(
            value(
                &page,
                "document.URL + document.readyState + document.body.childNodes.length"
            )
            .await,
            RemoteValue::String("about:blankcomplete0".into()),
            "{name}"
        );
        let realm = page.execution_context().await.expect("a realm");
        rig.load(&page, "/index.html").await;
        assert_ne!(
            page.execution_context().await.expect("new realm"),
            realm,
            "{name}"
        );

        // A page that runs no script keeps the blank document and no engine.
        let plain = rig
            .context
            .new_page(PageOptions::tier1())
            .await
            .expect("tier 1 page");
        assert!(plain.document().is_some(), "{name}");
        assert!(plain.evaluate("1").await.is_err(), "{name}");
        plain.close().await.expect("close");
        rig.finish(page).await;
    })
    .await;
}
