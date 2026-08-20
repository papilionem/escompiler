//! Map/Set/WeakMap/WeakSet method dispatch.
//!
//! Contains `dispatch_map_method` and `dispatch_set_method` for routing
//! method calls on Map/Set-type objects to their respective implementations.

use nanbox::JsValue;
use shapes::ShapeTable;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::iterator::JsIterator;
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut};
use crate::{exceptions, value_ops};

use super::{create_array_from_elements, read_argv};

// =========================================================================
// Map / Set method dispatch
// =========================================================================

/// Dispatch a `Map.prototype` method call by name.
///
/// Routes to the appropriate `Map.prototype` or `WeakMap.prototype` method
/// based on the `method` string. Handles `get`, `set`, `has`, `delete`,
/// `clear`, `forEach`, `entries`, `keys`, `values`, and `size`.
///
/// For `WeakMap.prototype.set`, validates that the key is an object per
/// [`WeakMap.prototype.set` step 2](https://tc39.es/ecma262/#sec-weakmap.prototype.set).
///
/// Returns the NaN-boxed result directly.
pub(crate) fn dispatch_map_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);
    let uni = unsafe {
        // SAFETY: caller ensures obj is a unified MapObj/WeakMapObj.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::undefined().raw_bits();
    };
    let is_weak = u.kind == InternalKind::WeakMapObj;
    let Some(InternalData::Map { entries }) = u.internal_data_mut() else {
        return JsValue::undefined().raw_bits();
    };
    // WeakMap.prototype.set ( key, value )
    // https://tc39.es/ecma262/#sec-weakmap.prototype.set
    // Step 2. If key is not an Object, throw a TypeError exception.
    if is_weak && method == "set" {
        let key = args.first().copied().unwrap_or(JsValue::undefined());
        if !key.is_object() {
            return throw_type_error_helper("Invalid value used as weak map key");
        }
    }
    dispatch_map_method_on_entries(entries, obj, method, &args)
}

