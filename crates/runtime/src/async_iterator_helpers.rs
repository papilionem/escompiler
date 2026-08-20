//! Async iterator helpers — lazy and eager helper methods for async iterators.
//!
//! Implements async variants of the ES2025 Iterator Helpers proposal:
//! - **Lazy helpers** (return new async iterators): `map`, `filter`, `take`, `drop`, `flatMap`
//! - **Eager helpers** (return Promises): `forEach`, `some`, `every`, `find`, `reduce`, `toArray`
//! - **Static method**: `AsyncIterator.from(obj)`
//!
//! ## Architecture
//!
//! An async iterator is an object whose `.next()` returns a `Promise<{value, done}>`.
//! Lazy helpers wrap a source async iterator and return a new object implementing
//! the async iterator protocol. Each call to `.next()` on the wrapper pulls from
//! the source, applies the transformation, and returns a Promise.
//!
//! Eager helpers consume the entire async iterator and return a single Promise
//! that resolves when iteration is complete.
//!
//! The source async iterator is driven by calling `.next()` on it and awaiting
//! the result via [`schedule_on_promise`], which registers an async continuation
//! on the returned Promise.

use nanbox::JsValue;
use shapes::ShapeTable;

use crate::internal_data::{InternalData, UnifiedObject};
use crate::rt_api::{
    __esc_rt_call_method, __esc_rt_get_prop, __esc_rt_promise_create, __esc_rt_promise_reject,
    __esc_rt_promise_resolve, create_array_from_elements, make_rt_string,
};
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged_mut, read_obj_tag};
use crate::value_ops;

// =========================================================================
// AsyncIteratorState — the state for a lazy async iterator helper
// =========================================================================

/// The kind of async iterator helper transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncHelperKind {
    /// `AsyncIterator.prototype.map(fn)` — transforms each value.
    Map,
    /// `AsyncIterator.prototype.filter(fn)` — skips non-matching values.
    Filter,
    /// `AsyncIterator.prototype.take(n)` — limits to first n values.
    Take,
    /// `AsyncIterator.prototype.drop(n)` — skips first n values.
    Drop,
    /// `AsyncIterator.prototype.flatMap(fn)` — maps then flattens one level.
    FlatMap,
}

/// State for an async iterator helper wrapper.
///
/// Stores the source async iterator, the optional callback, the helper kind,
/// and any extra state (counter for take/drop, inner iterator for flatMap).
#[derive(Debug)]
pub struct AsyncIteratorState {
    /// The source async iterator object (NaN-boxed).
    pub source: u64,
    /// The callback function (NaN-boxed), or 0 if none (take/drop).
    pub callback: u64,
    /// Which helper transformation to apply.
    pub kind: AsyncHelperKind,
    /// Counter for take/drop (remaining count).
    pub counter: u32,
    /// Whether the initial drop phase is complete.
    pub drop_done: bool,
    /// Whether the source iterator is exhausted.
    pub done: bool,
    /// Inner async iterator for flatMap (NaN-boxed), or 0 if none.
    pub inner_source: u64,
}

// =========================================================================
// Helpers for calling async iterator .next() and awaiting the Promise
// =========================================================================

/// Call `.next()` on an async iterator object.
///
/// Returns the raw result of calling `.next()` — typically a Promise for
/// async iterators, or an iterator result for sync iterators.
fn call_next(source: u64) -> u64 {
    let next_key = make_rt_string("next".to_string());
    unsafe {
        // SAFETY: source is a valid object; passing zero args with null argv.
        __esc_rt_call_method(source, next_key, 0, std::ptr::null())
    }
}

/// Call a callback function with one argument.
///
/// Uses `__esc_rt_call_indirect` to invoke the callback.
fn call_callback(callback: u64, arg: u64) -> u64 {
    let argv = [arg];
    unsafe {
        // SAFETY: argv points to 1 valid u64 value.
        crate::rt_api::__esc_rt_call_indirect(callback, 1, argv.as_ptr())
    }
}

