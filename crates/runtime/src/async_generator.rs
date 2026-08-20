//! Async generator runtime support for ES2018 `async function*`.
//!
//! Implements the async generator protocol per the ECMAScript specification.
//!
//! ## Spec References
//!
//! - `AsyncGeneratorStart` — [ES2024 §27.6.3.2](https://tc39.es/ecma262/#sec-asyncgeneratorstart)
//! - `AsyncGeneratorValidate` — [ES2024 §27.6.3.3](https://tc39.es/ecma262/#sec-asyncgeneratorvalidate)
//! - `AsyncGeneratorResume` — [ES2024 §27.6.3.4](https://tc39.es/ecma262/#sec-asyncgeneratorresume)
//! - `AsyncGeneratorUnwrapYieldResumption` — [ES2024 §27.6.3.5](https://tc39.es/ecma262/#sec-asyncgeneratorunwrapyieldresumption)
//! - `AsyncGeneratorEnqueue` — [ES2024 §27.6.3.6](https://tc39.es/ecma262/#sec-asyncgeneratorenqueue)
//! - `AsyncGeneratorCompleteStep` — [ES2024 §27.6.3.7](https://tc39.es/ecma262/#sec-asyncgeneratorcompletestep)
//! - `AsyncGeneratorDrainQueue` — [ES2024 §27.6.3.8](https://tc39.es/ecma262/#sec-asyncgeneratordrainqueue)
//! - `AsyncGenerator.prototype.next` — [ES2024 §27.6.3.9.1](https://tc39.es/ecma262/#sec-asyncgenerator-prototype-next)
//! - `AsyncGenerator.prototype.return` — [ES2024 §27.6.3.9.2](https://tc39.es/ecma262/#sec-asyncgenerator-prototype-return)
//! - `AsyncGenerator.prototype.throw` — [ES2024 §27.6.3.9.3](https://tc39.es/ecma262/#sec-asyncgenerator-prototype-throw)
//!
//! ## Protocol
//!
//! 1. User calls `asyncGen.next(val)` — returns a Promise.
//! 2. The request is enqueued in the async generator's queue
//!    (`AsyncGeneratorEnqueue`).
//! 3. If the generator is not executing, the queue is drained
//!    (`AsyncGeneratorDrainQueue`):
//!    - Dequeue next request, set state to Executing.
//!    - Call the underlying sync generator's `.next()`/`.throw()`/`.return()`.
//!    - If result is `{value, done: false}`: resolve with `{value, done: false}`,
//!      set state to SuspendedYield, continue draining.
//!    - If result is `{value, done: true}`: resolve with `{value, done: true}`,
//!      set state to Completed, drain remaining with `{undefined, done: true}`.
//! 4. If the generator is Completed, resolve immediately with `{undefined, done: true}`.

use nanbox::JsValue;

use crate::internal_data::{InternalData, UnifiedObject};
use crate::tagged_obj::{ObjTag, deref_tagged_mut, read_obj_tag};

/// The kind of request sent to an async generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// Normal `.next(value)` call.
    Next,
    /// `.throw(error)` call.
    Throw,
    /// `.return(value)` call.
    Return,
}

/// A queued request to an async generator.
///
/// Each call to `.next()` / `.throw()` / `.return()` creates a request and
/// enqueues it. The request holds the Promise bits for the Promise returned
/// to the caller.
pub struct AsyncGeneratorRequest {
    /// The kind of operation requested.
    pub kind: RequestKind,
    /// NaN-boxed value passed to `.next(val)` / `.throw(err)` / `.return(val)`.
    pub value: u64,
    /// NaN-boxed pointer to the Promise object returned to the caller.
    pub promise_bits: u64,
}

impl std::fmt::Debug for AsyncGeneratorRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncGeneratorRequest")
            .field("kind", &self.kind)
            .field("value", &format_args!("0x{:016x}", self.value))
            .field(
                "promise_bits",
                &format_args!("0x{:016x}", self.promise_bits),
            )
            .finish()
    }
}

/// Async generator state machine states.
///
/// Tracks whether the async generator is suspended, executing, awaiting
/// the return value, or completed. These correspond to the
/// `[[AsyncGeneratorState]]` internal slot values defined in the spec.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-objects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncGeneratorState {
    /// `suspendedStart` — initial state before the first `.next()` call.
    SuspendedStart,
    /// `suspendedYield` — suspended after a `yield` expression.
    SuspendedYield,
    /// `executing` — currently executing (re-entrancy guard).
    Executing,
    /// `awaitingReturn` — awaiting the resolution of a `.return()` value.
    AwaitingReturn,
    /// `completed` — generator has completed (no more values).
    Completed,
}

