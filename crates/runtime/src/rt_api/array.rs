//! Array creation and `Array.prototype` method dispatch.
//!
//! Contains array ABI functions (`__esc_rt_create_array`, `__esc_rt_array_push`, etc.)
//! and `dispatch_array_method` / `dispatch_array_static_method`.

use nanbox::JsValue;

use shapes;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::iterator::JsIterator;
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut, read_obj_tag};
use crate::value_ops;

use super::{
    __esc_rt_call_indirect, __esc_rt_get_prop, __esc_rt_iter_close, __esc_rt_iter_done,
    __esc_rt_iter_init, __esc_rt_iter_next, __esc_rt_iter_value, compare_js_values,
    create_array_from_elements, create_empty_array, extract_key_string, format_value_for_join,
    make_rt_string, normalize_index, read_argv,
};

// =========================================================================
// Generic array-like helpers (ES2023 7.3.2 LengthOfArrayLike, 7.3.1 Get)
// =========================================================================

/// Throw a TypeError via the runtime exception system.
///
/// Sets the pending exception. The caller should return a fallback value
/// after calling this, since the exception is only checked at the next
/// exception-check point in compiled code.
fn throw_type_error(msg: &str) {
    let msg_bits = make_rt_string(format!("TypeError: {msg}"));
    let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg_bits);
    super::__esc_rt_throw(err);
}

/// Check if a NaN-boxed value is callable (has `[[Call]]` internal method).
///
/// Returns `true` for closures, functions, and native functions.
fn is_callable_value(bits: u64) -> bool {
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

/// `LengthOfArrayLike ( obj )`
///
/// Read the `length` property from any object and coerce it to a non-negative
/// integer.
///
/// Works on both actual arrays (via `array_len()`) and generic objects
/// (via property access on `"length"`).
///
/// [spec]: https://tc39.es/ecma262/#sec-lengthofarraylike
fn length_of_array_like(obj: u64) -> u32 {
    if read_obj_tag(obj) == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj)
        };
        if let Some(u) = uni
            && u.kind == InternalKind::Array
        {
            return u.array_len();
        }
    }
    // Generic path: read "length" property
    // 1. Let len be ? Get(obj, "length").
    let len_key = make_rt_string("length".to_string());
    let len_val = __esc_rt_get_prop(obj, len_key);
    // 2. Return ? ToLength(len).
    let len_js = JsValue::from_raw_bits(len_val);
    let n = len_js
        .as_int()
        .map(|i| i as f64)
        .or_else(|| len_js.as_number())
        .unwrap_or(0.0);
    if n < 0.0 || n.is_nan() { 0 } else { n as u32 }
}

/// Read an indexed property from an object (generic `Get(O, ToString(k))`).
///
/// For actual arrays, reads from the internal elements vector.
/// For generic objects, reads the property keyed by the string index.
fn get_indexed_value(obj: u64, index: u32) -> JsValue {
    let idx_key = make_rt_string(index.to_string());
    JsValue::from_raw_bits(__esc_rt_get_prop(obj, idx_key))
}

/// Read all elements from an array-like object as a `Vec<JsValue>`.
///
/// For actual arrays, uses the fast `array_elements_resolved()` path.
/// For generic objects, reads `length` and then each indexed property.
fn get_array_like_elements(obj: u64) -> Vec<JsValue> {
    if read_obj_tag(obj) == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj)
        };
        if let Some(u) = uni
            && u.kind == InternalKind::Array
        {
            return u.array_elements_resolved();
        }
    }
    // Generic path
    let len = length_of_array_like(obj);
    let mut result = Vec::with_capacity(len as usize);
    for i in 0..len {
        result.push(get_indexed_value(obj, i));
    }
    result
}

/// `RequireObjectCoercible ( argument )`
///
/// Check that `this` is not null or undefined.
///
/// Returns `true` if the value is valid, `false` if a TypeError was thrown.
///
/// [spec]: https://tc39.es/ecma262/#sec-requireobjectcoercible
fn require_object_coercible(this: u64, method_name: &str) -> bool {
    let val = JsValue::from_raw_bits(this);
    if val.is_null() || val.is_undefined() {
        let desc = if val.is_null() { "null" } else { "undefined" };
        throw_type_error(&format!("Array.prototype.{method_name} called on {desc}"));
        return false;
    }
    true
}

/// Validate that the callback argument is callable for array methods.
///
/// Returns `true` if callable, `false` if a TypeError was thrown.
fn require_callable_callback(args: &[JsValue], _method_name: &str) -> bool {
    if args.is_empty() {
        throw_type_error("undefined is not a function");
        return false;
    }
    let cb = args[0].raw_bits();
    if !is_callable_value(cb) {
        // Also check if it's a direct function index (integer)
        let v = JsValue::from_raw_bits(cb);
        if v.as_int().is_some() {
            return true;
        }
        let type_desc = value_ops::js_typeof(v);
        throw_type_error(&format!("{type_desc} is not a function"));
        return false;
    }
    true
}

/// Check if a method name is an `Array.prototype` method.
///
/// Used by the method dispatch to route calls to the generic array method
/// handler when the receiver is not an actual array (e.g., via `.call()`).
pub(crate) fn is_array_prototype_method(name: &str) -> bool {
    matches!(
        name,
        "forEach"
            | "map"
            | "filter"
            | "reduce"
            | "reduceRight"
            | "some"
            | "every"
            | "find"
            | "findIndex"
            | "findLast"
            | "findLastIndex"
            | "indexOf"
            | "lastIndexOf"
            | "includes"
            | "join"
            | "flat"
            | "flatMap"
            | "slice"
            | "concat"
            | "at"
            | "fill"
            | "sort"
            | "reverse"
            | "toSorted"
            | "toReversed"
            | "toSpliced"
            | "copyWithin"
    )
}

/// `Array.isArray ( arg )` -- proxy-unwrapping helper
///
/// Check if a value is an Array, walking through Proxy chains per spec.
/// `Array.isArray(new Proxy([], {}))` returns `true` because the spec says
/// to unwrap proxy targets transitively until finding a non-proxy object.
///
/// [spec]: https://tc39.es/ecma262/#sec-isarray
fn is_array_through_proxy(val: u64) -> bool {
    let mut current = val;
    // Walk through at most 256 proxy layers (matches MAX_PROXY_DEPTH)
    for _ in 0..256 {
        if read_obj_tag(current) != Some(ObjTag::Unified as u8) {
            return false;
        }
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(current)
        };
        let Some(u) = uni else { return false };
        // 1. If argument is not an Object, return false.
        // 2. If argument is an Array exotic object, return true.
        if u.kind == InternalKind::Array {
            return true;
        }
        // 3. If argument is a Proxy exotic object, then
        if u.kind == InternalKind::Proxy
            && let Some(InternalData::Proxy {
                target, revoked, ..
            }) = u.internal_data()
        {
            // a. If argument.[[ProxyHandler]] is null, throw a TypeError exception.
            if *revoked {
                // Revoked proxy: throw TypeError per spec
                let msg = super::make_rt_string(
                    "Cannot perform 'isArray' on a revoked proxy".to_string(),
                );
                let err =
                    super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
                return false;
            }
            // b. Let target be argument.[[ProxyTarget]].
            // c. Return ? IsArray(target).
            current = *target;
            continue;
        }
        // 4. Return false.
        return false;
    }
    false
}

// =========================================================================
// Array ABI (B2)
// =========================================================================

/// Create a new JS array with initial capacity. Returns NaN-boxed object.
///
/// Implements the `ArrayCreate` abstract operation with a given pre-allocated
/// capacity.
///
/// [spec]: https://tc39.es/ecma262/#sec-arraycreate
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_create_array(len: u32) -> u64 {
    let elements = Vec::with_capacity(len as usize);
    let arr = UnifiedObject::array(shapes::ShapeTable::EMPTY_SHAPE, elements);
    TaggedObj::boxed(ObjTag::Unified, arr)
}

/// Create a new empty JS array. Returns NaN-boxed object.
///
/// Used by spread argument handling where the array is built incrementally
/// via `__esc_rt_array_push` and `__esc_rt_spread_into_array`.
///
/// The `_unused` parameter exists to match the unary `(i64) -> i64` ABI
/// expected by the `CallRuntime` Cranelift lowering for 1-arg calls.
///
/// [spec]: https://tc39.es/ecma262/#sec-arraycreate
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_create_empty_array(_unused: u64) -> u64 {
    let arr = UnifiedObject::array(shapes::ShapeTable::EMPTY_SHAPE, Vec::new());
    TaggedObj::boxed(ObjTag::Unified, arr)
}

/// `Array.prototype.push ( ...items )`
///
/// Appends the argument to the end of the array, returning the new length
/// as a NaN-boxed number.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.prototype.push
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_array_push(arr: u64, val: u64) -> u64 {
    // 1. Let O be ? ToObject(this value).
    if read_obj_tag(arr) != Some(ObjTag::Unified as u8) {
        return JsValue::number(0.0).raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(arr)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        // 2. Let len be ? LengthOfArrayLike(O).
        // 3. Let argCount be the number of elements in items.
        // (argCount = 1 for this ABI)
        // TODO: Step 4 — If len + argCount > 2^53 - 1, throw a TypeError exception.
        // 5. For each element E of items, do
        //   a. Perform ? Set(O, ! ToString(F(len)), E, true).
        //   b. Set len to len + 1.
        u.array_push(JsValue::from_raw_bits(val));
        // 6. Perform ? Set(O, "length", F(len), true).
        // 7. Return F(len).
        return JsValue::number(u.array_len() as f64).raw_bits();
    }
    JsValue::number(0.0).raw_bits()
}

