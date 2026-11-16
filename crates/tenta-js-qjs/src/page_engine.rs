//! Assembly boundary for a QuickJS-backed page.
use crate::{Engine, host};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tenta_core::{Origin, TimerRule};
use tenta_js::bindings::host::{
    SharedDocument, deliver_xhr_completion, encode_host, encode_node, insert_fetched_response,
};
use tenta_js::bindings::{Handle, Response, handle_to_quickjs};
use tenta_js::dom::events::{
    DispatchTarget, EventKind, EventRecord, HostValue, PromiseOutcome, ScriptHost, dispatch_at,
};
use tenta_js::engine::*;
use tenta_js::event_loop::{Clock, MonoTime, ProvenanceSink};
use tenta_js::fetch::{FetchCompletion, FetchDriver, FetchOutcome};
use tenta_js::geometry::GeometryHost;
use tenta_js::inspect::InspectorSink;
use tenta_js::xhr::{XhrCompletion, XhrDriver};
impl PageEngine for Engine {
    fn native_failure(&self) -> Option<&'static str> {
        Engine::native_failure(self)
    }
    fn document(&self) -> SharedDocument {
        self.state.world.host.doc.clone()
    }
    fn run_script(&mut self, source: &str, kind: ScriptKind) -> Result<(), ScriptError> {
        if !matches!(kind, ScriptKind::Classic) {
            return Err(ScriptError {
                kind: ScriptErrorKind::Uncaught,
                message: format!("QuickJS does not support {kind:?} page scripts yet"),
            });
        }
        let scheduler = self.event_loop.scheduler();
        scheduler.with_origin(Origin::Tracked, || {
            self.entry(self.script_timeout, |ctx| {
                crate::eval_classic::<rquickjs::Value>(ctx, source).map(|_| ())
            })
        })
    }
    fn evaluate(&mut self, source: &str, timeout: Duration) -> Result<String, ScriptError> {
        self.evaluate_with_timeout(source, Some(timeout))
    }
    fn set_inspector_sink(&mut self, sink: Rc<dyn InspectorSink>) {
        self.install_inspector(sink);
    }
    fn inspector_call(
        &mut self,
        op: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.call_inspector(op, argument, timeout)
    }
    fn inspector_run(
        &mut self,
        kind: &str,
        source: &str,
        argument: &str,
        timeout: Duration,
    ) -> Result<String, ScriptError> {
        self.run_inspector(kind, source, argument, timeout)
    }
    fn dispatch_lifecycle(&mut self, event: LifecycleEvent) {
        let scheduler = self.event_loop.scheduler();
        let state = self.state.clone();
        scheduler.with_origin(Origin::Tracked, || {
            let _ = self.entry(self.script_timeout, |_| {
                let host = &state.world.host;
                let document = {
                    let doc = host.doc.borrow();
                    doc.node_ref(doc.document_node())
                };
                let (target, name, bubbles) = match event {
                    LifecycleEvent::DomContentLoaded => (
                        DispatchTarget::node(host.slot, document),
                        "DOMContentLoaded",
                        true,
                    ),
                    LifecycleEvent::Load => (
                        DispatchTarget::window()
                            .with_target_override(encode_node(host.slot, document)),
                        "load",
                        false,
                    ),
                    _ => return Ok(()),
                };
                let mut record =
                    EventRecord::new(name.to_owned(), EventKind::Plain, host.time_stamp());
                record.bubbles = bubbles;
                record.flags.initialized = true;
                let key = host.events.borrow_mut().insert(record);
                let _ = dispatch_at(host, target, key, true);
                let orphan = host
                    .events
                    .borrow()
                    .get(key)
                    .is_some_and(|e| !e.has_wrapper);
                if orphan {
                    host.events.borrow_mut().remove(key);
                }
                Ok(())
            });
        });
    }
    fn set_timer_rule(&mut self, rule: TimerRule) {
        self.state.scheduler.set_timer_rule(rule)
    }
    fn set_provenance_sink(&mut self, sink: Rc<dyn ProvenanceSink>) {
        self.event_loop.task_sink().set_provenance_sink(sink)
    }
    fn set_fetch_driver(&mut self, driver: Rc<dyn FetchDriver>) {
        *self.state.fetch_driver.borrow_mut() = Some(driver)
    }
    fn set_xhr_driver(&mut self, driver: Rc<dyn XhrDriver>) {
        *self.state.xhr_driver.borrow_mut() = Some(driver)
    }
    fn set_geometry_host(&mut self, host: Rc<dyn GeometryHost>) {
        *self.state.geometry_host.borrow_mut() = Some(host)
    }
    fn settle_fetches(&mut self, completions: Vec<FetchCompletion>) {
        if completions.is_empty() {
            return;
        }
        let state = self.state.clone();
        let _ = self.entry(self.script_timeout, |ctx| {
            for completion in completions {
                match completion.outcome {
                    FetchOutcome::Response(response) => {
                        let key = insert_fetched_response(&state.world.host, response);
                        let value = handle_to_quickjs::<Response>(
                            ctx,
                            Handle::from_bits(encode_host(state.world.host.slot, key)),
                        )?;
                        state.settle(ctx, completion.resolver, value, false);
                    }
                    FetchOutcome::Failed(message) => {
                        state.settle_promise(
                            completion.resolver,
                            PromiseOutcome::Reject(HostValue::TypeError { message: &message }),
                        );
                    }
                    FetchOutcome::Opaque => {
                        let key =
                            tenta_js::bindings::host::insert_opaque_response(&state.world.host);
                        let value = handle_to_quickjs::<Response>(
                            ctx,
                            Handle::from_bits(encode_host(state.world.host.slot, key)),
                        )?;
                        state.settle(ctx, completion.resolver, value, false);
                    }
                }
            }
            Ok(())
        });
    }
    fn settle_xhr(&mut self, completions: Vec<XhrCompletion>) {
        if completions.is_empty() {
            return;
        }
        let state = self.state.clone();
        let _ = self.entry(self.script_timeout, |_| {
            for completion in completions {
                if let Some(target) = DispatchTarget::from_bits(completion.handle.target) {
                    deliver_xhr_completion(
                        &state.world.host,
                        target,
                        completion.handle.request,
                        &completion.outcome,
                    );
                }
            }
            Ok(())
        });
    }
    fn next_wakeup(&self) -> Option<Duration> {
        Engine::next_wakeup(self)
    }
    fn pump_due_timers(&mut self, max_tasks: usize) -> usize {
        Engine::pump_due_timers(self, max_tasks)
    }
    fn frame_due(&self) -> bool {
        self.event_loop.frame_due()
    }
    fn run_animation_frame(&mut self) {
        self.event_loop.run_animation_frame();
        loop {
            let item = self.state.frames.borrow_mut().pop_front();
            let Some((handler, timestamp)) = item else {
                break;
            };
            let _ = self.entry(self.script_timeout, |ctx| {
                host::State::call_frame(handler, timestamp, ctx)
            });
            self.run_microtasks();
        }
    }
    fn run_microtasks(&mut self) {
        Engine::run_microtasks(self)
    }
    fn trim_memory(&mut self) {
        Engine::trim_memory(self)
    }
    fn gc_epoch(&self) -> u64 {
        self.state.world.gc_epoch.get()
    }
    fn watchdog(&self) -> Option<Box<dyn EngineWatchdog>> {
        Some(Box::new(Watchdog(self.interrupt.clone())))
    }
}
struct Watchdog(Arc<AtomicBool>);
impl EngineWatchdog for Watchdog {
    fn terminate(&self) {
        self.0.store(true, Ordering::Release)
    }
}
/// Creates QuickJS engines on the page thread selected by the browser.
pub struct QuickJsEngineFactory;
impl PageEngineFactory for QuickJsEngineFactory {
    fn create(
        &self,
        document: SharedDocument,
        config: &PageEngineConfig,
    ) -> Result<Box<dyn PageEngine>, String> {
        if let Some(zone) = &config.timezone {
            return Err(format!(
                "QuickJS process timezone override {zone:?} requires the vendored timezone hook"
            ));
        }
        let clock = match config.clock {
            EngineClock::Real => Clock::real(),
            EngineClock::Virtual => Clock::virtual_at(MonoTime::ZERO),
        };
        let mut engine = Engine::with_document_on_clock_storage_and_memory(
            document,
            clock,
            config.max_heap_bytes,
            config.cookies.clone(),
            config.storage.clone(),
            config.native_memory.clone(),
        )
        .map_err(|e| e.message)?;
        engine.set_timer_rule(config.timer_rule);
        engine.set_script_timeout(config.script_timeout);
        engine.state.viewport.set(config.viewport);
        Ok(Box::new(engine))
    }
}
