//! Scripts that script inserts, on the engine's side
//! (`tenta_js::scripts`).
//!
//! The document records the script elements that became ready to run; this
//! prepares each of them at the first point script has control again, which
//! is the end of the binding call that made them ready. An inline classic
//! script runs right there, before that call returns to the page, as HTML's
//! "execute the script element" has it for a script that is not external.
//! Every other script that is going to run is left for the page to load
//! ([`State::take`]).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::NodeRef;
use tenta_js::bindings::host::SharedContext;
use tenta_js::scripts::{InsertedScript, Prepared, prepare};

use super::{HostState, host_state};
use crate::inspector;

/// The inserted scripts the page has not taken yet.
#[derive(Default)]
pub(crate) struct State {
    requests: RefCell<Vec<InsertedScript>>,
}

impl State {
    pub(crate) fn has_requests(&self) -> bool {
        !self.requests.borrow().is_empty()
    }

    pub(crate) fn take(&self) -> Vec<InsertedScript> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }
}

/// Prepares the script elements the document has recorded, if it has any and
/// no borrow of it is outstanding. Called after every binding call.
pub(crate) fn after_callback(scope: &mut v8::PinScope, context: &SharedContext) {
    if context
        .doc
        .try_borrow()
        .is_ok_and(|doc| doc.has_ready_scripts())
    {
        flush(scope, context);
    }
}

/// Prepares every script element the document has recorded. A script that
/// runs here can insert more; those are prepared by the binding call that
/// inserted them, or by the next round of this loop.
pub(crate) fn flush(scope: &mut v8::PinScope, context: &SharedContext) {
    let Some(host) = host_state(scope) else {
        return;
    };
    loop {
        let ready = match context.doc.try_borrow_mut() {
            Ok(mut doc) => doc.take_ready_scripts(),
            Err(_) => return,
        };
        if ready.is_empty() {
            return;
        }
        for script in ready {
            if scope.is_execution_terminating() {
                return;
            }
            let prepared = match context.doc.try_borrow_mut() {
                Ok(mut doc) => prepare(&mut doc, script, true, host.scheduler.current_origin()),
                Err(_) => return,
            };
            match prepared {
                Prepared::Skip => {}
                Prepared::Request(request) => host.scripts.requests.borrow_mut().push(request),
                Prepared::RunNow { element, source } => {
                    v8::scope!(let scope, scope);
                    run_inline(scope, &host, context, element, &source);
                }
            }
        }
    }
}

/// Runs an inline classic script with `element` as `document.currentScript`.
///
/// What it throws is reported as any uncaught exception of a script is, and
/// does not reach the page code whose DOM call inserted the element: that
/// call has succeeded.
fn run_inline(
    scope: &mut v8::PinScope,
    host: &Rc<HostState>,
    context: &SharedContext,
    element: NodeRef,
    source: &str,
) {
    let previous = {
        let mut doc = context.doc.borrow_mut();
        let previous = doc.current_script().map(|node| doc.node_ref(node));
        doc.set_current_script(Some(element));
        previous
    };
    {
        v8::tc_scope!(let tc, scope);
        let ran = v8::String::new(tc, source)
            .and_then(|code| v8::Script::compile(tc, code, None))
            .and_then(|script| script.run(tc));
        // A termination is the page's to see; anything else is this script's
        // own failure.
        if ran.is_none() && !tc.has_terminated() {
            inspector::report_caught(tc, host);
            tc.reset();
        }
    }
    if let Ok(mut doc) = context.doc.try_borrow_mut() {
        doc.set_current_script(previous);
    }
}
