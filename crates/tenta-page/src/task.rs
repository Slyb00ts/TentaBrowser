//! Panic containment for page-thread work (`docs/spec/02-architecture.md` §8).
//!
//! Every task on a page thread runs inside `catch_unwind`: a panic marks its
//! page crashed and leaves the thread's other pages alone. A Rust panic that
//! escaped a page thread would take the process down with it, so the guard is
//! not optional.

use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

/// A future that catches a panic from the future it wraps.
pub struct CatchUnwind<F> {
    future: Pin<Box<F>>,
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `Pin<Box<F>>` is `Unpin`, so the wrapper can be moved out of its own
        // pin safely and the inner future is still pinned.
        let this = self.get_mut();
        let future = &mut this.future;
        match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// Wraps `future` so a panic inside it becomes a value.
pub fn catch_unwind_future<F: Future>(future: F) -> CatchUnwind<F> {
    CatchUnwind {
        future: Box::pin(future),
    }
}

/// Runs `task` and turns a panic into a message.
pub fn catch_unwind_task(task: impl FnOnce()) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(task)).map_err(|payload| panic_message(&payload))
}

/// The message of a caught panic.
pub fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panic_in_a_future_becomes_a_value() {
        let caught = catch_unwind_future(async { panic!("parse exploded") }).await;
        let payload = caught.expect_err("the panic must be caught");
        assert_eq!(panic_message(&payload), "parse exploded");
    }

    #[tokio::test]
    async fn a_future_that_does_not_panic_yields_its_value() {
        let value = catch_unwind_future(async { 7u32 }).await.expect("no panic");
        assert_eq!(value, 7);
    }

    #[test]
    fn a_panicking_task_reports_its_message() {
        let error = catch_unwind_task(|| panic!("{}", "boom".to_owned())).expect_err("panic");
        assert_eq!(error, "boom");
        assert!(catch_unwind_task(|| ()).is_ok());
    }
}
