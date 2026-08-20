//! Promise and microtask queue for the runtime ABI.
//!
//! Provides a `JsPromise` type with pending/fulfilled/rejected states,
//! reaction handlers (then/catch), and a thread-local microtask queue
//! for scheduling promise reactions.

use std::cell::RefCell;
use std::collections::VecDeque;

use nanbox::JsValue;

/// The state of a promise.
///
/// Maps to the `[[PromiseState]]` internal slot defined in
/// [ES2024 SS27.2.1.1](https://tc39.es/ecma262/#sec-promise-objects):
/// - `"pending"` -- the promise has not yet settled.
/// - `"fulfilled"` -- the promise has been resolved with a value.
/// - `"rejected"` -- the promise has been rejected with a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    /// Not yet resolved or rejected.
    Pending,
    /// Successfully resolved with a value.
    Fulfilled,
    /// Rejected with a reason.
    Rejected,
}

/// A reaction handler attached via `.then()`.
///
/// Corresponds to the PromiseReaction Record defined in
/// [ES2024 SS27.2.1.2](https://tc39.es/ecma262/#sec-promisereaction-records).
///
/// Fields map to the record's slots:
/// - `on_fulfill` -- `[[Handler]]` when `[[Type]]` is `"Fulfill"`
/// - `on_reject`  -- `[[Handler]]` when `[[Type]]` is `"Reject"`
/// - `result_promise` -- `[[Capability]].[[Promise]]`
#[derive(Debug)]
pub struct Reaction {
    /// The on-fulfill handler (NaN-boxed function), or 0 if none.
    pub on_fulfill: u64,
    /// The on-reject handler (NaN-boxed function), or 0 if none.
    pub on_reject: u64,
    /// The promise returned by the `.then()` call.
    pub result_promise: *mut JsPromise,
}

/// A JavaScript Promise object.
///
/// Implements the Promise internal slots defined in
/// [ES2024 SS27.2.1.1](https://tc39.es/ecma262/#sec-promise-objects):
/// - `[[PromiseState]]` -- `state`
/// - `[[PromiseResult]]` -- `value`
/// - `[[PromiseFulfillReactions]]` / `[[PromiseRejectReactions]]` -- `reactions`
///
/// Manual `Debug` impl because `async_continuations` contains `Box<dyn FnOnce>`
/// which does not implement `Debug`.
pub struct JsPromise {
    /// Current state.
    pub state: PromiseState,
    /// The resolved/rejected value (valid only when state != Pending).
    pub value: u64,
    /// Reactions registered via `.then()`.
    pub reactions: Vec<Reaction>,
    /// Async continuation callbacks registered by `async_wrap`.
    ///
    /// These are Rust closures called with `(settled_value, is_fulfilled)` when
    /// the promise settles. Used by the async function wrapper to drive the
    /// generator without NaN-boxed JS function handlers.
    async_continuations: Vec<Box<dyn FnOnce(u64, bool)>>,
}

impl std::fmt::Debug for JsPromise {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsPromise")
            .field("state", &self.state)
            .field("value", &self.value)
            .field("reactions", &self.reactions)
            .field(
                "async_continuations",
                &format!("[{} closures]", self.async_continuations.len()),
            )
            .finish()
    }
}

impl JsPromise {
    /// `CreateResolvingFunctions ( promise )` -- creates a new pending promise.
    ///
    /// Allocates the internal slots described in
    /// [ES2024 SS27.2.3](https://tc39.es/ecma262/#sec-promise-constructor):
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-promise-objects
    pub fn new() -> Self {
        // 1. Set promise.[[PromiseState]] to "pending".
        // 2. Set promise.[[PromiseFulfillReactions]] to a new empty List.
        // 3. Set promise.[[PromiseRejectReactions]] to a new empty List.
        // 4. Set promise.[[PromiseResult]] to undefined.
        // (Steps from SS27.2.3.1 step 6-9, Promise ( executor ) initialization)
        Self {
            state: PromiseState::Pending,
            value: JsValue::undefined().raw_bits(),
            reactions: Vec::new(),
            async_continuations: Vec::new(),
        }
    }

    /// `FulfillPromise ( promise, value )`
    ///
    /// Transitions a pending promise to the fulfilled state with the given value,
    /// then triggers all fulfill reactions.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-fulfillpromise
    pub fn resolve(&mut self, val: u64) {
        // 1. Assert: The value of promise.[[PromiseState]] is "pending".
        if self.state != PromiseState::Pending {
            return; // Already settled (diverges from spec Assert for robustness)
        }
        // 2. Let reactions be promise.[[PromiseFulfillReactions]].
        // (handled in trigger_reactions)
        // 3. Set promise.[[PromiseResult]] to value.
        self.value = val;
        // 4. Set promise.[[PromiseFulfillReactions]] to undefined.
        // 5. Set promise.[[PromiseRejectReactions]] to undefined.
        // (reactions are drained in trigger_reactions via std::mem::take)
        // 6. Set promise.[[PromiseState]] to "fulfilled".
        self.state = PromiseState::Fulfilled;
        // 7. Perform TriggerPromiseReactions(reactions, value).
        self.trigger_reactions();
    }