/// Call a callback function with two arguments.
///
/// Used by `reduce` which passes `(accumulator, value)`.
fn call_callback_2(callback: u64, arg1: u64, arg2: u64) -> u64 {
    let argv = [arg1, arg2];
    unsafe {
        // SAFETY: argv points to 2 valid u64 values.
        crate::rt_api::__esc_rt_call_indirect(callback, 2, argv.as_ptr())
    }
}

/// Extract `{value, done}` from an iterator result object.
///
/// Returns `(value_bits, is_done)`.
fn extract_iter_result(result: u64) -> (u64, bool) {
    crate::async_wrap::extract_iter_result(result)
}

/// Wrap a value as a resolved Promise if it is not already a Promise.
fn promise_resolve_wrap(value: u64) -> u64 {
    crate::async_wrap::promise_resolve_wrap(value)
}

/// Schedule a callback to run when a Promise settles.
///
/// If the value is not a Promise, wraps it first. When the Promise
/// fulfills, calls `on_fulfill(value)`. When it rejects, calls
/// `on_reject(reason)`.
fn schedule_on_promise(maybe_promise: u64, on_settle: impl FnOnce(u64, bool) + 'static) {
    let prom = promise_resolve_wrap(maybe_promise);
    let tag = read_obj_tag(prom);
    if tag != Some(ObjTag::Unified as u8) {
        // Not an object — treat as fulfilled immediately
        on_settle(maybe_promise, true);
        return;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(prom)
    };

    let Some(u) = uni else {
        on_settle(maybe_promise, true);
        return;
    };

    let Some(InternalData::Promise { inner }) = u.internal_data_mut() else {
        on_settle(maybe_promise, true);
        return;
    };

    inner.register_async_continuation(Box::new(on_settle));
}

/// Create a boxed async iterator wrapper from an [`AsyncIteratorState`].
fn boxed_async_iterator(state: AsyncIteratorState) -> u64 {
    TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::async_iterator(ShapeTable::EMPTY_SHAPE, state),
    )
}

// =========================================================================
// Lazy helpers — create async iterator wrappers
// =========================================================================

