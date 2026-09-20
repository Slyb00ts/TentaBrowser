//! The promise seam: JavaScript promises the host creates and settles
//! (`docs/spec/05-javascript.md` §3).
//!
//! A host record holds no isolate, so it can neither build a promise nor
//! resolve one. [`ScriptHost`] is the boundary it uses instead: `new_promise`
//! makes the pair, and `settle_promise` hands back the outcome. This is the V8
//! side, and it is its own module because the settlement needs state neither the
//! script host nor the host state owns — a table of jobs that outlives the
//! binding call which asked for one.
//!
//! [`ScriptHost`]: tenta_js::dom::events::ScriptHost
//!
//! # Why the settlement is a microtask
//!
//! Fetch resolves a body read in a promise job, and this engine's
//! `queueMicrotask` is `Scope::enqueue_microtask`, i.e. V8's own microtask queue
//! *is* the page-visible one. Settling in place instead would break the
//! standard's ordering for a settlement requested from inside a microtask:
//! `Promise.resolve().then(() => r.text().then(f))` has to call `f` in the same
//! checkpoint pass, and a promise settled before the pass would call it a whole
//! checkpoint later.
//!
//! # What a job holds
//!
//! Applying a settlement needs a *typed* `v8::PromiseResolver`, and the world's
//! slab loads stored handles back only as plain `Local<Value>`. `rusty_v8`
//! provides the upcast — `impl From<Local<PromiseResolver>> for Local<Value>` —
//! and no downcast at all: there is no `TryFrom` for `PromiseResolver`, so
//! `Local::cast` is unavailable too. A `v8::Global` taken while the resolver is
//! still in hand is therefore the one safe route from a stored promise back to
//! the resolver type, and this is why a `Settlement` holds one.
//!
//! A `Global` has a price: it must be *dropped* while the isolate is alive, and
//! this state lives in a context slot, which V8 does not drop until the isolate
//! is disposed. [`PromiseState::clear`] is that obligation discharged, called by
//! `Engine::Drop` beside the other teardown drains.
//!
//! Everything else in a job is a bit pattern — the promise, the value to settle
//! with and the resolver's own slot — so the world keeps the single reference
//! accounting for all three.
//!
//! # Where the state lives
//!
//! `settle_promise` runs inside a binding call and has the script host; the
//! microtask runs from V8's queue and has only the entered context. The context
//! slot is where the two meet, which is why this is one `Rc<PromiseState>` set
//! on the context (`Engine::install_dom`) rather than a field on either side.
//!
//! # Reference accounting
//!
//! Every `u64` here is a stored reference of the world's value slab, and the
//! world frees a slot only when its last reference is released. The rules this
//! module enforces:
//!
//!  - `new_promise` stores both halves and registers the promise under its
//!    resolver, so a settlement — which is handed the resolver alone — can
//!    release the promise too;
//!  - a settlement takes the registration out of the table, so a second
//!    settlement of the same promise finds nothing and releases nothing;
//!  - the microtask releases the resolver, the promise and the value it settled
//!    with, once each, after the resolve/reject call;
//!  - a value the settlement did not build — the world-wide interned
//!    `undefined` — is *not* released, because that slot belongs to the world.
//!
//! The one way to leak is to create a promise and never settle it: the
//! registration and the two slots then all stay. Nothing in this slice does
//! that; `d-4a-1`'s network completion is where the possibility returns.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tenta_js::bindings::host::utf8_decode;
use tenta_js::dom::events::{HostValue, PromiseOutcome, PromiseRef};

use crate::script_host::V8ScriptHost;
use crate::world::WorldState;

/// The `SyntaxError` a `json()` read rejects with when its body is malformed.
/// The standard states the exception and no message, so the text is this
/// engine's own.
const JSON_PARSE_FAILED: &str = "JSON.parse: the body is not valid JSON";

