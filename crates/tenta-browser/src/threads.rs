//! The page-thread pool (`docs/spec/02-architecture.md` §3).
//!
//! A fixed pool, by default one thread per logical core. Each thread runs a
//! `current_thread` runtime and a local task set: page state never leaves the
//! thread it was created on, so the DOM, the parser and the settle machine
//! need no locks against other threads. Sockets are not here — they live on
//! the io runtime — so the thread only ever waits on channels and timers.
//!
//! Every unit of work runs inside `catch_unwind` (§8).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use tenta_core::{Error, Result};
use tenta_page::task::{catch_unwind_future, catch_unwind_task, panic_message};
use tenta_page::thread::{PageJob, PageTask, PageThread};
use tokio::sync::mpsc;

/// Stack size of a page thread. Native recursion over page data is bounded by
/// the DOM depth cap and the iterative traversals in `tenta-dom`, but 8 MB is
/// what `docs/spec/02-architecture.md` §3 requires.
pub const PAGE_THREAD_STACK: usize = 8 * 1024 * 1024;

enum Work {
    Task(PageTask),
    Job(PageJob),
    /// Ends the thread's loop; queued work behind it is dropped.
    Shutdown,
}

/// One page thread.
pub(crate) struct PageThreadHandle {
    index: usize,
    work: mpsc::UnboundedSender<Work>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl PageThreadHandle {
    fn start(index: usize) -> Result<Arc<Self>> {
        let (work, receiver) = mpsc::unbounded_channel();
        let join = std::thread::Builder::new()
            .name(format!("tenta-page-{index}"))
            .stack_size(PAGE_THREAD_STACK)
            .spawn(move || page_thread_main(receiver))
            .map_err(|err| Error::Other {
                code: "page_thread_spawn".to_owned(),
                message: format!("spawning page thread {index}: {err}"),
            })?;
        Ok(Arc::new(Self {
            index,
            work,
            join: Mutex::new(Some(join)),
        }))
    }

    fn send(&self, work: Work) -> Result<()> {
        self.work.send(work).map_err(|_| Error::Closed)
    }

    /// Ends the loop and waits for the thread to finish. Pages that still
    /// hold a handle get [`Error::Closed`] from then on.
    fn shutdown(&self) {
        let join = self.join.lock().take();
        if join.is_some() {
            let _ = self.work.send(Work::Shutdown);
        }
        if let Some(join) = join {
            let _ = join.join();
        }
    }
}

impl PageThread for PageThreadHandle {
    fn post_task(&self, task: PageTask) -> Result<()> {
        self.send(Work::Task(task))
    }

    fn post_job(&self, job: PageJob) -> Result<()> {
        self.send(Work::Job(job))
    }

    fn index(&self) -> usize {
        self.index
    }
}

fn page_thread_main(mut work: mpsc::UnboundedReceiver<Work>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "a page thread could not build its runtime");
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        while let Some(item) = work.recv().await {
            match item {
                Work::Task(task) => {
                    if let Err(reason) = catch_unwind_task(task) {
                        tracing::error!(%reason, "a page task panicked");
                    }
                }
                Work::Shutdown => break,
                Work::Job(job) => {
                    // The job builds its future here, on the page thread, so
                    // the future itself never has to be `Send`.
                    let future = job();
                    tokio::task::spawn_local(async move {
                        if let Err(payload) = catch_unwind_future(future).await {
                            tracing::error!(
                                reason = %panic_message(&payload),
                                "a page job panicked"
                            );
                        }
                    });
                }
            }
        }
    });
    // The tasks that used the engines are gone; tear the engines down here, while
    // the process can still do it in order, rather than during thread exit.
    drop(local);
    tenta_page::drop_thread_runtimes();
}

/// The fixed pool of page threads.
pub(crate) struct PagePool {
    threads: Vec<Arc<PageThreadHandle>>,
    next: AtomicUsize,
}

impl PagePool {
    pub(crate) fn start(threads: usize) -> Result<PagePool> {
        let threads = threads.max(1);
        let mut handles = Vec::with_capacity(threads);
        for index in 0..threads {
            handles.push(PageThreadHandle::start(index)?);
        }
        Ok(PagePool {
            threads: handles,
            next: AtomicUsize::new(0),
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.threads.len()
    }

    /// The next thread in round-robin order. A page group is pinned to the
    /// thread it is given here for its life.
    pub(crate) fn assign(&self) -> Arc<dyn PageThread> {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.threads.len();
        Arc::clone(&self.threads[index]) as Arc<dyn PageThread>
    }

    /// Stops every thread and waits for them.
    pub(crate) fn shutdown(&mut self) {
        for handle in std::mem::take(&mut self.threads) {
            handle.shutdown();
        }
    }
}

impl Drop for PagePool {
    fn drop(&mut self) {
        self.shutdown();
    }
}
