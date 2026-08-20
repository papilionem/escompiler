//! Iterator protocol runtime functions.
//!
//! Contains `__esc_rt_iter_init`, `__esc_rt_iter_next`, `__esc_rt_iter_done`,
//! `__esc_rt_iter_value`, `__esc_rt_iter_close`, and helper functions.
//!
//! Implements the abstract operations from the ECMAScript spec:
//! - GetIterator (§7.4.1)
//! - IteratorNext (§7.4.2)
//! - IteratorComplete (§7.4.3)
//! - IteratorValue (§7.4.4)
//! - IteratorStep (§7.4.5)
//! - IteratorClose (§7.4.6)
//! - CreateIterResultObject (§7.4.7)

use nanbox::JsValue;

use shapes::ShapeTable;

use crate::internal_data::UnifiedObject;
use crate::iterator::{IteratorKind, IteratorResult, JsIterator};
use crate::symbol::SYMBOL_ITERATOR;
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut, read_obj_tag};
use crate::{exceptions, string_ops, value_ops};

/// Create a unified iterator object from a [`JsIterator`].
///
/// Internal helper — wraps a `JsIterator` into a NaN-boxed unified object
/// tagged with `InternalKind::Iterator`.
fn boxed_iterator(iter: JsIterator) -> u64 {
    TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::iterator(ShapeTable::EMPTY_SHAPE, iter),
    )
}

/// Create a unified iterator result object from an [`IteratorResult`].
///
/// Corresponds to `CreateIterResultObject ( value, done )` — ES2024 §7.4.7.
///
/// [spec]: https://tc39.es/ecma262/#sec-createiterresultobject
///
/// Spec steps:
/// 1. Let obj be OrdinaryObjectCreate(%Object.prototype%).
/// 2. Perform ! CreateDataPropertyOrThrow(obj, "value", value).
/// 3. Perform ! CreateDataPropertyOrThrow(obj, "done", done).
/// 4. Return obj.
///
/// Note: Our implementation uses a specialized `InternalKind::IterResult`
/// rather than a generic object, but the observable shape is `{value, done}`.
fn boxed_iter_result(result: IteratorResult) -> u64 {
    TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::iter_result(result.value, result.done),
    )
}

use super::{
    __esc_rt_call_method, __esc_rt_create_error, __esc_rt_get_prop, __esc_rt_throw, CURRENT_THIS,
    create_array_from_elements, dispatch_generator_method, get_prop_by_symbol_key, make_rt_string,
};

/// Check if a NaN-boxed value is callable.
///
/// Returns `true` if the value is a unified object with the callable flag set.
/// Used internally to verify `[Symbol.iterator]` and `.return()` methods.
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

/// Call a `[Symbol.iterator]()` method on an object.
///
/// Sets `this` to the receiver object, invokes the iterator factory function
/// with zero arguments, and returns the resulting iterator object.
///
/// This is the "Call(method, obj)" portion of GetIterator step 4.
fn call_symbol_iterator(receiver: u64, method: u64) -> u64 {
    // Set `this` to the receiver so the iterator factory can access it
    let prev_this = CURRENT_THIS.with(|cell| cell.replace(receiver));
    let result = unsafe {
        // SAFETY: method was validated as callable; passing zero args with null argv.
        super::__esc_rt_call_indirect(method, 0, std::ptr::null())
    };
    CURRENT_THIS.with(|cell| cell.set(prev_this));
    result
}