/// A promise that has been created and not yet settled.
struct Pending {
    /// The resolver, held typed because the world's bits for it load back as a
    /// plain value (see the module docs).
    resolver: v8::Global<v8::PromiseResolver>,
    /// The promise's bits, released together with the resolver's.
    promise: u64,
}

/// One settlement waiting for its microtask: the two halves of the promise and
/// the value to settle it with.
struct Settlement {
    /// The resolver, taken from the registration so that the `Global` lives
    /// exactly as long as the job that still needs it.
    resolver: v8::Global<v8::PromiseResolver>,
    /// The resolver's slab bits, released when the job runs.
    resolver_bits: u64,
    /// The promise's bits, released with the resolver.
    promise: u64,
    /// The bits of the value to resolve or reject with.
    value: u64,
    /// Whether `value` is a stored reference this job owns. False for the
    /// world's interned `undefined`, whose slot the world keeps.
    owns_value: bool,
    /// Whether the settlement rejects rather than resolves.
    rejected: bool,
}

/// A slot table: values handed out under an index and taken back exactly once.
///
/// Taking a slot is what frees it, so a job cannot run twice and release a
/// reference twice, and the microtask carried by an enqueued function finds the
/// job it was enqueued for. That second property is worth stating precisely,
/// because the index is a *position* and not an identity: a freed slot is reused
/// by the next job, so taking an index twice yields the second occupant rather
/// than `None`. What makes it safe is that a slot is only pushed back once it has
/// been taken, and the only taker of an index is the microtask holding it — an
/// occupied slot is never handed out again, so the microtask that runs next
/// always finds its own job.
///
/// It is generic — and tested over `u64` — because those invariants are worth a
/// differential test that an isolate would otherwise be required for, a
/// `Settlement` holding a `v8::Global`.
struct Slots<T> {
    slots: RefCell<Vec<Option<T>>>,
    free: RefCell<Vec<u32>>,
}

impl<T> Slots<T> {
    fn new() -> Self {
        Self {
            slots: RefCell::new(Vec::new()),
            free: RefCell::new(Vec::new()),
        }
    }

    /// Stores `value` and returns the index that names it.
    fn push(&self, value: T) -> u32 {
        let mut slots = self.slots.borrow_mut();
        match self.free.borrow_mut().pop() {
            Some(index) => {
                slots[index as usize] = Some(value);
                index
            }
            None => {
                let index = slots.len() as u32;
                slots.push(Some(value));
                index
            }
        }
    }

    /// Takes the value at `index`, freeing the slot for the next job.
    fn take(&self, index: u32) -> Option<T> {
        let value = self.slots.borrow_mut().get_mut(index as usize)?.take();
        if value.is_some() {
            self.free.borrow_mut().push(index);
        }
        value
    }

    /// Drops every value still in the table.
    fn clear(&self) {
        self.slots.borrow_mut().clear();
        self.free.borrow_mut().clear();
    }
}

/// The promise state of one context: the promises created and not yet settled,
/// and the settlements waiting for their microtask.
pub struct PromiseState {
    /// The world whose value slab every bit in this file indexes. The microtask
    /// has no other route to it: the wrapper world sits in an isolate slot the
    /// generated prelude never exposes.
    world: Rc<RefCell<WorldState>>,
    /// The promises created and not yet settled, keyed by their resolver's
    /// bits. A settlement is handed the resolver alone, and this is how it
    /// reaches the promise that resolver belongs to.
    pending: RefCell<HashMap<u64, Pending>>,
    /// The settlements awaiting their microtask.
    jobs: Slots<Settlement>,
}

impl PromiseState {
    /// Builds the state over the world whose slab it indexes.
    pub fn new(world: Rc<RefCell<WorldState>>) -> Rc<Self> {
        Rc::new(Self {
            world,
            pending: RefCell::new(HashMap::new()),
            jobs: Slots::new(),
        })
    }

