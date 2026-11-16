//! Module scripts on V8: the module map, linking, evaluation, `import()` and
//! `import.meta`, against the engine alone. The page's loader is what fetches
//! in production; here the test plays it, compiling each module the engine
//! names. `crates/tenta-browser/tests/modules_js.rs` is the end-to-end half.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;
use url::Url;

const PAGE: &str = "http://fixture.test/dir/page.html";

fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::with_url(Url::parse(PAGE).expect("a URL"));
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

fn url(text: &str) -> Url {
    Url::parse(text).expect("a URL")
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Compiles the module at `at` and returns the URLs it imports.
fn compile(engine: &mut Engine, at: &str, source: &str) -> Vec<String> {
    engine
        .compile_module(at, source, &url(at))
        .unwrap_or_else(|error| panic!("{at} should compile: {error}"))
        .into_iter()
        .map(|url| url.to_string())
        .collect()
}

/// Loads every dynamic import the engine is waiting on from `files`, the way
/// the page's loader does, and settles it.
fn serve_imports(engine: &mut Engine, files: &[(&str, &str)]) -> Vec<String> {
    let imports = engine.take_dynamic_imports();
    let mut asked = Vec::new();
    for import in imports {
        asked.push(import.url.to_string());
        let mut queue = vec![import.url.clone()];
        let mut failed = None;
        while let Some(next) = queue.pop() {
            if engine.has_module(next.as_str()) {
                continue;
            }
            let Some((_, source)) = files.iter().find(|(name, _)| *name == next.as_str()) else {
                failed = Some(format!(
                    "Failed to fetch dynamically imported module: {next}"
                ));
                break;
            };
            match engine.compile_module(next.as_str(), source, &next) {
                Ok(more) => queue.extend(more),
                Err(error) => {
                    failed = Some(error.to_string());
                    break;
                }
            }
        }
        match failed {
            None => engine.finish_dynamic_import(import.id, Ok(import.url.as_str())),
            Some(message) => engine.finish_dynamic_import(import.id, Err(&message)),
        }
    }
    engine.run_microtasks();
    asked
}

/// A graph is compiled module by module, each naming what it imports, and
/// nothing runs until the root does. Then every module runs once, dependencies
/// first, in strict mode, with its own scope and its own `import.meta.url`.
#[test]
fn a_module_graph_links_and_runs_once_in_dependency_order() {
    let mut engine = Engine::with_document(document());
    eval(&mut engine, "globalThis.order = []; 'ready'");

    let main = "http://fixture.test/dir/main.js";
    assert_eq!(
        compile(
            &mut engine,
            main,
            "import { name, bump } from './lib/a.js';
             import shared from '/shared.js';
             import './lib/a.js';
             order.push('main ' + name + ' ' + shared.count + ' ' + bump());
             globalThis.metaUrl = import.meta.url;
             globalThis.moduleThis = String(this);
             var scoped = 1;",
        ),
        [
            "http://fixture.test/dir/lib/a.js",
            "http://fixture.test/shared.js"
        ],
        "each import once, resolved against the module, in source order"
    );
    assert_eq!(
        compile(
            &mut engine,
            "http://fixture.test/dir/lib/a.js",
            "import shared from '../../shared.js';
             order.push('a');
             export const name = 'alpha';
             export function bump() { return ++shared.count; }",
        ),
        ["http://fixture.test/shared.js"]
    );
    assert!(
        compile(
            &mut engine,
            "http://fixture.test/shared.js",
            "order.push('shared'); export default { count: 0 };",
        )
        .is_empty()
    );
    assert_eq!(
        eval(&mut engine, "order.length"),
        "0",
        "compiling runs nothing"
    );
    assert!(engine.has_module(main) && !engine.has_module("http://fixture.test/other.js"));

    engine.run_module(main).expect("the graph runs");
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "order.join('|')"),
        "shared|a|main alpha 0 1"
    );
    assert_eq!(eval(&mut engine, "metaUrl"), main);
    assert_eq!(eval(&mut engine, "moduleThis"), "undefined");
    assert_eq!(
        eval(&mut engine, "typeof scoped"),
        "undefined",
        "a module's declarations are not the global's"
    );

    // Running it again evaluates nothing a second time.
    engine.run_module(main).expect("an evaluated module");
    assert_eq!(eval(&mut engine, "order.length"), "3");
}

/// An inline module script has a key of the page's own and resolves against
/// the document, which is also its `import.meta.url`.
#[test]
fn an_inline_module_resolves_against_the_document() {
    let mut engine = Engine::with_document(document());
    let imports = engine
        .compile_module(
            "inline:1",
            "import { v } from './dep.js'; globalThis.seen = v + ' ' + import.meta.url;",
            &url(PAGE),
        )
        .expect("compiles");
    assert_eq!(imports, [url("http://fixture.test/dir/dep.js")]);
    compile(
        &mut engine,
        "http://fixture.test/dir/dep.js",
        "export const v = 'dep';",
    );
    engine.run_module("inline:1").expect("runs");
    assert_eq!(eval(&mut engine, "seen"), format!("dep {PAGE}"));
}