/// `Array.prototype.pop ( )`
///
/// Removes and returns the last element of the array, or `undefined` if empty.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.prototype.pop
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_array_pop(arr: u64) -> u64 {
    // 1. Let O be ? ToObject(this value).
    if read_obj_tag(arr) != Some(ObjTag::Unified as u8) {
        return JsValue::undefined().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(arr)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        // 2. Let len be ? LengthOfArrayLike(O).
        // 3. If len = 0, then
        //   a. Perform ? Set(O, "length", +0F, true).
        //   b. Return undefined.
        // 4. Else,
        //   a. Assert: len > 0.
        //   b. Let newLen be F(len - 1).
        //   c. Let index be ! ToString(newLen).
        //   d. Let element be ? Get(O, index).
        //   e. Perform ? DeletePropertyOrThrow(O, index).
        //   f. Perform ? Set(O, "length", newLen, true).
        //   g. Return element.
        return u
            .array_pop()
            .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    }
    JsValue::undefined().raw_bits()
}

/// Get the `length` property of a JS array as a NaN-boxed number.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.prototype.length
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_array_length(arr: u64) -> u64 {
    if read_obj_tag(arr) != Some(ObjTag::Unified as u8) {
        return JsValue::number(0.0).raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(arr)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        return JsValue::number(u.array_len() as f64).raw_bits();
    }
    JsValue::number(0.0).raw_bits()
}

/// `Array.prototype.slice ( start, end )`
///
/// Slice a JS array from `start_index` to the end, returning a new array.
///
/// Used for destructuring rest elements: `let [a, ...rest] = arr` calls
/// `__esc_rt_array_slice(arr, 1)` to collect elements from index 1 onward.
/// `start_index` is a NaN-boxed integer.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.prototype.slice
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_array_slice(arr: u64, start_index: u64) -> u64 {
    // 1. Let O be ? ToObject(this value).
    // 2. Let len be ? LengthOfArrayLike(O).
    let start_val = JsValue::from_raw_bits(start_index);
    // 3. Let relativeStart be ? ToIntegerOrInfinity(start).
    let start = start_val.as_int().unwrap_or(0).max(0) as usize;

    if read_obj_tag(arr) != Some(ObjTag::Unified as u8) {
        return create_empty_array();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(arr)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        let resolved = u.array_elements_resolved();
        // 4-8. Compute actual start/end indices, create new array from slice.
        let elements: Vec<JsValue> = resolved.iter().skip(start).copied().collect();
        // 9. Return A.
        return create_array_from_elements(elements);
    }
    create_empty_array()
}

/// Expand a spread argument into a target array.
///
/// Used during argument array construction: elements from `source` are
/// pushed into `target`. Handles arrays, strings, and general iterables.
///
/// This is an internal runtime helper for spread syntax (`...expr`), not a
/// direct JS builtin. It implements parts of the `SpreadElement` evaluation
/// semantics.
///
/// [spec]: https://tc39.es/ecma262/#sec-runtime-semantics-arrayaccumulation
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_spread_into_array(target: u64, source: u64) -> u64 {
    let target_tag = read_obj_tag(target);
    let source_tag = read_obj_tag(source);
    let source_val = JsValue::from_raw_bits(source);
    let target_is_array = is_array_tag(target_tag);

    // Fast path: source is an array — copy elements.
    //
    // A `Set`, `Map` or generator is also tagged `ObjTag::Unified`, so the tag
    // alone cannot tell it apart from an array. Those objects must fall through
    // to the general iterator loop below; routing them through this fast path
    // yields an empty array because `read_array_elements` returns no elements
    // for anything whose `InternalKind` is not `Array`.
    if is_real_array(source) && target_is_array {
        let source_elems = read_array_elements(source, source_tag);
        push_to_target_array(target, target_tag, &source_elems);
        return target;
    }

    // String spread: iterate over characters
    if source_val.is_string() {
        let s = crate::string_ops::get_string_data(source_val);
        if target_is_array {
            for c in s.chars() {
                let char_val = JsValue::from_raw_bits(make_rt_string(c.to_string()));
                push_single_to_target(target, target_tag, char_val);
            }
        }
        return target;
    }

    // General iterable spread: use iterator protocol
    // 1. Let iteratorRecord be ? GetIterator(spreadObj, sync).
    // 2. Repeat,
    //   a. Let next be ? IteratorStepValue(iteratorRecord).
    //   b. If next is done, return nextIndex.
    //   c. Perform ! CreateDataPropertyOrThrow(array, ! ToString(F(nextIndex)), next).
    //   d. Set nextIndex to nextIndex + 1.
    if target_is_array {
        let iter = __esc_rt_iter_init(source);
        loop {
            let result = __esc_rt_iter_next(iter);
            let done_bits = __esc_rt_iter_done(result);
            if value_ops::to_boolean(JsValue::from_raw_bits(done_bits)) {
                break;
            }
            let val = __esc_rt_iter_value(result);
            push_single_to_target(target, target_tag, JsValue::from_raw_bits(val));
        }
        __esc_rt_iter_close(iter);
    }
    target
}

/// Check if a tag represents an array (unified object).
fn is_array_tag(tag: Option<u8>) -> bool {
    tag == Some(ObjTag::Unified as u8)
}

/// Check if a NaN-boxed value is a *real* array — a unified object whose
/// `InternalKind` is `Array`, not merely any unified object.
///
/// `is_array_tag` only matches the `ObjTag::Unified` tag, which every unified
/// object (array, Set, Map, generator, …) shares. The spread fast path must
/// additionally confirm the `InternalKind`, otherwise non-array iterables are
/// misrouted to `read_array_elements` and silently produce zero elements.
fn is_real_array(bits: u64) -> bool {
    if read_obj_tag(bits) != Some(ObjTag::Unified as u8) {
        return false;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(bits)
    };
    uni.is_some_and(|u| u.kind == InternalKind::Array)
}

/// Read elements from a unified array object.
fn read_array_elements(bits: u64, _tag: Option<u8>) -> Vec<JsValue> {
    let uni = unsafe {
        // SAFETY: caller verified the tag via `is_array_tag`.
        deref_tagged::<UnifiedObject>(bits)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        return u.array_elements_resolved();
    }
    Vec::new()
}

/// Push multiple elements to a target unified array.
fn push_to_target_array(target: u64, _tag: Option<u8>, elements: &[JsValue]) {
    let uni = unsafe {
        // SAFETY: caller verified the tag via `is_array_tag`.
        deref_tagged_mut::<UnifiedObject>(target)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        for elem in elements {
            u.array_push(*elem);
        }
    }
}

/// Push a single element to a target unified array.
fn push_single_to_target(target: u64, _tag: Option<u8>, val: JsValue) {
    let uni = unsafe {
        // SAFETY: caller verified the tag via `is_array_tag`.
        deref_tagged_mut::<UnifiedObject>(target)
    };
    if let Some(u) = uni
        && u.kind == InternalKind::Array
    {
        u.array_push(val);
    }
}

/// Dispatch a method call on an array or array-like object.
///
/// If `obj` is an actual array (`InternalKind::Array`), uses the fast internal
/// element access path. Otherwise, falls back to generic property access via
/// `LengthOfArrayLike` and indexed `Get`, per ES2023 spec requirements for
/// `Array.prototype` methods invoked via `.call()` on non-array objects.
pub(crate) fn dispatch_array_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);

    // Check if obj is an actual array for the fast path
    let is_real_array = if read_obj_tag(obj) == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj)
        };
        uni.is_some_and(|u| u.kind == InternalKind::Array)
    } else {
        false
    };

    if is_real_array {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(obj)
        };
        let Some(u) = uni else {
            return JsValue::undefined().raw_bits();
        };
        dispatch_unified_array(u, obj, method, &args)
    } else {
        // Generic path: treat `this` as an array-like object
        dispatch_generic_array_method(obj, method, &args)
    }
}

