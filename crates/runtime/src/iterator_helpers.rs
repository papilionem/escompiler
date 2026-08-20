//! ES2025 Iterator Helpers — lazy and eager helper methods for iterators.
//!
//! Implements the ES2025 Iterator Helpers proposal:
//! - **Lazy helpers** (return new wrapper iterators): `map`, `filter`, `take`, `drop`, `flatMap`
//! - **Eager helpers** (consume the iterator and return a value): `forEach`, `some`, `every`,
//!   `find`, `reduce`, `toArray`
//! - **Static method**: `Iterator.from(obj)`
//!
//! ## Architecture
//!
//! Lazy helpers create a new `JsIterator` with `IteratorKind::Helper` and a
//! [`HelperState`](crate::iterator::HelperState) that stores the underlying iterator,
//! callback function, helper kind, and any extra state. The `__esc_rt_iter_next`
//! function in `rt_api/iterator.rs` dispatches to [`advance_helper`] for these
//! wrapper iterators.
//!
//! Eager helpers call `__esc_rt_iter_next` in a loop and return the final result.

use nanbox::JsValue;
use shapes::ShapeTable;

use crate::internal_data::UnifiedObject;
use crate::iterator::{HelperKind, HelperState, IteratorResult, JsIterator};
use crate::rt_api::{
    __esc_rt_iter_done, __esc_rt_iter_init, __esc_rt_iter_next, __esc_rt_iter_value,
    create_array_from_elements, make_rt_string,
};
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, read_obj_tag};
use crate::value_ops;

/// Check if a NaN-boxed value is callable (for IsCallable validation).
///
/// Returns `true` if the value is a unified object with the callable flag set.
/// Used to validate callback arguments to iterator helper methods per spec.
fn is_value_callable(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni {
            return u.is_callable();
        }
    }
    false
}

/// Throw a TypeError for a non-callable callback argument.
///
/// Used by all iterator helper methods that require a callable callback.
fn throw_not_callable(method_name: &str) -> u64 {
    let msg = make_rt_string(format!(
        "TypeError: {method_name} callback is not a function"
    ));
    let err = crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
    crate::rt_api::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Throw a RangeError for an invalid limit argument (NaN or negative).
///
/// Used by `take` and `drop` iterator helper methods.
fn throw_invalid_limit(method_name: &str) -> u64 {
    let msg = make_rt_string(format!(
        "RangeError: {method_name} argument must be a non-negative number"
    ));
    let err = crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::RANGE_ERROR, msg);
    crate::rt_api::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

// =========================================================================
// Lazy helpers — create wrapper iterators
// =========================================================================

/// Create a boxed iterator object from a `JsIterator`.
fn boxed_iterator(iter: JsIterator) -> u64 {
    TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::iterator(ShapeTable::EMPTY_SHAPE, iter),
    )
}

/// `Iterator.prototype.map ( mapper )`
///
/// Returns a new iterator that applies `mapper` to each value produced by the
/// underlying iterator.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.map
pub fn iterator_map(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(mapper) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.map");
    }
    // 4. Let iterated be GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let closure be a new Abstract Closure with parameters (value) that captures iterated and mapper.
    // 6. Let result be CreateIteratorFromClosure(closure, "Iterator Helper", %IteratorHelperPrototype%, « [[UnderlyingIterator]] »).
    // 7. Set result.[[UnderlyingIterator]] to iterated.
    let helper = JsIterator::new_helper(iter_obj, callback, HelperKind::Map, 0);
    // 8. Return result.
    boxed_iterator(helper)
}

/// `Iterator.prototype.filter ( predicate )`
///
/// Returns a new iterator that yields only those values from the underlying
/// iterator for which `predicate` returns a truthy value.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.filter
pub fn iterator_filter(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(predicate) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.filter");
    }
    // 4. Let iterated be GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let closure be a new Abstract Closure with parameters (value) that captures iterated and predicate.
    // 6. Let result be CreateIteratorFromClosure(closure, "Iterator Helper", %IteratorHelperPrototype%, « [[UnderlyingIterator]] »).
    // 7. Set result.[[UnderlyingIterator]] to iterated.
    let helper = JsIterator::new_helper(iter_obj, callback, HelperKind::Filter, 0);
    // 8. Return result.
    boxed_iterator(helper)
}