    /// Drops every handle this state still holds.
    ///
    /// The queued settlements hold `v8::Global<PromiseResolver>` handles, and
    /// this state lives in a context slot, which V8 drops only when the isolate
    /// is disposed — too late for a `Global` to be released. `Engine::Drop`
    /// calls this beside its other teardown drains, while the isolate is alive.
    pub fn clear(&self) {
        self.pending.borrow_mut().clear();
        self.jobs.clear();
    }

    /// The promise state of the context currently entered, if it has one.
    fn of(scope: &v8::PinScope<'_, '_>) -> Option<Rc<Self>> {
        scope.get_current_context().get_slot::<Self>()
    }

    /// Records that `resolver` settles `pending`.
    fn register(&self, resolver: u64, pending: Pending) {
        self.pending.borrow_mut().insert(resolver, pending);
    }

    /// Takes the promise `resolver` settles, if it is still pending.
    fn take(&self, resolver: u64) -> Option<Pending> {
        self.pending.borrow_mut().remove(&resolver)
    }
}

/// Creates the promise pair, stores both halves and registers the promise under
/// its resolver. See `ScriptHost::new_promise` for the ownership rules.
pub fn new_promise(host: &V8ScriptHost) -> Option<PromiseRef> {
    host.with_context(|scope| {
        let state = PromiseState::of(scope)?;
        let resolver = v8::PromiseResolver::new(scope)?;
        let promise = resolver.get_promise(scope);
        let world = host.world();
        let mut world = world.borrow_mut();
        let promise_bits = world.store_value(scope, promise.into());
        let resolver_bits = world.store_value(scope, resolver.into());
        drop(world);
        state.register(
            resolver_bits,
            Pending {
                resolver: v8::Global::new(scope, resolver),
                promise: promise_bits,
            },
        );
        Some(PromiseRef {
            promise: promise_bits,
            resolver: resolver_bits,
        })
    })
}

/// Builds the value the settlement carries, stores it, and enqueues the
/// microtask that applies it. See `ScriptHost::settle_promise` for the ordering
/// rule.
pub fn settle_promise(host: &V8ScriptHost, resolver: u64, outcome: PromiseOutcome<'_>) -> bool {
    host.with_context(|scope| settle_in(host, scope, resolver, outcome).unwrap_or(false))
}

/// The body of [`settle_promise`], in the scope its caller has already entered:
/// a second [`V8ScriptHost::with_context`] inside the first would nest a
/// `CallbackScope`, which is not legal.
fn settle_in(
    host: &V8ScriptHost,
    scope: &mut v8::PinScope<'_, '_>,
    resolver: u64,
    outcome: PromiseOutcome<'_>,
) -> Option<bool> {
    let state = PromiseState::of(scope)?;

    // The outcome is built before the registration is taken: a failure here
    // must not consume it, or the promise would be left pending with no route
    // to settle it and both of its slots leaked.
    let (rejected, value, owns_value) = match outcome {
        PromiseOutcome::Resolve(value) => {
            let owns = !matches!(value, HostValue::Undefined);
            (false, host.build_value_in(scope, value)?, owns)
        }
        PromiseOutcome::Reject(value) => {
            let owns = !matches!(value, HostValue::Undefined);
            (true, host.build_value_in(scope, value)?, owns)
        }
        // The standard's own step, and one the host cannot take: the parse
        // needs an isolate, and its result is an arbitrary object graph. A
        // failure rejects rather than throws, which is what `json()` requires
        // of a malformed body.
        PromiseOutcome::ParseJson(bytes) => match parse_json(scope, bytes) {
            Some(json) => (
                false,
                state.world.borrow_mut().store_value(scope, json),
                true,
            ),
            None => {
                // A real `SyntaxError`, not a `DOMException` that happens to be
                // named one: Fetch's "parse JSON from bytes" rejects with the
                // exception `JSON.parse` raises, and script tests it with
                // `instanceof SyntaxError`.
                let message = v8::String::new(scope, JSON_PARSE_FAILED)?;
                let error = v8::Exception::syntax_error(scope, message);
                (
                    true,
                    state.world.borrow_mut().store_value(scope, error),
                    true,
                )
            }
        },
    };

    // A resolver whose promise has already been settled names nothing, so this
    // is also the double-settle guard.
    let pending = state.take(resolver)?;
    let index = state.jobs.push(Settlement {
        resolver: pending.resolver,
        resolver_bits: resolver,
        promise: pending.promise,
        value,
        owns_value,
        rejected,
    });
    // The index travels as the function's data, which is what lets the microtask
    // find its own job without a pointer and without `unsafe`.
    let data = v8::Integer::new(scope, index as i32);
    let callback = v8::Function::builder(promise_microtask)
        .data(data.into())
        .build(scope)?;
    scope.enqueue_microtask(callback);
    Some(true)
}

