//! Loading a module graph (HTML's "fetch the descendants of and link a module
//! script").
//!
//! The engine evaluates modules; the page loads them, because loading awaits
//! the network and nothing may hold the engine across an await. The two take
//! turns: the page fetches a module and hands its text to the engine, the
//! engine compiles it and names what it imports, and the page fetches those.
//! When nothing is left to fetch, every module the root will ask for while it
//! links is in the engine's module map, and the root can run.
//!
//! One function serves both entries into it — a `<script type="module">` the
//! parser found and an `import()` a script made — since they differ only in
//! where the root comes from and in what happens once the graph is there.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use tenta_dom::Document as DomDocument;
use tokio::task::JoinHandle;
use url::Url;

use super::PageInner;
use crate::runtime::{self, ModuleRoot, PageRuntime, ScriptMeta};
use crate::settle::SettleTracker;

/// A graph larger than this is not a program a page ships; it is a loop of
/// generated specifiers, and following it would only end at the deadline.
const MAX_GRAPH_MODULES: usize = 4096;

const GONE: &str = "the document that asked for the module is gone";

/// A load that ends with the loader that started it: when the loader's own
/// task is dropped, by a navigation or a close, the loads it had in flight
/// stop too. A load another graph is also waiting for carries on there.
struct OwnedLoad(JoinHandle<Result<Vec<Url>, String>>);

impl Drop for OwnedLoad {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Enters the runtime of the page if it is still the one bound to `document`.
/// The tree is named weakly and upgraded only for the length of the entry, for
/// the reason a parallel script's task names it weakly: a loader may not keep
/// alive a tree its navigation is done with.
fn in_runtime<R>(
    inner: &PageInner,
    document: &Weak<RefCell<DomDocument>>,
    f: impl FnOnce(&mut PageRuntime) -> R,
) -> Result<R, String> {
    let document = document.upgrade().ok_or_else(|| GONE.to_owned())?;
    runtime::with_runtime_for(inner.config.id, &document, f).ok_or_else(|| GONE.to_owned())
}

/// Makes sure the module at `url` is fetched and compiled, and returns what
/// it imports. Every graph that needs the module goes through the same load
/// (`PageRuntime::module_load`), so it is fetched and compiled once however
/// many graphs are loading at the same time.
async fn ensure_module(
    inner: Arc<PageInner>,
    tracker: Rc<RefCell<SettleTracker>>,
    meta: ScriptMeta,
    document: Weak<RefCell<DomDocument>>,
    url: Url,
) -> Result<Vec<Url>, String> {
    let load = in_runtime(&inner, &document, |runtime| {
        runtime.module_load(url.as_str())
    })?;
    load.get_or_init(|| async {
        let fetched = runtime::fetch_module(&inner.config, &meta, &url, &tracker).await?;
        in_runtime(&inner, &document, |runtime| {
            runtime.compile_module(url.as_str(), &fetched.source, &fetched.url)
        })?
        .ok_or_else(|| format!("the module {url} did not compile"))
    })
    .await
    .clone()
}

/// Loads the module graph that starts at `root` into the engine's module map
/// and returns the root's key, ready to run. `Err` is why the graph cannot
/// run: a module that could not be fetched or did not compile.
pub(crate) async fn load_graph(
    inner: &Arc<PageInner>,
    tracker: &Rc<RefCell<SettleTracker>>,
    meta: &ScriptMeta,
    document: &Weak<RefCell<DomDocument>>,
    root: ModuleRoot,
) -> Result<String, String> {
    let mut queue: Vec<Url> = Vec::new();
    let key = match root {
        ModuleRoot::Inline { key, source, base } => {
            let imports = in_runtime(inner, document, |runtime| {
                runtime.compile_module(&key, &source, &base)
            })?
            .ok_or_else(|| "the module script did not compile".to_owned())?;
            queue.extend(imports);
            key
        }
        ModuleRoot::External(url) => {
            let key = url.to_string();
            queue.push(url);
            key
        }
    };
    let mut visited: HashSet<String> = HashSet::new();
    while !queue.is_empty() {
        // One level of the graph at a time: everything it needs is requested
        // together, and the next level is what those modules import.
        let mut loads: Vec<OwnedLoad> = Vec::new();
        for url in std::mem::take(&mut queue) {
            if !visited.insert(url.to_string()) {
                continue;
            }
            if visited.len() > MAX_GRAPH_MODULES {
                return Err(format!(
                    "the module graph has more than {MAX_GRAPH_MODULES} modules"
                ));
            }
            loads.push(OwnedLoad(tokio::task::spawn_local(ensure_module(
                Arc::clone(inner),
                Rc::clone(tracker),
                meta.clone(),
                document.clone(),
                url,
            ))));
        }
        for mut load in loads {
            let imports = (&mut load.0)
                .await
                .map_err(|_| "the load of a module did not finish".to_owned())??;
            queue.extend(imports);
        }
    }
    Ok(key)
}