/// Shared implementation for Map methods operating on an entries vector.
///
/// Dispatches individual `Map.prototype` methods:
/// - `get` — [spec](https://tc39.es/ecma262/#sec-map.prototype.get)
/// - `set` — [spec](https://tc39.es/ecma262/#sec-map.prototype.set)
/// - `has` — [spec](https://tc39.es/ecma262/#sec-map.prototype.has)
/// - `delete` — [spec](https://tc39.es/ecma262/#sec-map.prototype.delete)
/// - `clear` — [spec](https://tc39.es/ecma262/#sec-map.prototype.clear)
/// - `forEach` — [spec](https://tc39.es/ecma262/#sec-map.prototype.foreach)
/// - `entries` — [spec](https://tc39.es/ecma262/#sec-map.prototype.entries)
/// - `keys` — [spec](https://tc39.es/ecma262/#sec-map.prototype.keys)
/// - `values` — [spec](https://tc39.es/ecma262/#sec-map.prototype.values)
/// - `size` — [spec](https://tc39.es/ecma262/#sec-get-map.prototype.size)
fn dispatch_map_method_on_entries(
    entries: &mut Vec<(nanbox::JsValue, nanbox::JsValue)>,
    obj: u64,
    method: &str,
    args: &[nanbox::JsValue],
) -> u64 {
    match method {
        // =================================================================
        // Map.prototype.get ( key )
        // https://tc39.es/ecma262/#sec-map.prototype.get
        // =================================================================
        "get" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller — we already have entries.)
            // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            let key = args.first().copied().unwrap_or(JsValue::undefined());
            for (k, v) in entries.iter() {
                //    a. If p.[[Key]] is not empty and SameValueZero(p.[[Key]], key) is true,
                //       return p.[[Value]].
                if value_ops::strict_eq(*k, key) {
                    return v.raw_bits();
                }
            }
            // 4. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Map.prototype.set ( key, value )
        // https://tc39.es/ecma262/#sec-map.prototype.set
        // =================================================================
        "set" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            let key = args.first().copied().unwrap_or(JsValue::undefined());
            let val = args.get(1).copied().unwrap_or(JsValue::undefined());
            // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            for entry in entries.iter_mut() {
                //    a. If p.[[Key]] is not empty and SameValueZero(p.[[Key]], key) is true, then
                if value_ops::strict_eq(entry.0, key) {
                    //       i. Set p.[[Value]] to value.
                    entry.1 = val;
                    //       ii. Return M.
                    return obj; // Map.set returns the map
                }
            }
            // 4. If key is -0, set key to +0.
            // TODO: Step 4 — normalize -0 to +0.
            // 5. Let p be the Record { [[Key]]: key, [[Value]]: value }.
            // 6. Append p to M.[[MapData]].
            entries.push((key, val));
            // 7. Return M.
            obj
        }
        // =================================================================
        // Map.prototype.has ( key )
        // https://tc39.es/ecma262/#sec-map.prototype.has
        // =================================================================
        "has" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            let key = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            //    a. If p.[[Key]] is not empty and SameValueZero(p.[[Key]], key) is true,
            //       return true.
            let found = entries.iter().any(|(k, _)| value_ops::strict_eq(*k, key));
            // 4. Return false.
            JsValue::bool(found).raw_bits()
        }
        // =================================================================
        // Map.prototype.delete ( key )
        // https://tc39.es/ecma262/#sec-map.prototype.delete
        // =================================================================
        "delete" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            let key = args.first().copied().unwrap_or(JsValue::undefined());
            let len_before = entries.len();
            // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            //    a. If p.[[Key]] is not empty and SameValueZero(p.[[Key]], key) is true, then
            //       i. Set p.[[Key]] to empty.
            //       ii. Set p.[[Value]] to empty.
            //       iii. Return true.
            entries.retain(|(k, _)| !value_ops::strict_eq(*k, key));
            // 4. Return false.
            JsValue::bool(entries.len() != len_before).raw_bits()
        }
        // =================================================================
        // Map.prototype.clear ( )
        // https://tc39.es/ecma262/#sec-map.prototype.clear
        // =================================================================
        "clear" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            //    a. Set p.[[Key]] to empty.
            //    b. Set p.[[Value]] to empty.
            entries.clear();
            // 4. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // get Map.prototype.size
        // https://tc39.es/ecma262/#sec-get-map.prototype.size
        // =================================================================
        "size" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            // 3. Let count be 0.
            // 4. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
            //    a. If p.[[Key]] is not empty, set count to count + 1.
            // 5. Return F(count).
            JsValue::number(entries.len() as f64).raw_bits()
        }
        // =================================================================
        // Map.prototype.forEach ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-map.prototype.foreach
        // =================================================================
        "forEach" => {
            // 1. Let M be the this value.
            // 2. Perform ? RequireInternalSlot(M, [[MapData]]).
            //    (Handled by caller.)
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            let callback = args.first().copied().unwrap_or(JsValue::undefined());
            let cb_bits = callback.raw_bits();
            if !is_callable_for_collections(cb_bits) {
                return throw_type_error_helper("Map.prototype.forEach callback is not a function");
            }
            // 4. Let entries be M.[[MapData]].
            // 5. Let numEntries be the number of elements of entries.
            // 6. Let index be 0.
            // 7. Repeat, while index < numEntries,
            for (k, v) in entries.iter() {
                //    a. Let e be entries[index].
                //    b. Set index to index + 1.
                //    c. If e.[[Key]] is not empty, then
                //       i. Perform ? Call(callbackfn, thisArg, « e.[[Value]], e.[[Key]], M »).
                let argv = [v.raw_bits(), k.raw_bits(), obj];
                unsafe {
                    // SAFETY: callback is a closure created by compiled code.
                    super::__esc_rt_call_closure(cb_bits, 3, argv.as_ptr());
                }
                //       ii. NOTE: numEntries must be re-determined each iteration.
                //       iii. Set numEntries to the number of elements of entries.
                // TODO: Steps 7c.ii-iii — handle map mutation during iteration.
            }
            // 8. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Map.prototype.entries ( )
        // https://tc39.es/ecma262/#sec-map.prototype.entries
        // =================================================================
        "entries" => {
            // 1. Let M be the this value.
            // 2. Return ? CreateMapIterator(M, key+value).
            let iter = JsIterator::new_map_entries(obj);
            TaggedObj::boxed(
                ObjTag::Unified,
                UnifiedObject::iterator(ShapeTable::EMPTY_SHAPE, iter),
            )
        }
        // =================================================================
        // Map.prototype.keys ( )
        // https://tc39.es/ecma262/#sec-map.prototype.keys
        // =================================================================
        "keys" => {
            // 1. Let M be the this value.
            // 2. Return ? CreateMapIterator(M, key).
            // TODO: Return a proper MapIterator instead of an array snapshot.
            let keys: Vec<JsValue> = entries.iter().map(|(k, _)| *k).collect();
            create_array_from_elements(keys)
        }
        // =================================================================
        // Map.prototype.values ( )
        // https://tc39.es/ecma262/#sec-map.prototype.values
        // =================================================================
        "values" => {
            // 1. Let M be the this value.
            // 2. Return ? CreateMapIterator(M, value).
            // TODO: Return a proper MapIterator instead of an array snapshot.
            let values: Vec<JsValue> = entries.iter().map(|(_, v)| *v).collect();
            create_array_from_elements(values)
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// Dispatch a `Set.prototype` method call by name.
///
/// Routes to the appropriate `Set.prototype` or `WeakSet.prototype` method
/// based on the `method` string. Handles `add`, `has`, `delete`, `clear`,
/// `forEach`, `entries`, `keys`, `values`, `size`, and the ES2025 set methods
/// (`union`, `intersection`, `difference`, `symmetricDifference`, `isSubsetOf`,
/// `isSupersetOf`, `isDisjointFrom`).
///
/// For `WeakSet.prototype.add`, validates that the value is an object per
/// [`WeakSet.prototype.add` step 2](https://tc39.es/ecma262/#sec-weakset.prototype.add).
///
/// Per the ES spec, `keys()` is an alias for `values()`.
pub(crate) fn dispatch_set_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);
    let uni = unsafe {
        // SAFETY: caller ensures obj is a unified SetObj/WeakSetObj.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::undefined().raw_bits();
    };
    let is_weak = u.kind == InternalKind::WeakSetObj;
    let Some(InternalData::Set { values }) = u.internal_data_mut() else {
        return JsValue::undefined().raw_bits();
    };
    // WeakSet.prototype.add ( value )
    // https://tc39.es/ecma262/#sec-weakset.prototype.add
    // Step 2. If value is not an Object, throw a TypeError exception.
    if is_weak && method == "add" {
        let val = args.first().copied().unwrap_or(JsValue::undefined());
        if !val.is_object() {
            return throw_type_error_helper("Invalid value used in weak set");
        }
    }
    dispatch_set_method_on_values(values, obj, method, &args)
}