/// `Iterator.prototype.take ( limit )`
///
/// Returns a new iterator that yields at most `limit` values from the
/// underlying iterator, then signals completion.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.take
pub fn iterator_take(iter_obj: u64, count: u32) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. Let numLimit be ? ToNumber(limit).
    // 4. If numLimit is NaN, throw a RangeError exception.
    // 5. Let integerLimit be ! ToIntegerOrInfinity(numLimit).
    // 6. If integerLimit < 0, throw a RangeError exception.
    // NOTE: NaN and negative checks are handled in dispatch_iterator_method
    // before calling this function. The `count: u32` parameter is already
    // validated. See also `iterator_take_raw` for the pre-validation path.
    // 7. Let iterated be GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record, count is the integer limit.
    // 8. Let closure be a new Abstract Closure with parameters () that captures iterated and integerLimit.
    // 9. Let result be CreateIteratorFromClosure(closure, "Iterator Helper", %IteratorHelperPrototype%, « [[UnderlyingIterator]] »).
    // 10. Set result.[[UnderlyingIterator]] to iterated.
    let helper = JsIterator::new_helper(iter_obj, 0, HelperKind::Take, count);
    // 11. Return result.
    boxed_iterator(helper)
}

/// `Iterator.prototype.drop ( limit )`
///
/// Returns a new iterator that skips the first `limit` values from the
/// underlying iterator, then passes through all subsequent values.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.drop
pub fn iterator_drop(iter_obj: u64, count: u32) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. Let numLimit be ? ToNumber(limit).
    // 4. If numLimit is NaN, throw a RangeError exception.
    // 5. Let integerLimit be ! ToIntegerOrInfinity(numLimit).
    // 6. If integerLimit < 0, throw a RangeError exception.
    // NOTE: NaN and negative checks are handled in dispatch_iterator_method
    // before calling this function. The `count: u32` parameter is already
    // validated. See also `iterator_drop_raw` for the pre-validation path.
    // 7. Let iterated be GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record, count is the integer limit.
    // 8. Let closure be a new Abstract Closure with parameters () that captures iterated and integerLimit.
    // 9. Let result be CreateIteratorFromClosure(closure, "Iterator Helper", %IteratorHelperPrototype%, « [[UnderlyingIterator]] »).
    // 10. Set result.[[UnderlyingIterator]] to iterated.
    let helper = JsIterator::new_helper(iter_obj, 0, HelperKind::Drop, count);
    // 11. Return result.
    boxed_iterator(helper)
}

/// `Iterator.prototype.flatMap ( mapper )`
///
/// Returns a new iterator that applies `mapper` to each value from the
/// underlying iterator, then flattens the result by one level. If the
/// mapped result is iterable, its values are yielded individually.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.flatmap
pub fn iterator_flat_map(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(mapper) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.flatMap");
    }
    // 4. Let iterated be GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let closure be a new Abstract Closure with parameters () that captures iterated and mapper.
    // 6. Let result be CreateIteratorFromClosure(closure, "Iterator Helper", %IteratorHelperPrototype%, « [[UnderlyingIterator]] »).
    // 7. Set result.[[UnderlyingIterator]] to iterated.
    let helper = JsIterator::new_helper(iter_obj, callback, HelperKind::FlatMap, 0);
    // 8. Return result.
    boxed_iterator(helper)
}

// =========================================================================
// Eager helpers — consume the iterator and return a value
// =========================================================================