/// `AsyncIterator.prototype.map(fn)` — returns a new async iterator that
/// applies `fn` to each value and awaits the result.
pub fn async_iterator_map(source: u64, callback: u64) -> u64 {
    let state = AsyncIteratorState {
        source,
        callback,
        kind: AsyncHelperKind::Map,
        counter: 0,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    boxed_async_iterator(state)
}

/// `AsyncIterator.prototype.filter(fn)` — returns a new async iterator that
/// keeps values where `fn(value)` returns truthy (awaits `fn`).
pub fn async_iterator_filter(source: u64, callback: u64) -> u64 {
    let state = AsyncIteratorState {
        source,
        callback,
        kind: AsyncHelperKind::Filter,
        counter: 0,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    boxed_async_iterator(state)
}

/// `AsyncIterator.prototype.take(n)` — returns a new async iterator limited
/// to the first `n` values.
pub fn async_iterator_take(source: u64, count: u32) -> u64 {
    let state = AsyncIteratorState {
        source,
        callback: 0,
        kind: AsyncHelperKind::Take,
        counter: count,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    boxed_async_iterator(state)
}

/// `AsyncIterator.prototype.drop(n)` — returns a new async iterator that
/// skips the first `n` values, then yields the rest.
pub fn async_iterator_drop(source: u64, count: u32) -> u64 {
    let state = AsyncIteratorState {
        source,
        callback: 0,
        kind: AsyncHelperKind::Drop,
        counter: count,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    boxed_async_iterator(state)
}

/// `AsyncIterator.prototype.flatMap(fn)` — returns a new async iterator that
/// maps each value through `fn`, then flattens one level if the result is
/// iterable/async-iterable.
pub fn async_iterator_flat_map(source: u64, callback: u64) -> u64 {
    let state = AsyncIteratorState {
        source,
        callback,
        kind: AsyncHelperKind::FlatMap,
        counter: 0,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    boxed_async_iterator(state)
}

// =========================================================================
// .next() dispatch for async iterator helper wrappers
// =========================================================================

/// Advance an async iterator helper by one step.
///
/// Called when `.next()` is invoked on an async iterator wrapper object.
/// Returns a Promise that resolves with `{value, done}`.
pub fn async_iterator_next(wrapper: u64) -> u64 {
    let promise = __esc_rt_promise_create();

    let tag = read_obj_tag(wrapper);
    if tag != Some(ObjTag::Unified as u8) {
        resolve_done(promise);
        return promise;
    }

    // Extract the state from the wrapper
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(wrapper)
    };

    let Some(u) = uni else {
        resolve_done(promise);
        return promise;
    };

    let Some(InternalData::AsyncIterator { inner }) = u.internal_data_mut() else {
        resolve_done(promise);
        return promise;
    };

    if inner.done {
        resolve_done(promise);
        return promise;
    }

    let source = inner.source;
    let callback = inner.callback;
    let kind = inner.kind;
    let counter = inner.counter;
    let drop_done = inner.drop_done;
    let inner_source = inner.inner_source;

    match kind {
        AsyncHelperKind::Map => {
            advance_map(wrapper, source, callback, promise);
        }
        AsyncHelperKind::Filter => {
            advance_filter(wrapper, source, callback, promise);
        }
        AsyncHelperKind::Take => {
            if counter == 0 {
                set_wrapper_done(wrapper);
                resolve_done(promise);
            } else {
                // Decrement counter
                set_wrapper_counter(wrapper, counter - 1);
                advance_take(wrapper, source, promise);
            }
        }
        AsyncHelperKind::Drop => {
            if drop_done {
                // Pass-through phase: just forward source .next()
                advance_passthrough(wrapper, source, promise);
            } else if counter == 0 {
                set_wrapper_drop_done(wrapper, true);
                advance_passthrough(wrapper, source, promise);
            } else {
                // Skip phase: consume one value from source, then recurse
                advance_drop_skip(wrapper, source, counter, promise);
            }
        }
        AsyncHelperKind::FlatMap => {
            if inner_source != 0 {
                // Try to get next from inner source first
                advance_flat_map_inner(wrapper, source, callback, inner_source, promise);
            } else {
                advance_flat_map_outer(wrapper, source, callback, promise);
            }
        }
    }

    promise
}

/// Map helper: pull one value from source, apply callback, resolve with result.
fn advance_map(wrapper: u64, source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            resolve_done(promise);
            return;
        }
        let mapped = call_callback(callback, value);
        // Await the callback result in case it returns a Promise
        schedule_on_promise(mapped, move |mapped_val, map_fulfilled| {
            if !map_fulfilled {
                set_wrapper_done(wrapper);
                __esc_rt_promise_reject(promise, mapped_val);
                return;
            }
            resolve_with_value(promise, mapped_val);
        });
    });
}

/// Filter helper: pull values from source until callback returns truthy.
fn advance_filter(wrapper: u64, source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            resolve_done(promise);
            return;
        }
        let keep_result = call_callback(callback, value);
        // Await the callback result
        schedule_on_promise(keep_result, move |keep_val, keep_fulfilled| {
            if !keep_fulfilled {
                set_wrapper_done(wrapper);
                __esc_rt_promise_reject(promise, keep_val);
                return;
            }
            if value_ops::to_boolean(JsValue::from_raw_bits(keep_val)) {
                resolve_with_value(promise, value);
            } else {
                // Didn't match — try next value (re-enter filter loop)
                advance_filter(wrapper, source, callback, promise);
            }
        });
    });
}

/// Take helper: pull one value from source and forward it.
fn advance_take(wrapper: u64, source: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            resolve_done(promise);
            return;
        }
        resolve_with_value(promise, value);
    });
}

/// Passthrough: pull one value from source and forward it (used by drop after skip).
fn advance_passthrough(wrapper: u64, source: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            resolve_done(promise);
            return;
        }
        resolve_with_value(promise, value);
    });
}

/// Drop skip phase: consume one value from source, decrement counter, recurse.
fn advance_drop_skip(wrapper: u64, source: u64, remaining: u32, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (_value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            set_wrapper_drop_done(wrapper, true);
            resolve_done(promise);
            return;
        }
        let new_remaining = remaining - 1;
        set_wrapper_counter(wrapper, new_remaining);
        if new_remaining == 0 {
            set_wrapper_drop_done(wrapper, true);
            // Now pull the next real value
            advance_passthrough(wrapper, source, promise);
        } else {
            // Keep skipping
            advance_drop_skip(wrapper, source, new_remaining, promise);
        }
    });
}