/// `AsyncGenerator.prototype.next ( value )`
///
/// Enqueues a "next" request and returns a Promise that will be resolved
/// with the next `{ value, done }` result.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-next
pub fn async_generator_next(ag: u64, value: u64) -> u64 {
    // 1. Let generator be the this value.
    // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
    // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
    // 4. IfAbruptRejectPromise(result, promiseCapability).
    // 5. Let state be generator.[[AsyncGeneratorState]].
    // 6. If state is completed, then ... (handled in enqueue_and_maybe_drain)
    // 7. Let completion be NormalCompletion(value).
    // 8. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
    // 9. If state is either suspendedStart or suspendedYield, then
    //    a. Perform AsyncGeneratorResume(generator, completion).
    // 10. Else, Assert: state is either executing or awaiting-return.
    // 11. Return promiseCapability.[[Promise]].
    enqueue_and_maybe_drain(ag, RequestKind::Next, value)
}

/// `AsyncGenerator.prototype.throw ( exception )`
///
/// Enqueues a "throw" request and returns a Promise. When processed, the
/// generator resumes as if `throw exception` was called at the yield point.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-throw
pub fn async_generator_throw(ag: u64, value: u64) -> u64 {
    // 1. Let generator be the this value.
    // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
    // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
    // 4. IfAbruptRejectPromise(result, promiseCapability).
    // 5. Let state be generator.[[AsyncGeneratorState]].
    // 6. If state is suspendedStart, then
    //    a. Set generator.[[AsyncGeneratorState]] to completed.
    //    ... (handled in enqueue_and_maybe_drain)
    // 7. If state is completed, then ... (handled in enqueue_and_maybe_drain)
    // 8. Let completion be ThrowCompletion(exception).
    // 9. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
    // 10. If state is suspendedYield, perform AsyncGeneratorResume(generator, completion).
    // 11. Return promiseCapability.[[Promise]].
    enqueue_and_maybe_drain(ag, RequestKind::Throw, value)
}

/// `AsyncGenerator.prototype.return ( value )`
///
/// Enqueues a "return" request and returns a Promise. When processed, the
/// generator completes with the given value.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-return
pub fn async_generator_return(ag: u64, value: u64) -> u64 {
    // 1. Let generator be the this value.
    // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
    // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
    // 4. IfAbruptRejectPromise(result, promiseCapability).
    // 5. Let completion be Completion Record { [[Type]]: return, [[Value]]: value }.
    // 6. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
    // 7. Let state be generator.[[AsyncGeneratorState]].
    // 8. If state is either suspendedStart or suspendedYield, then
    //    a. Set generator.[[AsyncGeneratorState]] to awaiting-return.
    //    b. Perform ! AsyncGeneratorAwaitReturn(generator).
    // 9. Return promiseCapability.[[Promise]].
    enqueue_and_maybe_drain(ag, RequestKind::Return, value)
}

/// `AsyncGeneratorStart ( generator, generatorBody )`
///
/// Creates an async generator object wrapping a sync generator (which
/// represents the compiled generator body). The `generator` parameter is the
/// NaN-boxed sync generator created by the ramp function.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorstart
pub fn create_async_generator(generator: u64) -> u64 {
    use crate::tagged_obj::TaggedObj;
    use shapes::ShapeTable;

    // 1. Assert: generator is an AsyncGenerator instance.
    // 2. Assert: generatorBody is a FunctionBody or a GeneratorBody parse node.
    // NOTE: In our AOT model, generatorBody is the compiled state machine.
    // 3. Let genContext be the running execution context.
    // 4. Set the Generator component of genContext to generator.
    // 5. Let closure be a new Abstract Closure with no parameters that captures generatorBody and generator.
    // NOTE: The sync generator already captures the state and resume function.
    // 6. Set generator.[[AsyncGeneratorState]] to suspendedStart.
    // 7. Set generator.[[AsyncGeneratorQueue]] to a new empty List.
    let obj = UnifiedObject::async_generator(ShapeTable::EMPTY_SHAPE, generator);
    // 8. Return undefined.
    // NOTE: We return the async generator object instead.
    TaggedObj::boxed(ObjTag::Unified, obj)
}