    /// `RejectPromise ( promise, reason )`
    ///
    /// Transitions a pending promise to the rejected state with the given reason,
    /// then triggers all reject reactions.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-rejectpromise
    pub fn reject(&mut self, val: u64) {
        // 1. Assert: The value of promise.[[PromiseState]] is "pending".
        if self.state != PromiseState::Pending {
            return; // Already settled (diverges from spec Assert for robustness)
        }
        // 2. Let reactions be promise.[[PromiseRejectReactions]].
        // (handled in trigger_reactions)
        // 3. Set promise.[[PromiseResult]] to reason.
        self.value = val;
        // 4. Set promise.[[PromiseFulfillReactions]] to undefined.
        // 5. Set promise.[[PromiseRejectReactions]] to undefined.
        // (reactions are drained in trigger_reactions via std::mem::take)
        // 6. Set promise.[[PromiseState]] to "rejected".
        self.state = PromiseState::Rejected;
        // TODO: Step 7 — If promise.[[PromiseIsHandled]] is false, perform
        //   HostPromiseRejectionTracker(promise, "reject"). (unhandled rejection tracking)
        // 8. Perform TriggerPromiseReactions(reactions, reason).
        self.trigger_reactions();
    }

    /// `PerformPromiseThen ( promise, onFulfilled, onRejected )`
    ///
    /// Registers fulfill/reject handlers and returns a newly allocated chained
    /// promise. This is the simple path — allocates the result promise internally.
    ///
    /// Note: The returned `*mut JsPromise` is a raw `Box` allocation without
    /// a `TaggedObj` wrapper. Prefer [`then_with_chained`](Self::then_with_chained)
    /// when a `TaggedObj`-wrapped promise is needed (the runtime ABI path).
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-performpromisethen
    pub fn then(&mut self, on_fulfill: u64, on_reject: u64) -> *mut JsPromise {
        // Simplified: allocate result promise (spec uses NewPromiseCapability in
        // Promise.prototype.then before calling PerformPromiseThen).
        let chained = Box::into_raw(Box::new(JsPromise::new()));
        self.then_with_chained(on_fulfill, on_reject, chained);
        chained
    }

    /// `PerformPromiseThen ( promise, onFulfilled, onRejected, resultCapability )`
    ///
    /// Registers fulfill/reject handlers using a pre-allocated chained promise
    /// (the `resultCapability.[[Promise]]`).
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-performpromisethen
    pub fn then_with_chained(&mut self, on_fulfill: u64, on_reject: u64, chained: *mut JsPromise) {
        // 1. Assert: IsPromise(promise) is true.
        // (enforced by type system — self is &mut JsPromise)

        // 2. If resultCapability is not present, then set resultCapability to undefined.
        // (caller provides chained pointer, acting as resultCapability.[[Promise]])

        // 3. If IsCallable(onFulfilled) is false, then let onFulfilledJobCallback be empty.
        // 4. Else, let onFulfilledJobCallback be HostMakeJobCallback(onFulfilled).
        // 5. If IsCallable(onRejected) is false, then let onRejectedJobCallback be empty.
        // 6. Else, let onRejectedJobCallback be HostMakeJobCallback(onRejected).
        // (handler==0 or undefined is treated as "empty" in enqueue_reaction)

        // 7. Let fulfillReaction be the PromiseReaction Record { ... }.
        // 8. Let rejectReaction be the PromiseReaction Record { ... }.
        // (We combine both into a single Reaction struct)
        let reaction = Reaction {
            on_fulfill,
            on_reject,
            result_promise: chained,
        };

        match self.state {
            PromiseState::Pending => {
                // 9. If promise.[[PromiseState]] is "pending", then
                //   a. Append fulfillReaction to promise.[[PromiseFulfillReactions]].
                //   b. Append rejectReaction to promise.[[PromiseRejectReactions]].
                self.reactions.push(reaction);
            }
            PromiseState::Fulfilled => {
                // 10. Else if promise.[[PromiseState]] is "fulfilled", then
                //   a. Let value be promise.[[PromiseResult]].
                let val = self.value;
                //   b. Let fulfillJob be NewPromiseReactionJob(fulfillReaction, value).
                //   c. Perform HostEnqueuePromiseJob(fulfillJob.[[Job]], fulfillJob.[[Realm]]).
                enqueue_reaction(reaction, val, true);
            }
            PromiseState::Rejected => {
                // 11. Else (promise.[[PromiseState]] is "rejected"),
                //   a. Assert: The value of promise.[[PromiseState]] is "rejected".
                //   b. Let reason be promise.[[PromiseResult]].
                let val = self.value;
                // TODO: Step 11c — If promise.[[PromiseIsHandled]] is false, perform
                //   HostPromiseRejectionTracker(promise, "handle").
                //   d. Let rejectJob be NewPromiseReactionJob(rejectReaction, reason).
                //   e. Perform HostEnqueuePromiseJob(rejectJob.[[Job]], rejectJob.[[Realm]]).
                enqueue_reaction(reaction, val, false);
            }
        }

        // 12. Set promise.[[PromiseIsHandled]] to true.
        // TODO: Track [[PromiseIsHandled]] for unhandled rejection detection.

        // 13. If resultCapability is undefined, then return undefined.
        // 14. Else, return resultCapability.[[Promise]].
        // (handled by callers)
    }