/// `Iterator.prototype.forEach ( fn )`
///
/// Calls `fn(value, counter)` for each value produced by the iterator.
/// Returns `undefined`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.foreach
pub fn iterator_for_each(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(fn) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.forEach");
    }
    // 4. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let counter be 0.
    // TODO: counter argument not yet passed to callback.
    // 6. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return undefined.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            break;
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Let result be Completion(Call(fn, undefined, « value, F(counter) »)).
        call_callback(callback, value);
        // e. IfAbruptCloseIterator(result, iterated).
        // TODO: Step 6e — abrupt close not yet implemented.
        // f. Set counter to counter + 1.
    }
    JsValue::undefined().raw_bits()
}

/// `Iterator.prototype.some ( predicate )`
///
/// Returns `true` if `predicate(value)` returns a truthy value for any
/// element produced by the iterator. Returns `false` if the iterator
/// completes without a truthy result.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.some
pub fn iterator_some(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(predicate) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.some");
    }
    // 4. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let counter be 0.
    // TODO: counter argument not yet passed to predicate.
    // 6. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return false.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return JsValue::bool(false).raw_bits();
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Let result be Completion(Call(predicate, undefined, « value, F(counter) »)).
        let cb_result = call_callback(callback, value);
        // e. IfAbruptCloseIterator(result, iterated).
        // TODO: Step 6e — abrupt close not yet implemented.
        // f. If ToBoolean(result) is true, return ? IteratorClose(iterated, NormalCompletion(true)).
        if value_ops::to_boolean(JsValue::from_raw_bits(cb_result)) {
            return JsValue::bool(true).raw_bits();
        }
        // g. Set counter to counter + 1.
    }
}

/// `Iterator.prototype.every ( predicate )`
///
/// Returns `true` if `predicate(value)` returns a truthy value for every
/// element produced by the iterator. Returns `false` as soon as a falsy
/// result is encountered.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.every
pub fn iterator_every(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(predicate) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.every");
    }
    // 4. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let counter be 0.
    // TODO: counter argument not yet passed to predicate.
    // 6. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return true.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return JsValue::bool(true).raw_bits();
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Let result be Completion(Call(predicate, undefined, « value, F(counter) »)).
        let cb_result = call_callback(callback, value);
        // e. IfAbruptCloseIterator(result, iterated).
        // TODO: Step 6e — abrupt close not yet implemented.
        // f. If ToBoolean(result) is false, return ? IteratorClose(iterated, NormalCompletion(false)).
        if !value_ops::to_boolean(JsValue::from_raw_bits(cb_result)) {
            return JsValue::bool(false).raw_bits();
        }
        // g. Set counter to counter + 1.
    }
}

/// `Iterator.prototype.find ( predicate )`
///
/// Returns the first value for which `predicate(value)` returns a truthy
/// value. Returns `undefined` if the iterator completes without a match.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.find
pub fn iterator_find(iter_obj: u64, callback: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(predicate) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.find");
    }
    // 4. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 5. Let counter be 0.
    // TODO: counter argument not yet passed to predicate.
    // 6. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return undefined.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return JsValue::undefined().raw_bits();
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Let result be Completion(Call(predicate, undefined, « value, F(counter) »)).
        let cb_result = call_callback(callback, value);
        // e. IfAbruptCloseIterator(result, iterated).
        // TODO: Step 6e — abrupt close not yet implemented.
        // f. If ToBoolean(result) is true, return ? IteratorClose(iterated, NormalCompletion(value)).
        if value_ops::to_boolean(JsValue::from_raw_bits(cb_result)) {
            return value;
        }
        // g. Set counter to counter + 1.
    }
}