/// FlatMap: try to pull from inner source.
fn advance_flat_map_inner(
    wrapper: u64,
    source: u64,
    callback: u64,
    inner_source: u64,
    promise: u64,
) {
    let next_result = call_next(inner_source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            // Inner iterator failed — clear it and try outer
            set_wrapper_inner_source(wrapper, 0);
            advance_flat_map_outer(wrapper, source, callback, promise);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            // Inner iterator exhausted — move to next outer value
            set_wrapper_inner_source(wrapper, 0);
            advance_flat_map_outer(wrapper, source, callback, promise);
        } else {
            resolve_with_value(promise, value);
        }
    });
}

/// FlatMap: pull from outer source, apply callback, set up inner.
fn advance_flat_map_outer(wrapper: u64, source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            set_wrapper_done(wrapper);
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            set_wrapper_done(wrapper);
            resolve_done(promise);
            return;
        }
        let mapped = call_callback(callback, value);
        // Await the callback result
        schedule_on_promise(mapped, move |mapped_val, map_fulfilled| {
            if !map_fulfilled {
                set_wrapper_done(wrapper);
                __esc_rt_promise_reject(promise, mapped_val);
                return;
            }
            // Check if mapped value is iterable (has .next())
            let next_key = make_rt_string("next".to_string());
            let next_fn = __esc_rt_get_prop(mapped_val, next_key);
            let next_val = JsValue::from_raw_bits(next_fn);
            if !next_val.is_undefined() && next_val.is_object() {
                // Looks like an iterator/async iterator — use it as inner source
                set_wrapper_inner_source(wrapper, mapped_val);
                advance_flat_map_inner(wrapper, source, callback, mapped_val, promise);
            } else {
                // Non-iterable — yield it directly
                resolve_with_value(promise, mapped_val);
            }
        });
    });
}

// =========================================================================
// Eager helpers — consume the async iterator and return a Promise
// =========================================================================

/// `AsyncIterator.prototype.forEach(fn)` — calls `fn(value)` for each value,
/// returns `Promise<undefined>`.
pub fn async_iterator_for_each(source: u64, callback: u64) -> u64 {
    let promise = __esc_rt_promise_create();
    for_each_step(source, callback, promise);
    promise
}

/// One step of the forEach loop.
fn for_each_step(source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            __esc_rt_promise_resolve(promise, JsValue::undefined().raw_bits());
            return;
        }
        let cb_result = call_callback(callback, value);
        // Await callback result before continuing
        schedule_on_promise(cb_result, move |_cb_val, cb_fulfilled| {
            if !cb_fulfilled {
                __esc_rt_promise_reject(promise, _cb_val);
                return;
            }
            for_each_step(source, callback, promise);
        });
    });
}

/// `AsyncIterator.prototype.some(fn)` — returns `Promise<boolean>`,
/// short-circuits on first truthy callback result.
pub fn async_iterator_some(source: u64, callback: u64) -> u64 {
    let promise = __esc_rt_promise_create();
    some_step(source, callback, promise);
    promise
}

/// One step of the some loop.
fn some_step(source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            __esc_rt_promise_resolve(promise, JsValue::bool(false).raw_bits());
            return;
        }
        let cb_result = call_callback(callback, value);
        schedule_on_promise(cb_result, move |cb_val, cb_fulfilled| {
            if !cb_fulfilled {
                __esc_rt_promise_reject(promise, cb_val);
                return;
            }
            if value_ops::to_boolean(JsValue::from_raw_bits(cb_val)) {
                __esc_rt_promise_resolve(promise, JsValue::bool(true).raw_bits());
            } else {
                some_step(source, callback, promise);
            }
        });
    });
}

/// `AsyncIterator.prototype.every(fn)` — returns `Promise<boolean>`,
/// short-circuits on first falsy callback result.
pub fn async_iterator_every(source: u64, callback: u64) -> u64 {
    let promise = __esc_rt_promise_create();
    every_step(source, callback, promise);
    promise
}