    /// Register an async continuation callback on this promise.
    ///
    /// When the promise settles, `callback(settled_value, is_fulfilled)` is
    /// enqueued as a microtask. This is used by the async function wrapper
    /// to drive the generator state machine without requiring NaN-boxed
    /// JS function handlers.
    ///
    /// If the promise is already settled, the callback is enqueued immediately.
    ///
    /// This is an internal ESCompiler mechanism with no direct ES2024 spec
    /// equivalent. It implements the conceptual "await" continuation scheduling
    /// described in [ES2024 SS27.7.5.3 Await](https://tc39.es/ecma262/#await).
    pub fn register_async_continuation(&mut self, callback: Box<dyn FnOnce(u64, bool)>) {
        match self.state {
            PromiseState::Pending => {
                self.async_continuations.push(callback);
            }
            PromiseState::Fulfilled => {
                let val = self.value;
                queue_microtask_closure(Box::new(move || {
                    callback(val, true);
                }));
            }
            PromiseState::Rejected => {
                let val = self.value;
                queue_microtask_closure(Box::new(move || {
                    callback(val, false);
                }));
            }
        }
    }

    /// `TriggerPromiseReactions ( reactions, argument )`
    ///
    /// Enqueues all pending reactions as microtasks after the promise settles.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-triggerpromisereactions
    fn trigger_reactions(&mut self) {
        // 1. For each element reaction of reactions, do
        let reactions = std::mem::take(&mut self.reactions);
        let fulfilled = self.state == PromiseState::Fulfilled;
        let val = self.value;
        for reaction in reactions {
            //   a. Let job be NewPromiseReactionJob(reaction, argument).
            //   b. Perform HostEnqueuePromiseJob(job.[[Job]], job.[[Realm]]).
            enqueue_reaction(reaction, val, fulfilled);
        }

        // Also trigger async continuations (used by async function wrapper).
        // This is an ESCompiler extension — not part of the spec's
        // TriggerPromiseReactions, but conceptually equivalent for await.
        let continuations = std::mem::take(&mut self.async_continuations);
        for cb in continuations {
            let v = val;
            let f = fulfilled;
            queue_microtask_closure(Box::new(move || {
                cb(v, f);
            }));
        }
        // 2. Return unused.
    }
}

impl Default for JsPromise {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-local microtask queue for promise reactions.
struct MicrotaskQueueState {
    queue: VecDeque<Box<dyn FnOnce()>>,
}

impl MicrotaskQueueState {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}

thread_local! {
    static MICROTASK_QUEUE: RefCell<MicrotaskQueueState> = RefCell::new(MicrotaskQueueState::new());
}

/// `HostEnqueuePromiseJob ( job, realm )`
///
/// Enqueue a microtask function (NaN-boxed closure value). The function is
/// dispatched via `__esc_rt_call_indirect` when the microtask queue is drained.
///
/// This implements the host hook defined in
/// [ES2024 SS9.5.4](https://tc39.es/ecma262/#sec-hostenqueuepromisejob).
/// In our single-threaded AOT runtime, the "job queue" is a simple FIFO
/// thread-local deque drained by [`drain_microtasks`].
pub fn queue_microtask(func: u64) {
    MICROTASK_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        queue.queue.push_back(Box::new(move || {
            // Call the NaN-boxed function with no arguments
            call_handler(func, JsValue::undefined().raw_bits());
        }));
    });
}