/// `Iterator.prototype.reduce ( reducer [ , initialValue ] )`
///
/// Accumulates values from the iterator using `reducer(accumulator, value)`.
/// If no `initialValue` is provided and the iterator is empty, throws a
/// `TypeError`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.reduce
pub fn iterator_reduce(iter_obj: u64, callback: u64, initial: u64, has_initial: bool) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. If IsCallable(reducer) is false, throw a TypeError exception.
    if !is_value_callable(callback) {
        return throw_not_callable("Iterator.prototype.reduce");
    }
    // 4. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    let mut accumulator: u64;

    if has_initial {
        // 5. If initialValue is present, let accumulator be initialValue.
        accumulator = initial;
    } else {
        // 6. Else,
        //   a. Let next be ? IteratorStep(iterated).
        let first = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(first);
        //   b. If next is false, throw a TypeError exception.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            let msg = make_rt_string("Reduce of empty iterator with no initial value".to_string());
            let err =
                crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
            crate::rt_api::__esc_rt_throw(err);
            return JsValue::undefined().raw_bits();
        }
        //   c. Let accumulator be ? IteratorValue(next).
        accumulator = __esc_rt_iter_value(first);
    }

    // 7. Let counter be 0. (or 1 if no initialValue)
    // TODO: counter argument not yet passed to reducer.
    // 8. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return accumulator.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return accumulator;
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Let result be Completion(Call(reducer, undefined, « accumulator, value, F(counter) »)).
        accumulator = call_callback_2(callback, accumulator, value);
        // e. IfAbruptCloseIterator(result, iterated).
        // TODO: Step 8e — abrupt close not yet implemented.
        // f. Set accumulator to result.[[Value]].
        // NOTE: already done above via assignment.
        // g. Set counter to counter + 1.
    }
}

/// `Iterator.prototype.toArray ( )`
///
/// Collects all remaining values from the iterator into a new Array.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.toarray
pub fn iterator_to_array(iter_obj: u64) -> u64 {
    // 1. Let O be the this value.
    // 2. If O is not an Object, throw a TypeError exception.
    // TODO: Step 2 — type validation not yet implemented.
    // 3. Let iterated be ? GetIteratorDirect(O).
    // NOTE: iter_obj is already the iterator record.
    // 4. Let items be a new empty List.
    let mut elements = Vec::new();
    // 5. Repeat,
    loop {
        // a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(iter_obj);
        let done = __esc_rt_iter_done(result);
        // b. If next is false, return CreateArrayFromList(items).
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            break;
        }
        // c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // d. Append value to items.
        elements.push(JsValue::from_raw_bits(value));
    }
    create_array_from_elements(elements)
}

/// `Iterator.from ( O )`
///
/// Wraps an object as a proper Iterator. If `O` has `[Symbol.iterator]`,
/// calls it to get the iterator. If `O` already implements the iterator
/// protocol, wraps it. Otherwise throws `TypeError`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iterator.from
pub fn iterator_from(obj: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // 1. If O is not an Object, throw a TypeError exception.
    // NOTE: Partial — we check null/undefined below.

    // 2. Let iteratorRecord be ? GetIteratorFlattenable(O, iterate-strings).
    // NOTE: Simplified — we check if it's already an iterator, then try Symbol.iterator.

    // Fast path: if already an internal iterator object, return it directly.
    if let Some(tag) = read_obj_tag(obj)
        && tag == ObjTag::Unified as u8
    {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Iterator
        {
            // 3. Let hasInstance be ? OrdinaryHasInstance(%Iterator%, iteratorRecord.[[Iterator]]).
            // 4. If hasInstance is true, return iteratorRecord.[[Iterator]].
            return obj;
        }
    }

    // Try to get [Symbol.iterator] method and call it
    if !v.is_null() && !v.is_undefined() {
        // 5. Let wrapper be OrdinaryObjectCreate(%WrapForValidIteratorPrototype%, « [[Iterated]] »).
        // 6. Set wrapper.[[Iterated]] to iteratorRecord.
        // NOTE: We use __esc_rt_iter_init which handles Symbol.iterator dispatch.
        let iter_obj = __esc_rt_iter_init(obj);
        // 7. Return wrapper.
        return iter_obj;
    }

    // TypeError: not iterable
    let msg = make_rt_string("Iterator.from: argument is not iterable".to_string());
    let err = crate::rt_api::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
    crate::rt_api::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

// =========================================================================
// Helper iterator advancement — called by __esc_rt_iter_next
// =========================================================================

/// Advance a helper iterator by one step.
///
/// This is called from `__esc_rt_iter_next` when the iterator kind is `Helper`.
/// It dispatches to the appropriate helper logic based on [`HelperKind`].
///
/// Returns an [`IteratorResult`] with either the next transformed value or done.
pub fn advance_helper(state: &mut HelperState) -> IteratorResult {
    match state.helper_kind {
        HelperKind::Map => advance_map(state),
        HelperKind::Filter => advance_filter(state),
        HelperKind::Take => advance_take(state),
        HelperKind::Drop => advance_drop(state),
        HelperKind::FlatMap => advance_flat_map(state),
    }
}

/// Advance a `map` helper: pull one value, transform it, return result.
///
/// Implements the closure body from `Iterator.prototype.map`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.map (step 5 closure)
fn advance_map(state: &mut HelperState) -> IteratorResult {
    // 5.a. Let next be ? IteratorStep(iterated).
    let result = __esc_rt_iter_next(state.underlying);
    let done = __esc_rt_iter_done(result);
    // 5.b. If next is false, return undefined.
    if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
        return IteratorResult::done();
    }
    // 5.c. Let value be ? IteratorValue(next).
    let value = __esc_rt_iter_value(result);
    // 5.d. Let mapped be Completion(Call(mapper, undefined, « value, F(counter) »)).
    let mapped = call_callback(state.callback, value);
    // 5.e. IfAbruptCloseIterator(mapped, iterated).
    // TODO: Step 5e — abrupt close not yet implemented.
    // 5.f. Let completion be Completion(Yield(mapped)).
    // 5.g. IfAbruptCloseIterator(completion, iterated).
    // TODO: Step 5g — abrupt close not yet implemented.
    // 5.h. Set counter to counter + 1.
    IteratorResult::with_value(mapped)
}