/// One step of the every loop.
fn every_step(source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            __esc_rt_promise_resolve(promise, JsValue::bool(true).raw_bits());
            return;
        }
        let cb_result = call_callback(callback, value);
        schedule_on_promise(cb_result, move |cb_val, cb_fulfilled| {
            if !cb_fulfilled {
                __esc_rt_promise_reject(promise, cb_val);
                return;
            }
            if !value_ops::to_boolean(JsValue::from_raw_bits(cb_val)) {
                __esc_rt_promise_resolve(promise, JsValue::bool(false).raw_bits());
            } else {
                every_step(source, callback, promise);
            }
        });
    });
}

/// `AsyncIterator.prototype.find(fn)` — returns `Promise<value|undefined>`,
/// resolves with the first value where `fn(value)` is truthy.
pub fn async_iterator_find(source: u64, callback: u64) -> u64 {
    let promise = __esc_rt_promise_create();
    find_step(source, callback, promise);
    promise
}

/// One step of the find loop.
fn find_step(source: u64, callback: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            __esc_rt_promise_resolve(promise, JsValue::undefined().raw_bits());
            return;
        }
        let cb_result = call_callback(callback, value);
        schedule_on_promise(cb_result, move |cb_val, cb_fulfilled| {
            if !cb_fulfilled {
                __esc_rt_promise_reject(promise, cb_val);
                return;
            }
            if value_ops::to_boolean(JsValue::from_raw_bits(cb_val)) {
                __esc_rt_promise_resolve(promise, value);
            } else {
                find_step(source, callback, promise);
            }
        });
    });
}

/// `AsyncIterator.prototype.reduce(fn, init)` — returns `Promise<value>`,
/// accumulates values through a reducer function.
pub fn async_iterator_reduce(source: u64, callback: u64, initial: u64, has_initial: bool) -> u64 {
    let promise = __esc_rt_promise_create();
    if has_initial {
        reduce_step(source, callback, initial, promise);
    } else {
        // Use first element as initial value
        let next_result = call_next(source);
        schedule_on_promise(next_result, move |val, fulfilled| {
            if !fulfilled {
                __esc_rt_promise_reject(promise, val);
                return;
            }
            let (value, is_done) = extract_iter_result(val);
            if is_done {
                // Empty iterator with no initial value — TypeError
                let msg = make_rt_string(
                    "Reduce of empty async iterator with no initial value".to_string(),
                );
                let err = crate::rt_api::__esc_rt_create_error(
                    crate::exceptions::error_tag::TYPE_ERROR,
                    msg,
                );
                __esc_rt_promise_reject(promise, err);
                return;
            }
            reduce_step(source, callback, value, promise);
        });
    }
    promise
}

/// One step of the reduce loop.
fn reduce_step(source: u64, callback: u64, accumulator: u64, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            __esc_rt_promise_resolve(promise, accumulator);
            return;
        }
        let new_acc = call_callback_2(callback, accumulator, value);
        // Await the callback result
        schedule_on_promise(new_acc, move |acc_val, acc_fulfilled| {
            if !acc_fulfilled {
                __esc_rt_promise_reject(promise, acc_val);
                return;
            }
            reduce_step(source, callback, acc_val, promise);
        });
    });
}

/// `AsyncIterator.prototype.toArray()` — collects all values into an array,
/// returns `Promise<Array>`.
pub fn async_iterator_to_array(source: u64) -> u64 {
    let promise = __esc_rt_promise_create();
    to_array_step(source, Vec::new(), promise);
    promise
}

/// One step of the toArray loop.
fn to_array_step(source: u64, mut elements: Vec<JsValue>, promise: u64) {
    let next_result = call_next(source);
    schedule_on_promise(next_result, move |val, fulfilled| {
        if !fulfilled {
            __esc_rt_promise_reject(promise, val);
            return;
        }
        let (value, is_done) = extract_iter_result(val);
        if is_done {
            let arr = create_array_from_elements(elements);
            __esc_rt_promise_resolve(promise, arr);
            return;
        }
        elements.push(JsValue::from_raw_bits(value));
        to_array_step(source, elements, promise);
    });
}