/// Shared implementation for Set methods operating on a values vector.
///
/// Dispatches individual `Set.prototype` methods:
/// - `add` — [spec](https://tc39.es/ecma262/#sec-set.prototype.add)
/// - `has` — [spec](https://tc39.es/ecma262/#sec-set.prototype.has)
/// - `delete` — [spec](https://tc39.es/ecma262/#sec-set.prototype.delete)
/// - `clear` — [spec](https://tc39.es/ecma262/#sec-set.prototype.clear)
/// - `forEach` — [spec](https://tc39.es/ecma262/#sec-set.prototype.foreach)
/// - `entries` — [spec](https://tc39.es/ecma262/#sec-set.prototype.entries)
/// - `keys`/`values` — [spec](https://tc39.es/ecma262/#sec-set.prototype.values)
/// - `size` — [spec](https://tc39.es/ecma262/#sec-get-set.prototype.size)
/// - `union` — [spec](https://tc39.es/ecma262/#sec-set.prototype.union)
/// - `intersection` — [spec](https://tc39.es/ecma262/#sec-set.prototype.intersection)
/// - `difference` — [spec](https://tc39.es/ecma262/#sec-set.prototype.difference)
/// - `symmetricDifference` — [spec](https://tc39.es/ecma262/#sec-set.prototype.symmetricdifference)
/// - `isSubsetOf` — [spec](https://tc39.es/ecma262/#sec-set.prototype.issubsetof)
/// - `isSupersetOf` — [spec](https://tc39.es/ecma262/#sec-set.prototype.issupersetof)
/// - `isDisjointFrom` — [spec](https://tc39.es/ecma262/#sec-set.prototype.isdisjointfrom)
fn dispatch_set_method_on_values(
    values: &mut Vec<nanbox::JsValue>,
    obj: u64,
    method: &str,
    args: &[nanbox::JsValue],
) -> u64 {
    match method {
        // =================================================================
        // Set.prototype.add ( value )
        // https://tc39.es/ecma262/#sec-set.prototype.add
        // =================================================================
        "add" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. For each element e of S.[[SetData]], do
            //    a. If e is not empty and SameValueZero(e, value) is true, then
            //       i. Return S.
            if !values.iter().any(|v| value_ops::strict_eq(*v, val)) {
                // 4. If value is -0, set value to +0.
                // TODO: Step 4 — normalize -0 to +0.
                // 5. Append value to S.[[SetData]].
                values.push(val);
            }
            // 6. Return S.
            obj // Set.add returns the set
        }
        // =================================================================
        // Set.prototype.has ( value )
        // https://tc39.es/ecma262/#sec-set.prototype.has
        // =================================================================
        "has" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. For each element e of S.[[SetData]], do
            //    a. If e is not empty and SameValueZero(e, value) is true,
            //       return true.
            let found = values.iter().any(|v| value_ops::strict_eq(*v, val));
            // 4. Return false.
            JsValue::bool(found).raw_bits()
        }
        // =================================================================
        // Set.prototype.delete ( value )
        // https://tc39.es/ecma262/#sec-set.prototype.delete
        // =================================================================
        "delete" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            let len_before = values.len();
            // 3. For each element e of S.[[SetData]], do
            //    a. If e is not empty and SameValueZero(e, value) is true, then
            //       i. Replace the element of S.[[SetData]] whose value is e with
            //          an element whose value is empty.
            //       ii. Return true.
            values.retain(|v| !value_ops::strict_eq(*v, val));
            // 4. Return false.
            JsValue::bool(values.len() != len_before).raw_bits()
        }
        // =================================================================
        // Set.prototype.clear ( )
        // https://tc39.es/ecma262/#sec-set.prototype.clear
        // =================================================================
        "clear" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            // 3. For each element e of S.[[SetData]], do
            //    a. Replace the element of S.[[SetData]] whose value is e with
            //       an element whose value is empty.
            values.clear();
            // 4. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // get Set.prototype.size
        // https://tc39.es/ecma262/#sec-get-set.prototype.size
        // =================================================================
        "size" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            // 3. Let count be 0.
            // 4. For each element e of S.[[SetData]], do
            //    a. If e is not empty, set count to count + 1.
            // 5. Return F(count).
            JsValue::number(values.len() as f64).raw_bits()
        }
        // =================================================================
        // Set.prototype.forEach ( callbackfn [ , thisArg ] )
        // https://tc39.es/ecma262/#sec-set.prototype.foreach
        // =================================================================
        "forEach" => {
            // 1. Let S be the this value.
            // 2. Perform ? RequireInternalSlot(S, [[SetData]]).
            //    (Handled by caller.)
            // 3. If IsCallable(callbackfn) is false, throw a TypeError exception.
            let callback = args.first().copied().unwrap_or(JsValue::undefined());
            let cb_bits = callback.raw_bits();
            if !is_callable_for_collections(cb_bits) {
                return throw_type_error_helper("Set.prototype.forEach callback is not a function");
            }
            // 4. Let entries be S.[[SetData]].
            // 5. Let numEntries be the number of elements of entries.
            // 6. Let index be 0.
            // 7. Repeat, while index < numEntries,
            for v in values.iter() {
                //    a. Let e be entries[index].
                //    b. Set index to index + 1.
                //    c. If e is not empty, then
                //       i. Perform ? Call(callbackfn, thisArg, « e, e, S »).
                let argv = [v.raw_bits(), v.raw_bits(), obj];
                unsafe {
                    // SAFETY: callback is a closure created by compiled code.
                    super::__esc_rt_call_closure(cb_bits, 3, argv.as_ptr());
                }
                //       ii. NOTE: numEntries must be re-determined each iteration.
                //       iii. Set numEntries to the number of elements of entries.
                // TODO: Steps 7c.ii-iii — handle set mutation during iteration.
            }
            // 8. Return undefined.
            JsValue::undefined().raw_bits()
        }
        // =================================================================
        // Set.prototype.entries ( )
        // https://tc39.es/ecma262/#sec-set.prototype.entries
        // =================================================================
        "entries" => {
            // 1. Let S be the this value.
            // 2. Return ? CreateSetIterator(S, key+value).
            // NOTE: entries() returns [value, value] pairs per spec.
            // We use a values iterator since for-of iteration only needs values.
            let iter = JsIterator::new_set_values(obj);
            TaggedObj::boxed(
                ObjTag::Unified,
                UnifiedObject::iterator(ShapeTable::EMPTY_SHAPE, iter),
            )
        }
        // =================================================================
        // Set.prototype.keys ( )  — alias for Set.prototype.values
        // Set.prototype.values ( )
        // https://tc39.es/ecma262/#sec-set.prototype.values
        // NOTE: %Set.prototype.keys% is %Set.prototype.values% per spec.
        // =================================================================
        "keys" | "values" => {
            // 1. Let S be the this value.
            // 2. Return ? CreateSetIterator(S, value).
            let iter = JsIterator::new_set_values(obj);
            TaggedObj::boxed(
                ObjTag::Unified,
                UnifiedObject::iterator(ShapeTable::EMPTY_SHAPE, iter),
            )
        }
        // =================================================================
        // Set.prototype.union ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.union
        // =================================================================
        "union" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. Let keysIter be ? GetIteratorFromMethod(otherRec.[[Set]], otherRec.[[Keys]]).
            // 5. Let resultSetData be a copy of O.[[SetData]].
            let mut result_values: Vec<JsValue> = values.clone();
            // 6. Let next be NOT-STARTED.
            // 7. Repeat, while next is not DONE,
            //    a. Set next to ? IteratorStepValue(keysIter).
            //    b. If next is not DONE, then
            //       i. If next is -0, set next to +0.
            //       ii. If SetDataHas(resultSetData, next) is false, then
            //           1. Append next to resultSetData.
            for v in &other_values {
                if !result_values.iter().any(|e| value_ops::strict_eq(*e, *v)) {
                    result_values.push(*v);
                }
            }
            // 8. Let result be OrdinaryObjectCreate(%Set.prototype%, « [[SetData]] »).
            // 9. Set result.[[SetData]] to resultSetData.
            // 10. Return result.
            create_set_from_values(result_values)
        }
        // =================================================================
        // Set.prototype.intersection ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.intersection
        // =================================================================
        "intersection" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. Let resultSetData be a new empty List.
            // 5. If SetDataSize(O.[[SetData]]) ≤ otherRec.[[Size]], then
            //    a. Let thisIter be CreateSetIterator(O, value) ... (iterate this, check other.has)
            // 6. Else,
            //    a. Let keysIter be ... (iterate other, check this.has)
            // Simplified: iterate this, keep elements that are also in other.
            let result_values: Vec<JsValue> = values
                .iter()
                .filter(|v| other_values.iter().any(|ov| value_ops::strict_eq(**v, *ov)))
                .copied()
                .collect();
            // 7. Let result be OrdinaryObjectCreate(%Set.prototype%, « [[SetData]] »).
            // 8. Set result.[[SetData]] to resultSetData.
            // 9. Return result.
            create_set_from_values(result_values)
        }
        // =================================================================
        // Set.prototype.difference ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.difference
        // =================================================================
        "difference" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. Let resultSetData be a copy of O.[[SetData]].
            // 5. If SetDataSize(O.[[SetData]]) ≤ otherRec.[[Size]], then
            //    a. Iterate this, remove elements found in other.
            // 6. Else,
            //    a. Iterate other, remove matching elements from resultSetData.
            // Simplified: keep elements in this that are NOT in other.
            let result_values: Vec<JsValue> = values
                .iter()
                .filter(|v| !other_values.iter().any(|ov| value_ops::strict_eq(**v, *ov)))
                .copied()
                .collect();
            // 7. Let result be OrdinaryObjectCreate(%Set.prototype%, « [[SetData]] »).
            // 8. Set result.[[SetData]] to resultSetData.
            // 9. Return result.
            create_set_from_values(result_values)
        }
        // =================================================================
        // Set.prototype.symmetricDifference ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.symmetricdifference
        // =================================================================
        "symmetricDifference" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. Let keysIter be ? GetIteratorFromMethod(otherRec.[[Set]], otherRec.[[Keys]]).
            // 5. Let resultSetData be a copy of O.[[SetData]].
            // 6. Let next be NOT-STARTED.
            // 7. Repeat, while next is not DONE,
            //    a. Set next to ? IteratorStepValue(keysIter).
            //    b. If next is not DONE, then
            //       i. If SetDataHas(resultSetData, next) is true, then
            //          1. Remove next from resultSetData.
            //       ii. Else, append next to resultSetData.
            // Elements in this but NOT in other:
            let mut result_values: Vec<JsValue> = values
                .iter()
                .filter(|v| !other_values.iter().any(|ov| value_ops::strict_eq(**v, *ov)))
                .copied()
                .collect();
            // Elements in other but NOT in this:
            for v in &other_values {
                if !values.iter().any(|sv| value_ops::strict_eq(*sv, *v)) {
                    result_values.push(*v);
                }
            }
            // 8. Let result be OrdinaryObjectCreate(%Set.prototype%, « [[SetData]] »).
            // 9. Set result.[[SetData]] to resultSetData.
            // 10. Return result.
            create_set_from_values(result_values)
        }
        // =================================================================
        // Set.prototype.isSubsetOf ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.issubsetof
        // =================================================================
        "isSubsetOf" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. If SetDataSize(O.[[SetData]]) > otherRec.[[Size]], return false.
            // TODO: Step 4 — fast-path size check.
            // 5. Let thisIter be CreateSetIterator(O, value).
            // 6. Let next be NOT-STARTED.
            // 7. Repeat, while next is not DONE,
            //    a. Set next to ? IteratorStepValue(thisIter).
            //    b. If next is not DONE, then
            //       i. If ? Call(otherRec.[[Has]], otherRec.[[Set]], « next ») is false, then
            //          1. Perform ? IteratorClose(thisIter, NormalCompletion(unused)).
            //          2. Return false.
            let is_subset = values
                .iter()
                .all(|v| other_values.iter().any(|ov| value_ops::strict_eq(*v, *ov)));
            // 8. Return true.
            JsValue::bool(is_subset).raw_bits()
        }
        // =================================================================
        // Set.prototype.isSupersetOf ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.issupersetof
        // =================================================================
        "isSupersetOf" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. If SetDataSize(O.[[SetData]]) < otherRec.[[Size]], return false.
            // TODO: Step 4 — fast-path size check.
            // 5. Let keysIter be ? GetIteratorFromMethod(otherRec.[[Set]], otherRec.[[Keys]]).
            // 6. Let next be NOT-STARTED.
            // 7. Repeat, while next is not DONE,
            //    a. Set next to ? IteratorStepValue(keysIter).
            //    b. If next is not DONE, then
            //       i. If SetDataHas(O.[[SetData]], next) is false, then
            //          1. Perform ? IteratorClose(keysIter, NormalCompletion(unused)).
            //          2. Return false.
            let is_superset = other_values
                .iter()
                .all(|ov| values.iter().any(|v| value_ops::strict_eq(*v, *ov)));
            // 8. Return true.
            JsValue::bool(is_superset).raw_bits()
        }
        // =================================================================
        // Set.prototype.isDisjointFrom ( other )
        // https://tc39.es/ecma262/#sec-set.prototype.isdisjointfrom
        // =================================================================
        "isDisjointFrom" => {
            // 1. Let O be the this value.
            // 2. Perform ? RequireInternalSlot(O, [[SetData]]).
            //    (Handled by caller.)
            let other_arg = args.first().copied().unwrap_or(JsValue::undefined());
            // 3. Let otherRec be ? GetSetRecord(other).
            // TODO: Step 3 — full GetSetRecord (coerce Set-like objects).
            let other_values = extract_set_values_from_arg(other_arg.raw_bits());
            // 4. If SetDataSize(O.[[SetData]]) ≤ otherRec.[[Size]], then
            //    a. Let thisIter be CreateSetIterator(O, value).
            //    b. Repeat for each element: if other.has(e), return false.
            // 5. Else,
            //    a. Let keysIter be ... iterate other, check this.has(e).
            // Simplified: check if any element of this exists in other.
            let is_disjoint = !values
                .iter()
                .any(|v| other_values.iter().any(|ov| value_ops::strict_eq(*v, *ov)));
            // 6. Return true.
            JsValue::bool(is_disjoint).raw_bits()
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Check if a NaN-boxed value is callable (has `[[Call]]` internal method).
///
/// Returns `true` for closures, functions, and native functions.
fn is_callable_for_collections(bits: u64) -> bool {
    use crate::tagged_obj::deref_tagged;
    let tag = crate::tagged_obj::read_obj_tag(bits);
    if tag != Some(ObjTag::Unified as u8) {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    uni.is_some_and(|u| u.is_callable())
}

/// Throw a TypeError with the given message string.
///
/// Sets the pending exception and returns `undefined` raw bits.
fn throw_type_error_helper(msg: &str) -> u64 {
    let err_msg = super::make_rt_string(msg.to_string());
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, err_msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Extract Set values from an argument that may be a Set object.
///
/// If the argument is a Set, returns a clone of its values. Otherwise returns
/// an empty vector. This is a simplified version of the spec's `GetSetRecord`
/// abstract operation — it only handles actual Set objects, not arbitrary
/// Set-like objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-getsetrecord
fn extract_set_values_from_arg(bits: u64) -> Vec<JsValue> {
    let uni = unsafe {
        // SAFETY: caller ensures bits is a valid tagged pointer or non-object.
        deref_tagged::<UnifiedObject>(bits)
    };
    let Some(u) = uni else {
        return Vec::new();
    };
    if u.kind != InternalKind::SetObj {
        return Vec::new();
    }
    if let Some(InternalData::Set { values }) = u.internal_data() {
        values.clone()
    } else {
        Vec::new()
    }
}

/// Create a new Set from the given values vector.
///
/// Returns the NaN-boxed Set object. Used by the ES2025 Set composition
/// methods (`union`, `intersection`, `difference`, `symmetricDifference`)
/// to construct result Sets.
fn create_set_from_values(values: Vec<JsValue>) -> u64 {
    let mut set_obj = UnifiedObject::set(ShapeTable::EMPTY_SHAPE);
    if let Some(InternalData::Set { values: set_values }) = set_obj.internal_data_mut() {
        *set_values = values;
    }
    TaggedObj::boxed(ObjTag::Unified, set_obj)
}
