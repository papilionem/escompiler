//! Promise built-in methods.
//!
//! Provides `Promise.resolve()`, `Promise.reject()`, `Promise.all()`,
//! `Promise.race()`, `Promise.allSettled()`, `Promise.any()`,
//! `Promise.withResolvers()` (ES2024), and prototype methods (`then`,
//! `catch`, `finally`).
//!
//! The combinator methods (`all`, `race`, `allSettled`, `any`) operate
//! synchronously on already-settled promise values. Each accepts an array of
//! promise objects (created by [`make_promise`]) as the first argument.
//! Pending input promises are treated according to each combinator's spec
//! semantics:
//!
//! - `race`: first settled wins; if all pending the result is pending.
//! - `allSettled`: only resolves when all inputs are settled (non-pending).
//! - `any`: first fulfilled wins; all rejected yields `AggregateError`.

use nanbox::JsValue;

use crate::error_types;

/// Promise state enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    /// Initial state — neither fulfilled nor rejected.
    Pending,
    /// Operation completed successfully.
    Fulfilled,
    /// Operation failed.
    Rejected,
}

/// Internal promise representation stored behind an object pointer.
#[repr(C)]
struct PromiseInner {
    state: PromiseState,
    value: JsValue,
}

/// Array layout matching `crate::array::RtArray` for interop.
#[repr(C)]
struct RtArray {
    elements: Vec<JsValue>,
}

/// Create a promise object from the given state and value.
fn make_promise(state: PromiseState, value: JsValue) -> JsValue {
    let inner = Box::new(PromiseInner { state, value });
    let raw_ptr = Box::into_raw(inner) as *const ();
    JsValue::object(raw_ptr)
}

/// Create an array `JsValue` from a `Vec<JsValue>`.
fn make_array(elements: Vec<JsValue>) -> JsValue {
    let arr = Box::new(RtArray { elements });
    let raw_ptr = Box::into_raw(arr) as *const ();
    JsValue::object(raw_ptr)
}

/// Extract the promise inner from an object JsValue.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_promise`.
unsafe fn extract_promise(val: &JsValue) -> Option<&PromiseInner> {
    let ptr = val.as_object()? as *const PromiseInner;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_promise
    Some(unsafe { &*ptr })
}

/// Extract the element slice from an array JsValue.
///
/// # Safety
/// Caller must ensure the pointer was created by `make_array` or
/// `crate::array::make_array` (same layout).
unsafe fn extract_array_elements(val: &JsValue) -> Option<&[JsValue]> {
    let ptr = val.as_object()? as *const RtArray;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees ptr was created by make_array
    Some(unsafe { &(*ptr).elements })
}

/// Try to read promise inputs from the first argument.
///
/// If `args[0]` is an object, assumes it is an array of promise values and
/// returns the elements. Returns an empty slice for missing / non-object args.
fn read_iterable(args: &[JsValue]) -> Vec<JsValue> {
    let Some(arg) = args.first() else {
        return Vec::new();
    };
    if !arg.is_object() {
        return Vec::new();
    }
    let elems = unsafe {
        // SAFETY: in the stdlib context the iterable argument is an array
        // created by make_array (same RtArray layout).
        extract_array_elements(arg)
    };
    match elems {
        Some(e) => e.to_vec(),
        None => Vec::new(),
    }
}

// === Static methods ===

/// `Promise.resolve(value)` — return a promise resolved with the given value.
pub fn resolve(args: &[JsValue]) -> JsValue {
    let value = args.first().copied().unwrap_or_else(JsValue::undefined);
    make_promise(PromiseState::Fulfilled, value)
}

/// `Promise.reject(reason)` — return a promise rejected with the given reason.
pub fn reject(args: &[JsValue]) -> JsValue {
    let reason = args.first().copied().unwrap_or_else(JsValue::undefined);
    make_promise(PromiseState::Rejected, reason)
}