/// The `JSON.parse` of `bytes`, or `None` when they are not valid JSON.
///
/// The decode is [`utf8_decode`], the same one `text()` resolves with, because
/// the standard's "parse JSON from bytes" is exactly a UTF-8 decode followed by
/// a parse: a byte sequence that is not valid UTF-8 is not valid JSON either,
/// and the replacement characters it decodes to fail the parse rather than
/// raising a decode error.
fn parse_json<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> Option<v8::Local<'s, v8::Value>> {
    let source = utf8_decode(bytes);
    let source = v8::String::new(scope, &source)?;
    v8::json::parse(scope, source)
}

/// Applies one settlement: opens the resolver, settles the promise, and gives
/// back every reference the job holds. The job's `Global` drops with `job`.
fn promise_microtask(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments<'_>,
    _retval: v8::ReturnValue<v8::Value>,
) {
    let Some(state) = PromiseState::of(scope) else {
        return;
    };
    let Some(index) = args.data().integer_value(scope).filter(|index| *index >= 0) else {
        return;
    };
    // Taken rather than borrowed: the job is consumed exactly once, and taking
    // it is what frees the slot for the next settlement.
    let Some(job) = state.jobs.take(index as u32) else {
        return;
    };

    let resolver = v8::Local::new(scope, &job.resolver);
    let world = state.world.clone();
    let mut world = world.borrow_mut();
    let Some(value) = world.load_value(scope, job.value) else {
        return;
    };
    let _ = if job.rejected {
        resolver.reject(scope, value)
    } else {
        resolver.resolve(scope, value)
    };
    // The resolver and the promise have served their purpose: the promise
    // object is in script's hands by now, and the world's hold on both is over.
    world.release_value(job.resolver_bits);
    world.release_value(job.promise);
    if job.owns_value {
        world.release_value(job.value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_slot_is_handed_to_the_next_job() {
        let slots = Slots::new();
        let first = slots.push(1u64);
        let second = slots.push(2u64);
        assert_ne!(first, second, "two live jobs occupy two slots");
        assert_eq!(slots.take(first), Some(1));
        assert_eq!(
            slots.push(3u64),
            first,
            "the freed slot is reused rather than the table growing"
        );
        assert_eq!(
            slots.take(first),
            Some(3),
            "a reused slot holds the new job, which is why an occupied slot is never handed out again"
        );
        assert_eq!(slots.take(second), Some(2), "the other job is untouched");
    }

    #[test]
    fn a_slot_that_was_never_taken_is_not_handed_out_twice() {
        let slots = Slots::new();
        let held = slots.push(1u64);
        let next = slots.push(2u64);
        assert_ne!(held, next, "a live slot is not reusable");
        assert_eq!(slots.take(held), Some(1));
        assert_eq!(slots.take(held), None, "and is empty once taken");
    }

    #[test]
    fn a_cleared_table_hands_out_nothing() {
        let slots = Slots::new();
        let index = slots.push(1u64);
        slots.clear();
        assert_eq!(slots.take(index), None, "a cleared slot holds no job");
        assert_eq!(slots.push(2u64), index, "and is available again");
    }
}
