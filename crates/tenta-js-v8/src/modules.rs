//! Module scripts on V8: the module map, linking, evaluation, `import()` and
//! `import.meta` (`docs/spec/05-javascript.md`; HTML "module scripts").
//!
//! The page loads a module graph and the engine evaluates it, and the two meet
//! over the *module map* kept here. The page compiles each module it fetched
//! under a key — the module's URL, or a name of the page's own for an inline
//! `<script type="module">` — and is told which URLs that module imports, so
//! it can fetch those too. Nothing is linked until the page runs the root:
//! V8 asks for each dependency synchronously while it links, and by then every
//! one of them is in the map.
//!
//! `import()` is the same flow started by script: the host callback hands
//! script a promise and leaves a request behind, the page loads that graph,
//! and [`finish_import`] evaluates it and settles the promise with the
//! module's namespace.
//!
//! # What a record holds
//!
//! A `v8::Global<v8::Module>`, which has to be dropped while the isolate is
//! alive; [`ModuleState::clear`] is that obligation, and `Engine::Drop` calls
//! it beside the other teardown drains.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use tenta_js::bindings::v8_registered_slow_callback_boundary;
use tenta_js::engine::ScriptErrorKind;
use tenta_js::modules::{DynamicImport, ImportMap, resolve_specifier};
use url::Url;

use crate::ScriptFailure;
use crate::host::{HostState, host_state};
use crate::inspector;

/// One compiled module.
struct Record {
    module: v8::Global<v8::Module>,
    /// What the module's own specifiers resolve against, and its
    /// `import.meta.url`: its URL, or the document's base for an inline one.
    base: Url,
}

/// An `import()` whose specifier has not been resolved yet. Resolution needs
/// the document's base URL for a classic script, which the host callback has
/// no way to reach, so it happens when the page collects the requests.
struct RawImport {
    id: u64,
    specifier: String,
    /// The resource name of the script that called `import()`: a module's key,
    /// or nothing for a classic script.
    referrer: Option<String>,
    /// Why the import cannot be served at all, decided in the callback.
    refused: Option<String>,
}

/// The module map of one realm and the dynamic imports in flight.
#[derive(Default)]
pub(crate) struct ModuleState {
    records: RefCell<HashMap<String, Record>>,
    /// Keys by `Module::get_identity_hash`, which is how a callback that is
    /// handed a module finds its record. A hash may collide, so each bucket
    /// is confirmed by handle identity.
    by_hash: RefCell<HashMap<i32, Vec<String>>>,
    import_map: RefCell<ImportMap>,
    raw: RefCell<Vec<RawImport>>,
    waiting: RefCell<HashMap<u64, v8::Global<v8::PromiseResolver>>>,
    next_import: Cell<u64>,
}

impl ModuleState {
    /// Drops every V8 handle. Must run while the isolate is alive.
    pub(crate) fn clear(&self) {
        self.records.borrow_mut().clear();
        self.by_hash.borrow_mut().clear();
        self.raw.borrow_mut().clear();
        self.waiting.borrow_mut().clear();
    }

    /// Whether script has made an `import()` the page has not collected yet.
    /// The page asks after every engine entry, so the answer has to cost
    /// nothing when there is none.
    pub(crate) fn has_imports(&self) -> bool {
        !self.raw.borrow().is_empty()
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.records.borrow().contains_key(key)
    }

    pub(crate) fn merge_import_map(&self, map: ImportMap) {
        self.import_map.borrow_mut().merge(map);
    }

    /// The key and base of the record `module` belongs to.
    fn record_of(
        &self,
        scope: &mut v8::PinScope,
        module: v8::Local<v8::Module>,
    ) -> Option<(String, Url)> {
        let hash = module.get_identity_hash().get();
        let keys = self.by_hash.borrow().get(&hash).cloned()?;
        let records = self.records.borrow();
        keys.into_iter().find_map(|key| {
            let record = records.get(&key)?;
            (v8::Local::new(scope, &record.module) == module).then(|| (key, record.base.clone()))
        })
    }