/// `GetIterator ( obj, kind )` — ES2024 §7.4.1
///
/// Returns an iterator record for the given object by looking up
/// `[Symbol.iterator]` and calling it.
///
/// [spec]: https://tc39.es/ecma262/#sec-getiterator
///
/// For `kind = sync` (the only kind we support):
/// 1. Let method be ? GetMethod(obj, @@iterator).
/// 2. If method is undefined, throw a TypeError exception.
/// 3. Let iterator be ? Call(method, obj).
/// 4. If iterator is not an Object, throw a TypeError exception.
/// 5. Let nextMethod be ? GetV(iterator, "next").
/// 6. Let iteratorRecord be the Iterator Record { [[Iterator]]: iterator,
///    [[NextMethod]]: nextMethod, [[Done]]: false }.
/// 7. Return iteratorRecord.
///
/// Note: Our AOT implementation uses specialized fast-paths for known
/// built-in iterables (Array, Map, Set, String, Generator) before falling
/// back to the generic `[Symbol.iterator]()` protocol. Steps 5-6 are
/// deferred — we store the iterator object and look up `.next()` lazily
/// in `__esc_rt_iter_next`.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_init(obj: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Fast path: String iteration — iterate over code points.
    // Spec: String has a built-in @@iterator (§22.1.5.1) that creates
    // a String Iterator object per §22.1.5.2 (StringIteratorPrototype.next).
    if v.is_string() {
        let s = string_ops::get_string_data(v);
        let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
        let iter = JsIterator::new_string_chars(obj, chars);
        return boxed_iterator(iter);
    }

    let tag = read_obj_tag(obj);

    // Unified object: dispatch by InternalKind (fast paths for known iterables)
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(obj)
        };
        if let Some(u) = uni {
            // Fast path: Map — uses %MapIteratorPrototype%.next (§24.1.5.2)
            if u.kind == crate::internal_data::InternalKind::MapObj
                || u.kind == crate::internal_data::InternalKind::WeakMapObj
            {
                let iter = JsIterator::new_map_entries(obj);
                return boxed_iterator(iter);
            }
            // Fast path: Set — uses %SetIteratorPrototype%.next (§24.2.5.2)
            if u.kind == crate::internal_data::InternalKind::SetObj
                || u.kind == crate::internal_data::InternalKind::WeakSetObj
            {
                let iter = JsIterator::new_set_values(obj);
                return boxed_iterator(iter);
            }
            // Fast path: Generator — already an iterator (§27.5.1)
            if u.kind == crate::internal_data::InternalKind::Generator {
                let iter = JsIterator::new_generator(obj);
                return boxed_iterator(iter);
            }
            // Fast path: Array — uses %ArrayIteratorPrototype%.next (§23.1.5.2)
            if u.kind == crate::internal_data::InternalKind::Array {
                let iter = JsIterator::new_array(obj);
                return boxed_iterator(iter);
            }

            // Step 1: Let method be ? GetMethod(obj, @@iterator).
            let sym_iter_fn = get_prop_by_symbol_key(obj, SYMBOL_ITERATOR);
            let sym_val = JsValue::from_raw_bits(sym_iter_fn);
            if !sym_val.is_undefined() && is_value_callable(sym_iter_fn) {
                // Step 3: Let iterator be ? Call(method, obj).
                let iterator_obj = call_symbol_iterator(obj, sym_iter_fn);
                // Step 4: If iterator is not an Object, throw a TypeError exception.
                let iter_val = JsValue::from_raw_bits(iterator_obj);
                if !iter_val.is_object() {
                    let msg = make_rt_string(
                        "TypeError: Result of the Symbol.iterator method is not an object"
                            .to_string(),
                    );
                    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    __esc_rt_throw(err);
                    let iter = JsIterator::new_array(obj);
                    return boxed_iterator(iter);
                }
                let iter = JsIterator::new_custom(iterator_obj);
                return boxed_iterator(iter);
            }

            // Object without @@iterator — not iterable per §7.2.5 GetIterator.
            // for-of should throw TypeError. for-in uses __esc_rt_for_in_init instead.
            // Fall through to the TypeError below.
        }
    }

    // Step 2: If method is undefined, throw a TypeError exception.
    let type_name = if v.is_null() {
        "null"
    } else if v.is_undefined() {
        "undefined"
    } else if v.is_number() || v.is_int() {
        "number"
    } else if v.is_bool() {
        "boolean"
    } else {
        "value"
    };
    let msg = make_rt_string(format!("{type_name} is not iterable"));
    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    __esc_rt_throw(err);
    // Return an empty done iterator as fallback
    let iter = JsIterator::new_array(obj);
    boxed_iterator(iter)
}

