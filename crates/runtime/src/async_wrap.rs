//! Async function wrapper -- drives a generator via Promise resolution.
//!
//! Implements the `AsyncFunctionStart` abstract operation from the spec.
//!
//! [spec]: https://tc39.es/ecma262/#sec-async-functions-abstract-operations-async-function-start
//!
//! An async function is structurally identical to a generator after the
//! state machine transform. The only difference is that the ramp function
//! wraps the result in a Promise (not a Generator object) and the runtime
//! drives the state machine via microtask callbacks.
//!
//! ## Protocol (AsyncFunctionStart, §27.7.5.2)
//!
//! 1. The ramp function creates a generator (state + resume function index)
//! 2. `async_wrap` takes that generator and returns a Promise
//! 3. `async_step` drives the generator forward:
//!    - If done: resolve/reject the outer Promise
//!    - If not done: the yielded value is the thing being awaited;
//!      wrap it in `Promise.resolve()` and chain a continuation to resume

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, deref_tagged, deref_tagged_mut, read_obj_tag};

/// `AsyncFunctionStart ( promiseCapability, asyncFunctionBody )`
///
/// Wraps a generator-based async function into a Promise. Called by the ramp
/// function of an async function. Takes the generator (already created by
/// the ramp) and returns a Promise that resolves when the generator completes.
///
/// The generator is driven by [`async_step`], which uses the microtask queue
/// to schedule each step of the async function.
///
/// [spec]: https://tc39.es/ecma262/#sec-async-functions-abstract-operations-async-function-start
pub fn async_wrap(generator: u64) -> u64 {
    // 1. Let runningContext be the running execution context.
    // 2. Let asyncContext be a copy of runningContext.
    // NOTE: In our AOT model, the generator state object serves as the execution context.

    // 3. Let promiseCapability be ! NewPromiseCapability(%Promise%).
    let promise_bits = crate::rt_api::__esc_rt_promise_create();

    // __esc_rt_promise_create produces a UnifiedObject with an empty shape and
    // no [[Prototype]] link, so `p instanceof Promise` walks a chain that never
    // reaches Promise.prototype and returns false. Link it explicitly so the
    // returned promise behaves like a real Promise instance.
    let promise_proto = crate::rt_api::get_or_create_builtin_prototype("Promise");
    crate::rt_api::set_prototype_on_new_object(promise_bits, promise_proto);

    // 4. Set the code evaluation state of asyncContext such that when evaluation is
    //    resumed with a completion, the following steps will be performed:
    //    (This is implemented by async_step, which resumes the generator state machine.)
    // 5. Push asyncContext onto the execution context stack.
    // 6. Resume the suspended evaluation of asyncContext using NormalCompletion(undefined).
    async_step(
        generator,
        promise_bits,
        JsValue::undefined().raw_bits(),
        false,
    );

    // 7. Assert: When we reach here, asyncContext has been removed from the execution context stack.
    // 8. Return promiseCapability.[[Promise]].
    promise_bits
}

/// Drive one step of the async function (continuation of `AsyncFunctionStart`).
///
/// This implements the async function body evaluation steps from the spec:
/// when the generator yields (an `await` expression), we wrap the yielded
/// value in `Promise.resolve()` and schedule a continuation. When the
/// generator returns, we resolve the outer promise.
///
/// [spec]: https://tc39.es/ecma262/#sec-async-functions-abstract-operations-async-function-start
/// (step 4 — the code evaluation state closure)
fn async_step(generator: u64, promise_bits: u64, value: u64, is_throw: bool) {
    // 4.a. Let result be the Completion Record that is the result of evaluating asyncFunctionBody.
    // NOTE: We call the generator's next/throw to resume the state machine.
    let result = if is_throw {
        crate::rt_api::__esc_rt_generator_throw(generator, value)
    } else {
        crate::rt_api::__esc_rt_generator_next(generator, value)
    };

    // 4.b. Assert: If we return here, the async function either returned or threw.
    // Check if the generator threw an exception
    if crate::exceptions::is_exception() {
        let exc = crate::exceptions::get_exception();
        crate::exceptions::clear_exception();
        // 4.e. Else if result is a throw completion, then
        //   i. Perform ! Call(promiseCapability.[[Reject]], undefined, « result.[[Value]] »).
        crate::rt_api::__esc_rt_promise_reject(promise_bits, exc);
        return;
    }

    // Extract {value, done} from the iterator result
    let (result_value, is_done) = extract_iter_result(result);

    if is_done {
        // 4.c. If result is a return completion, then
        //   i. Perform ! Call(promiseCapability.[[Resolve]], undefined, « result.[[Value]] »).
        crate::rt_api::__esc_rt_promise_resolve(promise_bits, result_value);
    } else {
        // 4.d. Else (result is a yield — i.e., await expression),
        //   The yielded value is the expression being awaited.
        // Perform Await(result_value) by wrapping in Promise.resolve() and
        // scheduling a microtask continuation.
        let awaited = promise_resolve_wrap(result_value);
        schedule_continuation(awaited, generator, promise_bits);
    }
}