    fn local<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        key: &str,
    ) -> Option<v8::Local<'s, v8::Module>> {
        let records = self.records.borrow();
        Some(v8::Local::new(scope, &records.get(key)?.module))
    }

    /// The dynamic imports script has asked for since the last call, resolved
    /// against the script that asked. One that cannot be resolved is rejected
    /// here and not returned.
    pub(crate) fn take_imports(
        &self,
        scope: &mut v8::PinScope,
        document_base: Option<&Url>,
    ) -> Vec<DynamicImport> {
        let raw = std::mem::take(&mut *self.raw.borrow_mut());
        let mut out = Vec::new();
        for import in raw {
            let resolved = match import.refused {
                Some(reason) => Err(reason),
                None => {
                    let base = import
                        .referrer
                        .as_deref()
                        .and_then(|key| {
                            self.records
                                .borrow()
                                .get(key)
                                .map(|record| record.base.clone())
                        })
                        .or_else(|| document_base.cloned());
                    match base {
                        Some(base) => {
                            resolve_specifier(&import.specifier, &base, &self.import_map.borrow())
                        }
                        None => Err("import() has no base URL to resolve against".to_owned()),
                    }
                }
            };
            match resolved {
                Ok(url) => out.push(DynamicImport { id: import.id, url }),
                Err(message) => self.reject(scope, import.id, &message),
            }
        }
        out
    }

    /// Rejects the promise of import `id` with a `TypeError`.
    fn reject(&self, scope: &mut v8::PinScope, id: u64, message: &str) {
        let Some(resolver) = self.waiting.borrow_mut().remove(&id) else {
            return;
        };
        let resolver = v8::Local::new(scope, &resolver);
        let Some(message) = v8::String::new(scope, message) else {
            return;
        };
        let error = v8::Exception::type_error(scope, message);
        let _ = resolver.reject(scope, error);
    }
}

fn failure(kind: ScriptErrorKind, message: impl Into<String>) -> ScriptFailure {
    ScriptFailure {
        kind,
        message: message.into(),
    }
}

/// The failure a `TryCatch` holds, reported to the inspector as an uncaught
/// exception of the page.
fn caught(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
    kind: ScriptErrorKind,
) -> ScriptFailure {
    if tc.has_terminated() {
        return failure(
            ScriptErrorKind::Terminated,
            "script execution was terminated",
        );
    }
    let message = crate::exception_message(tc);
    inspector::report_caught(tc, host);
    tc.reset();
    failure(kind, message)
}

/// Throws a `TypeError` with `message` and reports it, for a failure the
/// engine found rather than script.
fn refuse(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
    message: &str,
) -> ScriptFailure {
    if let Some(text) = v8::String::new(tc, message) {
        let error = v8::Exception::type_error(tc, text);
        tc.throw_exception(error);
        inspector::report_caught(tc, host);
        tc.reset();
    }
    failure(ScriptErrorKind::Uncaught, message)
}