/// `Promise.all(iterable)` — resolve when all promises resolve, reject on first rejection.
///
/// Iterates the input promises synchronously. If all are fulfilled, resolves
/// with an array of their values. If any is rejected, rejects with that reason.
/// If any is still pending, the result is pending.
pub fn all(args: &[JsValue]) -> JsValue {
    let promises = read_iterable(args);
    if promises.is_empty() {
        // Empty iterable: resolve with empty array.
        return make_promise(PromiseState::Fulfilled, make_array(Vec::new()));
    }

    let mut values = Vec::with_capacity(promises.len());
    for p in &promises {
        let inner = unsafe {
            // SAFETY: elements are promise objects created by make_promise.
            extract_promise(p)
        };
        match inner {
            Some(pi) => match pi.state {
                PromiseState::Fulfilled => values.push(pi.value),
                PromiseState::Rejected => {
                    return make_promise(PromiseState::Rejected, pi.value);
                }
                PromiseState::Pending => {
                    return make_promise(PromiseState::Pending, JsValue::undefined());
                }
            },
            // Non-promise value: treat as fulfilled with that value.
            None => values.push(*p),
        }
    }
    make_promise(PromiseState::Fulfilled, make_array(values))
}

/// `Promise.race(iterable)` — resolve/reject with the first settled promise.
///
/// Returns a promise that settles with the value/reason of the first settled
/// input promise. If the iterable is empty, the returned promise stays pending
/// forever (per spec).
pub fn race(args: &[JsValue]) -> JsValue {
    let promises = read_iterable(args);

    // Empty iterable: the promise stays pending forever (per spec).
    if promises.is_empty() {
        return make_promise(PromiseState::Pending, JsValue::undefined());
    }

    // Find the first settled promise.
    for p in &promises {
        let inner = unsafe {
            // SAFETY: elements are promise objects created by make_promise.
            extract_promise(p)
        };
        match inner {
            Some(pi) => match pi.state {
                PromiseState::Fulfilled => {
                    return make_promise(PromiseState::Fulfilled, pi.value);
                }
                PromiseState::Rejected => {
                    return make_promise(PromiseState::Rejected, pi.value);
                }
                PromiseState::Pending => continue,
            },
            // Non-promise value: treat as immediately fulfilled.
            None => {
                return make_promise(PromiseState::Fulfilled, *p);
            }
        }
    }

    // All pending — result is pending.
    make_promise(PromiseState::Pending, JsValue::undefined())
}

/// `Promise.allSettled(iterable)` — resolve when all promises settle.
///
/// Waits for every input promise to settle (fulfill or reject). The result
/// is always fulfilled with an array of outcome descriptor objects
/// (`{ status, value }` or `{ status, reason }`). Since this module uses
/// simple NaN-boxed values, each descriptor is represented as a 2-element
/// array: `[status_string, value_or_reason]`.
///
/// If any input is still pending, the aggregate result is pending.
/// An empty iterable resolves immediately with an empty array.
pub fn all_settled(args: &[JsValue]) -> JsValue {
    let promises = read_iterable(args);

    // Empty iterable: resolve with empty array immediately.
    if promises.is_empty() {
        return make_promise(PromiseState::Fulfilled, make_array(Vec::new()));
    }

    let mut results = Vec::with_capacity(promises.len());
    for p in &promises {
        let inner = unsafe {
            // SAFETY: elements are promise objects created by make_promise.
            extract_promise(p)
        };
        match inner {
            Some(pi) => match pi.state {
                PromiseState::Fulfilled => {
                    // { status: "fulfilled", value: pi.value }
                    let descriptor = make_settlement_descriptor(true, pi.value);
                    results.push(descriptor);
                }
                PromiseState::Rejected => {
                    // { status: "rejected", reason: pi.value }
                    let descriptor = make_settlement_descriptor(false, pi.value);
                    results.push(descriptor);
                }
                PromiseState::Pending => {
                    // Cannot settle yet — result is pending.
                    return make_promise(PromiseState::Pending, JsValue::undefined());
                }
            },
            // Non-promise value: treat as fulfilled.
            None => {
                let descriptor = make_settlement_descriptor(true, *p);
                results.push(descriptor);
            }
        }
    }

    make_promise(PromiseState::Fulfilled, make_array(results))
}