// =========================================================================
// AsyncIterator.from(obj) — wraps a sync or async iterable
// =========================================================================

/// `AsyncIterator.from(obj)` — wraps a sync or async iterable into an
/// async iterator.
///
/// If `obj` already has `[Symbol.asyncIterator]`, calls it to get the async
/// iterator. Otherwise, if it has `[Symbol.iterator]`, wraps the sync
/// iterator in an async wrapper that wraps each `.next()` result in a
/// resolved Promise.
pub fn async_iterator_from(obj: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Check for [Symbol.asyncIterator]
    if !v.is_null() && !v.is_undefined() && v.is_object() {
        let async_iter_fn =
            crate::rt_api::get_prop_by_symbol_key(obj, crate::symbol::SYMBOL_ASYNC_ITERATOR);
        let async_val = JsValue::from_raw_bits(async_iter_fn);
        if !async_val.is_undefined() {
            // Call the [Symbol.asyncIterator]() method
            let prev_this = crate::rt_api::CURRENT_THIS.with(|cell| cell.replace(obj));
            let iter_obj = unsafe {
                // SAFETY: async_iter_fn was validated; passing zero args.
                crate::rt_api::__esc_rt_call_indirect(async_iter_fn, 0, std::ptr::null())
            };
            crate::rt_api::CURRENT_THIS.with(|cell| cell.set(prev_this));
            return iter_obj;
        }
    }

    // Fallback: wrap a sync iterable as an async iterator using IterInitAsync
    if !v.is_null() && !v.is_undefined() {
        // Get the sync iterator
        let sync_iter = crate::rt_api::__esc_rt_iter_init(obj);
        // Wrap it: create an async iterator whose .next() wraps each sync
        // result in a resolved Promise
        return wrap_sync_as_async(sync_iter);
    }

    // TypeError: not iterable
    let msg = make_rt_string("AsyncIterator.from: argument is not iterable".to_string());
    let err = crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
    crate::rt_api::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Wrap a sync iterator as an async iterator.
///
/// Creates an object whose `.next()` calls the sync iterator's `.next()`
/// and wraps the result in a resolved Promise.
fn wrap_sync_as_async(sync_iter: u64) -> u64 {
    // We use a Map-like approach: create a simple async iterator
    // whose .next() pulls from the sync iterator and wraps in Promise
    let state = AsyncIteratorState {
        source: sync_iter,
        callback: 0,
        kind: AsyncHelperKind::Map,
        counter: 0,
        drop_done: false,
        done: false,
        inner_source: 0,
    };
    // We use a special "identity map" — callback is 0, which we handle
    // by not applying any transformation
    boxed_async_iterator(state)
}

// =========================================================================
// __esc_rt_iter_init_async — runtime ABI for IterInitAsync opcode
// =========================================================================

/// Initialize an async iterator for `for-await-of`.
///
/// Checks for `[Symbol.asyncIterator]` first, then falls back to
/// `[Symbol.iterator]` wrapped as an async iterator. Returns an
/// object implementing the async iterator protocol.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_init_async(obj: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Check for [Symbol.asyncIterator] on unified objects
    if v.is_object() {
        let tag = read_obj_tag(obj);
        if tag == Some(ObjTag::Unified as u8) {
            let async_iter_fn =
                crate::rt_api::get_prop_by_symbol_key(obj, crate::symbol::SYMBOL_ASYNC_ITERATOR);
            let async_val = JsValue::from_raw_bits(async_iter_fn);
            if !async_val.is_undefined() {
                // Call [Symbol.asyncIterator]() with obj as this
                let prev_this = crate::rt_api::CURRENT_THIS.with(|cell| cell.replace(obj));
                let iter_obj = unsafe {
                    // SAFETY: validated as callable; zero args.
                    crate::rt_api::__esc_rt_call_indirect(async_iter_fn, 0, std::ptr::null())
                };
                crate::rt_api::CURRENT_THIS.with(|cell| cell.set(prev_this));
                return iter_obj;
            }
        }
    }

    // Fallback: use [Symbol.iterator] and wrap as async
    let sync_iter = crate::rt_api::__esc_rt_iter_init(obj);
    wrap_sync_as_async(sync_iter)
}