/// Compiles `source` as the module `key` and returns the URLs it imports
/// statically, each once, in source order.
pub(crate) fn compile(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
    key: &str,
    source: &str,
    base: &Url,
) -> Result<Vec<Url>, ScriptFailure> {
    let state = &host.modules;
    let (Some(code), Some(name)) = (v8::String::new(tc, source), v8::String::new(tc, key)) else {
        return Err(failure(
            ScriptErrorKind::Syntax,
            "source string exceeds the V8 string length limit",
        ));
    };
    let origin = v8::ScriptOrigin::new(
        tc,
        name.into(),
        0,
        0,
        false,
        -1,
        None,
        false,
        false,
        true,
        None,
    );
    let mut compiled = v8::script_compiler::Source::new(code, Some(&origin));
    let Some(module) = v8::script_compiler::compile_module(tc, &mut compiled) else {
        return Err(caught(tc, host, ScriptErrorKind::Syntax));
    };

    let requests = module.get_module_requests();
    let mut urls: Vec<Url> = Vec::new();
    for index in 0..requests.length() {
        let Some(request) = requests
            .get(tc, index)
            .and_then(|data| v8::Local::<v8::ModuleRequest>::try_from(data).ok())
        else {
            continue;
        };
        let specifier = request.get_specifier().to_rust_string_lossy(tc);
        if request.get_import_attributes().length() > 0 {
            return Err(refuse(
                tc,
                host,
                &format!("import attributes are not supported (importing \"{specifier}\")"),
            ));
        }
        let resolved = resolve_specifier(&specifier, base, &state.import_map.borrow());
        match resolved {
            Ok(url) => {
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
            Err(message) => return Err(refuse(tc, host, &message)),
        }
    }

    let hash = module.get_identity_hash().get();
    state.records.borrow_mut().insert(
        key.to_owned(),
        Record {
            module: v8::Global::new(tc, module),
            base: base.clone(),
        },
    );
    state
        .by_hash
        .borrow_mut()
        .entry(hash)
        .or_default()
        .push(key.to_owned());
    Ok(urls)
}

/// Runs the module `key` as a module script of the document: links it if it
/// is not linked yet and evaluates it. A module that throws rejects its
/// evaluation promise, and the realm's unhandled-rejection report is what
/// tells the page.
pub(crate) fn run(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
    key: &str,
) -> Result<(), ScriptFailure> {
    let Some(module) = host.modules.local(tc, key) else {
        return Err(failure(
            ScriptErrorKind::Uncaught,
            format!("the module {key} was never compiled"),
        ));
    };
    if module.get_status() == v8::ModuleStatus::Uninstantiated
        && module.instantiate_module(tc, resolve_callback) != Some(true)
    {
        return Err(caught(tc, host, ScriptErrorKind::Uncaught));
    }
    match module.evaluate(tc) {
        Some(_) => Ok(()),
        None => Err(caught(tc, host, ScriptErrorKind::Uncaught)),
    }
}

/// Settles the promise of import `id`: with the namespace of the module `key`
/// once it has evaluated, or with why its graph could not be loaded.
pub(crate) fn finish_import(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    host: &HostState,
    id: u64,
    outcome: Result<&str, &str>,
) {
    let state = &host.modules;
    let key = match outcome {
        Ok(key) => key,
        Err(message) => {
            state.reject(tc, id, message);
            return;
        }
    };
    let Some(module) = state.local(tc, key) else {
        state.reject(tc, id, &format!("the module {key} was never compiled"));
        return;
    };
    let linked = module.get_status() != v8::ModuleStatus::Uninstantiated
        || module.instantiate_module(tc, resolve_callback) == Some(true);
    let evaluated = if linked { module.evaluate(tc) } else { None };
    let Some(evaluated) = evaluated else {
        // A graph that does not link, or a module that cannot start: the
        // import rejects with the reason, as `import()` of a module with a bad
        // import does.
        let reason = tc.exception();
        tc.reset();
        if let (Some(reason), Some(resolver)) = (reason, state.waiting.borrow_mut().remove(&id)) {
            let resolver = v8::Local::new(tc, &resolver);
            let _ = resolver.reject(tc, reason);
        }
        return;
    };
    let namespace = module.get_module_namespace();
    // The evaluation promise settles when the module and everything it awaits
    // at the top level has run; the import settles with it. The reactions
    // find the resolver by the import's id, which is exact in a double for
    // any count a page can reach.
    let Ok(evaluation) = v8::Local::<v8::Promise>::try_from(evaluated) else {
        settle_import(tc, state, id, Ok(namespace));
        return;
    };
    let pair = v8::Array::new(tc, 2);
    let number = v8::Number::new(tc, id as f64);
    let _ = pair.set_index(tc, 0, number.into());
    let _ = pair.set_index(tc, 1, namespace);
    let fulfilled = v8::Function::builder(import_fulfilled)
        .data(pair.into())
        .build(tc);
    let rejected = v8::Function::builder(import_rejected)
        .data(pair.into())
        .build(tc);
    if let (Some(fulfilled), Some(rejected)) = (fulfilled, rejected) {
        let _ = evaluation.then2(tc, fulfilled, rejected);
    }
}

/// Resolves or rejects the promise of import `id` and forgets it.
fn settle_import(
    scope: &mut v8::PinScope,
    state: &ModuleState,
    id: u64,
    outcome: Result<v8::Local<v8::Value>, v8::Local<v8::Value>>,
) {
    let Some(resolver) = state.waiting.borrow_mut().remove(&id) else {
        return;
    };
    let resolver = v8::Local::new(scope, &resolver);
    let _ = match outcome {
        Ok(value) => resolver.resolve(scope, value),
        Err(reason) => resolver.reject(scope, reason),
    };
}

/// The import id and the namespace an import's reaction was built with.
fn import_pair<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
) -> Option<(u64, v8::Local<'s, v8::Value>)> {
    let pair = v8::Local::<v8::Array>::try_from(args.data()).ok()?;
    let id = pair.get_index(scope, 0)?.number_value(scope)?;
    Some((id as u64, pair.get_index(scope, 1)?))
}