/// `Promise.any(iterable)` — resolve with the first fulfilled promise.
///
/// Returns a promise that resolves with the value of the first fulfilled
/// input promise. If ALL input promises reject, the result is rejected with
/// an `AggregateError` containing all rejection reasons. If the iterable is
/// empty, rejects immediately with an `AggregateError` with an empty errors
/// array.
pub fn any(args: &[JsValue]) -> JsValue {
    let promises = read_iterable(args);

    // Empty iterable: reject with AggregateError with empty errors array.
    if promises.is_empty() {
        let agg = error_types::aggregate_error(&[
            make_array(Vec::new()),
            error_types::make_error_string("All promises were rejected".to_string()),
        ]);
        return make_promise(PromiseState::Rejected, agg);
    }

    let mut rejections = Vec::new();
    let mut has_pending = false;

    for p in &promises {
        let inner = unsafe {
            // SAFETY: elements are promise objects created by make_promise.
            extract_promise(p)
        };
        match inner {
            Some(pi) => match pi.state {
                PromiseState::Fulfilled => {
                    // First fulfilled wins.
                    return make_promise(PromiseState::Fulfilled, pi.value);
                }
                PromiseState::Rejected => {
                    rejections.push(pi.value);
                }
                PromiseState::Pending => {
                    has_pending = true;
                }
            },
            // Non-promise value: treat as fulfilled.
            None => {
                return make_promise(PromiseState::Fulfilled, *p);
            }
        }
    }

    if has_pending {
        // Some promises are still pending — result is pending.
        return make_promise(PromiseState::Pending, JsValue::undefined());
    }

    // All rejected — reject with AggregateError.
    let errors_array = make_array(rejections);
    let agg = error_types::aggregate_error(&[
        errors_array,
        error_types::make_error_string("All promises were rejected".to_string()),
    ]);
    make_promise(PromiseState::Rejected, agg)
}

// === Helper functions ===

/// Create an `allSettled` settlement descriptor.
///
/// Returns a 2-element array `[status_string, value_or_reason]` representing
/// a settlement outcome. The first element is `"fulfilled"` or `"rejected"`.
fn make_settlement_descriptor(fulfilled: bool, value: JsValue) -> JsValue {
    let status = if fulfilled {
        error_types::make_error_string("fulfilled".to_string())
    } else {
        error_types::make_error_string("rejected".to_string())
    };
    make_array(vec![status, value])
}

// === Prototype methods ===

/// `Promise.prototype.then(onFulfilled, onRejected)` — register callbacks.
///
/// Returns a new pending promise. Actual callback invocation requires the
/// microtask queue.
pub fn then(_args: &[JsValue]) -> JsValue {
    make_promise(PromiseState::Pending, JsValue::undefined())
}

/// `Promise.prototype.catch(onRejected)` — shorthand for `.then(undefined, onRejected)`.
///
/// Returns a new pending promise.
pub fn catch(_args: &[JsValue]) -> JsValue {
    make_promise(PromiseState::Pending, JsValue::undefined())
}

/// `Promise.prototype.finally(onFinally)` — register a handler regardless of outcome.
///
/// Semantics (synchronous model):
/// - If the promise is fulfilled: returns a new promise fulfilled with the
///   ORIGINAL value (not callback's return). In a full async runtime the
///   callback would be invoked first; in the synchronous stdlib model the
///   callback invocation is deferred to the ABI layer.
/// - If the promise is rejected: returns a new promise rejected with the
///   ORIGINAL reason (same callback semantics as above).
/// - If the promise is pending: returns a new pending promise.
/// - If callback is not provided or not callable: passes through the
///   promise value/reason unchanged.
///
/// Args: `[this_promise, onFinally]`
pub fn finally_method(args: &[JsValue]) -> JsValue {
    let this = args.first().copied().unwrap_or_else(JsValue::undefined);

    // Extract the promise inner from `this`.
    let inner = unsafe {
        // SAFETY: this was created by make_promise in the stdlib context.
        extract_promise(&this)
    };
    let Some(inner) = inner else {
        // Not a valid promise — return a pending promise (defensive).
        return make_promise(PromiseState::Pending, JsValue::undefined());
    };

    let state = inner.state;
    let value = inner.value;

    match state {
        PromiseState::Pending => {
            // Cannot settle yet — return a new pending promise.
            make_promise(PromiseState::Pending, JsValue::undefined())
        }
        PromiseState::Fulfilled => {
            // In a full async runtime, the callback would be invoked here
            // (with no args) before returning. In the synchronous stdlib
            // model we return a promise with the original value directly.
            make_promise(PromiseState::Fulfilled, value)
        }
        PromiseState::Rejected => {
            // Same as fulfilled: return a promise with the original reason.
            make_promise(PromiseState::Rejected, value)
        }
    }
}