/// `EnumerateObjectProperties ( O )` — ES2024 §14.7.5.9
///
/// Creates an iterator that yields the enumerable string-keyed own AND
/// inherited properties of `obj`, with own properties shadowing inherited ones.
/// This is used exclusively by `for..in` loops.
///
/// [spec]: https://tc39.es/ecma262/#sec-enumerate-object-properties
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_for_in_init(obj: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // for..in on null/undefined produces no iterations (per spec, step 2 of
    // ForIn/OfHeadEvaluation: If exprValue is undefined or null, return break).
    if v.is_null() || v.is_undefined() {
        let iter = JsIterator::new_object_keys(obj, Vec::new());
        return boxed_iterator(iter);
    }

    // Collect enumerable string-keyed properties from the entire prototype chain.
    // Own properties shadow inherited ones (de-duplicated via insertion order).
    let mut seen = std::collections::HashSet::new();
    let mut keys = Vec::new();

    let mut current = obj;
    for _ in 0..100 {
        let tag = super::read_obj_tag(current);
        if tag != Some(crate::tagged_obj::ObjTag::Unified as u8) {
            break;
        }
        let uni = unsafe { super::deref_tagged::<crate::internal_data::UnifiedObject>(current) };
        let Some(u) = uni else { break };

        // For arrays, enumerate integer indices first ("0", "1", ...)
        if let Some(len) = u.as_array_length() {
            for i in 0..len as usize {
                let key = i.to_string();
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
        }

        // Collect enumerable keys from shape-based properties
        let obj_keys = super::SHAPES.with(|shapes| {
            super::INTERNER.with(|interner| {
                let shapes = shapes.borrow();
                let interner = interner.borrow();
                u.enumerable_keys(&shapes, &interner)
            })
        });
        // Filter out deleted properties
        let deleted = super::DELETED_PROPS.with(|dp| dp.borrow().get(&current).cloned());
        for key in obj_keys {
            if deleted.as_ref().is_some_and(|d| d.contains(&key)) {
                continue;
            }
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }

        // Also collect string-keyed properties from the OBJECT_PROPS side-table
        super::OBJECT_PROPS.with(|props| {
            let props = props.borrow();
            if let Some(map) = props.get(&current) {
                for key in map.keys() {
                    // Skip internal/non-enumerable markers
                    if (!key.starts_with('_') || !key.starts_with("__")) && seen.insert(key.clone())
                    {
                        keys.push(key.clone());
                    }
                }
            }
        });

        // Walk to prototype
        match super::get_prototype_object(u) {
            Some(proto_bits) if proto_bits != current => current = proto_bits,
            _ => break,
        }
    }

    let iter = JsIterator::new_object_keys(obj, keys);
    boxed_iterator(iter)
}

/// `IteratorNext ( iteratorRecord [ , value ] )` — ES2024 §7.4.2
///
/// Advances the iterator and returns the next result object `{ value, done }`.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratornext
///
/// Spec steps:
/// 1. If value is not present, then
///    a. Let result be ? Call(iteratorRecord.[[NextMethod]],
///    iteratorRecord.[[Iterator]]).
/// 2. Else,
///    a. Let result be ? Call(iteratorRecord.[[NextMethod]],
///    iteratorRecord.[[Iterator]], << value >>).
/// 3. If result is not an Object, throw a TypeError exception.
/// 4. Return result.
///
/// Note: Our implementation dispatches based on `IteratorKind` — each kind
/// (Array, Map, Set, Custom, Generator, etc.) has its own next logic. For
/// Custom and Generator iterators, we call `.next()` on the JS iterator
/// object (matching spec steps 1-2). Step 3 (type-check result) is handled
/// by `__esc_rt_iter_validate_result` when needed.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_next(iter: u64) -> u64 {
    let tag = read_obj_tag(iter);

    // Extract the JsIterator from the unified path
    let it: Option<&mut JsIterator> = if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(iter)
        };
        uni.and_then(|u| {
            if let Some(crate::internal_data::InternalData::IteratorState { inner }) =
                u.internal_data_mut()
            {
                Some(&mut **inner)
            } else {
                None
            }
        })
    } else {
        None
    };

    let Some(it) = it else {
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    };

    if it.done {
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Helper iterator: delegate to the iterator helpers module
    // (Iterator Helpers proposal — §2.1.3.1.1 %IteratorHelperPrototype%.next)
    if it.kind == IteratorKind::Helper {
        if let Some(ref mut helper_state) = it.helper {
            let result = crate::iterator_helpers::advance_helper(helper_state);
            if value_ops::to_boolean(JsValue::from_raw_bits(result.done)) {
                it.done = true;
            }
            return boxed_iter_result(result);
        }
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Custom iterator: call .next() on the JS iterator object
    // Step 1a: Let result be ? Call(iteratorRecord.[[NextMethod]],
    //          iteratorRecord.[[Iterator]]).
    if it.kind == IteratorKind::Custom {
        let next_key = make_rt_string("next".to_string());
        let result_obj = unsafe {
            // SAFETY: the iterator object was created by compiled code.
            __esc_rt_call_method(it.target, next_key, 0, std::ptr::null())
        };
        // Step 3: If result is not an Object, throw a TypeError exception.
        let result_val = JsValue::from_raw_bits(result_obj);
        if !result_val.is_object() {
            let msg = make_rt_string("TypeError: Iterator result is not an object".to_string());
            let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
            __esc_rt_throw(err);
            it.done = true;
            let done_result = IteratorResult::done();
            return boxed_iter_result(done_result);
        }
        // result_obj should be a plain object with {value, done}
        let done_key = make_rt_string("done".to_string());
        let done_val = __esc_rt_get_prop(result_obj, done_key);
        let is_done = value_ops::to_boolean(JsValue::from_raw_bits(done_val));
        if is_done {
            it.done = true;
            let result = IteratorResult::done();
            return boxed_iter_result(result);
        }
        let value_key = make_rt_string("value".to_string());
        let value = __esc_rt_get_prop(result_obj, value_key);
        let result = IteratorResult::with_value(value);
        return boxed_iter_result(result);
    }

    // Generator iterator: delegate to the generator .next() protocol
    // Per §27.5.3.3 GeneratorResume, this calls the generator's next step.
    // Step 1a: Let result be ? Call(iteratorRecord.[[NextMethod]],
    //          iteratorRecord.[[Iterator]]).
    if it.kind == IteratorKind::Generator {
        let result_obj = dispatch_generator_method(it.target, "next");
        // result_obj is a plain object with {value, done}
        let done_key = make_rt_string("done".to_string());
        let done_val = __esc_rt_get_prop(result_obj, done_key);
        let is_done = value_ops::to_boolean(JsValue::from_raw_bits(done_val));
        if is_done {
            it.done = true;
            let result = IteratorResult::done();
            return boxed_iter_result(result);
        }
        let value_key = make_rt_string("value".to_string());
        let value = __esc_rt_get_prop(result_obj, value_key);
        let result = IteratorResult::with_value(value);
        return boxed_iter_result(result);
    }

    // Map entries iterator: yields [key, value] pairs as arrays.
    // Per §24.1.5.2 %MapIteratorPrototype%.next, each result is
    // CreateArrayFromList(« key, value ») for "key+value" iteration.
    if it.kind == IteratorKind::MapEntries {
        let uni = unsafe {
            // SAFETY: iterator target was set to a unified Map object in iter_init.
            deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
        };
        if let Some(u) = uni
            && let Some(crate::internal_data::InternalData::Map { entries }) = u.internal_data()
            && (it.index as usize) < entries.len()
        {
            let (k, v) = entries[it.index as usize];
            it.index += 1;
            let pair = create_key_value_pair(k.raw_bits(), v.raw_bits());
            let result = IteratorResult::with_value(pair);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Set values iterator: yields each value.
    // Per §24.2.5.2 %SetIteratorPrototype%.next, each result yields the
    // value from the set (for "value" iteration kind).
    if it.kind == IteratorKind::SetValues {
        let uni = unsafe {
            // SAFETY: iterator target was set to a unified Set object in iter_init.
            deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
        };
        if let Some(u) = uni
            && let Some(crate::internal_data::InternalData::Set { values }) = u.internal_data()
            && (it.index as usize) < values.len()
        {
            let val = values[it.index as usize].raw_bits();
            it.index += 1;
            let result = IteratorResult::with_value(val);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Object-key iterator: yields property keys as strings (for for-in).
    // Per §14.7.5.10 ForIn/OfHeadEvaluation — uses EnumerateObjectProperties.
    // String-char iterator: yields individual characters per §22.1.5.2.
    if it.kind == IteratorKind::ObjectKeys || it.kind == IteratorKind::StringChars {
        if (it.index as usize) < it.keys.len() {
            let key = &it.keys[it.index as usize];
            let val = make_rt_string(key.clone());
            it.index += 1;
            let result = IteratorResult::with_value(val);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Array entries iterator: yields [index, value] pairs.
    // Per §23.1.5.2.1 %ArrayIteratorPrototype%.next step 11.a:
    //   Let result be CreateArrayFromList(« index, value »).
    if it.kind == IteratorKind::ArrayEntries {
        let uni = unsafe {
            // SAFETY: iterator target was set to a unified Array object.
            deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
            && it.index < u.array_len()
        {
            let val = u
                .get_element(it.index)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let pair = create_key_value_pair(JsValue::int(it.index as i32).raw_bits(), val);
            it.index += 1;
            let result = IteratorResult::with_value(pair);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Array keys iterator: yields indices.
    // Per §23.1.5.2.1 %ArrayIteratorPrototype%.next step 11.b:
    //   Let result be index (for "key" iteration kind).
    if it.kind == IteratorKind::ArrayKeys {
        let uni = unsafe {
            // SAFETY: iterator target was set to a unified Array object.
            deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
            && it.index < u.array_len()
        {
            let val = JsValue::int(it.index as i32).raw_bits();
            it.index += 1;
            let result = IteratorResult::with_value(val);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Array values iterator: yields element values.
    // Per §23.1.5.2.1 %ArrayIteratorPrototype%.next step 11.c:
    //   Let result be value (for "value" iteration kind).
    if it.kind == IteratorKind::ArrayValues {
        let uni = unsafe {
            // SAFETY: iterator target was set to a unified Array object.
            deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
            && it.index < u.array_len()
        {
            let val = u
                .get_element(it.index)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            it.index += 1;
            let result = IteratorResult::with_value(val);
            return boxed_iter_result(result);
        }
        it.done = true;
        let result = IteratorResult::done();
        return boxed_iter_result(result);
    }

    // Default array iterator: read the current element from a unified array.
    // Per §23.1.5.2.1 %ArrayIteratorPrototype%.next:
    // 1. Let index be O.[[ArrayIteratorNextIndex]].
    // 2. Let len be ? LengthOfArrayLike(a).
    // 3. If index >= len, return CreateIterResultObject(undefined, true).
    // 4. Set O.[[ArrayIteratorNextIndex]] to index + 1.
    // 5-11. (Kind-specific — default is "value") Let result be value.
    // 12. Return CreateIterResultObject(result, false).
    let uni = unsafe {
        // SAFETY: iterator target was set to a unified Array object in iter_init.
        deref_tagged::<crate::internal_data::UnifiedObject>(it.target)
    };
    if let Some(u) = uni
        && u.kind == crate::internal_data::InternalKind::Array
        && it.index < u.array_len()
    {
        let val = u
            .get_element(it.index)
            .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
        it.index += 1;
        let result = IteratorResult::with_value(val);
        return boxed_iter_result(result);
    }

    it.done = true;
    let result = IteratorResult::done();
    boxed_iter_result(result)
}

/// Create a 2-element array `[key, value]` for Map entry / Array entries iteration.
///
/// Implements `CreateArrayFromList ( elements )` — ES2024 §7.3.18 — for
/// exactly two elements, used by Map and Array entry iterators.
///
/// [spec]: https://tc39.es/ecma262/#sec-createarrayfromlist
///
/// Returns a NaN-boxed array containing two elements.
pub(crate) fn create_key_value_pair(key: u64, value: u64) -> u64 {
    create_array_from_elements(vec![
        JsValue::from_raw_bits(key),
        JsValue::from_raw_bits(value),
    ])
}

/// `IteratorComplete ( iterResult )` — ES2024 §7.4.3
///
/// Reads the `.done` field from an iterator result object.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorcomplete
///
/// Spec steps:
/// 1. Return ToBoolean(? Get(iterResult, "done")).
///
/// Note: Our implementation reads from the specialized `InternalKind::IterResult`
/// internal data rather than performing a generic property lookup, since we
/// control the shape of iterator result objects.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_done(result: u64) -> u64 {
    let tag = read_obj_tag(result);
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(result)
        };
        if let Some(u) = uni
            && let Some(crate::internal_data::InternalData::IterResult { done, .. }) =
                u.internal_data()
        {
            // Step 1: Return ToBoolean(? Get(iterResult, "done")).
            return *done;
        }
    }
    // Fallback: treat as done if we can't read the result
    JsValue::bool(true).raw_bits()
}

/// `IteratorValue ( iterResult )` — ES2024 §7.4.4
///
/// Reads the `.value` field from an iterator result object.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorvalue
///
/// Spec steps:
/// 1. Return ? Get(iterResult, "value").
///
/// Note: Our implementation reads from the specialized `InternalKind::IterResult`
/// internal data rather than performing a generic property lookup.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_value(result: u64) -> u64 {
    let tag = read_obj_tag(result);
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(result)
        };
        if let Some(u) = uni
            && let Some(crate::internal_data::InternalData::IterResult { value, .. }) =
                u.internal_data()
        {
            // Step 1: Return ? Get(iterResult, "value").
            return *value;
        }
    }
    // Fallback: return undefined if we can't read the result
    JsValue::undefined().raw_bits()
}

/// `IteratorClose ( iteratorRecord, completion )` — ES2024 §7.4.6
///
/// Closes an iterator by calling `.return()` when a for-of loop terminates
/// early (break, return, throw). The `completion` is a normal completion.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorclose
///
/// See `__esc_rt_iter_close_inner` for the full spec step mapping.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_close(iter: u64) {
    __esc_rt_iter_close_inner(iter, false);
}

/// `IteratorClose ( iteratorRecord, completion )` — ES2024 §7.4.6
///
/// Closes an iterator when the loop body threw an exception. Per spec
/// step 7, if the original completion is an abrupt completion of type
/// `throw`, any error from calling `.return()` is suppressed — the
/// original error takes precedence.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorclose
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_close_throw(iter: u64) {
    __esc_rt_iter_close_inner(iter, true);
}

/// `IteratorClose ( iteratorRecord, completion )` — ES2024 §7.4.6
///
/// Inner implementation of IteratorClose.
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratorclose
///
/// Spec steps:
/// 1. Assert: iteratorRecord.[[Iterator]] is an Object.
/// 2. Let iterator be iteratorRecord.[[Iterator]].
/// 3. Let innerResult be Completion(GetMethod(iterator, "return")).
/// 4. If innerResult.[[Type]] is normal, then
///    a. Let return be innerResult.[[Value]].
///    b. If return is undefined, return ? completion.
///    c. Set innerResult to Completion(Call(return, iterator)).
/// 5. If completion.[[Type]] is throw, return ? completion.
/// 6. If innerResult.[[Type]] is throw, return ? innerResult.
/// 7. If innerResult.[[Value]] is not an Object, throw a TypeError exception.
/// 8. Return ? completion.
fn __esc_rt_iter_close_inner(iter: u64, is_throw: bool) {
    let tag = read_obj_tag(iter);

    // Steps 1-2: Extract the iterator from our internal representation.
    let it: Option<&JsIterator> = if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(iter)
        };
        uni.and_then(|u| {
            if let Some(crate::internal_data::InternalData::IteratorState { inner }) =
                u.internal_data()
            {
                Some(&**inner)
            } else {
                None
            }
        })
    } else {
        None
    };

    let Some(it) = it else {
        return;
    };

    // Already done — no need to close
    if it.done {
        return;
    }

    // Custom iterators: call .return() if the method exists and is callable
    if it.kind == IteratorKind::Custom {
        // Step 3: Let innerResult be Completion(GetMethod(iterator, "return")).
        let return_key = make_rt_string("return".to_string());
        let return_fn = __esc_rt_get_prop(it.target, return_key);
        let return_val = JsValue::from_raw_bits(return_fn);

        // Step 4b: If return is undefined, return ? completion.
        if return_val.is_undefined() || return_val.is_null() {
            return;
        }

        // Implicit in step 3: if return is not callable, throw TypeError
        if !is_value_callable(return_fn) {
            // Step 5: If completion.[[Type]] is throw, return ? completion
            // (suppress close errors when is_throw)
            if !is_throw {
                let msg =
                    make_rt_string("TypeError: iterator.return is not a function".to_string());
                let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
            }
            return;
        }

        // Step 4c: Set innerResult to Completion(Call(return, iterator)).
        let had_exception_before = exceptions::is_exception();
        let result = unsafe {
            // SAFETY: the closure was created by compiled code.
            __esc_rt_call_method(it.target, return_key, 0, std::ptr::null())
        };
        let threw_during_close = !had_exception_before && exceptions::is_exception();

        // Step 5-6: If completion.[[Type]] is throw, return ? completion.
        // If innerResult.[[Type]] is throw, return ? innerResult.
        // When is_throw=true, the original throw takes precedence — suppress close errors.
        if threw_during_close && is_throw {
            exceptions::clear_exception();
            return;
        }

        // Step 7: If innerResult.[[Value]] is not an Object, throw a TypeError exception.
        if !threw_during_close {
            let result_val = JsValue::from_raw_bits(result);
            if !result_val.is_object() && !is_throw {
                let msg = make_rt_string("TypeError: Iterator result is not an object".to_string());
                let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
            }
        }
    }

    // Generator iterators: call .return() on the generator
    // Per §27.5.3.4 GeneratorResumeAbrupt with abruptCompletion of type return
    if it.kind == IteratorKind::Generator {
        dispatch_generator_method(it.target, "return");
    }

    // Helper iterators: close the underlying iterator
    // Per Iterator Helpers proposal, closing a helper closes the underlying source.
    if it.kind == IteratorKind::Helper
        && let Some(ref helper_state) = it.helper
    {
        __esc_rt_iter_close_inner(helper_state.underlying, is_throw);
    }
}

/// Validate that an iterator result is a proper object with `{ value, done }`.
///
/// Implements the type check from `IteratorNext` step 3 — ES2024 §7.4.2:
/// "If result is not an Object, throw a TypeError exception."
///
/// [spec]: https://tc39.es/ecma262/#sec-iteratornext
///
/// Also used as a standalone check for `IteratorStep` (§7.4.5) which calls
/// IteratorNext and then checks IteratorComplete.
///
/// Returns `result` unchanged if valid, or throws TypeError.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_iter_validate_result(result: u64) -> u64 {
    let v = JsValue::from_raw_bits(result);

    // IteratorNext step 3: If result is not an Object, throw a TypeError exception.
    if !v.is_object() {
        let msg = make_rt_string("TypeError: Iterator result is not an object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return result;
    }

    result
}

/// Check that a value is iterable for array destructuring.
///
/// Per ES2024 §13.15.5.3, array destructuring calls `GetIterator(value)`.
/// Non-objects (except strings) are not iterable and should throw TypeError.
/// Null and undefined always throw.
///
/// [spec]: <https://tc39.es/ecma262/#sec-getiterator>
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_check_iterable(value: u64) -> u64 {
    let v = JsValue::from_raw_bits(value);
    // Strings are iterable
    if v.is_string() {
        return value;
    }
    // Objects (arrays, etc.) are iterable
    if v.is_object() {
        return value;
    }
    // Everything else (null, undefined, boolean, number, symbol) is not iterable
    let type_name = if v.is_null() {
        "null"
    } else if v.is_undefined() {
        "undefined"
    } else if v.is_bool() {
        "a boolean"
    } else if v.is_number() || v.is_int() {
        "a number"
    } else {
        "a non-iterable value"
    };
    let msg = make_rt_string(format!("{type_name} is not iterable"));
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    super::__esc_rt_throw(err);
    value
}