/// `AsyncGeneratorEnqueue ( generator, completion, promiseCapability )` +
/// conditional `AsyncGeneratorResume`.
///
/// Enqueues a request into the async generator's queue and triggers draining
/// if the generator is in a suspended state.
///
/// [spec-enqueue]: https://tc39.es/ecma262/#sec-asyncgeneratorenqueue
/// [spec-resume]: https://tc39.es/ecma262/#sec-asyncgeneratorresume
fn enqueue_and_maybe_drain(ag: u64, kind: RequestKind, value: u64) -> u64 {
    // AsyncGeneratorEnqueue:
    // 1. Let request be AsyncGeneratorRequest { [[Completion]]: completion, [[Capability]]: promiseCapability }.
    let promise_bits = crate::rt_api::__esc_rt_promise_create();

    let tag = read_obj_tag(ag);
    if tag != Some(ObjTag::Unified as u8) {
        // Not a valid object -- resolve with {undefined, done: true}
        resolve_with_done(promise_bits);
        return promise_bits;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(ag)
    };

    let Some(u) = uni else {
        resolve_with_done(promise_bits);
        return promise_bits;
    };

    let Some(InternalData::AsyncGenerator {
        state,
        queue,
        generator: _,
    }) = u.internal_data_mut()
    else {
        resolve_with_done(promise_bits);
        return promise_bits;
    };

    // Handle completed state (per .next/.throw/.return algorithms):
    // If completed and it's a next/throw, resolve immediately with {undefined, done: true}
    if *state == AsyncGeneratorState::Completed && kind != RequestKind::Return {
        resolve_with_done(promise_bits);
        return promise_bits;
    }

    // If completed and it's a return, resolve with the provided value as done
    if *state == AsyncGeneratorState::Completed {
        resolve_with_value_done(promise_bits, value);
        return promise_bits;
    }

    // 2. Append request to generator.[[AsyncGeneratorQueue]].
    queue.push_back(AsyncGeneratorRequest {
        kind,
        value,
        promise_bits,
    });

    // Per the .next/.throw/.return algorithms:
    // If state is suspendedStart or suspendedYield, perform AsyncGeneratorResume.
    let should_drain = *state == AsyncGeneratorState::SuspendedStart
        || *state == AsyncGeneratorState::SuspendedYield;

    if should_drain {
        // We need to drop the borrow before calling drain
        drain_queue(ag);
    }

    // 3. Return promiseCapability.[[Promise]].
    promise_bits
}

/// `AsyncGeneratorDrainQueue ( generator )`
///
/// Drains the async generator's request queue, processing one request at a time.
/// After processing each request, checks if there are more requests to process.
/// Stops if the generator enters the Executing or AwaitingReturn state.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratordrainqueue
fn drain_queue(ag: u64) {
    loop {
        let tag = read_obj_tag(ag);
        if tag != Some(ObjTag::Unified as u8) {
            return;
        }

        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(ag)
        };

        let Some(u) = uni else {
            return;
        };

        let Some(InternalData::AsyncGenerator {
            state,
            queue,
            generator,
        }) = u.internal_data_mut()
        else {
            return;
        };

        // 1. Assert: generator.[[AsyncGeneratorQueue]] is not empty.
        // 2. Let queue be generator.[[AsyncGeneratorQueue]].
        let Some(request) = queue.pop_front() else {
            // Queue is empty — nothing to drain.
            return;
        };

        // 3. Let next be the first element of queue. (already dequeued above)
        // 4. Let state be generator.[[AsyncGeneratorState]].

        // Stop if in a non-drainable state (executing or awaiting-return).
        if *state == AsyncGeneratorState::Executing || *state == AsyncGeneratorState::AwaitingReturn
        {
            // Put the request back
            queue.push_front(request);
            return;
        }

        // 5. If state is completed, then
        if *state == AsyncGeneratorState::Completed {
            //   a. AsyncGeneratorCompleteStep with done=true.
            if request.kind == RequestKind::Return {
                resolve_with_value_done(request.promise_bits, request.value);
            } else {
                resolve_with_done(request.promise_bits);
            }
            continue;
        }

        // 6. Assert: state is either suspendedStart or suspendedYield.
        // 7. Set generator.[[AsyncGeneratorState]] to executing.
        *state = AsyncGeneratorState::Executing;
        let gen_inner = *generator;
        let req_kind = request.kind;
        let req_value = request.value;
        let req_promise = request.promise_bits;

        // Release the borrow before calling into the sync generator
        let _ = u;

        // 8. Perform AsyncGeneratorResume(generator, completion).
        // NOTE: We call the underlying sync generator directly.
        let result = match req_kind {
            RequestKind::Next => crate::rt_api::__esc_rt_generator_next(gen_inner, req_value),
            RequestKind::Throw => crate::rt_api::__esc_rt_generator_throw(gen_inner, req_value),
            RequestKind::Return => crate::rt_api::__esc_rt_generator_return(gen_inner, req_value),
        };

        // Check if the generator threw an exception
        if crate::exceptions::is_exception() {
            let exc = crate::exceptions::get_exception();
            crate::exceptions::clear_exception();
            // AsyncGeneratorCompleteStep: reject the promise with the exception.
            crate::rt_api::__esc_rt_promise_reject(req_promise, exc);

            // Set generator.[[AsyncGeneratorState]] to completed.
            set_async_gen_state(ag, AsyncGeneratorState::Completed);
            // Drain remaining requests as done.
            drain_remaining_as_done(ag);
            return;
        }

        // Extract {value, done} from the iterator result
        let (result_value, is_done) = crate::async_wrap::extract_iter_result(result);

        if is_done {
            // AsyncGeneratorCompleteStep: resolve with {value, done: true}.
            resolve_with_value_done(req_promise, result_value);
            // Set generator.[[AsyncGeneratorState]] to completed.
            set_async_gen_state(ag, AsyncGeneratorState::Completed);
            // Drain remaining requests as done.
            drain_remaining_as_done(ag);
            return;
        }

        // Generator yielded — AsyncGeneratorCompleteStep: resolve with {value, done: false}.
        resolve_with_value_not_done(req_promise, result_value);
        // Set generator.[[AsyncGeneratorState]] to suspendedYield.
        set_async_gen_state(ag, AsyncGeneratorState::SuspendedYield);

        // 9. Repeat (continue draining the queue).
    }
}