fn import_fulfilled<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _retval: v8::ReturnValue<'s, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let (Some(host), Some((id, namespace))) = (host_state(scope), import_pair(scope, &args))
        else {
            return;
        };
        settle_import(scope, &host.modules, id, Ok(namespace));
    };
    // SAFETY: a registered V8 function callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "import_fulfilled", (), call) };
}

fn import_rejected<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _retval: v8::ReturnValue<'s, v8::Value>,
) {
    let call = |scope: &mut v8::PinScope<'s, '_>| {
        let (Some(host), Some((id, _))) = (host_state(scope), import_pair(scope, &args)) else {
            return;
        };
        settle_import(scope, &host.modules, id, Err(args.get(0)));
    };
    // SAFETY: a registered V8 function callback, where allocation is allowed.
    unsafe { v8_registered_slow_callback_boundary(scope, "import_rejected", (), call) };
}

/// Runs `call` so that a panic in it poisons the isolate instead of unwinding
/// into V8 (`docs/spec/02-architecture.md` §8).
fn contained<R>(
    scope: &mut v8::PinScope,
    name: &'static str,
    fallback: R,
    call: impl FnOnce(&mut v8::PinScope) -> R,
) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(scope))) {
        Ok(value) => value,
        Err(payload) => {
            tenta_js::bindings::poison_native_callback(scope, name);
            if let Err(nested) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
            {
                std::mem::forget(nested);
            }
            fallback
        }
    }
}

/// V8's question while linking: which module does `specifier`, written in
/// `referrer`, name? Every answer is already in the map, because the page
/// loaded the whole graph before it asked for the root to run.
fn resolve_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe let scope, context);
    let found = contained(scope, "module_resolve", None, |scope| {
        let host = host_state(scope)?;
        let state = &host.modules;
        let specifier = specifier.to_rust_string_lossy(scope);
        let resolved = state
            .record_of(scope, referrer)
            .ok_or_else(|| "the importing module is not in the module map".to_owned())
            .and_then(|(_, base)| resolve_specifier(&specifier, &base, &state.import_map.borrow()));
        let message = match resolved {
            Ok(url) => match state.records.borrow().get(url.as_str()) {
                Some(record) => return Some(v8::Global::clone(&record.module)),
                None => format!("the module {url} was not loaded"),
            },
            Err(message) => message,
        };
        if let Some(text) = v8::String::new(scope, &message) {
            let error = v8::Exception::type_error(scope, text);
            scope.throw_exception(error);
        }
        None
    })?;
    let local = v8::Local::new(scope, &found);
    // SAFETY: the handle lives in the caller's handle scope, which is the one
    // `context` came from; only the lifetime the callback scope gave it is
    // widened to that one.
    Some(unsafe {
        std::mem::transmute::<v8::Local<'_, v8::Module>, v8::Local<'s, v8::Module>>(local)
    })
}

/// `import()`: hands script a promise and leaves the request for the page.
pub(crate) fn dynamic_import_callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);
    contained(scope, "dynamic_import", (), |scope| {
        let Some(host) = host_state(scope) else {
            return;
        };
        let state = &host.modules;
        let id = state.next_import.get() + 1;
        state.next_import.set(id);
        let referrer = resource_name
            .is_string()
            .then(|| resource_name.to_rust_string_lossy(scope));
        let refused = (import_attributes.length() > 0)
            .then(|| "import attributes are not supported".to_owned());
        state
            .waiting
            .borrow_mut()
            .insert(id, v8::Global::new(scope, resolver));
        state.raw.borrow_mut().push(RawImport {
            id,
            specifier: specifier.to_rust_string_lossy(scope),
            referrer,
            refused,
        });
        // The page collects the request when this entry returns.
        host.owe_checkpoint();
    });
    Some(promise)
}

/// `import.meta`: `url` is the module's own URL.
pub(crate) unsafe extern "C" fn import_meta_callback(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    v8::callback_scope!(unsafe let scope, context);
    contained(scope, "import_meta", (), |scope| {
        let Some(host) = host_state(scope) else {
            return;
        };
        let Some((_, base)) = host.modules.record_of(scope, module) else {
            return;
        };
        let (Some(name), Some(url)) = (
            v8::String::new(scope, "url"),
            v8::String::new(scope, base.as_str()),
        ) else {
            return;
        };
        let _ = meta.create_data_property(scope, name.into(), url.into());
    });
}