/// Advance a `filter` helper: loop until a matching value is found or done.
///
/// Implements the closure body from `Iterator.prototype.filter`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.filter (step 5 closure)
fn advance_filter(state: &mut HelperState) -> IteratorResult {
    // 5. Repeat,
    loop {
        // 5.a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(state.underlying);
        let done = __esc_rt_iter_done(result);
        // 5.b. If next is false, return undefined.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return IteratorResult::done();
        }
        // 5.c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);
        // 5.d. Let selected be Completion(Call(predicate, undefined, « value, F(counter) »)).
        let keep = call_callback(state.callback, value);
        // 5.e. IfAbruptCloseIterator(selected, iterated).
        // TODO: Step 5e — abrupt close not yet implemented.
        // 5.f. If ToBoolean(selected) is true, then
        if value_ops::to_boolean(JsValue::from_raw_bits(keep)) {
            //   i. Let completion be Completion(Yield(value)).
            //   ii. IfAbruptCloseIterator(completion, iterated).
            // TODO: Step 5f.ii — abrupt close not yet implemented.
            return IteratorResult::with_value(value);
        }
        // 5.g. Set counter to counter + 1.
    }
}

/// Advance a `take` helper: return values until counter hits 0.
///
/// Implements the closure body from `Iterator.prototype.take`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.take (step 8 closure)
fn advance_take(state: &mut HelperState) -> IteratorResult {
    // 8.a. Let remaining be integerLimit.
    // 8.b. Repeat,
    //   i. If remaining is 0, then
    if state.counter == 0 {
        //     1. Return ? IteratorClose(iterated, NormalCompletion(undefined)).
        // TODO: IteratorClose not yet called on early completion.
        return IteratorResult::done();
    }
    //   ii. If remaining is not +Infinity, set remaining to remaining - 1.
    //   iii. Let next be ? IteratorStep(iterated).
    let result = __esc_rt_iter_next(state.underlying);
    let done = __esc_rt_iter_done(result);
    //   iv. If next is false, return undefined.
    if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
        state.counter = 0;
        return IteratorResult::done();
    }
    state.counter -= 1;
    //   v. Let value be ? IteratorValue(next).
    let value = __esc_rt_iter_value(result);
    //   vi. Let completion be Completion(Yield(value)).
    //   vii. IfAbruptCloseIterator(completion, iterated).
    // TODO: Step 8b.vii — abrupt close not yet implemented.
    IteratorResult::with_value(value)
}