// =========================================================================
// Method dispatch for async iterator objects
// =========================================================================

/// Dispatch a method call on an async iterator object.
///
/// Routes `.map()`, `.filter()`, `.take()`, `.drop()`, `.flatMap()`,
/// `.forEach()`, `.some()`, `.every()`, `.find()`, `.reduce()`,
/// `.toArray()`, and `.next()` to their implementations.
///
/// Returns `Some(result)` if the method was handled, `None` otherwise.
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub unsafe fn dispatch_async_iterator_method(
    obj: u64,
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = crate::rt_api::read_argv(argc, argv);

    match method {
        "map" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_map(obj, callback))
        }
        "filter" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_filter(obj, callback))
        }
        "take" => {
            let n = args.first().map_or(0.0, crate::rt_api::val_to_f64);
            let count = if n.is_finite() && n >= 0.0 {
                n as u32
            } else {
                0
            };
            Some(async_iterator_take(obj, count))
        }
        "drop" => {
            let n = args.first().map_or(0.0, crate::rt_api::val_to_f64);
            let count = if n.is_finite() && n >= 0.0 {
                n as u32
            } else {
                0
            };
            Some(async_iterator_drop(obj, count))
        }
        "flatMap" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_flat_map(obj, callback))
        }
        "forEach" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_for_each(obj, callback))
        }
        "some" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_some(obj, callback))
        }
        "every" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_every(obj, callback))
        }
        "find" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_find(obj, callback))
        }
        "reduce" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let has_initial = args.len() > 1;
            let initial = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(async_iterator_reduce(obj, callback, initial, has_initial))
        }
        "toArray" => Some(async_iterator_to_array(obj)),
        "next" => Some(async_iterator_next(obj)),
        _ => None,
    }
}

// =========================================================================
// Internal helpers — wrapper state mutation
// =========================================================================

/// Set the `done` flag on an async iterator wrapper.
fn set_wrapper_done(wrapper: u64) {
    let tag = read_obj_tag(wrapper);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(wrapper)
    };
    let Some(u) = uni else { return };
    let Some(InternalData::AsyncIterator { inner }) = u.internal_data_mut() else {
        return;
    };
    inner.done = true;
}

/// Set the counter on an async iterator wrapper (used by take/drop).
fn set_wrapper_counter(wrapper: u64, counter: u32) {
    let tag = read_obj_tag(wrapper);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(wrapper)
    };
    let Some(u) = uni else { return };
    let Some(InternalData::AsyncIterator { inner }) = u.internal_data_mut() else {
        return;
    };
    inner.counter = counter;
}

/// Set the `drop_done` flag on an async iterator wrapper.
fn set_wrapper_drop_done(wrapper: u64, drop_done: bool) {
    let tag = read_obj_tag(wrapper);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(wrapper)
    };
    let Some(u) = uni else { return };
    let Some(InternalData::AsyncIterator { inner }) = u.internal_data_mut() else {
        return;
    };
    inner.drop_done = drop_done;
}

/// Set the inner source on an async iterator wrapper (used by flatMap).
fn set_wrapper_inner_source(wrapper: u64, inner_source: u64) {
    let tag = read_obj_tag(wrapper);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(wrapper)
    };
    let Some(u) = uni else { return };
    let Some(InternalData::AsyncIterator { inner }) = u.internal_data_mut() else {
        return;
    };
    inner.inner_source = inner_source;
}

// =========================================================================
// Promise resolution helpers
// =========================================================================

/// Resolve a promise with `{value: undefined, done: true}`.
fn resolve_done(promise: u64) {
    let result = crate::rt_api::create_iterator_result(JsValue::undefined().raw_bits(), true);
    __esc_rt_promise_resolve(promise, result);
}

/// Resolve a promise with `{value, done: false}`.
fn resolve_with_value(promise: u64, value: u64) {
    let result = crate::rt_api::create_iterator_result(value, false);
    __esc_rt_promise_resolve(promise, result);
}