/// A bare specifier means something only through the import map, and a
/// specifier that resolves nowhere fails the module that wrote it.
#[test]
fn bare_specifiers_go_through_the_import_map() {
    let mut engine = Engine::with_document(document());
    let error = engine
        .compile_module("inline:1", "import 'lodash';", &url(PAGE))
        .expect_err("no import map");
    assert!(
        error
            .to_string()
            .contains("Failed to resolve module specifier \"lodash\""),
        "{error}"
    );
    assert!(!engine.has_module("inline:1"));

    engine
        .add_import_map(
            r#"{"imports": {"lodash": "/vendor/lodash.js", "pkg/": "https://cdn.test/pkg/"}}"#,
            &url(PAGE),
        )
        .expect("an import map");
    assert!(engine.add_import_map("not json", &url(PAGE)).is_err());
    let imports = engine
        .compile_module(
            "inline:2",
            "import _ from 'lodash'; import { x } from 'pkg/x.js'; globalThis.got = _ + x;",
            &url(PAGE),
        )
        .expect("compiles");
    assert_eq!(
        imports,
        [
            url("http://fixture.test/vendor/lodash.js"),
            url("https://cdn.test/pkg/x.js")
        ]
    );
    compile(
        &mut engine,
        "http://fixture.test/vendor/lodash.js",
        "export default 'lo';",
    );
    compile(
        &mut engine,
        "https://cdn.test/pkg/x.js",
        "export const x = 'dash';",
    );
    engine.run_module("inline:2").expect("runs");
    assert_eq!(eval(&mut engine, "got"), "lodash");
}

/// A graph that does not link — a missing export, a module that was never
/// loaded — fails the root without running any of it, and a module that
/// throws does not take the engine with it.
#[test]
fn a_graph_that_does_not_link_or_throws_fails_alone() {
    let mut engine = Engine::with_document(document());
    eval(&mut engine, "globalThis.ran = []; 'ready'");

    let main = "http://fixture.test/main.js";
    compile(
        &mut engine,
        main,
        "import { missing } from './a.js'; ran.push('main');",
    );
    compile(
        &mut engine,
        "http://fixture.test/a.js",
        "ran.push('a'); export const present = 1;",
    );
    let error = engine.run_module(main).expect_err("a missing export");
    assert!(error.to_string().contains("missing"), "{error}");
    assert_eq!(eval(&mut engine, "ran.length"), "0");

    let orphan = "http://fixture.test/orphan.js";
    compile(&mut engine, orphan, "import './never-loaded.js';");
    let error = engine
        .run_module(orphan)
        .expect_err("a module that was not loaded");
    assert!(error.to_string().contains("was not loaded"), "{error}");

    let syntax = engine
        .compile_module("http://fixture.test/bad.js", "export const = ;", &url(PAGE))
        .expect_err("a syntax error");
    assert!(syntax.to_string().contains("SyntaxError"), "{syntax}");

    let throws = "http://fixture.test/throws.js";
    compile(
        &mut engine,
        throws,
        "ran.push('before'); throw new Error('boom');",
    );
    // The evaluation promise rejects; the entry itself returns.
    let _ = engine.run_module(throws);
    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "ran.join()"), "before");
    assert_eq!(
        eval(&mut engine, "1 + 1"),
        "2",
        "the engine is still usable"
    );
}

/// `import()` hands script a promise and leaves a request for the page. Once
/// the page has loaded the graph the promise resolves with the namespace; a
/// graph that cannot be loaded rejects it.
#[test]
fn a_dynamic_import_resolves_with_the_namespace() {
    let mut engine = Engine::with_document(document());
    eval(
        &mut engine,
        "globalThis.log = [];
         import('./widgets/a.js').then(ns => { log.push('a ' + ns.default + ' ' + ns.helper()); });
         import('./widgets/a.js').then(ns => { log.push('again ' + Object.keys(ns).join()); });
         import('/missing.js').then(() => log.push('resolved'), e => { log.push(e.name + ': ' + e.message); });
         import('bare').catch(e => { log.push(e.name); });
         'started'",
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "log.length"),
        "0",
        "nothing settles before the page loads"
    );

    let asked = serve_imports(
        &mut engine,
        &[
            (
                "http://fixture.test/dir/widgets/a.js",
                "import { helper } from './b.js'; export { helper }; export default 'widget';",
            ),
            (
                "http://fixture.test/dir/widgets/b.js",
                "export function helper() { return 'helped'; }",
            ),
        ],
    );
    assert_eq!(
        asked,
        [
            "http://fixture.test/dir/widgets/a.js",
            "http://fixture.test/dir/widgets/a.js",
            "http://fixture.test/missing.js"
        ],
        "resolved against the document; the bare specifier never reached the page"
    );
    // The imports are independent, so only what each settled with is pinned,
    // not the order between them.
    assert_eq!(
        eval(&mut engine, "log.slice().sort().join('|')"),
        "TypeError|\
         TypeError: Failed to fetch dynamically imported module: http://fixture.test/missing.js|\
         a widget helped|again default,helper"
    );
    assert!(engine.take_dynamic_imports().is_empty());
}

/// An `import()` written in a module resolves against that module, and one of
/// a module with top-level `await` settles only when the module has finished.
#[test]
fn a_dynamic_import_follows_its_module_and_waits_for_top_level_await() {
    let mut engine = Engine::with_document(document());
    eval(
        &mut engine,
        "globalThis.log = []; globalThis.release = null; 'ready'",
    );
    let main = "http://fixture.test/app/main.js";
    compile(
        &mut engine,
        main,
        "import('./lazy.js').then(ns => { log.push('lazy ' + ns.value); }, e => { log.push('failed ' + e); });",
    );
    engine.run_module(main).expect("runs");
    engine.run_microtasks();

    let asked = serve_imports(
        &mut engine,
        &[(
            "http://fixture.test/app/lazy.js",
            "log.push('start');
             await new Promise(resolve => { release = resolve; });
             log.push('resumed');
             export const value = 42;",
        )],
    );
    assert_eq!(asked, ["http://fixture.test/app/lazy.js"]);
    assert_eq!(
        eval(&mut engine, "log.join('|')"),
        "start",
        "the import waits"
    );

    eval(&mut engine, "release(); 'released'");
    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "log.join('|')"), "start|resumed|lazy 42");
}