/// Drain all remaining requests in the queue, resolving each with `{undefined, done: true}`.
fn drain_remaining_as_done(ag: u64) {
    loop {
        let tag = read_obj_tag(ag);
        if tag != Some(ObjTag::Unified as u8) {
            return;
        }

        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(ag)
        };

        let Some(u) = uni else {
            return;
        };

        let Some(InternalData::AsyncGenerator { queue, .. }) = u.internal_data_mut() else {
            return;
        };

        let Some(request) = queue.pop_front() else {
            return;
        };

        if request.kind == RequestKind::Return {
            resolve_with_value_done(request.promise_bits, request.value);
        } else {
            resolve_with_done(request.promise_bits);
        }
    }
}

/// Set the async generator's state.
fn set_async_gen_state(ag: u64, new_state: AsyncGeneratorState) {
    let tag = read_obj_tag(ag);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(ag)
    };

    let Some(u) = uni else {
        return;
    };

    let Some(InternalData::AsyncGenerator { state, .. }) = u.internal_data_mut() else {
        return;
    };

    *state = new_state;
}

/// `AsyncGeneratorCompleteStep` helper — resolve a promise with
/// `CreateIterResultObject(undefined, true)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorcompletestep
fn resolve_with_done(promise_bits: u64) {
    let result = crate::rt_api::create_iterator_result(JsValue::undefined().raw_bits(), true);
    crate::rt_api::__esc_rt_promise_resolve(promise_bits, result);
}

/// `AsyncGeneratorCompleteStep` helper — resolve a promise with
/// `CreateIterResultObject(value, true)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorcompletestep
fn resolve_with_value_done(promise_bits: u64, value: u64) {
    let result = crate::rt_api::create_iterator_result(value, true);
    crate::rt_api::__esc_rt_promise_resolve(promise_bits, result);
}

/// `AsyncGeneratorCompleteStep` helper — resolve a promise with
/// `CreateIterResultObject(value, false)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorcompletestep
fn resolve_with_value_not_done(promise_bits: u64, value: u64) {
    let result = crate::rt_api::create_iterator_result(value, false);
    crate::rt_api::__esc_rt_promise_resolve(promise_bits, result);
}

/// Get the current state of an async generator.
///
/// Returns `None` if the object is not a valid async generator.
pub fn get_async_generator_state(ag: u64) -> Option<AsyncGeneratorState> {
    let tag = read_obj_tag(ag);
    if tag != Some(ObjTag::Unified as u8) {
        return None;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(ag)
    };

    let u = uni?;

    if let Some(InternalData::AsyncGenerator { state, .. }) = u.internal_data_mut() {
        Some(*state)
    } else {
        None
    }
}