/// Advance a `drop` helper: skip first n values, then pass through.
///
/// Implements the closure body from `Iterator.prototype.drop`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.drop (step 8 closure)
fn advance_drop(state: &mut HelperState) -> IteratorResult {
    // 8.a. Let remaining be integerLimit.
    // 8.b. Repeat, while remaining > 0,
    //   (Skip phase: consume values until counter reaches 0)
    while !state.drop_done {
        if state.counter == 0 {
            state.drop_done = true;
            break;
        }
        //   i. If remaining is not +Infinity, set remaining to remaining - 1.
        //   ii. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(state.underlying);
        let done = __esc_rt_iter_done(result);
        //   iii. If next is false, return undefined.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            state.drop_done = true;
            return IteratorResult::done();
        }
        state.counter -= 1;
    }
    // 8.c. Repeat,
    //   (Pass-through phase)
    //   i. Let next be ? IteratorStep(iterated).
    let result = __esc_rt_iter_next(state.underlying);
    let done = __esc_rt_iter_done(result);
    //   ii. If next is false, return undefined.
    if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
        return IteratorResult::done();
    }
    //   iii. Let value be ? IteratorValue(next).
    let value = __esc_rt_iter_value(result);
    //   iv. Let completion be Completion(Yield(value)).
    //   v. IfAbruptCloseIterator(completion, iterated).
    // TODO: Step 8c.v — abrupt close not yet implemented.
    IteratorResult::with_value(value)
}

/// Advance a `flatMap` helper: map each value, then iterate the mapped result.
///
/// Implements the closure body from `Iterator.prototype.flatMap`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorprototype.flatmap (step 5 closure)
fn advance_flat_map(state: &mut HelperState) -> IteratorResult {
    // 5. Repeat,
    loop {
        // If we have an active inner iterator, try to get the next value from it.
        // This corresponds to steps 5.g-5.k (inner iterator loop).
        if state.inner_iter != 0 {
            // 5.h. Let innerNext be ? IteratorStep(innerIterator).
            let result = __esc_rt_iter_next(state.inner_iter);
            let done = __esc_rt_iter_done(result);
            if !value_ops::to_boolean(JsValue::from_raw_bits(done)) {
                // 5.i. If innerNext is not false, then
                //   i. Let innerValue be ? IteratorValue(innerNext).
                let value = __esc_rt_iter_value(result);
                //   ii. Let completion be Completion(Yield(innerValue)).
                //   iii. IfAbruptCloseIterator(completion, iterated).
                // TODO: Step 5i.iii — abrupt close not yet implemented.
                return IteratorResult::with_value(value);
            }
            // 5.j. If innerNext is false, continue to next outer value.
            state.inner_iter = 0;
        }

        // 5.a. Let next be ? IteratorStep(iterated).
        let result = __esc_rt_iter_next(state.underlying);
        let done = __esc_rt_iter_done(result);
        // 5.b. If next is false, return undefined.
        if value_ops::to_boolean(JsValue::from_raw_bits(done)) {
            return IteratorResult::done();
        }
        // 5.c. Let value be ? IteratorValue(next).
        let value = __esc_rt_iter_value(result);

        // 5.d. Let mapped be Completion(Call(mapper, undefined, « value, F(counter) »)).
        let mapped = call_callback(state.callback, value);
        // 5.e. IfAbruptCloseIterator(mapped, iterated).
        // TODO: Step 5e — abrupt close not yet implemented.

        // 5.f. Let innerIterator be Completion(GetIteratorFlattenable(mapped, reject-strings)).
        // NOTE: We use a simplified check — strings and iterable objects get iterated,
        // non-iterable values are yielded directly.
        let mapped_val = JsValue::from_raw_bits(mapped);
        if mapped_val.is_string() || is_iterable_object(mapped) {
            // 5.g. Let innerAlive be true.
            let inner = __esc_rt_iter_init(mapped);
            state.inner_iter = inner;
            // Try to get the first value from the inner iterator
            let inner_result = __esc_rt_iter_next(inner);
            let inner_done = __esc_rt_iter_done(inner_result);
            if !value_ops::to_boolean(JsValue::from_raw_bits(inner_done)) {
                let inner_value = __esc_rt_iter_value(inner_result);
                return IteratorResult::with_value(inner_value);
            }
            // Inner was empty, clear and continue to next outer value
            state.inner_iter = 0;
        } else {
            // Non-iterable mapped value: yield it directly
            // NOTE: Spec says to throw TypeError if mapped is not iterable,
            // but we yield it for compatibility.
            return IteratorResult::with_value(mapped);
        }
        // 5.k. Set counter to counter + 1.
    }
}