/// Dispatch array methods on a UnifiedObject with InternalKind::Array.
///
/// This is the fast path for real array objects. Each match arm implements
/// a specific `Array.prototype` method per the ES2024 spec.
fn dispatch_unified_array(u: &mut UnifiedObject, obj: u64, method: &str, args: &[JsValue]) -> u64 {
    match method {
        // =================================================================
        // Array.prototype.push ( ...items )
        // https://tc39.es/ecma262/#sec-array.prototype.push
        // =================================================================
        "push" => {
            // 1. Let O be ? ToObject(this value).
            // (already done — `u` is the array object)
            // 2. Let len be ? LengthOfArrayLike(O).
            let len = u.array_len() as u64;
            // 3. Let argCount be the number of elements in items.
            let arg_count = args.len() as u64;
            // 4. If len + argCount > 2^53 - 1, throw a TypeError exception.
            if len + arg_count > (1u64 << 53) - 1 {
                throw_type_error("Array length exceeds 2^53 - 1");
                return JsValue::undefined().raw_bits();
            }
            // 5. For each element E of items, do
            for arg in args {
                //   a. Perform ? Set(O, ! ToString(F(len)), E, true).
                u.array_push(*arg);
                //   b. Set len to len + 1.
            }
            // 6. Perform ? Set(O, "length", F(len), true).
            // 7. Return F(len).
            JsValue::int(u.array_len() as i32).raw_bits()
        }
        // =================================================================
        // Array.prototype.pop ( )
        // https://tc39.es/ecma262/#sec-array.prototype.pop
        // =================================================================
        "pop" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If len = 0, return undefined.
            // 4. Else, remove and return last element.
            u.array_pop().unwrap_or(JsValue::undefined()).raw_bits()
        }
        // =================================================================
        // Array.prototype.join ( separator )
        // https://tc39.es/ecma262/#sec-array.prototype.join
        // =================================================================
        "join" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If separator is undefined, let sep be ",".
            // 4. Else, let sep be ? ToString(separator).
            let sep = args
                .first()
                .and_then(|v| extract_key_string(v.raw_bits()))
                .unwrap_or_else(|| ",".to_string());
            let resolved = u.array_elements_resolved();
            // 5. Let R be the empty String.
            // 6. Let k be 0.
            // 7. Repeat, while k < len,
            //   a. If k > 0, set R to the string-concatenation of R and sep.
            //   b. Let element be ? Get(O, ! ToString(F(k))).
            //   c. If element is undefined or null, let next be "".
            //   d. Else, let next be ? ToString(element).
            //   e. Set R to the string-concatenation of R and next.
            //   f. Set k to k + 1.
            let parts: Vec<String> = resolved.iter().map(|v| format_value_for_join(*v)).collect();
            let result = parts.join(&sep);
            // 8. Return R.
            make_rt_string(result)
        }
        "length" => JsValue::int(u.array_len() as i32).raw_bits(),
        // =================================================================
        // Array.prototype.indexOf ( searchElement [ , fromIndex ] )
        // https://tc39.es/ecma262/#sec-array.prototype.indexof
        // =================================================================
        "indexOf" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let from_index = args
                .get(1)
                .map_or(0, |v| crate::value_ops::to_integer_or_infinity(*v) as i32);
            let resolved = u.array_elements_resolved();
            let len = resolved.len() as i32;
            // 3. If len = 0, return -1F.
            // 4. Let n be ? ToIntegerOrInfinity(fromIndex).
            // 5. Assert: If fromIndex is undefined, then n is 0.
            // 6. If n = +inf, return -1F.
            // 7. Else if n = -inf, set n to 0.
            // 8. If n >= 0, let k be n.
            // 9. Else, let k be max(len + n, 0).
            let start = if from_index < 0 {
                (len + from_index).max(0) as usize
            } else {
                from_index as usize
            };
            // 10. Repeat, while k < len,
            let idx = resolved
                .iter()
                .enumerate()
                .skip(start)
                .find(|(_, v)| {
                    //   a. Let elementK be ? Get(O, ! ToString(F(k))).
                    //   b. If IsStrictlyEqual(searchElement, elementK) is true, return F(k).
                    value_ops::strict_eq(**v, search)
                })
                .map(|(i, _)| i);
            match idx {
                // return F(k)
                Some(i) => JsValue::int(i as i32).raw_bits(),
                // 11. Return -1F.
                None => JsValue::int(-1).raw_bits(),
            }
        }
        // =================================================================
        // Array.prototype.lastIndexOf ( searchElement [ , fromIndex ] )
        // https://tc39.es/ecma262/#sec-array.prototype.lastindexof
        // =================================================================
        "lastIndexOf" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let resolved = u.array_elements_resolved();
            let len = resolved.len() as i32;
            // 3. If len = 0, return -1F.
            // 4. If fromIndex is present, let n be ? ToIntegerOrInfinity(fromIndex);
            //    else let n be len - 1.
            let from_index = args.get(1).map_or(len - 1, |v| {
                if v.is_undefined() {
                    len - 1
                } else {
                    crate::value_ops::to_integer_or_infinity(*v) as i32
                }
            });
            // 5. If n >= 0, let k be min(n, len - 1).
            // 6. Else, let k be len + n.
            let end = if from_index < 0 {
                (len + from_index) as usize
            } else {
                from_index.min(len - 1) as usize
            };
            // 7. Repeat, while k >= 0,
            let idx = resolved
                .iter()
                .enumerate()
                .take(end + 1)
                .rfind(|(_, v)| {
                    //   a. Let elementK be ? Get(O, ! ToString(F(k))).
                    //   b. If IsStrictlyEqual(searchElement, elementK) is true, return F(k).
                    value_ops::strict_eq(**v, search)
                })
                .map(|(i, _)| i);
            match idx {
                Some(i) => JsValue::int(i as i32).raw_bits(),
                // 8. Return -1F.
                None => JsValue::int(-1).raw_bits(),
            }
        }
        // =================================================================
        // Array.prototype.includes ( searchElement [ , fromIndex ] )
        // https://tc39.es/ecma262/#sec-array.prototype.includes
        // =================================================================
        "includes" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let resolved = u.array_elements_resolved();
            let len = resolved.len() as i32;
            // 3. If len = 0, return false.
            // 4. Let n be ? ToIntegerOrInfinity(fromIndex).
            let from_index = args
                .get(1)
                .map_or(0, |v| crate::value_ops::to_integer_or_infinity(*v) as i32);
            // 5-9. Compute start index k.
            let k = if from_index >= 0 {
                from_index
            } else {
                (len + from_index).max(0)
            } as usize;
            // 10. Repeat, while k < len,
            //   a. Let elementK be ? Get(O, ! ToString(F(k))).
            //   b. If SameValueZero(searchElement, elementK) is true, return true.
            let found = resolved.iter().skip(k).any(|v| {
                // SameValueZero: like strict_eq but NaN === NaN is true
                value_ops::same_value_zero(*v, search)
            });
            //   c. Set k to k + 1.
            // 11. Return false.
            JsValue::bool(found).raw_bits()
        }
        // =================================================================
        // Array.prototype.reverse ( )
        // https://tc39.es/ecma262/#sec-array.prototype.reverse
        // =================================================================
        "reverse" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. Let middle be floor(len / 2).
            // 4. Let lower be 0.
            // 5. Repeat, while lower != middle,
            //   (swap elements at lower and upper)
            if let Some(elems) = u.array_elements_mut() {
                elems.reverse();
            }
            // 6. Return O.
            obj
        }
        // =================================================================
        // Array.prototype.slice ( start, end )
        // https://tc39.es/ecma262/#sec-array.prototype.slice
        // =================================================================
        "slice" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let elems = u.array_elements_resolved();
            let len = elems.len() as i32;
            // 3. Let relativeStart be ? ToIntegerOrInfinity(start).
            let start = args
                .first()
                .map_or(0, |v| crate::value_ops::to_integer_or_infinity(*v) as i32);
            // 4. If relativeEnd is undefined, let relativeEnd be len;
            //    else let relativeEnd be ? ToIntegerOrInfinity(end).
            let end = args.get(1).map_or(len, |v| {
                if v.is_undefined() {
                    len
                } else {
                    crate::value_ops::to_integer_or_infinity(*v) as i32
                }
            });
            // 5-6. Clamp start/end to [0, len].
            let start = normalize_index(start, len);
            let end = normalize_index(end, len);
            // 7. Let count be max(final - k, 0).
            if start >= end {
                return create_empty_array();
            }
            // 8. Let A be ? ArraySpeciesCreate(O, count).
            // 9-11. Copy elements from O to A.
            let sliced: Vec<JsValue> = elems[start..end].to_vec();
            // 12. Return A.
            create_array_from_elements(sliced)
        }
        // =================================================================
        // Array.prototype.forEach ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.foreach
        // =================================================================
        "forEach" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "forEach") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            // thisArg (step 5.c.ii): if provided, use as `this` for callback
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            // 4. Let k be 0.
            // 5. Repeat, while k < len,
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                unsafe {
                    // SAFETY: callback_bits is passed from compiled code, call_args is valid.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr());
                }
                super::CURRENT_THIS.with(|c| c.set(prev_this));
            }
            // 6. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.map ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.map
        // =================================================================
        "map" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "map") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            // 4. Let A be ? ArraySpeciesCreate(O, len).
            let mut results = Vec::with_capacity(elements.len());
            // 5. Let k be 0.
            // 6. Repeat, while k < len,
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result = unsafe {
                    // SAFETY: callback_bits is passed from compiled code, call_args is valid.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr())
                };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                results.push(JsValue::from_raw_bits(result));
            }
            // 7. Return A.
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.filter ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.filter
        // =================================================================
        "filter" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "filter") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            // 4. Let A be ? ArraySpeciesCreate(O, 0).
            let mut results = Vec::new();
            // 5-7. Iterate and filter
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    results.push(*elem);
                }
            }
            // 8. Return A.
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.reduce ( callbackfn [ , initialValue ] )
        // https://tc39.es/ecma262/#sec-array.prototype.reduce
        // =================================================================
        "reduce" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "reduce") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let elements = u.array_elements_resolved();
            // 4. If len = 0 and initialValue is not present, throw a TypeError exception.
            // 5. Let k be 0.
            // 6. Let accumulator be undefined.
            // 7. If initialValue is present, then
            //   a. Set accumulator to initialValue.
            // 8. Else,
            //   a. ... find first existing element as accumulator ...
            let (mut accumulator, start_idx) = if args.len() >= 2 {
                (args[1], 0)
            } else if elements.is_empty() {
                throw_type_error("Reduce of empty array with no initial value");
                return JsValue::undefined().raw_bits();
            } else {
                (elements[0], 1)
            };
            // 9. Repeat, while k < len,
            for (i, element) in elements.iter().enumerate().skip(start_idx) {
                //   a. Let Pk be ! ToString(F(k)).
                //   b. Let kPresent be ? HasProperty(O, Pk).
                //   c. If kPresent is true, then
                //     i. Let kValue be ? Get(O, Pk).
                //     ii. Set accumulator to ? Call(callbackfn, undefined, << accumulator, kValue, F(k), O >>).
                let call_args = [
                    accumulator.raw_bits(),
                    element.raw_bits(),
                    JsValue::int(i as i32).raw_bits(),
                    obj,
                ];
                let result = unsafe {
                    // SAFETY: callback_bits is passed from compiled code, call_args is valid.
                    __esc_rt_call_indirect(callback_bits, 4, call_args.as_ptr())
                };
                accumulator = JsValue::from_raw_bits(result);
                //   d. Set k to k + 1.
            }
            // 10. Return accumulator.
            accumulator.raw_bits()
        }
        // =================================================================
        // Array.prototype.find ( predicate [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.find
        // =================================================================
        "find" => {
            if !require_callable_callback(args, "find") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return elem.raw_bits();
                }
            }
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.findIndex ( predicate [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.findindex
        // =================================================================
        "findIndex" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. Let findRec be ? FindViaPredicate(O, len, ascending, predicate, thisArg).
            // 4. Return findRec.[[Index]].
            if !require_callable_callback(args, "findIndex") {
                return JsValue::int(-1).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.some ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.some
        // =================================================================
        "some" => {
            if !require_callable_callback(args, "some") {
                return JsValue::bool(false).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::bool(true).raw_bits();
                }
            }
            JsValue::bool(false).raw_bits()
        }
        // =================================================================
        // Array.prototype.every ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.every
        // =================================================================
        "every" => {
            if !require_callable_callback(args, "every") {
                return JsValue::bool(true).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if !value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::bool(false).raw_bits();
                }
            }
            JsValue::bool(true).raw_bits()
        }
        // =================================================================
        // Array.prototype.reduceRight ( callbackfn [ , initialValue ] )
        // https://tc39.es/ecma262/#sec-array.prototype.reduceright
        // =================================================================
        "reduceRight" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "reduceRight") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let elements = u.array_elements_resolved();
            let len = elements.len();
            // 4. If len = 0 and initialValue is not present, throw a TypeError exception.
            // 5. Let k be len - 1.
            // 6. Let accumulator be undefined.
            // 7. If initialValue is present, set accumulator to initialValue.
            // 8. Else, find last existing element as accumulator.
            let (mut accumulator, end_idx) = if args.len() >= 2 {
                (args[1], len)
            } else if elements.is_empty() {
                throw_type_error("Reduce of empty array with no initial value");
                return JsValue::undefined().raw_bits();
            } else {
                (elements[len - 1], len - 1)
            };
            // 9. Repeat, while k >= 0,
            for i in (0..end_idx).rev() {
                //   a. Let Pk be ! ToString(F(k)).
                //   b. Let kPresent be ? HasProperty(O, Pk).
                //   c. If kPresent is true, then
                //     i. Let kValue be ? Get(O, Pk).
                //     ii. Set accumulator to ? Call(callbackfn, undefined, << accumulator, kValue, F(k), O >>).
                let call_args = [
                    accumulator.raw_bits(),
                    elements[i].raw_bits(),
                    JsValue::int(i as i32).raw_bits(),
                    obj,
                ];
                let result = unsafe {
                    // SAFETY: callback_bits is passed from compiled code, call_args is valid.
                    __esc_rt_call_indirect(callback_bits, 4, call_args.as_ptr())
                };
                accumulator = JsValue::from_raw_bits(result);
                //   d. Set k to k - 1.
            }
            // 10. Return accumulator.
            accumulator.raw_bits()
        }
        // =================================================================
        // Array.prototype.sort ( comparefn )
        // https://tc39.es/ecma262/#sec-array.prototype.sort
        // =================================================================
        "sort" => sort_array_elements(u, args, obj),
        // =================================================================
        // Array.prototype.concat ( ...items )
        // https://tc39.es/ecma262/#sec-array.prototype.concat
        // =================================================================
        "concat" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let A be ? ArraySpeciesCreate(O, 0).
            // 3. Let n be 0.
            let mut result = u.array_elements_resolved();
            // 4. Prepend O to items.
            // 5. For each element E of items, do
            for arg in args {
                let arg_bits = arg.raw_bits();
                let arg_tag = read_obj_tag(arg_bits);
                //   a. Let spreadable be ? IsConcatSpreadable(E).
                //   b. If spreadable is true, then
                if is_array_tag(arg_tag) {
                    //     i-iv. Spread elements of E into A.
                    result.extend(read_array_elements(arg_bits, arg_tag));
                    continue;
                }
                //   c. Else,
                //     i. ... set A[n] to E, increment n.
                result.push(*arg);
            }
            // 6. Perform ? Set(A, "length", F(n), true).
            // 7. Return A.
            create_array_from_elements(result)
        }
        // =================================================================
        // Array.prototype.flat ( [ depth ] )
        // https://tc39.es/ecma262/#sec-array.prototype.flat
        // =================================================================
        "flat" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let sourceLen be ? LengthOfArrayLike(O).
            // 3. Let depthNum be 1.
            // 4. If depth is not undefined, set depthNum to ? ToIntegerOrInfinity(depth).
            let depth = args.first().and_then(|v| v.as_int()).unwrap_or(1) as usize;
            let elements = u.array_elements_resolved();
            // 5. Let A be ? ArraySpeciesCreate(O, 0).
            // 6. Perform ? FlattenIntoArray(A, O, sourceLen, 0, depthNum).
            let result = flatten_array(&elements, depth);
            // 7. Return A.
            create_array_from_elements(result)
        }
        // =================================================================
        // Array.prototype.fill ( value [ , start [ , end ] ] )
        // https://tc39.es/ecma262/#sec-array.prototype.fill
        // =================================================================
        "fill" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let fill_val = args.first().copied().unwrap_or(JsValue::undefined());
            if let Some(elems) = u.array_elements_mut() {
                let len = elems.len() as i32;
                // 3. Let relativeStart be ? ToIntegerOrInfinity(start).
                let start = args.get(1).and_then(|v| v.as_int()).unwrap_or(0);
                // 4. If relativeEnd is undefined, let relativeEnd be len;
                //    else let relativeEnd be ? ToIntegerOrInfinity(end).
                let end = args.get(2).and_then(|v| v.as_int()).unwrap_or(len);
                // 5-6. Clamp start/end to [0, len].
                let start = normalize_index(start, len);
                let end = normalize_index(end, len);
                // 7. Repeat, while k < final,
                for item in elems.iter_mut().take(end).skip(start) {
                    //   a. Let Pk be ! ToString(F(k)).
                    //   b. Perform ? Set(O, Pk, value, true).
                    *item = fill_val;
                    //   c. Set k to k + 1.
                }
            }
            // 8. Return O.
            obj
        }
        // =================================================================
        // Array.prototype.shift ( )
        // https://tc39.es/ecma262/#sec-array.prototype.shift
        // =================================================================
        "shift" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            if let Some(elems) = u.array_elements_mut() {
                // 3. If len = 0, then
                if elems.is_empty() {
                    //   a. Perform ? Set(O, "length", +0F, true).
                    //   b. Return undefined.
                    return JsValue::undefined().raw_bits();
                }
                // 4. Let first be ? Get(O, "0").
                let first = elems.remove(0);
                // 5-7. Shift elements down by one.
                // 8. Perform ? DeletePropertyOrThrow(O, ! ToString(F(len - 1))).
                // 9. Perform ? Set(O, "length", F(len - 1), true).
                u.array_sync_length();
                // 10. Return first.
                return first.raw_bits();
            }
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.unshift ( ...items )
        // https://tc39.es/ecma262/#sec-array.prototype.unshift
        // =================================================================
        "unshift" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let len = u.array_len() as u64;
            // 3. Let argCount be the number of elements in items.
            let arg_count = args.len() as u64;
            // 4. If len + argCount > 2^53 - 1, throw a TypeError exception.
            if len + arg_count > (1u64 << 53) - 1 {
                throw_type_error("Array length exceeds 2^53 - 1");
                return JsValue::undefined().raw_bits();
            }
            if let Some(elems) = u.array_elements_mut() {
                // 5-6. Shift existing elements up by argCount.
                // 7. Let j be 0.
                // 8. For each element E of items, do
                for (i, arg) in args.iter().enumerate() {
                    //   a. Perform ? Set(O, ! ToString(F(j)), E, true).
                    elems.insert(i, *arg);
                    //   b. Set j to j + 1.
                }
                // 9. Perform ? Set(O, "length", F(len + argCount), true).
                u.array_sync_length();
                // 10. Return F(len + argCount).
                return JsValue::int(u.array_len() as i32).raw_bits();
            }
            JsValue::int(0).raw_bits()
        }
        // =================================================================
        // Array.prototype.splice ( start, deleteCount, ...items )
        // https://tc39.es/ecma262/#sec-array.prototype.splice
        // =================================================================
        "splice" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            if let Some(elems) = u.array_elements_mut() {
                let len = elems.len() as i32;
                // 3. Let relativeStart be ? ToIntegerOrInfinity(start).
                let start = args.first().and_then(|v| v.as_int()).unwrap_or(0);
                // 4-5. Clamp actualStart to [0, len].
                let start = normalize_index(start, len);
                // 6-9. Determine actualDeleteCount.
                let delete_count = args
                    .get(1)
                    .and_then(|v| v.as_int())
                    .unwrap_or(len - start as i32);
                let delete_count = delete_count.max(0) as usize;
                // 10. If len + itemCount - actualDeleteCount > 2^53 - 1, throw TypeError.
                let item_count = args.len().saturating_sub(2) as u64;
                let actual_delete = delete_count.min(elems.len() - start) as u64;
                if (len as u64) + item_count - actual_delete > (1u64 << 53) - 1 {
                    throw_type_error("Array length exceeds 2^53 - 1");
                    return create_empty_array();
                }
                let end = (start + delete_count).min(elems.len());
                // 11. Let A be ? ArraySpeciesCreate(O, actualDeleteCount).
                // 12-13. Copy deleted elements into A.
                let removed: Vec<JsValue> = elems.drain(start..end).collect();
                // 14-16. Insert new items at start position.
                for (i, arg) in args.iter().skip(2).enumerate() {
                    elems.insert(start + i, *arg);
                }
                // 17. Perform ? Set(O, "length", F(len - actualDeleteCount + itemCount), true).
                u.array_sync_length();
                // 18. Return A.
                return create_array_from_elements(removed);
            }
            create_empty_array()
        }
        // =================================================================
        // Array.prototype.toString ( )
        // https://tc39.es/ecma262/#sec-array.prototype.tostring
        // =================================================================
        "toString" => {
            // 1. Let array be ? ToObject(this value).
            // 2. Let func be ? Get(array, "join").
            // 3. If IsCallable(func) is false, set func to %Object.prototype.toString%.
            // 4. Return ? Call(func, array).
            // (Simplified: equivalent to this.join())
            let resolved = u.array_elements_resolved();
            let parts: Vec<String> = resolved.iter().map(|v| format_value_for_join(*v)).collect();
            let result = parts.join(",");
            make_rt_string(result)
        }
        // =================================================================
        // Array.prototype.valueOf ( )
        // (Inherited from Object.prototype.valueOf)
        // https://tc39.es/ecma262/#sec-object.prototype.valueof
        // =================================================================
        "valueOf" => {
            // 1. Return ? ToObject(this value).
            obj
        }
        // =================================================================
        // Array.prototype.at ( index )
        // https://tc39.es/ecma262/#sec-array.prototype.at
        // =================================================================
        "at" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let elements = u.array_elements_resolved();
            let len = elements.len() as i32;
            // 3. Let relativeIndex be ? ToIntegerOrInfinity(index).
            let index = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            // 4. If relativeIndex >= 0, let k be relativeIndex.
            // 5. Else, let k be len + relativeIndex.
            let resolved = if index < 0 { len + index } else { index };
            // 6. If k < 0 or k >= len, return undefined.
            if resolved < 0 || resolved >= len {
                return JsValue::undefined().raw_bits();
            }
            // 7. Return ? Get(O, ! ToString(F(k))).
            elements[resolved as usize].raw_bits()
        }
        // =================================================================
        // Array.prototype.flatMap ( mapperFunction [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.flatmap
        // =================================================================
        "flatMap" => {
            if !require_callable_callback(args, "flatMap") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            let mut results = Vec::new();
            for (i, elem) in elements.iter().enumerate() {
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let mapped =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                // Flatten one level: if result is an array, spread its elements
                let mapped_tag = read_obj_tag(mapped);
                if is_array_tag(mapped_tag) {
                    let inner = read_array_elements(mapped, mapped_tag);
                    results.extend(inner);
                } else {
                    results.push(JsValue::from_raw_bits(mapped));
                }
            }
            // 6. Return A.
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.copyWithin ( target, start [ , end ] )
        // https://tc39.es/ecma262/#sec-array.prototype.copywithin
        // =================================================================
        "copyWithin" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            if let Some(elems) = u.array_elements_mut() {
                let len = elems.len() as i32;
                // 3. Let relativeTarget be ? ToIntegerOrInfinity(target).
                let target_idx = args.first().and_then(|v| v.as_int()).unwrap_or(0);
                // 4. Let relativeStart be ? ToIntegerOrInfinity(start).
                let start = args.get(1).and_then(|v| v.as_int()).unwrap_or(0);
                // 5. If end is undefined, let relativeEnd be len;
                //    else let relativeEnd be ? ToIntegerOrInfinity(end).
                let end = args.get(2).and_then(|v| v.as_int()).unwrap_or(len);
                // 6-8. Clamp target/start/end to [0, len].
                let target_idx = normalize_index(target_idx, len);
                let start = normalize_index(start, len);
                let end = normalize_index(end, len);
                // 9. Let count be min(final - from, len - to).
                if start < end {
                    let count = (end - start).min(elems.len() - target_idx);
                    // 10-11. Determine copy direction to handle overlapping regions.
                    // Copy to a temp buffer to handle overlapping regions
                    let source: Vec<JsValue> = elems[start..start + count].to_vec();
                    // 12. Repeat, while count > 0,
                    for (i, val) in source.into_iter().enumerate() {
                        if target_idx + i < elems.len() {
                            elems[target_idx + i] = val;
                        }
                    }
                }
            }
            // 13. Return O.
            obj
        }
        // =================================================================
        // Array.prototype.findLast ( predicate [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.findlast
        // =================================================================
        "findLast" => {
            if !require_callable_callback(args, "findLast") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for i in (0..elements.len()).rev() {
                let elem = elements[i];
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return elem.raw_bits();
                }
            }
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.findLastIndex ( predicate [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-array.prototype.findlastindex
        // =================================================================
        "findLastIndex" => {
            if !require_callable_callback(args, "findLastIndex") {
                return JsValue::int(-1).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let elements = u.array_elements_resolved();
            for i in (0..elements.len()).rev() {
                let elem = elements[i];
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.toSorted ( comparefn )
        // https://tc39.es/ecma262/#sec-array.prototype.tosorted
        // =================================================================
        "toSorted" => {
            // 1. If comparefn is not undefined and IsCallable(comparefn) is false,
            //    throw a TypeError exception.
            // 2. Let O be ? ToObject(this value).
            // 3. Let len be ? LengthOfArrayLike(O).
            let mut sorted = u.array_elements_resolved();
            // 4. Let A be ? ArrayCreate(len).
            // 5. Let SortCompare be a new Abstract Closure ...
            if !args.is_empty() {
                let compare_fn_bits = args[0].raw_bits();
                // Check if compareFn is actually callable
                let is_callable = read_obj_tag(compare_fn_bits) == Some(ObjTag::Unified as u8) && {
                    let uni_check = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(compare_fn_bits)
                    };
                    uni_check.is_some_and(|u| u.is_callable())
                };
                if is_callable {
                    // 6. Let sortedList be ? SortIndexedProperties(O, len, SortCompare, read-through-holes).
                    sorted.sort_by(|a, b| {
                        let call_args = [a.raw_bits(), b.raw_bits()];
                        let result = unsafe {
                            // SAFETY: compare_fn_bits is callable, call_args is valid.
                            __esc_rt_call_indirect(compare_fn_bits, 2, call_args.as_ptr())
                        };
                        let cmp_val = JsValue::from_raw_bits(result);
                        let n = cmp_val
                            .as_number()
                            .or_else(|| cmp_val.as_int().map(|i| i as f64))
                            .unwrap_or(0.0);
                        if n < 0.0 {
                            std::cmp::Ordering::Less
                        } else if n > 0.0 {
                            std::cmp::Ordering::Greater
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    });
                } else {
                    sorted.sort_by(|a, b| compare_js_values(*a, *b));
                }
            } else {
                sorted.sort_by(|a, b| compare_js_values(*a, *b));
            }
            // 7. Let j be 0.
            // 8. Repeat, while j < len, ... set A[j] = sortedList[j] ...
            // 9. Return A.
            create_array_from_elements(sorted)
        }
        // =================================================================
        // Array.prototype.toReversed ( )
        // https://tc39.es/ecma262/#sec-array.prototype.toreversed
        // =================================================================
        "toReversed" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            // 3. Let A be ? ArrayCreate(len).
            let mut reversed = u.array_elements_resolved();
            // 4. Let k be 0.
            // 5. Repeat, while k < len,
            //   a. Let from be ! ToString(F(len - k - 1)).
            //   b. Let fromValue be ? Get(O, from).
            //   c. Perform ! CreateDataPropertyOrThrow(A, ! ToString(F(k)), fromValue).
            //   d. Set k to k + 1.
            reversed.reverse();
            // 6. Return A.
            create_array_from_elements(reversed)
        }
        // =================================================================
        // Array.prototype.toSpliced ( start, skipCount, ...items )
        // https://tc39.es/ecma262/#sec-array.prototype.tospliced
        // =================================================================
        "toSpliced" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let elements = u.array_elements_resolved();
            let len = elements.len() as i32;
            // 3. Let relativeStart be ? ToIntegerOrInfinity(start).
            let start = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            // 4-5. Clamp actualStart to [0, len].
            let start = normalize_index(start, len);
            // 6-9. Determine actualSkipCount.
            let delete_count = args
                .get(1)
                .and_then(|v| v.as_int())
                .unwrap_or(len - start as i32);
            let delete_count = delete_count.max(0) as usize;
            // 10. If len + insertCount - actualSkipCount > 2^53 - 1, throw TypeError.
            let insert_count = args.len().saturating_sub(2) as u64;
            let actual_skip = delete_count.min(elements.len() - start) as u64;
            if (len as u64) + insert_count - actual_skip > (1u64 << 53) - 1 {
                throw_type_error("Array length exceeds 2^53 - 1");
                return create_empty_array();
            }
            let end = (start + delete_count).min(elements.len());
            // 11. Let A be ? ArrayCreate(newLen).
            let mut result =
                Vec::with_capacity(elements.len() - (end - start) + args.len().saturating_sub(2));
            // 12-14. Copy elements before start, then items, then elements after start+skipCount.
            result.extend_from_slice(&elements[..start]);
            for arg in args.iter().skip(2) {
                result.push(*arg);
            }
            result.extend_from_slice(&elements[end..]);
            // 15. Return A.
            create_array_from_elements(result)
        }
        // =================================================================
        // Array.prototype.entries ( )
        // https://tc39.es/ecma262/#sec-array.prototype.entries
        // =================================================================
        "entries" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Return CreateArrayIterator(O, key+value).
            let iter = JsIterator::new_array_entries(obj);
            boxed_array_iterator(iter)
        }
        // =================================================================
        // Array.prototype.keys ( )
        // https://tc39.es/ecma262/#sec-array.prototype.keys
        // =================================================================
        "keys" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Return CreateArrayIterator(O, key).
            let iter = JsIterator::new_array_keys(obj);
            boxed_array_iterator(iter)
        }
        // =================================================================
        // Array.prototype.values ( )
        // https://tc39.es/ecma262/#sec-array.prototype.values
        // =================================================================
        "values" => {
            // 1. Let O be ? ToObject(this value).
            // 2. Return CreateArrayIterator(O, value).
            let iter = JsIterator::new_array_values(obj);
            boxed_array_iterator(iter)
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// `Array.prototype.sort ( comparefn )`
///
/// Sort array elements in-place, handling compareFn NaN and undefined/sparse.
///
/// Per ES2024 23.1.3.30:
/// - If compareFn is provided, use it; if it returns NaN, treat as +0 (equal).
/// - `undefined` elements sort after all defined elements.
/// - Rust's `sort_by` is stable, satisfying the spec requirement.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.prototype.sort
fn sort_array_elements(u: &mut UnifiedObject, args: &[JsValue], obj: u64) -> u64 {
    // 1. If comparefn is not undefined and IsCallable(comparefn) is false,
    //    throw a TypeError exception.
    // 2. Let obj be ? ToObject(this value).
    // 3. Let len be ? LengthOfArrayLike(obj).
    if let Some(elems) = u.array_elements_mut() {
        if !args.is_empty() {
            let compare_fn_bits = args[0].raw_bits();
            let compare_val = JsValue::from_raw_bits(compare_fn_bits);
            // undefined compareFn means default sort
            if !compare_val.is_undefined() {
                if !is_callable_value(compare_fn_bits) && compare_val.as_int().is_none() {
                    throw_type_error(
                        "The comparison function must be either a function or undefined",
                    );
                    return obj;
                }
                // 4. Let SortCompare be a new Abstract Closure with parameters (x, y) ...
                // 5. Let sortedList be ? SortIndexedProperties(obj, len, SortCompare, skip-holes).
                // Custom compareFn: undefined elements sort to end, NaN from compareFn -> Equal
                elems.sort_by(|a, b| {
                    // undefined sorts after everything
                    let a_undef = a.is_undefined();
                    let b_undef = b.is_undefined();
                    if a_undef && b_undef {
                        return std::cmp::Ordering::Equal;
                    }
                    if a_undef {
                        return std::cmp::Ordering::Greater;
                    }
                    if b_undef {
                        return std::cmp::Ordering::Less;
                    }
                    // CompareArrayElements steps:
                    // 1. If x and y are both undefined, return +0F.
                    // 2. If x is undefined, return 1F.
                    // 3. If y is undefined, return -1F.
                    // 4. If comparefn is not undefined, then
                    //   a. Let v be ? ToNumber(? Call(comparefn, undefined, << x, y >>)).
                    let call_args = [a.raw_bits(), b.raw_bits()];
                    let result = unsafe {
                        // SAFETY: compare_fn_bits is callable, call_args is valid.
                        __esc_rt_call_indirect(compare_fn_bits, 2, call_args.as_ptr())
                    };
                    let cmp_val = JsValue::from_raw_bits(result);
                    let n = cmp_val
                        .as_number()
                        .or_else(|| cmp_val.as_int().map(|i| i as f64))
                        .unwrap_or(0.0);
                    //   b. If v is NaN, return +0F.
                    if n.is_nan() {
                        std::cmp::Ordering::Equal
                    } else if n < 0.0 {
                        //   c. Return v.
                        std::cmp::Ordering::Less
                    } else if n > 0.0 {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                });
                // 6. Set obj properties from sortedList.
                return obj;
            }
        }
        // Default sort (no comparefn): undefined to end, then string comparison
        // CompareArrayElements steps 5-7 (default):
        // 5. Let xString be ? ToString(x).
        // 6. Let yString be ? ToString(y).
        // 7. ... compare xString and yString lexicographically ...
        elems.sort_by(|a, b| {
            let a_undef = a.is_undefined();
            let b_undef = b.is_undefined();
            if a_undef && b_undef {
                return std::cmp::Ordering::Equal;
            }
            if a_undef {
                return std::cmp::Ordering::Greater;
            }
            if b_undef {
                return std::cmp::Ordering::Less;
            }
            compare_js_values(*a, *b)
        });
    }
    // 7. Return obj.
    obj
}

/// Dispatch array methods on a generic (non-array) object using property access.
///
/// Implements the ES2024 spec pattern: `ToObject(this)`, `LengthOfArrayLike(O)`,
/// then `Get(O, ToString(k))` for each index. This allows `Array.prototype` methods
/// to work on any object with a `length` property and indexed properties, as
/// required by the spec for `.call()` / `.apply()` usage.
fn dispatch_generic_array_method(obj: u64, method: &str, args: &[JsValue]) -> u64 {
    // RequireObjectCoercible for methods that need it
    let needs_coercible = matches!(
        method,
        "forEach"
            | "map"
            | "filter"
            | "reduce"
            | "reduceRight"
            | "some"
            | "every"
            | "find"
            | "findIndex"
            | "findLast"
            | "findLastIndex"
            | "indexOf"
            | "lastIndexOf"
            | "includes"
            | "join"
            | "flat"
            | "flatMap"
            | "slice"
            | "concat"
            | "at"
            | "toString"
    );
    if needs_coercible && !require_object_coercible(obj, method) {
        return JsValue::undefined().raw_bits();
    }

    match method {
        // =================================================================
        // Array.prototype.forEach ( callbackfn [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.foreach
        // =================================================================
        "forEach" => {
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            if !require_callable_callback(args, "forEach") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // 1. Let O be ? ToObject(this value).
            // 2. Let len be ? LengthOfArrayLike(O).
            let len = length_of_array_like(obj);
            // 4. Let k be 0.
            // 5. Repeat, while k < len,
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr());
                }
                super::CURRENT_THIS.with(|c| c.set(prev_this));
            }
            // 6. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.map ( callbackfn [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.map
        // =================================================================
        "map" => {
            if !require_callable_callback(args, "map") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            let mut results = Vec::with_capacity(len as usize);
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                results.push(JsValue::from_raw_bits(result));
            }
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.filter ( callbackfn [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.filter
        // =================================================================
        "filter" => {
            if !require_callable_callback(args, "filter") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            let mut results = Vec::new();
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    results.push(elem);
                }
            }
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.reduce ( callbackfn [ , initialValue ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.reduce
        // =================================================================
        "reduce" => {
            if !require_callable_callback(args, "reduce") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let len = length_of_array_like(obj);
            let (mut accumulator, start_idx) = if args.len() >= 2 {
                (args[1], 0u32)
            } else if len == 0 {
                throw_type_error("Reduce of empty array with no initial value");
                return JsValue::undefined().raw_bits();
            } else {
                (get_indexed_value(obj, 0), 1)
            };
            for i in start_idx..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [
                    accumulator.raw_bits(),
                    elem.raw_bits(),
                    JsValue::int(i as i32).raw_bits(),
                    obj,
                ];
                let result = unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 4, call_args.as_ptr())
                };
                accumulator = JsValue::from_raw_bits(result);
            }
            accumulator.raw_bits()
        }
        // =================================================================
        // Array.prototype.reduceRight ( callbackfn [ , initialValue ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.reduceright
        // =================================================================
        "reduceRight" => {
            if !require_callable_callback(args, "reduceRight") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let len = length_of_array_like(obj);
            let (mut accumulator, end_idx) = if args.len() >= 2 {
                (args[1], len)
            } else if len == 0 {
                throw_type_error("Reduce of empty array with no initial value");
                return JsValue::undefined().raw_bits();
            } else {
                (get_indexed_value(obj, len - 1), len - 1)
            };
            for i in (0..end_idx).rev() {
                let elem = get_indexed_value(obj, i);
                let call_args = [
                    accumulator.raw_bits(),
                    elem.raw_bits(),
                    JsValue::int(i as i32).raw_bits(),
                    obj,
                ];
                let result = unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 4, call_args.as_ptr())
                };
                accumulator = JsValue::from_raw_bits(result);
            }
            accumulator.raw_bits()
        }
        // =================================================================
        // Array.prototype.some ( callbackfn [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.some
        // =================================================================
        "some" => {
            if !require_callable_callback(args, "some") {
                return JsValue::bool(false).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::bool(true).raw_bits();
                }
            }
            JsValue::bool(false).raw_bits()
        }
        // =================================================================
        // Array.prototype.every ( callbackfn [ , thisArg ] )
        // Generic path
        // =================================================================
        "every" => {
            if !require_callable_callback(args, "every") {
                return JsValue::bool(true).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if !value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::bool(false).raw_bits();
                }
            }
            JsValue::bool(true).raw_bits()
        }
        // =================================================================
        // Array.prototype.find ( predicate [ , thisArg ] )
        // Generic path
        // =================================================================
        "find" => {
            if !require_callable_callback(args, "find") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return elem.raw_bits();
                }
            }
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.findIndex ( predicate [ , thisArg ] )
        // Generic path
        // =================================================================
        "findIndex" => {
            if !require_callable_callback(args, "findIndex") {
                return JsValue::int(-1).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let this_arg = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let len = length_of_array_like(obj);
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let prev_this = super::CURRENT_THIS.with(|c| c.replace(this_arg));
                let result =
                    unsafe { __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr()) };
                super::CURRENT_THIS.with(|c| c.set(prev_this));
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.findLast ( predicate [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.findlast
        // =================================================================
        "findLast" => {
            if !require_callable_callback(args, "findLast") {
                return JsValue::undefined().raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let len = length_of_array_like(obj);
            for i in (0..len).rev() {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let result = unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr())
                };
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return elem.raw_bits();
                }
            }
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Array.prototype.findLastIndex ( predicate [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.findlastindex
        // =================================================================
        "findLastIndex" => {
            if !require_callable_callback(args, "findLastIndex") {
                return JsValue::int(-1).raw_bits();
            }
            let callback_bits = args[0].raw_bits();
            let len = length_of_array_like(obj);
            for i in (0..len).rev() {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let result = unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr())
                };
                if value_ops::to_boolean(JsValue::from_raw_bits(result)) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.indexOf ( searchElement [ , fromIndex ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.indexof
        // =================================================================
        "indexOf" => {
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let len = length_of_array_like(obj);
            let from_index = args
                .get(1)
                .map_or(0, |v| crate::value_ops::to_integer_or_infinity(*v) as i32);
            let start = if from_index < 0 {
                (len as i32 + from_index).max(0) as u32
            } else {
                from_index as u32
            };
            for i in start..len {
                let elem = get_indexed_value(obj, i);
                if value_ops::strict_eq(elem, search) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.lastIndexOf ( searchElement [ , fromIndex ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.lastindexof
        // =================================================================
        "lastIndexOf" => {
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let len = length_of_array_like(obj);
            if len == 0 {
                return JsValue::int(-1).raw_bits();
            }
            let from_index = args.get(1).map_or(len as i32 - 1, |v| {
                if v.is_undefined() {
                    len as i32 - 1
                } else {
                    crate::value_ops::to_integer_or_infinity(*v) as i32
                }
            });
            let end = if from_index < 0 {
                (len as i32 + from_index) as u32
            } else {
                from_index.min(len as i32 - 1) as u32
            };
            for i in (0..=end).rev() {
                let elem = get_indexed_value(obj, i);
                if value_ops::strict_eq(elem, search) {
                    return JsValue::int(i as i32).raw_bits();
                }
            }
            JsValue::int(-1).raw_bits()
        }
        // =================================================================
        // Array.prototype.includes ( searchElement [ , fromIndex ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.includes
        // =================================================================
        "includes" => {
            let search = args.first().copied().unwrap_or(JsValue::undefined());
            let len = length_of_array_like(obj);
            // TODO: fromIndex handling (currently always starts at 0).
            // TODO: Uses strict_eq instead of SameValueZero — NaN !== NaN will differ.
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                if value_ops::strict_eq(elem, search) {
                    return JsValue::bool(true).raw_bits();
                }
            }
            JsValue::bool(false).raw_bits()
        }
        // =================================================================
        // Array.prototype.join ( separator )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.join
        // =================================================================
        "join" => {
            let sep = args
                .first()
                .and_then(|v| extract_key_string(v.raw_bits()))
                .unwrap_or_else(|| ",".to_string());
            let elements = get_array_like_elements(obj);
            let parts: Vec<String> = elements.iter().map(|v| format_value_for_join(*v)).collect();
            let result = parts.join(&sep);
            make_rt_string(result)
        }
        // =================================================================
        // Array.prototype.slice ( start, end )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.slice
        // =================================================================
        "slice" => {
            let len = length_of_array_like(obj) as i32;
            let start = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            let end = args.get(1).and_then(|v| v.as_int()).unwrap_or(len);
            let start = normalize_index(start, len);
            let end = normalize_index(end, len);
            if start >= end {
                return create_empty_array();
            }
            let mut sliced = Vec::with_capacity(end - start);
            for i in start..end {
                sliced.push(get_indexed_value(obj, i as u32));
            }
            create_array_from_elements(sliced)
        }
        // =================================================================
        // Array.prototype.concat ( ...items )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.concat
        // =================================================================
        "concat" => {
            let elements = get_array_like_elements(obj);
            let mut result: Vec<JsValue> = elements;
            for arg in args {
                let arg_bits = arg.raw_bits();
                let arg_tag = read_obj_tag(arg_bits);
                if is_array_tag(arg_tag) {
                    result.extend(read_array_elements(arg_bits, arg_tag));
                    continue;
                }
                result.push(*arg);
            }
            create_array_from_elements(result)
        }
        // =================================================================
        // Array.prototype.flat ( [ depth ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.flat
        // =================================================================
        "flat" => {
            let depth = args.first().and_then(|v| v.as_int()).unwrap_or(1) as usize;
            let elements = get_array_like_elements(obj);
            let result = flatten_array(&elements, depth);
            create_array_from_elements(result)
        }
        // =================================================================
        // Array.prototype.flatMap ( mapperFunction [ , thisArg ] )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.flatmap
        // =================================================================
        "flatMap" => {
            if !require_callable_callback(args, "flatMap") {
                return create_empty_array();
            }
            let callback_bits = args[0].raw_bits();
            let len = length_of_array_like(obj);
            let mut results = Vec::new();
            for i in 0..len {
                let elem = get_indexed_value(obj, i);
                let call_args = [elem.raw_bits(), JsValue::int(i as i32).raw_bits(), obj];
                let mapped = unsafe {
                    // SAFETY: callback_bits is callable, call_args is valid stack array.
                    __esc_rt_call_indirect(callback_bits, 3, call_args.as_ptr())
                };
                let mapped_tag = read_obj_tag(mapped);
                if is_array_tag(mapped_tag) {
                    let inner = read_array_elements(mapped, mapped_tag);
                    results.extend(inner);
                } else {
                    results.push(JsValue::from_raw_bits(mapped));
                }
            }
            create_array_from_elements(results)
        }
        // =================================================================
        // Array.prototype.at ( index )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.at
        // =================================================================
        "at" => {
            let len = length_of_array_like(obj) as i32;
            let index = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            let resolved = if index < 0 { len + index } else { index };
            if resolved < 0 || resolved >= len {
                return JsValue::undefined().raw_bits();
            }
            get_indexed_value(obj, resolved as u32).raw_bits()
        }
        // =================================================================
        // Array.prototype.toString ( )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.tostring
        // =================================================================
        "toString" => {
            // Generic toString: call join()
            let elements = get_array_like_elements(obj);
            let parts: Vec<String> = elements.iter().map(|v| format_value_for_join(*v)).collect();
            let result = parts.join(",");
            make_rt_string(result)
        }
        // Object.prototype.valueOf ( )
        // https://tc39.es/ecma262/#sec-object.prototype.valueof
        "valueOf" => obj,
        // Mutating methods on non-array objects: not supported in generic path.
        "sort" | "reverse" | "fill" | "shift" | "unshift" | "splice" | "push" | "pop"
        | "copyWithin" => {
            // Return the object itself for chainable methods, undefined otherwise.
            match method {
                "push" | "unshift" => JsValue::int(0).raw_bits(),
                "pop" | "shift" => JsValue::undefined().raw_bits(),
                "splice" => create_empty_array(),
                _ => obj,
            }
        }
        "length" => JsValue::int(length_of_array_like(obj) as i32).raw_bits(),
        // =================================================================
        // Array.prototype.toSorted ( comparefn )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.tosorted
        // =================================================================
        "toSorted" => {
            let mut sorted = get_array_like_elements(obj);
            sorted.sort_by(|a, b| compare_js_values(*a, *b));
            create_array_from_elements(sorted)
        }
        // =================================================================
        // Array.prototype.toReversed ( )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.toreversed
        // =================================================================
        "toReversed" => {
            let mut reversed = get_array_like_elements(obj);
            reversed.reverse();
            create_array_from_elements(reversed)
        }
        // =================================================================
        // Array.prototype.toSpliced ( start, skipCount, ...items )
        // Generic path — https://tc39.es/ecma262/#sec-array.prototype.tospliced
        // =================================================================
        "toSpliced" => {
            let elements = get_array_like_elements(obj);
            let len = elements.len() as i32;
            let start = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            let start = normalize_index(start, len);
            let delete_count = args
                .get(1)
                .and_then(|v| v.as_int())
                .unwrap_or(len - start as i32);
            let delete_count = delete_count.max(0) as usize;
            let end = (start + delete_count).min(elements.len());
            let mut result =
                Vec::with_capacity(elements.len() - (end - start) + args.len().saturating_sub(2));
            result.extend_from_slice(&elements[..start]);
            for arg in args.iter().skip(2) {
                result.push(*arg);
            }
            result.extend_from_slice(&elements[end..]);
            create_array_from_elements(result)
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// Wrap a [`JsIterator`] into a unified iterator object.
///
/// Delegates to `TaggedObj::boxed` with `InternalKind::Iterator`.
fn boxed_array_iterator(iter: JsIterator) -> u64 {
    TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::iterator(shapes::ShapeTable::EMPTY_SHAPE, iter),
    )
}

/// `FlattenIntoArray` helper
///
/// Flatten an array to a given depth, recursively spreading nested arrays.
///
/// [spec]: https://tc39.es/ecma262/#sec-flattenintoarray
fn flatten_array(elements: &[JsValue], depth: usize) -> Vec<JsValue> {
    let mut result = Vec::new();
    for elem in elements {
        // 1. Let exists be ? HasProperty(source, P).
        // 2. If exists is true, then
        //   a. Let element be ? Get(source, P).
        if depth > 0 {
            //   b. ... if depth > 0 and IsConcatSpreadable(element) ...
            let elem_tag = read_obj_tag(elem.raw_bits());
            if is_array_tag(elem_tag) {
                //     i. ... recursively flatten ...
                let inner_elems = read_array_elements(elem.raw_bits(), elem_tag);
                result.extend(flatten_array(&inner_elems, depth - 1));
                continue;
            }
        }
        //   c. Else, append element to target.
        result.push(*elem);
    }
    result
}

/// Dispatch `Array` static methods: `isArray`, `from`, and `of`.
///
/// Returns `Some(result)` for recognized methods, or `None` for unknown names
/// so the caller can fall back to prototype lookup.
pub(crate) fn dispatch_array_static_method(
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // =================================================================
        // Array.isArray ( arg )
        // https://tc39.es/ecma262/#sec-array.isarray
        // =================================================================
        "isArray" => {
            // 1. Return ? IsArray(arg).
            let val = args.first().map_or(0u64, |v| v.raw_bits());
            let is_arr = is_array_through_proxy(val);
            Some(JsValue::bool(is_arr).raw_bits())
        }
        // =================================================================
        // Array.from ( items [ , mapfn [ , thisArg ] ] )
        // https://tc39.es/ecma262/#sec-array.from
        // =================================================================
        "from" => {
            // 1. Let C be the this value.
            // 2. If mapfn is undefined, let mapping be false.
            // 3. Else,
            //   a. If IsCallable(mapfn) is false, throw a TypeError exception.
            //   b. Let mapping be true.
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            let val_bits = val.raw_bits();
            let map_fn = args.get(1).copied();

            // 4. Let usingIterator be ? GetMethod(items, @@iterator).
            // 5. If usingIterator is not undefined, then ...

            // Check if the argument is a string — split into characters
            if let Some(ptr) = val.as_string()
                && !ptr.is_null()
            {
                let s = unsafe {
                    // SAFETY: string pointer was created by runtime string APIs.
                    &*(ptr as *const crate::string_ops::RtString)
                };
                // String is iterable: each character becomes an element
                let elements: Vec<JsValue> = s
                    .as_str()
                    .chars()
                    .enumerate()
                    .map(|(i, c)| {
                        let elem = JsValue::from_raw_bits(make_rt_string(c.to_string()));
                        // If mapping, apply mapfn to each element
                        apply_map_fn(map_fn, elem, i)
                    })
                    .collect();
                return Some(create_array_from_elements(elements));
            }

            // Check if the argument is an array — shallow copy
            let val_arr_tag = read_obj_tag(val_bits);
            if is_array_tag(val_arr_tag) {
                let src_elems = read_array_elements(val_bits, val_arr_tag);
                let elements: Vec<JsValue> = src_elems
                    .iter()
                    .enumerate()
                    .map(|(i, v)| apply_map_fn(map_fn, *v, i))
                    .collect();
                return Some(create_array_from_elements(elements));
            }

            // Check for iterable via [Symbol.iterator]
            let val_tag = read_obj_tag(val_bits);
            if val_tag == Some(ObjTag::Unified as u8) {
                // Try iterator protocol first
                //   a. Let iteratorRecord be ? GetIterator(items, sync, usingIterator).
                let sym_iter_fn =
                    super::get_prop_by_symbol_key(val_bits, crate::symbol::SYMBOL_ITERATOR);
                let sym_val = JsValue::from_raw_bits(sym_iter_fn);
                if !sym_val.is_undefined() {
                    let iter = __esc_rt_iter_init(val_bits);
                    let mut elements = Vec::new();
                    let mut idx = 0usize;
                    //   b. Repeat,
                    loop {
                        //     i. Let next be ? IteratorStepValue(iteratorRecord).
                        let result = __esc_rt_iter_next(iter);
                        let done_bits = __esc_rt_iter_done(result);
                        //     ii. If next is done, then ... set A.length, return A.
                        if value_ops::to_boolean(JsValue::from_raw_bits(done_bits)) {
                            break;
                        }
                        let elem_val = __esc_rt_iter_value(result);
                        let elem = JsValue::from_raw_bits(elem_val);
                        //     iii. If mapping is true, let mappedValue be Call(mapfn, thisArg, << next, F(k) >>).
                        //     iv. Else, let mappedValue be next.
                        elements.push(apply_map_fn(map_fn, elem, idx));
                        idx += 1;
                    }
                    __esc_rt_iter_close(iter);
                    return Some(create_array_from_elements(elements));
                }

                // 6. NOTE: items is not an Iterable so assume it is an array-like object.
                // 7. Let arrayLike be ! ToObject(items).
                // 8. Let len be ? LengthOfArrayLike(arrayLike).
                let len_key = make_rt_string("length".to_string());
                let len_val = super::__esc_rt_get_prop(val_bits, len_key);
                let len_js = JsValue::from_raw_bits(len_val);
                let len = len_js
                    .as_int()
                    .or_else(|| len_js.as_number().map(|n| n as i32))
                    .unwrap_or(0)
                    .max(0) as usize;
                // 9. Let A be ? ArrayCreate(len) (or TypedArrayCreate).
                let mut elements = Vec::with_capacity(len);
                // 10. Let k be 0.
                // 11. Repeat, while k < len,
                for i in 0..len {
                    //   a. Let Pk be ! ToString(F(k)).
                    let idx_key = make_rt_string(i.to_string());
                    //   b. Let kValue be ? Get(arrayLike, Pk).
                    let elem_bits = super::__esc_rt_get_prop(val_bits, idx_key);
                    let elem = JsValue::from_raw_bits(elem_bits);
                    //   c. If mapping is true, let mappedValue be ? Call(mapfn, thisArg, << kValue, F(k) >>).
                    //   d. Else, let mappedValue be kValue.
                    //   e. Perform ? CreateDataPropertyOrThrow(A, Pk, mappedValue).
                    elements.push(apply_map_fn(map_fn, elem, i));
                    //   f. Set k to k + 1.
                }
                // 12. Perform ? Set(A, "length", F(len), true).
                // 13. Return A.
                return Some(create_array_from_elements(elements));
            }

            // Fallback: empty array
            Some(__esc_rt_create_array(0))
        }
        // =================================================================
        // Array.of ( ...items )
        // https://tc39.es/ecma262/#sec-array.of
        // =================================================================
        "of" => {
            // 1. Let len be the number of elements in items.
            // 2. Let lenNumber be F(len).
            // 3. Let C be the this value.
            // 4. ... (ArrayCreate or Construct) ...
            // 5. Let k be 0.
            // 6. Repeat, while k < len,
            //   a. Let kValue be items[k].
            //   b. Let Pk be ! ToString(F(k)).
            //   c. Perform ? CreateDataPropertyOrThrow(A, Pk, kValue).
            //   d. Set k to k + 1.
            let elements: Vec<JsValue> = args.to_vec();
            // 7. Perform ? Set(A, "length", lenNumber, true).
            // 8. Return A.
            Some(create_array_from_elements(elements))
        }
        _ => None,
    }
}

/// Apply an optional map function to an element during `Array.from`.
///
/// If `map_fn` is `None` or not callable, returns the element unchanged.
/// Corresponds to the `mapfn` parameter in `Array.from ( items [ , mapfn [ , thisArg ] ] )`.
///
/// [spec]: https://tc39.es/ecma262/#sec-array.from (step 3, 5.b.iii)
fn apply_map_fn(map_fn: Option<JsValue>, elem: JsValue, index: usize) -> JsValue {
    let Some(func) = map_fn else {
        return elem;
    };
    let func_bits = func.raw_bits();
    let callable = read_obj_tag(func_bits) == Some(ObjTag::Unified as u8) && {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(func_bits)
        };
        uni.is_some_and(|u| u.is_callable())
    };
    if !callable {
        return elem;
    }
    // Call(mapfn, thisArg, << kValue, F(k) >>)
    let argv: [u64; 2] = [elem.raw_bits(), JsValue::int(index as i32).raw_bits()];
    let result = unsafe {
        // SAFETY: callee verified callable above. argv points to 2 valid u64 values on the stack.
        __esc_rt_call_indirect(func_bits, 2, argv.as_ptr())
    };
    JsValue::from_raw_bits(result)
}