/// Extract `{ value, done }` from an iterator result object.
///
/// Implements the abstract operations `IteratorValue` and `IteratorComplete`:
/// - `IteratorValue(iterResult)` — [spec §7.4.4](https://tc39.es/ecma262/#sec-iteratorvalue)
/// - `IteratorComplete(iterResult)` — [spec §7.4.3](https://tc39.es/ecma262/#sec-iteratorcomplete)
///
/// Returns `(value_bits, is_done)`.
pub(crate) fn extract_iter_result(result: u64) -> (u64, bool) {
    let tag = read_obj_tag(result);
    if tag != Some(ObjTag::Unified as u8) {
        // Not an object -- treat as done with the raw value
        return (result, true);
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a tagged unified object.
        deref_tagged::<UnifiedObject>(result)
    };

    let Some(u) = uni else {
        return (result, true);
    };

    // Fast path: InternalKind::IterResult with typed internal data
    if u.kind == InternalKind::IterResult
        && let Some(InternalData::IterResult { value, done }) = u.internal_data()
    {
        // IteratorComplete: 1. Return ToBoolean(? Get(iterResult, "done")).
        let done_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(*done));
        // IteratorValue: 1. Return ? Get(iterResult, "value").
        return (*value, done_bool);
    }

    // Slow path: read "value" and "done" properties from a plain object
    // IteratorValue §7.4.4: 1. Return ? Get(iterResult, "value").
    let value_key = crate::rt_api::make_rt_string("value".to_string());
    let done_key = crate::rt_api::make_rt_string("done".to_string());
    let val = crate::rt_api::__esc_rt_get_prop(result, value_key);
    // IteratorComplete §7.4.3: 1. Return ToBoolean(? Get(iterResult, "done")).
    let done = crate::rt_api::__esc_rt_get_prop(result, done_key);
    let done_bool = crate::value_ops::to_boolean(JsValue::from_raw_bits(done));
    (val, done_bool)
}

/// `Promise.resolve ( x )` — simplified for the `await` use case.
///
/// If `x` is already a Promise, returns it as-is. Otherwise wraps it
/// in a new immediately-fulfilled Promise. This is a subset of the full
/// `Promise.resolve` algorithm.
///
/// [spec]: https://tc39.es/ecma262/#sec-promise.resolve
pub fn promise_resolve_wrap(value: u64) -> u64 {
    // 1. Let C be the this value (implicit: %Promise%).
    // 2. If Type(C) is not Object, throw a TypeError exception.
    // (skipped — always called with %Promise%)

    // 3. Return ? PromiseResolve(C, x).
    // PromiseResolve (§27.2.4.7.1):
    // 1. If IsPromise(x) is true, then
    let tag = read_obj_tag(value);
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a tagged unified object.
            deref_tagged::<UnifiedObject>(value)
        };
        if let Some(u) = uni
            && u.kind == InternalKind::Promise
        {
            //   a. Let xConstructor be ? Get(x, "constructor").
            //   b. If SameValue(xConstructor, C) is true, return x.
            // NOTE: Simplified — we assume same constructor.
            return value;
        }
    }

    // 2. Let promiseCapability be ? NewPromiseCapability(C).
    let prom = crate::rt_api::__esc_rt_promise_create();
    // 3. Perform ? Call(promiseCapability.[[Resolve]], undefined, « x »).
    crate::rt_api::__esc_rt_promise_resolve(prom, value);
    // 4. Return promiseCapability.[[Promise]].
    prom
}

/// `Await ( value )` — schedule an async continuation on the awaited promise.
///
/// Implements the "upon fulfillment/rejection" steps from the `Await`
/// abstract operation. When the awaited promise settles, `async_step` is
/// called again with the settled value to resume the generator state machine.
///
/// [spec]: https://tc39.es/ecma262/#await
fn schedule_continuation(awaited_promise: u64, generator: u64, outer_promise: u64) {
    // Await §6.2.3.1:
    // 1. Let promise be ? PromiseResolve(%Promise%, value).
    // (Already done by the caller via promise_resolve_wrap.)

    let tag = read_obj_tag(awaited_promise);
    if tag != Some(ObjTag::Unified as u8) {
        // Not a proper promise -- resume immediately with the value
        async_step(generator, outer_promise, awaited_promise, false);
        return;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a tagged unified object.
        deref_tagged_mut::<UnifiedObject>(awaited_promise)
    };

    let Some(u) = uni else {
        async_step(generator, outer_promise, awaited_promise, false);
        return;
    };

    let Some(InternalData::Promise { inner }) = u.internal_data_mut() else {
        async_step(generator, outer_promise, awaited_promise, false);
        return;
    };

    // 2. Let fulfilledClosure be a new Abstract Closure with parameters (value) that captures asyncContext.
    //    a. Let prevContext be the running execution context.
    //    b. Suspend prevContext.
    //    c. Push asyncContext onto the execution context stack.
    //    d. Resume asyncContext passing NormalCompletion(value).
    //    e. Assert: asyncContext has already been removed.
    //    f. Resume prevContext.
    // 3. Let onFulfilled be CreateBuiltinFunction(fulfilledClosure, 1, "", « »).
    // 4. Let rejectedClosure be a new Abstract Closure with parameters (reason) that captures asyncContext.
    //    a-f. (symmetric with fulfilled, but passes ThrowCompletion(reason))
    // 5. Let onRejected be CreateBuiltinFunction(rejectedClosure, 1, "", « »).
    // 6. Perform PerformPromiseThen(promise, onFulfilled, onRejected).
    inner.register_async_continuation(Box::new(move |val, is_fulfilled| {
        async_step(generator, outer_promise, val, !is_fulfilled);
    }));
}