/// Enqueue a Rust closure as a microtask.
///
/// Used by the async function wrapper to schedule continuation steps
/// without going through the NaN-boxed function dispatch path.
///
/// This is an internal ESCompiler extension of
/// [`HostEnqueuePromiseJob`](https://tc39.es/ecma262/#sec-hostenqueuepromisejob)
/// that accepts a Rust closure instead of a NaN-boxed JS function.
pub fn queue_microtask_closure(task: Box<dyn FnOnce()>) {
    MICROTASK_QUEUE.with(|q| {
        q.borrow_mut().queue.push_back(task);
    });
}

/// Enqueue a raw Rust closure as a microtask (for test use).
#[cfg(test)]
pub(crate) fn enqueue_raw_microtask(task: Box<dyn FnOnce()>) {
    MICROTASK_QUEUE.with(|q| {
        q.borrow_mut().queue.push_back(task);
    });
}

/// `RunJobs ()` -- drain all microtasks, running them in FIFO order.
///
/// Microtasks enqueued during drain are also processed (nested drain),
/// implementing the "run until the queue is empty" semantics of the
/// [ES2024 SS9.4 Jobs and Host Operations](https://tc39.es/ecma262/#sec-jobs)
/// event loop model.
///
/// [spec]: https://tc39.es/ecma262/#sec-runjobs
pub fn drain_microtasks() {
    loop {
        let task = MICROTASK_QUEUE.with(|q| {
            let mut queue = q.borrow_mut();
            queue.queue.pop_front()
        });
        match task {
            Some(task) => task(),
            None => break,
        }
    }
}

/// Call the indirect dispatch function to invoke a NaN-boxed handler.
///
/// During tests, `__esc_rt_call_indirect` is not linked via the real
/// dispatch trampoline, but the test stub in `rt_api` returns `undefined`.
/// For production, this routes through `__esc_dispatch`.
fn call_handler(handler: u64, arg: u64) -> u64 {
    // SAFETY: handler is a valid NaN-boxed function value, and we pass
    // exactly one argument via a stack-allocated array.
    unsafe {
        let argv = [arg];
        crate::rt_api::__esc_rt_call_indirect(handler, 1, argv.as_ptr())
    }
}

/// `NewPromiseReactionJob ( reaction, argument )`
///
/// Enqueues a single promise reaction as a microtask. When the microtask
/// runs, it invokes the appropriate handler (fulfill or reject) and
/// resolves/rejects the chained promise with the handler's result.
///
/// [spec]: https://tc39.es/ecma262/#sec-newpromisereactionjob
fn enqueue_reaction(reaction: Reaction, val: u64, fulfilled: bool) {
    MICROTASK_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        queue.queue.push_back(Box::new(move || {
            // NewPromiseReactionJob returns a Record { [[Job]], [[Realm]] }.
            // The [[Job]] closure body:

            // 1. Let reaction be the PromiseReaction Record for this job.
            // (captured in closure)

            // 2. Let promiseCapability be reaction.[[Capability]].
            // (reaction.result_promise is the [[Capability]].[[Promise]])

            // 3. Let type be reaction.[[Type]].
            // 4. Let handler be reaction.[[Handler]].
            let handler = if fulfilled {
                reaction.on_fulfill
            } else {
                reaction.on_reject
            };

            // 5. If handler is empty, then
            if handler == 0 || handler == JsValue::undefined().raw_bits() {
                if !reaction.result_promise.is_null() {
                    let chained = unsafe {
                        // SAFETY: result_promise was created via Box::into_raw.
                        &mut *reaction.result_promise
                    };
                    if fulfilled {
                        //   a. If type is "Fulfill", let handlerResult be NormalCompletion(argument).
                        //   (propagate the value to the chained promise)
                        chained.resolve(val);
                    } else {
                        //   b. Else (type is "Reject"), let handlerResult be
                        //      ThrowCompletion(argument).
                        //   (propagate the rejection to the chained promise)
                        chained.reject(val);
                    }
                }
                return;
            }

            // 6. Else, let handlerResult be Completion(HostCallJobCallback(handler, undefined, << argument >>)).
            let result = call_handler(handler, val);
            // TODO: Step 7 — If handlerResult is an abrupt completion, then
            //   a. Return ? Call(promiseCapability.[[Reject]], undefined, << handlerResult.[[Value]] >>).
            //   (currently we always resolve; need to catch handler exceptions and reject instead)
            // 8. Else, return ? Call(promiseCapability.[[Resolve]], undefined, << handlerResult.[[Value]] >>).
            if !reaction.result_promise.is_null() {
                let chained = unsafe {
                    // SAFETY: result_promise was created via Box::into_raw.
                    &mut *reaction.result_promise
                };
                chained.resolve(result);
            }
        }));
    });
}