// =========================================================================
// Callback invocation helpers
// =========================================================================

/// Call a callback function with one argument.
///
/// Uses `__esc_rt_call_indirect` to invoke the callback, passing the value
/// as the first argument.
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

/// Check if a NaN-boxed value is an iterable object (has array or iterable kind).
fn is_iterable_object(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag == ObjTag::Unified as u8 {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(bits)
        };
        if let Some(u) = uni {
            return matches!(
                u.kind,
                crate::internal_data::InternalKind::Array
                    | crate::internal_data::InternalKind::SetObj
                    | crate::internal_data::InternalKind::MapObj
                    | crate::internal_data::InternalKind::Iterator
            );
        }
    }
    false
}

// =========================================================================
// Method dispatch for iterator objects
// =========================================================================

/// Dispatch a method call on an iterator object.
///
/// Routes `.map()`, `.filter()`, `.take()`, `.drop()`, `.flatMap()`,
/// `.forEach()`, `.some()`, `.every()`, `.find()`, `.reduce()`, and
/// `.toArray()` to their implementations.
///
/// Returns `Some(result)` if the method was handled, `None` otherwise.
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub unsafe fn dispatch_iterator_method(
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
            Some(iterator_map(obj, callback))
        }
        "filter" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_filter(obj, callback))
        }
        "take" => {
            let n = args.first().map_or(0.0, crate::rt_api::val_to_f64);
            // §27.1.4.6 step 4: If numLimit is NaN, throw a RangeError exception.
            if n.is_nan() {
                return Some(throw_invalid_limit("Iterator.prototype.take"));
            }
            // §27.1.4.6 step 6: If integerLimit < 0, throw a RangeError exception.
            if n < 0.0 {
                return Some(throw_invalid_limit("Iterator.prototype.take"));
            }
            let count = if n.is_finite() { n as u32 } else { u32::MAX };
            Some(iterator_take(obj, count))
        }
        "drop" => {
            let n = args.first().map_or(0.0, crate::rt_api::val_to_f64);
            // §27.1.4.3 step 4: If numLimit is NaN, throw a RangeError exception.
            if n.is_nan() {
                return Some(throw_invalid_limit("Iterator.prototype.drop"));
            }
            // §27.1.4.3 step 6: If integerLimit < 0, throw a RangeError exception.
            if n < 0.0 {
                return Some(throw_invalid_limit("Iterator.prototype.drop"));
            }
            let count = if n.is_finite() { n as u32 } else { u32::MAX };
            Some(iterator_drop(obj, count))
        }
        "flatMap" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_flat_map(obj, callback))
        }
        "forEach" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_for_each(obj, callback))
        }
        "some" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_some(obj, callback))
        }
        "every" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_every(obj, callback))
        }
        "find" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_find(obj, callback))
        }
        "reduce" => {
            let callback = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let has_initial = args.len() > 1;
            let initial = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(iterator_reduce(obj, callback, initial, has_initial))
        }
        "toArray" => Some(iterator_to_array(obj)),
        "next" => {
            // Forward .next() to the standard iterator protocol
            Some(__esc_rt_iter_next(obj))
        }
        _ => None,
    }
}