/// `Promise.withResolvers()` — create a promise with exposed resolve/reject controls (ES2024).
///
/// Returns a 3-element array `[promise, resolve_fn, reject_fn]`:
/// - `promise` is a new pending promise.
/// - `resolve_fn` is a placeholder (undefined) — full callable resolve requires
///   runtime mutable state integration (v0.6 TODO: wire through `__esc_rt_promise_create`).
/// - `reject_fn` is a placeholder (undefined) — same as resolve.
///
/// In the stdlib's synchronous `PromiseInner` model, the resolve/reject
/// functions cannot mutate the promise after creation. The pending promise
/// is still useful for type-checking and structural tests.
pub fn with_resolvers(_args: &[JsValue]) -> JsValue {
    let promise = make_promise(PromiseState::Pending, JsValue::undefined());
    // TODO("v0.6: wire resolve_fn/reject_fn through __esc_rt_promise_create for mutable promises")
    let resolve_fn = JsValue::undefined();
    let reject_fn = JsValue::undefined();
    make_array(vec![promise, resolve_fn, reject_fn])
}

/// Check the state of a promise value.
///
/// Returns the [`PromiseState`] if the value is a promise object, `None` otherwise.
pub fn get_state(val: &JsValue) -> Option<PromiseState> {
    let inner = unsafe {
        // SAFETY: called on values created by this module's make_promise
        extract_promise(val)
    };
    inner.map(|p| p.state)
}

/// Get the resolved/rejected value of a promise.
///
/// Returns the inner value if the value is a promise object, `None` otherwise.
pub fn get_value(val: &JsValue) -> Option<JsValue> {
    let inner = unsafe {
        // SAFETY: called on values created by this module's make_promise
        extract_promise(val)
    };
    inner.map(|p| p.value)
}

/// Get the result array elements from a fulfilled combinator promise.
///
/// Extracts the inner array from a promise whose value is an array
/// (as returned by `all`, `allSettled`). Returns `None` if the value
/// is not a valid array.
pub fn get_result_array(val: &JsValue) -> Option<Vec<JsValue>> {
    let inner = unsafe {
        // SAFETY: called on values created by this module's make_promise
        extract_promise(val)
    };
    let inner = inner?;
    let elems = unsafe {
        // SAFETY: inner.value is an array created by make_array
        extract_array_elements(&inner.value)
    };
    elems.map(|e| e.to_vec())
}

/// Extract the components of a `withResolvers` result.
///
/// The result is a 3-element array `[promise, resolve_fn, reject_fn]`.
/// Returns `(promise, resolve_fn, reject_fn)` or `None` if the format is wrong.
pub fn get_resolvers_components(val: &JsValue) -> Option<(JsValue, JsValue, JsValue)> {
    let elems = unsafe {
        // SAFETY: val was created by with_resolvers → make_array
        extract_array_elements(val)
    };
    let elems = elems?;
    if elems.len() >= 3 {
        Some((elems[0], elems[1], elems[2]))
    } else {
        None
    }
}

/// Get the settlement descriptor components from an `allSettled` result entry.
///
/// Each descriptor is a 2-element array `[status_string, value_or_reason]`.
/// Returns `(status_string, value_or_reason)` or `None` if the format is wrong.
pub fn get_settlement_entry(descriptor: &JsValue) -> Option<(JsValue, JsValue)> {
    let elems = unsafe {
        // SAFETY: descriptor was created by make_settlement_descriptor → make_array
        extract_array_elements(descriptor)
    };
    let elems = elems?;
    if elems.len() >= 2 {
        Some((elems[0], elems[1]))
    } else {
        None
    }
}
