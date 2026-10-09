//! Budgets (`docs/spec/14-memory.md` §1 and §3, `05-javascript.md` §11).
//! Exceeding a hard budget crashes the page, never the process.

use std::fmt;
use std::time::Duration;

/// Which budget a limit belongs to; reported in [`crate::Error::BudgetExceeded`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BudgetKind {
    PageMemory,
    ContextMemory,
    JsHeap,
    ScriptCpu,
    EvaluateTime,
    DomNodes,
    WorkerThreads,
    NetworkBytes,
    NetworkRequests,
}

impl fmt::Display for BudgetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BudgetKind::PageMemory => "page memory",
            BudgetKind::ContextMemory => "context memory",
            BudgetKind::JsHeap => "js heap",
            BudgetKind::ScriptCpu => "script cpu",
            BudgetKind::EvaluateTime => "evaluate time",
            BudgetKind::DomNodes => "dom nodes",
            BudgetKind::WorkerThreads => "worker threads",
            BudgetKind::NetworkBytes => "network bytes",
            BudgetKind::NetworkRequests => "network requests",
        })
    }
}

/// Engine-wide defaults; contexts and pages may lower them, never raise them
/// above the engine's values.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Budgets {
    /// Hard RSS-equivalent budget per page in MB (desktop default 256).
    pub page_memory_mb: u32,
    /// Storage plus cache bytes per context in MB (desktop default 64).
    pub context_memory_mb: u32,
    /// JS heap limit per isolate or runtime in MB (desktop default 256).
    pub js_heap_mb: u32,
    /// Script CPU time a page may consume within any sliding 30 s window of wall
    /// time (`docs/spec/18-decisions.md` D-033). A rate limit, not a lifetime
    /// total: a long-lived idle page never reaches it.
    pub script_cpu: Duration,
    /// Per-call deadline for `evaluate`.
    pub evaluate_time: Duration,
    /// Maximum live DOM nodes per document.
    pub dom_nodes: u32,
    /// Worker OS threads per page (desktop default 32, mobile 4).
    pub worker_threads: u16,
    /// Response body bytes per document load.
    pub network_bytes: u64,
    /// Requests per document load.
    pub network_requests: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self::desktop()
    }
}

impl Budgets {
    pub fn desktop() -> Self {
        Self {
            page_memory_mb: 256,
            context_memory_mb: 64,
            js_heap_mb: 256,
            script_cpu: Duration::from_secs(15),
            evaluate_time: Duration::from_secs(30),
            dom_nodes: 2_000_000,
            worker_threads: 32,
            network_bytes: 512 * 1024 * 1024,
            network_requests: 10_000,
        }
    }

    pub fn mobile() -> Self {
        Self {
            page_memory_mb: 96,
            context_memory_mb: 24,
            js_heap_mb: 96,
            script_cpu: Duration::from_millis(7_500),
            evaluate_time: Duration::from_secs(15),
            dom_nodes: 500_000,
            worker_threads: 4,
            network_bytes: 128 * 1024 * 1024,
            network_requests: 4_000,
        }
    }

    /// Defaults for the platform this binary was built for.
    pub fn platform_default() -> Self {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            Self::mobile()
        } else {
            Self::desktop()
        }
    }

    pub fn page_memory_mb(mut self, mb: u32) -> Self {
        self.page_memory_mb = mb;
        self
    }

    pub fn js_heap_mb(mut self, mb: u32) -> Self {
        self.js_heap_mb = mb;
        self
    }

    pub fn script_cpu(mut self, d: Duration) -> Self {
        self.script_cpu = d;
        self
    }

    pub fn evaluate_time(mut self, d: Duration) -> Self {
        self.evaluate_time = d;
        self
    }

    pub fn worker_threads(mut self, n: u16) -> Self {
        self.worker_threads = n;
        self
    }

    /// Returns a copy where every field is at most the corresponding field
    /// of `ceiling`. Contexts and pages derive their budgets this way.
    pub fn clamped_to(&self, ceiling: &Budgets) -> Budgets {
        Budgets {
            page_memory_mb: self.page_memory_mb.min(ceiling.page_memory_mb),
            context_memory_mb: self.context_memory_mb.min(ceiling.context_memory_mb),
            js_heap_mb: self.js_heap_mb.min(ceiling.js_heap_mb),
            script_cpu: self.script_cpu.min(ceiling.script_cpu),
            evaluate_time: self.evaluate_time.min(ceiling.evaluate_time),
            dom_nodes: self.dom_nodes.min(ceiling.dom_nodes),
            worker_threads: self.worker_threads.min(ceiling.worker_threads),
            network_bytes: self.network_bytes.min(ceiling.network_bytes),
            network_requests: self.network_requests.min(ceiling.network_requests),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_and_clamping() {
        let engine = Budgets::desktop().page_memory_mb(128);
        let page = Budgets::desktop().page_memory_mb(512).worker_threads(8);
        let effective = page.clamped_to(&engine);
        assert_eq!(effective.page_memory_mb, 128);
        assert_eq!(effective.worker_threads, 8);
        assert!(Budgets::mobile().page_memory_mb < Budgets::desktop().page_memory_mb);
    }

    #[test]
    fn the_script_cpu_builder_sets_the_field_and_clamps() {
        let lowered = Budgets::desktop().script_cpu(Duration::from_millis(250));
        assert_eq!(lowered.script_cpu, Duration::from_millis(250));

        let engine = Budgets::desktop();
        let page = Budgets::desktop().script_cpu(Duration::from_secs(600));
        assert_eq!(
            page.clamped_to(&engine).script_cpu,
            engine.script_cpu,
            "a page may lower the engine's budget but never raise it"
        );
    }
}
