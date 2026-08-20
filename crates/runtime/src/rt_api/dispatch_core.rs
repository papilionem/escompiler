//! Core function call dispatch, constructors, and Function.prototype methods.
//!
//! Contains `__esc_rt_call_closure`, `__esc_rt_call_indirect`, `__esc_rt_call_method`,
//! `__esc_rt_call_new`, `__esc_rt_super_call`, URI encoding/decoding helpers,
//! and `Function.prototype.call/apply/bind` implementations.

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut, read_obj_tag};
use crate::{exceptions, string_ops, value_ops};

use super::{
    __esc_dispatch, __esc_rt_create_error, __esc_rt_create_map, __esc_rt_create_object,
    __esc_rt_create_proxy, __esc_rt_create_regexp, __esc_rt_create_set, __esc_rt_create_symbol,
    __esc_rt_create_weakmap, __esc_rt_create_weakref, __esc_rt_create_weakset, __esc_rt_get_prop,
    __esc_rt_set_prop, __esc_rt_throw, __esc_rt_to_boolean, __esc_rt_to_number, __esc_rt_to_string,
    CURRENT_ARGC, CURRENT_ARGV, CURRENT_CALLEE, CURRENT_CLOSURE_ENV, CURRENT_NEW_TARGET,
    CURRENT_THIS, create_array_from_elements, create_empty_array, dispatch_array_method,
    dispatch_math_method, dispatch_number_instance_method, dispatch_number_static_method,
    dispatch_object_static_method, dispatch_string_method, extract_key_string,
    is_array_prototype_method, make_rt_string, read_argv,
};

// =========================================================================
// Unified object helpers
// =========================================================================

/// Extract closure data (func_idx, env) from a `UnifiedObject` with
/// `InternalKind::Closure` / `InternalKind::Function`.
///
/// Returns `None` if the value is not a closure/function.
pub(crate) fn extract_closure_data(bits: u64) -> Option<(u32, u64)> {
    let tag = read_obj_tag(bits)?;
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) }?;
        if (uni.kind == InternalKind::Closure || uni.kind == InternalKind::Function)
            && let Some(InternalData::Function { code_idx, env, .. }) = uni.internal_data()
        {
            return Some((*code_idx, *env));
        }
    }
    None
}

/// Extract the `is_strict` and `is_arrow` flags from a closure's internal data.
///
/// Returns `(is_strict, is_arrow)`, defaulting to `(false, false)` when the
/// value is not a closure/function.
fn extract_closure_flags(bits: u64) -> (bool, bool) {
    let Some(tag) = read_obj_tag(bits) else {
        return (false, false);
    };
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni
            && (u.kind == InternalKind::Closure || u.kind == InternalKind::Function)
            && let Some(InternalData::Function {
                is_strict,
                is_arrow,
                ..
            }) = u.internal_data()
        {
            return (*is_strict, *is_arrow);
        }
    }
    (false, false)
}

/// Register a prototype on a newly constructed object using the shape mechanism.
///
/// Sets the prototype shape on the object's current shape and registers the
/// prototype object bits in the PROTO_OBJECTS registry.
pub(crate) fn set_prototype_on_new_object(obj_bits: u64, proto_bits: u64) {
    let tag = read_obj_tag(obj_bits);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj_bits)
    };
    let Some(u) = uni else { return };

    super::SHAPES.with(|shapes| {
        let mut shapes = shapes.borrow_mut();
        let proto_shape_id = shapes::ShapeId(shapes.shape_count() as u32);
        let new_shape_id = shapes.set_prototype(u.shape_id, proto_shape_id);
        u.shape_id = new_shape_id;
        if let Some(sid) = shapes.get_prototype(new_shape_id) {
            super::PROTO_OBJECTS.with(|protos| {
                protos.borrow_mut().insert(sid, proto_bits);
            });
        }
    });
}

/// Check if a NaN-boxed value is callable.
///
/// Returns `true` if the value is a `UnifiedObject` with the callable flag set.
pub(crate) fn is_callable(bits: u64) -> bool {
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

/// Check if a NaN-boxed value is a Proxy wrapping a callable target.
///
/// Returns `true` if the value is a `UnifiedObject` with `InternalKind::Proxy`
/// and the proxy target (transitively) is callable.
fn is_callable_proxy(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    let Some(u) = uni else { return false };
    // Proxy objects are callable (dispatch will check target callability)
    u.kind == InternalKind::Proxy
}

/// Check if a NaN-boxed value is a Proxy wrapping a constructable target.
///
/// Returns `true` if the value is a `UnifiedObject` with `InternalKind::Proxy`.
/// The actual constructability of the target is verified inside `proxy_construct`.
fn is_constructable_proxy(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    let Some(u) = uni else { return false };
    u.kind == InternalKind::Proxy
}

/// Type alias for a native function pointer (takes context, returns result).
pub(crate) type NativeFn = fn(u64) -> u64;

/// Extract native function data (func, context) from a `UnifiedObject`
/// with `InternalKind::NativeFunc`.
///
/// Returns `None` if the value is not a native function.
pub(crate) fn extract_native_func(bits: u64) -> Option<(NativeFn, u64)> {
    let tag = read_obj_tag(bits)?;
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) }?;
        if uni.kind == InternalKind::NativeFunc
            && let Some(InternalData::NativeFunc { func, context }) = uni.internal_data()
        {
            return Some((*func, *context));
        }
    }
    None
}

/// Check if a NaN-boxed closure value represents a generator function.
///
/// Checks both the `InternalData::Function { is_generator, .. }` flag and
/// the fallback `__is_generator` JS property set by desugar.
fn is_generator_function(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni
            && (u.kind == InternalKind::Closure || u.kind == InternalKind::Function)
            && let Some(InternalData::Function {
                is_generator: true, ..
            }) = u.internal_data()
        {
            return true;
        }
    }
    // Fallback: check for __is_generator JS property set by desugar
    let gen_key = make_rt_string("__is_generator".to_string());
    let gen_val = __esc_rt_get_prop(bits, gen_key);
    gen_val == JsValue::bool(true).raw_bits()
}

/// Check if a NaN-boxed closure value represents a class constructor.
///
/// A class constructor is identified by having a `__is_class_constructor`
/// property set to `true` by the desugar layer, or by having a `prototype`
/// property and not being an arrow function (heuristic for `class` syntax).
///
/// Per §10.2.1 step 3, class constructors cannot be called without `new`.
fn is_class_constructor(bits: u64) -> bool {
    // Check for __is_class_constructor JS property set by desugar
    let key = make_rt_string("__is_class_constructor".to_string());
    let val = __esc_rt_get_prop(bits, key);
    if val == JsValue::bool(true).raw_bits() {
        return true;
    }
    false
}

// =========================================================================
// Closure / indirect call dispatch
// =========================================================================

/// Implements the internal `[[Call]]` mechanism for user-defined closures.
///
/// `[[Call]] ( thisArgument, argumentsList )`
///
/// Extracts func_idx + env and dispatches through `__esc_dispatch`.
/// Sets the thread-local closure environment before the call and restores it after.
/// For sloppy (non-strict, non-arrow) functions, substitutes `globalThis` for an
/// `undefined` or zero `this` value per the sloppy-mode `this` substitution rule.
///
/// [spec]: https://tc39.es/ecma262/#sec-ecmascript-function-objects-call-thisargument-argumentslist
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_call_closure(closure: u64, argc: u32, argv: *const u64) -> u64 {
    // 1. Let callerContext be the running execution context.
    // (Implicit — we use thread-locals for execution context state.)

    // 2. Let calleeContext be PrepareForOrdinaryCall(F, undefined).
    // 2a. Extract the closure's code index and environment.
    let Some((code_idx, env)) = extract_closure_data(closure) else {
        return JsValue::undefined().raw_bits();
    };

    let func_idx = code_idx as i32;

    // 3. If F.[[IsClassConstructor]] is true, throw a TypeError.
    // Check if this closure is a class constructor being called without `new`.
    // Class constructors have a `prototype` property but are not arrow functions
    // and new.target must be set for valid [[Construct]] calls.
    let new_target = CURRENT_NEW_TARGET.with(|cell| cell.get());
    if new_target == 0 && is_class_constructor(closure) {
        let fn_name = get_function_name(closure);
        let display_name = if fn_name.is_empty() {
            "anonymous".to_string()
        } else {
            fn_name
        };
        let msg = make_rt_string(format!(
            "TypeError: Class constructor {display_name} cannot be invoked without 'new'"
        ));
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 4. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument).
    // Per ES2024 §10.2.1.2 OrdinaryCallBindThis:
    // - Strict mode, no explicit receiver: this = undefined
    // - Sloppy mode, no explicit receiver: this = globalThis
    // - Explicit receiver (method call, .call/.apply): this = receiver
    let (is_strict, is_arrow) = extract_closure_flags(closure);
    let this_was_set = super::THIS_EXPLICITLY_SET.with(|cell| {
        let was = cell.get();
        cell.set(false); // Reset for next call
        was
    });
    let prev_this = if is_strict && !is_arrow && !this_was_set {
        // Strict mode without explicit receiver: this = undefined
        Some(CURRENT_THIS.with(|cell| cell.replace(0)))
    } else if !is_strict && !is_arrow {
        let current_this = CURRENT_THIS.with(|cell| cell.get());
        let this_val = JsValue::from_raw_bits(current_this);
        if (this_val.is_undefined() || current_this == 0) && !this_was_set {
            // Sloppy mode without receiver: substitute globalThis
            let global = super::__esc_rt_get_global_this();
            Some(CURRENT_THIS.with(|cell| cell.replace(global)))
        } else {
            None
        }
    } else {
        None
    };

    // Save and set closure environment, callee, and call args
    let prev_env = CURRENT_CLOSURE_ENV.with(|cell| cell.replace(env));
    let prev_callee = CURRENT_CALLEE.with(|cell| cell.replace(closure));
    let prev_argc = CURRENT_ARGC.with(|cell| cell.replace(argc));
    let prev_argv = CURRENT_ARGV.with(|cell| cell.replace(argv));

    // 5. Let result be Completion(OrdinaryCallEvaluateBody(F, argumentsList)).
    let result = unsafe {
        // SAFETY: __esc_dispatch is generated by Cranelift and linked in the final binary.
        // argv validity is guaranteed by the caller's contract.
        __esc_dispatch(func_idx, argc as i32, argv)
    };

    // 6. Remove calleeContext from the execution context stack and restore callerContext.
    CURRENT_CLOSURE_ENV.with(|cell| cell.set(prev_env));
    CURRENT_CALLEE.with(|cell| cell.set(prev_callee));
    CURRENT_ARGC.with(|cell| cell.set(prev_argc));
    CURRENT_ARGV.with(|cell| cell.set(prev_argv));

    // Restore CURRENT_THIS if we substituted globalThis
    if let Some(old_this) = prev_this {
        CURRENT_THIS.with(|cell| cell.set(old_this));
    }

    // 7. Return Completion(result).
    result
}

/// Implements the abstract operation `Call ( F, V [ , argumentsList ] )`.
///
/// Indirect function call: dispatches based on the callee value type.
///
/// - If callee is a unified closure/function: delegates to `__esc_rt_call_closure`
/// - If callee is an integer (function index): dispatches directly through `__esc_dispatch`
/// - If callee is a native function: calls the native function directly
/// - Otherwise: throws TypeError
///
/// Clears `CURRENT_NEW_TARGET` for the duration of the call so that called
/// functions see `new.target === undefined` (only `__esc_rt_call_new` sets it).
///
/// Fallback argument buffer for calls with `argc == 0` and `argv == null`.
/// Each slot is the NaN-boxed `undefined` constant (QNAN | tag 0x0004 << 48).
/// Compiled functions read up to their declared arity from argv regardless of
/// argc, so a null pointer must never reach generated code.
static NULL_ARGV_FALLBACK: [u64; 16] = [0x7FF8_0004_0000_0000_u64; 16];

/// [spec]: https://tc39.es/ecma262/#sec-call
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_call_indirect(callee: u64, argc: i32, argv: *const u64) -> u64 {
    // 1. If argumentsList is not present, set argumentsList to a new empty List.
    // (Handled by caller — argc/argv represent the argument list.)

    // Null-argv guard: compiled functions unconditionally read up to their
    // declared arity from argv (missing params are padded with undefined by
    // the dispatch trampoline). A null argv would dereference address 0.
    // Accessor getters are invoked with argc=0/argv=null but may declare
    // formal parameters (e.g., `function(arg1) {}`), so substitute a static
    // undefined buffer. 16 slots cover any realistic getter arity.
    let argv = if argv.is_null() && argc == 0 {
        NULL_ARGV_FALLBACK.as_ptr()
    } else {
        argv
    };

    // 2. If IsCallable(F) is false, throw a TypeError exception.
    // (Checked in call_indirect_inner — non-callable falls through to TypeError.)

    // Save and clear new.target — regular [[Call]] must see new.target === undefined
    let prev_new_target = CURRENT_NEW_TARGET.with(|cell| cell.replace(0));

    // 3. Return ? F.[[Call]](V, argumentsList).
    let result = unsafe { call_indirect_inner(callee, argc, argv) };

    // Restore previous new.target
    CURRENT_NEW_TARGET.with(|cell| cell.set(prev_new_target));

    result
}

/// Inner dispatch logic for `[[Call]]` (separated to allow save/restore
/// of `CURRENT_NEW_TARGET` in the outer wrapper).
///
/// Implements the multi-type dispatch for `F.[[Call]](V, argumentsList)`:
/// - ECMAScript function objects: §10.2.1
/// - Built-in function objects: §10.3.1
/// - Proxy exotic objects: §10.5.12
///
/// [spec]: https://tc39.es/ecma262/#sec-ecmascript-function-objects-call-thisargument-argumentslist
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn call_indirect_inner(callee: u64, argc: i32, argv: *const u64) -> u64 {
    // Dispatch path 1: ECMAScript closure/function → [[Call]] per §10.2.1
    if extract_closure_data(callee).is_some() {
        if is_generator_function(callee) {
            // Generator functions: §27.3.3 GeneratorFunction [[Call]]
            // SAFETY: callee is a closure (verified above), argc/argv are valid
            // per the caller's contract on __esc_rt_call_indirect.
            return unsafe { super::create_generator_from_closure(callee, argc as u32, argv) };
        }
        // SAFETY: callee is a closure (verified above), argc/argv are valid
        // per the caller's contract on __esc_rt_call_indirect.
        return unsafe { __esc_rt_call_closure(callee, argc as u32, argv) };
    }

    // Dispatch path 2: Direct function index (integer) — internal AOT dispatch
    let v = JsValue::from_raw_bits(callee);
    if let Some(idx) = v.as_int() {
        let prev_argc = CURRENT_ARGC.with(|cell| cell.replace(argc as u32));
        let prev_argv = CURRENT_ARGV.with(|cell| cell.replace(argv));
        let result = unsafe {
            // SAFETY: __esc_dispatch is generated by Cranelift and linked in the final binary.
            __esc_dispatch(idx, argc, argv)
        };
        CURRENT_ARGC.with(|cell| cell.set(prev_argc));
        CURRENT_ARGV.with(|cell| cell.set(prev_argv));
        return result;
    }

    // Dispatch path 3: Built-in function by name → §10.3.1
    if v.is_string() {
        let name = string_ops::get_string_data(v);
        // SAFETY: argv is valid per the caller's contract on __esc_rt_call_indirect.
        return unsafe { call_builtin_function(&name, argc as u32, argv) };
    }

    // Dispatch path 4: Native function (includes bound functions) → §10.3.1
    if let Some((func, context)) = extract_native_func(callee) {
        let prev_argc = CURRENT_ARGC.with(|cell| cell.replace(argc as u32));
        let prev_argv = CURRENT_ARGV.with(|cell| cell.replace(argv));
        let result = func(context);
        CURRENT_ARGC.with(|cell| cell.set(prev_argc));
        CURRENT_ARGV.with(|cell| cell.set(prev_argv));
        return result;
    }

    // Dispatch path 5: Proxy exotic object → §10.5.12 [[Call]]
    if is_callable_proxy(callee) {
        let this_arg = CURRENT_THIS.with(|cell| cell.get());
        let args = if argc > 0 && !argv.is_null() {
            // SAFETY: argc > 0 and argv is non-null per caller's contract.
            unsafe { std::slice::from_raw_parts(argv, argc as usize) }
        } else {
            &[]
        };
        match crate::proxy::proxy_call(callee, this_arg, args) {
            Ok(result) => return result,
            Err(e) => {
                let msg = make_rt_string(e.to_string());
                let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                __esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }
        }
    }

    // Not callable — throw TypeError per §7.3.13 Call, step 2:
    // "If IsCallable(F) is false, throw a TypeError exception."
    let type_desc = value_ops::js_typeof(JsValue::from_raw_bits(callee));
    let msg = make_rt_string(format!("TypeError: {type_desc} is not a function"));
    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    __esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Internal runtime helper for spread call: `func(...args)`.
///
/// Implements the `EvaluateCall` + `ArgumentListEvaluation` path for spread
/// arguments. Extracts elements from `args_array` and delegates to
/// `__esc_rt_call_indirect` (the `Call` abstract operation).
///
/// [spec]: https://tc39.es/ecma262/#sec-evaluatecall
///
/// # Safety
///
/// `callee` must be a callable value (closure, function index, or string).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_apply(callee: u64, args_array: u64) -> u64 {
    // §12.3.6.2 EvaluateCall: Let argList be ? ArgumentListEvaluation of arguments.
    // Extract elements from the array (unified path only)
    if let Some(tag) = read_obj_tag(args_array)
        && tag == ObjTag::Unified as u8
    {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(args_array)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
        {
            let argc = u.array_len() as i32;
            let argv: Vec<u64> = u
                .array_elements_resolved()
                .iter()
                .map(|v| v.raw_bits())
                .collect();
            // Call(func, thisValue, argList)
            return unsafe {
                // SAFETY: argv is valid for argc elements.
                __esc_rt_call_indirect(callee, argc, argv.as_ptr())
            };
        }
    }
    // Not an array — call with zero args
    // SAFETY: passing null argv with argc=0 is valid per __esc_rt_call_indirect's contract.
    unsafe { __esc_rt_call_indirect(callee, 0, std::ptr::null()) }
}

/// Internal runtime helper for spread construction: `new F(...args)`.
///
/// Implements the `EvaluateNew` + `ArgumentListEvaluation` path for spread
/// arguments in `new` expressions. Extracts elements from `args_array` and
/// delegates to `__esc_rt_call_new` (the `Construct` abstract operation).
///
/// [spec]: https://tc39.es/ecma262/#sec-evaluatenew
///
/// # Safety
///
/// `callee` must be a constructable value (closure, function index, or string).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_apply_new(callee: u64, args_array: u64) -> u64 {
    // §13.3.5.1.1 EvaluateNew: Let argList be ? ArgumentListEvaluation of arguments.
    // Extract elements from the array (unified path only)
    if let Some(tag) = read_obj_tag(args_array)
        && tag == ObjTag::Unified as u8
    {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(args_array)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
        {
            let argc = u.array_len();
            let argv: Vec<u64> = u
                .array_elements_resolved()
                .iter()
                .map(|v| v.raw_bits())
                .collect();
            // Construct(constructor, argList)
            return unsafe {
                // SAFETY: argv is valid for argc elements.
                __esc_rt_call_new(callee, argc, argv.as_ptr())
            };
        }
    }
    // Not an array — call with zero args
    // SAFETY: passing null argv with argc=0 is valid per __esc_rt_call_new's contract.
    unsafe { __esc_rt_call_new(callee, 0, std::ptr::null()) }
}

/// Internal runtime helper for spread method call: `obj.method(...args)`.
///
/// Mirrors `__esc_rt_apply` (plain spread call) but preserves the receiver:
/// it extracts elements from `args_array` and delegates to
/// `__esc_rt_call_method`, which implements `EvaluateCall` with `obj` as the
/// `this` value.
///
/// [spec]: https://tc39.es/ecma262/#sec-evaluatecall
///
/// # Safety
///
/// `args_array` must be a valid JSValue; when it is not an array the call is
/// made with zero arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_apply_method(obj: u64, key: u64, args_array: u64) -> u64 {
    // §12.3.6.2 EvaluateCall: Let argList be ? ArgumentListEvaluation of arguments.
    // Extract elements from the array (unified path only)
    if let Some(tag) = read_obj_tag(args_array)
        && tag == ObjTag::Unified as u8
    {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(args_array)
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
        {
            let argc = u.array_len();
            let argv: Vec<u64> = u
                .array_elements_resolved()
                .iter()
                .map(|v| v.raw_bits())
                .collect();
            // Call(func, thisValue=obj, argList)
            return unsafe {
                // SAFETY: argv is valid for argc elements.
                __esc_rt_call_method(obj, key, argc, argv.as_ptr())
            };
        }
    }
    // Not an array — call with zero args
    // SAFETY: passing null argv with argc=0 is valid per __esc_rt_call_method's contract.
    unsafe { __esc_rt_call_method(obj, key, 0, std::ptr::null()) }
}

/// Dispatch a built-in global function call by name.
///
/// Handles `Symbol(...)`, `Number(...)`, `String(...)`, `Boolean(...)`,
/// `parseInt(...)`, `parseFloat(...)`, `isNaN(...)`, `isFinite(...)`,
/// `encodeURI(...)`, `decodeURI(...)`, `encodeURIComponent(...)`,
/// `decodeURIComponent(...)`, `Function(...)`, and constructor-only guards.
///
/// Each branch implements the "called as a function" path for the corresponding
/// global built-in per the ES2024 spec.
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn call_builtin_function(name: &str, argc: u32, argv: *const u64) -> u64 {
    let arg0 = || -> u64 {
        if argc > 0 && !argv.is_null() {
            // SAFETY: argc > 0 and argv is non-null, so argv[0] is valid
            // per the caller's contract on call_builtin_function.
            unsafe { *argv }
        } else {
            JsValue::undefined().raw_bits()
        }
    };
    match name {
        // §20.4.1.1 Symbol ( [ description ] )
        // 1. If NewTarget is not undefined, throw a TypeError exception.
        // (Handled by __esc_rt_call_new guard — here we are in the [[Call]] path.)
        // 2-4. Create and return a new unique Symbol.
        "Symbol" => __esc_rt_create_symbol(arg0()),

        // §21.1.1.1 Number ( value )
        // When Number is called with argument value:
        // 1. If value is not present, let n be +0.
        // 2. Else, let n be ? ToNumeric(value).
        // 3. If NewTarget is undefined, return n.
        "Number" => {
            if argc == 0 {
                // Step 1: no argument → +0
                JsValue::number(0.0).raw_bits()
            } else {
                // Step 2: ToNumeric(value)
                __esc_rt_to_number(arg0())
            }
        }

        // §22.1.1.1 String ( value )
        // 1. If value is not present, let s be "".
        // 2. Else, if NewTarget is undefined and value is a Symbol, return SymbolDescriptiveString(value).
        // 3. Else, let s be ? ToString(value).
        // 4. If NewTarget is undefined, return s.
        "String" => __esc_rt_to_string(arg0()),

        // §20.3.1.1 Boolean ( value )
        // 1. Let b be ToBoolean(value).
        // 2. If NewTarget is undefined, return b.
        "Boolean" => {
            let b = __esc_rt_to_boolean(arg0());
            JsValue::bool(b != 0).raw_bits()
        }

        // §19.2.5 parseInt ( string, radix )
        "parseInt" => {
            // 1. Let inputString be ? ToString(string).
            // Use __esc_rt_to_string which invokes ToPrimitive for objects,
            // ensuring custom valueOf/toString methods are honoured.
            let val = JsValue::from_raw_bits(arg0());
            let str_bits = super::__esc_rt_to_string(val.raw_bits());
            // If ToString threw (e.g., Symbol or ToPrimitive error), propagate.
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            // 2. Let R be ? ToInt32(radix).
            // ToInt32 per §7.1.7: NaN, ±Infinity, ±0 all become 0 (auto-detect).
            let radix_arg = if argc > 1 && !argv.is_null() {
                // SAFETY: argc > 1 and argv is non-null per contract.
                let r = unsafe { *argv.add(1) };
                let rv = value_ops::to_number(JsValue::from_raw_bits(r));
                if rv.is_nan() || rv.is_infinite() || rv == 0.0 {
                    0i32
                } else {
                    let int_val = (rv.signum() * rv.abs().floor()) as i64;
                    (((int_val as u64) % (1u64 << 32)) as u32) as i32
                }
            } else {
                0 // auto-detect
            };
            // Steps 3-12: Parse integer from string with given radix.
            let result = es_parse_int(&s, radix_arg);
            JsValue::number(result).raw_bits()
        }

        // §19.2.4 parseFloat ( string )
        "parseFloat" => {
            // 1. Let inputString be ? ToString(string).
            // Use __esc_rt_to_string which invokes ToPrimitive for objects.
            let val = JsValue::from_raw_bits(arg0());
            let str_bits = super::__esc_rt_to_string(val.raw_bits());
            // If ToString threw, propagate.
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            // 2-4: Parse the longest valid numeric prefix.
            let result = es_parse_float(&s);
            JsValue::number(result).raw_bits()
        }

        // §19.2.3 isNaN ( number )
        // 1. Let num be ? ToNumber(number).
        // 2. If num is NaN, return true.
        // 3. Otherwise, return false.
        "isNaN" => {
            let n = value_ops::to_number(JsValue::from_raw_bits(arg0()));
            // If ToNumber threw (e.g. Symbol, non-callable @@toPrimitive), propagate.
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            JsValue::bool(n.is_nan()).raw_bits()
        }

        // §19.2.2 isFinite ( number )
        // 1. Let num be ? ToNumber(number).
        // 2. If num is not finite, return false.
        // 3. Otherwise, return true.
        "isFinite" => {
            let n = value_ops::to_number(JsValue::from_raw_bits(arg0()));
            // If ToNumber threw (e.g. Symbol, non-callable @@toPrimitive), propagate.
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            JsValue::bool(n.is_finite()).raw_bits()
        }

        // §19.2.6.4 encodeURI ( uri )
        // 1. Let uriString be ? ToString(uri).
        // 2. Let unescapedURISet be a String containing uriReserved, uriUnescaped, and "#".
        // 3. Return ? Encode(uriString, unescapedURISet).
        "encodeURI" => {
            // Use __esc_rt_to_string which invokes ToPrimitive (handles Symbol throws, etc.)
            let str_bits = super::__esc_rt_to_string(arg0());
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            match es_encode_uri(&s) {
                Ok(encoded) => super::make_rt_string(encoded),
                Err(msg) => {
                    let err_msg = super::make_rt_string(msg);
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::URI_ERROR,
                        err_msg,
                    );
                    super::__esc_rt_throw(err);
                    JsValue::undefined().raw_bits()
                }
            }
        }

        // §19.2.6.1 decodeURI ( encodedURI )
        // 1. Let uriString be ? ToString(encodedURI).
        // 2. Let reservedURISet be a String containing uriReserved and "#".
        // 3. Return ? Decode(uriString, reservedURISet).
        "decodeURI" => {
            // Use __esc_rt_to_string which invokes ToPrimitive (handles Symbol throws, etc.)
            let str_bits = super::__esc_rt_to_string(arg0());
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            match es_decode_uri(&s) {
                Ok(decoded) => super::make_rt_string(decoded),
                Err(msg) => {
                    let err_msg = super::make_rt_string(msg);
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::URI_ERROR,
                        err_msg,
                    );
                    super::__esc_rt_throw(err);
                    JsValue::undefined().raw_bits()
                }
            }
        }

        // §19.2.6.5 encodeURIComponent ( uriComponent )
        // 1. Let componentString be ? ToString(uriComponent).
        // 2. Let unescapedURIComponentSet be a String containing uriUnescaped.
        // 3. Return ? Encode(componentString, unescapedURIComponentSet).
        "encodeURIComponent" => {
            // Use __esc_rt_to_string which invokes ToPrimitive (handles Symbol throws, etc.)
            let str_bits = super::__esc_rt_to_string(arg0());
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            match es_encode_uri_component(&s) {
                Ok(encoded) => super::make_rt_string(encoded),
                Err(msg) => {
                    let err_msg = super::make_rt_string(msg);
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::URI_ERROR,
                        err_msg,
                    );
                    super::__esc_rt_throw(err);
                    JsValue::undefined().raw_bits()
                }
            }
        }

        // §19.2.6.2 decodeURIComponent ( encodedURIComponent )
        // 1. Let componentString be ? ToString(encodedURIComponent).
        // 2. Let reservedURIComponentSet be the empty String.
        // 3. Return ? Decode(componentString, reservedURIComponentSet).
        "decodeURIComponent" => {
            // Use __esc_rt_to_string which invokes ToPrimitive (handles Symbol throws, etc.)
            let str_bits = super::__esc_rt_to_string(arg0());
            if exceptions::is_exception() {
                return JsValue::undefined().raw_bits();
            }
            let s = string_ops::get_string_data(JsValue::from_raw_bits(str_bits));
            match es_decode_uri_component(&s) {
                Ok(decoded) => super::make_rt_string(decoded),
                Err(msg) => {
                    let err_msg = super::make_rt_string(msg);
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::URI_ERROR,
                        err_msg,
                    );
                    super::__esc_rt_throw(err);
                    JsValue::undefined().raw_bits()
                }
            }
        }

        // §20.2.1.1 Function ( ...parameterArgs, bodyArg )
        // "If NewTarget is undefined, ... set NewTarget to the active function object."
        // Calling Function() without new works identically per spec.
        "Function" => {
            // SAFETY: argv validity guaranteed by caller's contract on call_builtin_function.
            unsafe { super::construct_function(argc, argv) }
        }

        // §24.1.2 Map, §24.2.2 Set, §24.3.2 WeakMap, §24.4.2 WeakSet, §26.1.2 WeakRef
        // These constructors require `new` — calling without it is a TypeError.
        "Map" | "Set" | "WeakMap" | "WeakSet" | "WeakRef" => {
            let msg = format!("Constructor {name} requires 'new'");
            let err_msg = super::make_rt_string(msg);
            let err =
                super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, err_msg);
            super::__esc_rt_throw(err);
            JsValue::undefined().raw_bits()
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// `parseInt ( string, radix )`
///
/// Parses a string argument and returns an integer of the specified radix.
///
/// [spec]: https://tc39.es/ecma262/#sec-parseint-string-radix
pub(crate) fn es_parse_int(s: &str, radix_arg: i32) -> f64 {
    // 1. Let inputString be ? ToString(string).
    // (Already done by caller — `s` is the string representation.)

    // 2. Let S be ! TrimString(inputString, start).
    let trimmed = s.trim();

    // 3. If S is the empty String, return NaN.
    if trimmed.is_empty() {
        return f64::NAN;
    }
    let mut chars = trimmed.chars().peekable();

    // 4. Let sign be 1.
    // 5. If S is not empty and the first code unit of S is U+002D (HYPHEN-MINUS), then
    //    a. Set sign to -1.
    // 6. If S is not empty and the first code unit is U+002B (PLUS SIGN) or U+002D, then
    //    a. Remove the first code unit from S.
    let negative = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };

    // 7. Let R be ? ToInt32(radix).
    // 8. Let stripPrefix be true.
    // 9. If R != 0, then
    //    a. If R < 2 or R > 36, return NaN.
    //    b. If R != 16, set stripPrefix to false.
    // 10. Else, set R to 10.
    let mut radix = if radix_arg == 0 { 10 } else { radix_arg };
    if !(2..=36).contains(&radix) {
        if radix_arg != 0 {
            return f64::NAN;
        }
        radix = 10;
    }

    // 11. If stripPrefix is true, then
    //     a. If the length of S >= 2 and the first two code units are "0x" or "0X", then
    //        i. Remove the first two code units from S.
    //        ii. Set R to 16.
    if (radix == 16 || radix_arg == 0) && chars.peek() == Some(&'0') {
        let mut look = chars.clone();
        look.next();
        if let Some('x' | 'X') = look.peek() {
            chars.next(); // skip '0'
            chars.next(); // skip 'x'/'X'
            radix = 16;
        }
    }

    // 12. If S contains a code unit that is not a radix-R digit, let end be
    //     the index of the first such code unit; otherwise let end be the length of S.
    // 13. Let Z be the substring of S from 0 to end.
    // 14. If Z is the empty String, return NaN.
    // 15. Let mathInt be the mathematical integer value represented by Z in radix-R notation.
    let mut result: f64 = 0.0;
    let mut parsed_any = false;
    for ch in chars {
        let digit = match ch.to_digit(radix as u32) {
            Some(d) => d,
            None => break,
        };
        parsed_any = true;
        result = result * (radix as f64) + digit as f64;
    }
    if !parsed_any {
        return f64::NAN;
    }

    // 16. If mathInt = 0, then
    //     a. If sign = -1, return -0.
    //     b. Return +0.
    // 17. Return sign * mathInt.
    if negative { -result } else { result }
}

/// `parseFloat ( string )`
///
/// Parses a string argument and returns a floating-point number.
///
/// [spec]: https://tc39.es/ecma262/#sec-parsefloat-string
pub(crate) fn es_parse_float(s: &str) -> f64 {
    // 1. Let inputString be ? ToString(string).
    // (Already done by caller — `s` is the string representation.)

    // 2. Let trimmedString be ! TrimString(inputString, start).
    let trimmed = s.trim();

    // 3. If neither trimmedString nor any prefix of trimmedString satisfies the
    //    syntax of a StrDecimalLiteral, return NaN.
    if trimmed.is_empty() {
        return f64::NAN;
    }
    // Handle Infinity/-Infinity (part of StrDecimalLiteral production)
    if trimmed.starts_with("Infinity") || trimmed.starts_with("+Infinity") {
        return f64::INFINITY;
    }
    if trimmed.starts_with("-Infinity") {
        return f64::NEG_INFINITY;
    }

    // 4. Let numberString be the longest prefix of trimmedString that satisfies
    //    the syntax of a StrDecimalLiteral.
    let mut end = 0;
    let bytes = trimmed.as_bytes();
    // Optional sign
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    // Digits before decimal
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    // Optional decimal point and digits after
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    // Optional exponent part (ExponentPart)
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let e_pos = end;
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        let exp_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == exp_start {
            // No digits after exponent — revert to position before 'e'
            end = e_pos;
        }
    }
    if end == 0 {
        return f64::NAN;
    }
    // Handle sign-only prefix (not a valid StrDecimalLiteral)
    if end == 1 && (bytes[0] == b'+' || bytes[0] == b'-') {
        return f64::NAN;
    }

    // 5. Let parsedNumber be ParseText(StringToCodePoints(numberString), StrDecimalLiteral).
    // 6. Return StringNumericValue of parsedNumber.
    let prefix = &trimmed[..end];
    prefix.parse::<f64>().unwrap_or(f64::NAN)
}

// =========================================================================
// URI encoding/decoding (ES2024 §19.2.6)
// =========================================================================

/// Characters that `encodeURI` does NOT encode.
///
/// This is the union of `uriReserved`, `uriUnescaped`, and `#`
/// per ES2024 §19.2.6.4, step 2.
///
/// [spec]: https://tc39.es/ecma262/#sec-encodeuri-uri
const ENCODE_URI_UNESCAPED: &[u8; 82] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789;,/?:@&=+$-_.!~*'()#";

/// Characters that `encodeURIComponent` does NOT encode.
///
/// This is `uriUnescaped` only per ES2024 §19.2.6.5, step 2.
///
/// [spec]: https://tc39.es/ecma262/#sec-encodeuricomponent-uricomponent
const ENCODE_URI_COMPONENT_UNESCAPED: &[u8; 71] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()";

/// Reserved characters that `decodeURI` does NOT decode.
///
/// This is `uriReserved` plus `#` per ES2024 §19.2.6.1, step 2.
///
/// [spec]: https://tc39.es/ecma262/#sec-decodeuri-encodeduri
const DECODE_URI_RESERVED: &[u8; 11] = b"#$&+,/:;=?@";

/// Check if a character (as a byte) is in the given allow list.
fn is_in_set(byte: u8, set: &[u8]) -> bool {
    set.contains(&byte)
}

/// Percent-encode a single byte as `%XX` (uppercase hex).
fn percent_encode_byte(byte: u8, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0F) as usize] as char);
}

/// Implements the abstract operation `Encode ( string, unescapedSet )`.
///
/// Encodes each UTF-8 byte of characters not in the unescaped set as
/// percent-encoded `%XX` sequences.
///
/// [spec]: https://tc39.es/ecma262/#sec-encode
fn encode_uri_impl(input: &str, unescaped_set: &[u8]) -> Result<String, String> {
    // 1. Let strLen be the length of string.
    // 2. Let R be the empty String.
    let mut result = String::with_capacity(input.len());
    // 3. Let alwaysUnescaped be the string-concatenation of uriUnescaped and extraUnescaped.
    // (Provided as `unescaped_set` parameter.)

    // 4. Let k be 0.
    // 5. Repeat, while k < strLen,
    for ch in input.chars() {
        //   a. Let C be the code unit at index k within string.
        //   b. If C is in unescapedSet, then
        if ch.is_ascii() && is_in_set(ch as u8, unescaped_set) {
            //      i. Set R to the string-concatenation of R and C.
            result.push(ch);
        } else {
            //   c. Else,
            //      i. Let cp be CodePointAt(string, k).
            //      ii. If cp.[[IsUnpairedSurrogate]] is true, throw a URIError exception.
            //          (Cannot happen in valid Rust strings.)
            //      iii. Let Octets be the List of octets resulting from applying the
            //           UTF-8 transformation to cp.[[CodePoint]].
            //      iv. For each element octet of Octets, do
            //          1. Set R to the string-concatenation of R and %XX.
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            for &byte in encoded.as_bytes() {
                percent_encode_byte(byte, &mut result);
            }
        }
    }
    // 6. Return R.
    Ok(result)
}

/// Decode a hex digit character to its numeric value (0-15).
///
/// Returns `None` if the character is not a valid hex digit.
fn hex_digit(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        _ => None,
    }
}

/// Implements the abstract operation `Decode ( string, reservedSet )`.
///
/// Decodes a percent-encoded string. When `reserved_set` is non-empty, any
/// percent-encoded sequence that decodes to a character in that set is left
/// as-is (used by `decodeURI`). When `reserved_set` is empty, all sequences
/// are decoded (used by `decodeURIComponent`).
///
/// Returns `Err` with a URIError message on malformed percent-encoding or
/// invalid UTF-8 sequences.
///
/// [spec]: https://tc39.es/ecma262/#sec-decode
fn decode_uri_impl(input: &str, reserved_set: &[u8]) -> Result<String, String> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == b'%' {
            // Need at least 2 more hex digits
            if i + 2 >= len {
                return Err("URIError: URI malformed".to_string());
            }
            let hi =
                hex_digit(bytes[i + 1]).ok_or_else(|| "URIError: URI malformed".to_string())?;
            let lo =
                hex_digit(bytes[i + 2]).ok_or_else(|| "URIError: URI malformed".to_string())?;
            let byte0 = (hi << 4) | lo;

            // Check if this is a single-byte ASCII character
            if byte0 < 0x80 {
                // If the decoded character is in the reserved set, keep it encoded
                if is_in_set(byte0, reserved_set) {
                    // Preserve the %XX sequence as-is (uppercased)
                    percent_encode_byte(byte0, &mut result);
                } else {
                    result.push(byte0 as char);
                }
                i += 3;
            } else {
                // Multi-byte UTF-8 sequence: determine expected byte count
                let expected_bytes = if byte0 & 0xE0 == 0xC0 {
                    2
                } else if byte0 & 0xF0 == 0xE0 {
                    3
                } else if byte0 & 0xF8 == 0xF0 {
                    4
                } else {
                    return Err("URIError: URI malformed".to_string());
                };

                // Collect all continuation bytes
                let mut utf8_bytes = vec![byte0];
                let mut j = i + 3;
                for _ in 1..expected_bytes {
                    if j >= len || bytes[j] != b'%' {
                        return Err("URIError: URI malformed".to_string());
                    }
                    if j + 2 >= len {
                        return Err("URIError: URI malformed".to_string());
                    }
                    let h = hex_digit(bytes[j + 1])
                        .ok_or_else(|| "URIError: URI malformed".to_string())?;
                    let l = hex_digit(bytes[j + 2])
                        .ok_or_else(|| "URIError: URI malformed".to_string())?;
                    let continuation = (h << 4) | l;
                    if continuation & 0xC0 != 0x80 {
                        return Err("URIError: URI malformed".to_string());
                    }
                    utf8_bytes.push(continuation);
                    j += 3;
                }

                // Validate the UTF-8 sequence
                let decoded = std::str::from_utf8(&utf8_bytes)
                    .map_err(|_| "URIError: URI malformed".to_string())?;
                result.push_str(decoded);
                i = j;
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    Ok(result)
}

/// `encodeURI ( uri )`
///
/// Encodes a URI by replacing each instance of certain characters with
/// one to four percent-encoded UTF-8 escape sequences. Preserves URI-reserved
/// characters (`; , / ? : @ & = + $ #`) and unreserved characters
/// (`A-Z a-z 0-9 - _ . ! ~ * ' ( )`).
///
/// [spec]: https://tc39.es/ecma262/#sec-encodeuri-uri
pub(crate) fn es_encode_uri(input: &str) -> Result<String, String> {
    // 1. Let uriString be ? ToString(uri).
    // (Already done by caller.)
    // 2. Let unescapedURISet be uriReserved + uriUnescaped + "#".
    // 3. Return ? Encode(uriString, unescapedURISet).
    encode_uri_impl(input, ENCODE_URI_UNESCAPED)
}

/// `encodeURIComponent ( uriComponent )`
///
/// Encodes a URI component by replacing each instance of certain characters
/// with one to four percent-encoded UTF-8 escape sequences. Only preserves
/// unreserved characters (`A-Z a-z 0-9 - _ . ! ~ * ' ( )`).
///
/// [spec]: https://tc39.es/ecma262/#sec-encodeuricomponent-uricomponent
pub(crate) fn es_encode_uri_component(input: &str) -> Result<String, String> {
    // 1. Let componentString be ? ToString(uriComponent).
    // (Already done by caller.)
    // 2. Let unescapedURIComponentSet be uriUnescaped.
    // 3. Return ? Encode(componentString, unescapedURIComponentSet).
    encode_uri_impl(input, ENCODE_URI_COMPONENT_UNESCAPED)
}

/// `decodeURI ( encodedURI )`
///
/// Decodes a URI by replacing each percent-encoded UTF-8 escape sequence with
/// the corresponding character. Does NOT decode reserved characters
/// (`# $ & + , / : ; = ? @`), leaving them in their percent-encoded form.
///
/// [spec]: https://tc39.es/ecma262/#sec-decodeuri-encodeduri
pub(crate) fn es_decode_uri(input: &str) -> Result<String, String> {
    // 1. Let uriString be ? ToString(encodedURI).
    // (Already done by caller.)
    // 2. Let reservedURISet be uriReserved + "#".
    // 3. Return ? Decode(uriString, reservedURISet).
    decode_uri_impl(input, DECODE_URI_RESERVED)
}

/// `decodeURIComponent ( encodedURIComponent )`
///
/// Decodes a URI component by replacing all percent-encoded UTF-8 escape
/// sequences with the corresponding characters. Decodes ALL sequences,
/// including those for reserved characters.
///
/// [spec]: https://tc39.es/ecma262/#sec-decodeuricomponent-encodeduricomponent
pub(crate) fn es_decode_uri_component(input: &str) -> Result<String, String> {
    // 1. Let componentString be ? ToString(encodedURIComponent).
    // (Already done by caller.)
    // 2. Let reservedURIComponentSet be the empty String.
    // 3. Return ? Decode(componentString, reservedURIComponentSet).
    decode_uri_impl(input, &[])
}

// =========================================================================
// Constructor & call ABI (B6)
// =========================================================================

/// Implements the abstract operation `Construct ( F [ , argumentsList [ , newTarget ] ] )`.
///
/// `[[Construct]] ( argumentsList, newTarget )`
///
/// Handles the `new` operator for all callable types:
/// 1. Built-in constructors dispatched by name (string callee).
/// 2. User-defined closure constructors — creates a new object, wires prototype,
///    calls the constructor body, returns the constructed object.
/// 3. Native function constructors (e.g., `new Map()` via LoadGlobal).
/// 4. Proxy exotic objects — delegates to proxy `construct` trap.
///
/// [spec]: https://tc39.es/ecma262/#sec-ecmascript-function-objects-construct-argumentslist-newtarget
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_call_new(callee: u64, argc: u32, argv: *const u64) -> u64 {
    // Check for built-in constructors dispatched by name identifier.
    // When the desugar layer resolves `new Map()`, the callee arrives as a
    // global identifier which may be a string literal naming the builtin.
    let callee_val = JsValue::from_raw_bits(callee);
    if callee_val.is_string() {
        let name = string_ops::get_string_data(callee_val);
        // SAFETY: argv validity guaranteed by caller's contract on __esc_rt_call_new.
        return unsafe { call_builtin_constructor(&name, argc, argv) };
    }

    // ECMAScript Function Objects [[Construct]] — §10.2.2
    if extract_closure_data(callee).is_some() {
        // §15.5.2: GeneratorFunction objects do not have a [[Construct]] internal method.
        if is_generator_function(callee) {
            let msg =
                make_rt_string("TypeError: cannot construct a generator function".to_string());
            __esc_rt_throw(msg);
            return JsValue::undefined().raw_bits();
        }

        // 1. Let callerContext be the running execution context.
        // (Implicit — thread-locals represent execution context.)

        // 2-4. (PrepareForOrdinaryCall — handled inside __esc_rt_call_closure)

        // 5. Let kind be F.[[ConstructorKind]].
        // 6. If kind is "base", then
        //    a. Let thisArgument be ? OrdinaryCreateFromConstructor(newTarget, "%Object.prototype%").
        let new_obj = __esc_rt_create_object();

        //    OrdinaryCreateFromConstructor step 3: Get prototype from constructor.prototype.
        //    If Constructor.prototype is not an object, fall back to Object.prototype.
        let proto_key = make_rt_string("prototype".to_string());
        let proto = __esc_rt_get_prop(callee, proto_key);
        if JsValue::from_raw_bits(proto).is_object() {
            // Set legacy __proto__ link
            let proto_link_key = make_rt_string("__proto__".to_string());
            __esc_rt_set_prop(new_obj, proto_link_key, proto);

            // Register shape-based prototype on the constructed object
            set_prototype_on_new_object(new_obj, proto);
        }
        // else: new_obj already has Object.prototype as its implicit prototype

        // 7. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument).
        super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
        let prev_this = CURRENT_THIS.with(|cell| cell.replace(new_obj));
        // 8. Set newTarget on the execution context.
        let prev_new_target = CURRENT_NEW_TARGET.with(|cell| cell.replace(callee));

        // 9. Let result be Completion(OrdinaryCallEvaluateBody(F, argumentsList)).
        // SAFETY: callee is a closure (verified above), argc/argv are valid
        // per the caller's contract on __esc_rt_call_new.
        let result = unsafe { __esc_rt_call_closure(callee, argc, argv) };

        // 10. Remove calleeContext and restore callerContext.
        CURRENT_THIS.with(|cell| cell.set(prev_this));
        CURRENT_NEW_TARGET.with(|cell| cell.set(prev_new_target));

        // 11. If result is a return completion and result.[[Value]] is an Object, return result.[[Value]].
        let result_val = JsValue::from_raw_bits(result);
        if result_val.is_object() {
            return result;
        }
        // 12. If kind is "base", return thisArgument.
        return new_obj;
        // TODO: Step 13 — If result.[[Value]] is not undefined (derived constructor),
        // throw a TypeError. Currently not enforced for derived class constructors.
    }

    // Built-in function objects [[Construct]] — §10.3.2
    // NativeFunc constructor (e.g., `new Map()` via LoadGlobal)
    if let Some((func, context)) = extract_native_func(callee) {
        // §10.3.2 step 1: Built-in methods without [[Construct]] throw TypeError.
        // Check for the __non_ctor__ marker set by get_or_create_builtin_method.
        let is_non_ctor = super::OBJECT_PROPS.with(|props| {
            let props: std::cell::Ref<'_, _> = props.borrow();
            props
                .get(&callee)
                .and_then(|m| m.get("__non_ctor__"))
                .is_some()
        });
        if is_non_ctor {
            // Get the function name for a helpful error message
            let name = super::OBJECT_PROPS.with(|props| {
                let props: std::cell::Ref<'_, _> = props.borrow();
                props
                    .get(&callee)
                    .and_then(|m| m.get("name").copied())
                    .map(|bits| crate::string_ops::get_string_data(JsValue::from_raw_bits(bits)))
                    .unwrap_or_default()
            });
            let msg = make_rt_string(format!("TypeError: {name} is not a constructor"));
            let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
            __esc_rt_throw(err);
            return JsValue::undefined().raw_bits();
        }
        let prev_argc = CURRENT_ARGC.with(|cell| cell.replace(argc));
        let prev_argv = CURRENT_ARGV.with(|cell| cell.replace(argv));
        let prev_new_target = CURRENT_NEW_TARGET.with(|cell| cell.replace(callee));
        let result = func(context);
        CURRENT_ARGC.with(|cell| cell.set(prev_argc));
        CURRENT_ARGV.with(|cell| cell.set(prev_argv));
        CURRENT_NEW_TARGET.with(|cell| cell.set(prev_new_target));
        return result;
    }

    // Proxy exotic objects [[Construct]] — §10.5.13
    if is_constructable_proxy(callee) {
        let args = if argc > 0 && !argv.is_null() {
            // SAFETY: argc > 0 and argv is non-null per caller's contract.
            unsafe { std::slice::from_raw_parts(argv, argc as usize) }
        } else {
            &[]
        };
        match crate::proxy::proxy_construct(callee, args, callee) {
            Ok(result) => return result,
            Err(e) => {
                let msg = make_rt_string(e.to_string());
                let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                __esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }
        }
    }

    // §7.3.14 Construct, step 5: If IsConstructor(F) is false, throw a TypeError.
    let type_desc = value_ops::js_typeof(callee_val);
    let msg = make_rt_string(format!("TypeError: {type_desc} is not a constructor"));
    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    __esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Dispatch to a built-in constructor by name.
///
/// Routes `new Array()`, `new Object()`, `new Map()`, etc. to their
/// respective constructor implementations per the ES2024 spec.
///
/// Returns the constructed value, or an empty plain object for unrecognized names.
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn call_builtin_constructor(name: &str, argc: u32, argv: *const u64) -> u64 {
    match name {
        "Array" => {
            // SAFETY: argv validity guaranteed by caller's contract on __esc_rt_call_new.
            unsafe { construct_array(argc, argv) }
        }
        "Object" => {
            // SAFETY: argv validity guaranteed by caller's contract on __esc_rt_call_new.
            unsafe { construct_object(argc, argv) }
        }
        "Map" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_map(argc, argv) }
        }
        "Set" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_set(argc, argv) }
        }
        "WeakMap" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_weakmap(argc, argv) }
        }
        "WeakSet" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_weakset(argc, argv) }
        }
        "WeakRef" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_weakref(argc, argv) }
        }
        "RegExp" => {
            // SAFETY: argv validity guaranteed by caller's contract.
            unsafe { construct_regexp(argc, argv) }
        }
        "Proxy" => {
            if argc >= 2 {
                // SAFETY: argc >= 2, so argv[0] is valid per caller's contract.
                let target = unsafe { *argv };
                // SAFETY: argc >= 2, so argv[1] is valid per caller's contract.
                let handler = unsafe { *argv.add(1) };
                __esc_rt_create_proxy(target, handler)
            } else {
                JsValue::undefined().raw_bits()
            }
        }
        "Date" => {
            // SAFETY: argv validity guaranteed by caller's contract on __esc_rt_call_new.
            unsafe { construct_date(argc, argv) }
        }
        "Function" => {
            // Function(...args) constructor — creates function from strings
            // SAFETY: argv validity guaranteed by caller's contract on __esc_rt_call_new.
            unsafe { super::construct_function(argc, argv) }
        }
        "Error" | "TypeError" | "RangeError" | "ReferenceError" | "SyntaxError" | "URIError"
        | "EvalError" => {
            let msg = if argc > 0 {
                // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
                let raw_arg = unsafe { *argv };
                let arg_val = JsValue::from_raw_bits(raw_arg);
                // If the argument is undefined (no message), use empty string
                if arg_val.is_undefined() {
                    make_rt_string(String::new())
                } else if arg_val.is_string() {
                    raw_arg
                } else {
                    // Convert non-string to string per spec
                    __esc_rt_to_string(raw_arg)
                }
            } else {
                make_rt_string(String::new())
            };
            let tag = match name {
                "TypeError" => exceptions::error_tag::TYPE_ERROR,
                "RangeError" => exceptions::error_tag::RANGE_ERROR,
                "ReferenceError" => exceptions::error_tag::REFERENCE_ERROR,
                "SyntaxError" => exceptions::error_tag::SYNTAX_ERROR,
                "URIError" => exceptions::error_tag::URI_ERROR,
                "EvalError" => exceptions::error_tag::EVAL_ERROR,
                _ => exceptions::error_tag::ERROR,
            };
            __esc_rt_create_error(tag, msg)
        }
        // §20.3.1.1 Boolean ( value )
        // 1. Let b be ToBoolean(value).
        // 2. If NewTarget is undefined, return b (function call).
        // 3. If NewTarget is not undefined (constructor call):
        //    a. Let O be OrdinaryCreateFromConstructor(NewTarget, "%Boolean.prototype%",
        //       « [[BooleanData]] »).
        //    b. Set O.[[BooleanData]] to b.
        //    c. Return O.
        "Boolean" => {
            let arg = if argc > 0 {
                unsafe {
                    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
                    *argv
                }
            } else {
                JsValue::undefined().raw_bits()
            };
            let b = __esc_rt_to_boolean(arg) != 0;
            // Step 2: If NewTarget is undefined, return the primitive boolean.
            let new_target = CURRENT_NEW_TARGET.with(|c| c.get());
            if new_target == 0 {
                return JsValue::bool(b).raw_bits();
            }
            // Step 3: Create a Boolean wrapper object.
            let wrapper = crate::internal_data::UnifiedObject::boolean_wrapper(
                shapes::ShapeTable::EMPTY_SHAPE,
                JsValue::bool(b).raw_bits(),
            );
            let bits = TaggedObj::boxed(ObjTag::Unified, wrapper);
            // Step 3a: Set prototype to Boolean.prototype
            let proto = super::get_or_create_builtin_prototype("Boolean");
            set_prototype_on_new_object(bits, proto);
            bits
        }
        // §21.1.1.1 Number ( value )
        // 1-2. Let n be +0 or ToNumeric(value).
        // 3. If NewTarget is undefined, return n (function call).
        // 5. If NewTarget is not undefined (constructor call): create NumberObj.
        "Number" => {
            let n = if argc > 0 {
                let raw = unsafe {
                    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
                    *argv
                };
                __esc_rt_to_number(raw)
            } else {
                JsValue::number(0.0).raw_bits()
            };
            let new_target = CURRENT_NEW_TARGET.with(|c| c.get());
            if new_target == 0 {
                return n;
            }
            let wrapper = crate::internal_data::UnifiedObject::number_wrapper(
                shapes::ShapeTable::EMPTY_SHAPE,
                n,
            );
            let bits = TaggedObj::boxed(ObjTag::Unified, wrapper);
            let proto = super::get_or_create_builtin_prototype("Number");
            set_prototype_on_new_object(bits, proto);
            bits
        }
        // §22.1.1.1 String ( value )
        // 1-3. Let s be "" or ToString(value).
        // 4. If NewTarget is undefined, return s (function call).
        // 6. If NewTarget is not undefined (constructor call): create StringObj.
        "String" => {
            let s = if argc > 0 {
                let raw = unsafe {
                    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
                    *argv
                };
                __esc_rt_to_string(raw)
            } else {
                make_rt_string(String::new())
            };
            let new_target = CURRENT_NEW_TARGET.with(|c| c.get());
            if new_target == 0 {
                return s;
            }
            let wrapper = crate::internal_data::UnifiedObject::string_wrapper(
                shapes::ShapeTable::EMPTY_SHAPE,
                s,
            );
            let bits = TaggedObj::boxed(ObjTag::Unified, wrapper);
            let proto = super::get_or_create_builtin_prototype("String");
            set_prototype_on_new_object(bits, proto);
            bits
        }
        // §20.4.1 Math is not a constructor — it's a namespace object.
        // §25.6.1 JSON is not a constructor — it's a namespace object.
        // §27.1.1 Reflect is not a constructor — it's a namespace object.
        // §25.4.1 Atomics is not a constructor — it's a namespace object.
        // §20.1.1.1 Symbol is not a constructor — calling new Symbol() throws TypeError.
        "JSON" | "Math" | "Reflect" | "Atomics" | "console" | "Symbol" => {
            let msg = make_rt_string(format!("TypeError: {name} is not a constructor"));
            let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
            __esc_rt_throw(err);
            JsValue::undefined().raw_bits()
        }
        _ => __esc_rt_create_object(),
    }
}

/// `Array ( ...values )`
///
/// Implements the Array constructor per ES2024 spec.
///
/// - `new Array()` returns an empty array `[]`
/// - `new Array(len)` returns an array with the given length (holes)
/// - `new Array(a, b, c)` returns `[a, b, c]`
///
/// [spec]: https://tc39.es/ecma262/#sec-array-constructor
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn construct_array(argc: u32, argv: *const u64) -> u64 {
    // §23.1.1.1 Array ( )
    // 1. If numberOfArgs = 0, then
    //    a. Return ! ArrayCreate(0).
    if argc == 0 {
        return create_empty_array();
    }

    // §23.1.1.2 Array ( len )
    // 1. If numberOfArgs = 1, then
    if argc == 1 {
        let arg = JsValue::from_raw_bits(unsafe {
            // SAFETY: argc > 0, so argv[0] is valid.
            *argv
        });

        //    a. Let len be values[0].
        //    b. If Type(len) is Number, then
        let is_numeric = arg.is_number() || arg.is_int();
        if is_numeric {
            let n = value_ops::to_number(arg);

            //       i. If len is not an integer, or len < 0, or len > 2^32 - 1,
            //          throw a RangeError exception.
            let len_u32 = n as u32;
            if (len_u32 as f64) != n || n < 0.0 || n > u32::MAX as f64 {
                let msg = "Invalid array length".to_string();
                let err =
                    __esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, make_rt_string(msg));
                __esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }

            //       ii. Return ! ArrayCreate(len).
            let mut arr = crate::internal_data::UnifiedObject::array(
                shapes::ShapeTable::EMPTY_SHAPE,
                Vec::new(),
            );
            arr.array_set_length(len_u32);
            return TaggedObj::boxed(ObjTag::Unified, arr);
        }

        //    c. Else (len is not a Number),
        //       i. Return CreateArrayFromList(« len »).
        return create_array_from_elements(vec![arg]);
    }

    // §23.1.1.3 Array ( ...items )
    // 1. Return CreateArrayFromList(items).
    let mut elements = Vec::with_capacity(argc as usize);
    for i in 0..argc as usize {
        let bits = unsafe {
            // SAFETY: i < argc, and argv has argc valid elements per caller contract.
            *argv.add(i)
        };
        elements.push(JsValue::from_raw_bits(bits));
    }
    create_array_from_elements(elements)
}

/// `Object ( [ value ] )`
///
/// Implements the Object constructor per ES2024 spec.
///
/// - `new Object()` returns an empty plain object `{}`
/// - `new Object(val)` returns the value if it is already an object, or wraps
///   primitives (number, string, boolean) in their corresponding wrapper objects.
///   Currently, primitive wrapping returns a plain object with a `.valueOf` property.
///
/// [spec]: https://tc39.es/ecma262/#sec-object-constructor
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn construct_object(argc: u32, argv: *const u64) -> u64 {
    // §20.1.1.1 Object ( [ value ] )
    // 1. If NewTarget is neither undefined nor the active function object, then
    //    a. Return ? OrdinaryCreateFromConstructor(NewTarget, "%Object.prototype%").
    // TODO: Step 1 — NewTarget check for subclassing not yet implemented.

    // 2. If value is undefined or null, return OrdinaryObjectCreate(%Object.prototype%).
    if argc == 0 {
        return __esc_rt_create_object();
    }

    let arg = JsValue::from_raw_bits(unsafe {
        // SAFETY: argc > 0, so argv[0] is valid.
        *argv
    });

    // 3. Return ! ToObject(value).
    //    ToObject for objects returns the object itself.
    if arg.is_object() {
        return arg.raw_bits();
    }

    // ToObject for null/undefined — would throw in strict spec, but
    // Object() constructor treats them as "create empty object".
    if arg.is_null() || arg.is_undefined() {
        return __esc_rt_create_object();
    }

    // ToObject for primitives creates a wrapper object.
    // TODO: Implement proper Boolean/Number/String wrapper objects.
    // Currently creates a plain object with valueOf as a workaround.
    let wrapper = __esc_rt_create_object();
    let value_of_key = make_rt_string("valueOf".to_string());
    __esc_rt_set_prop(wrapper, value_of_key, arg.raw_bits());
    wrapper
}

/// Helper: extract dense array elements from a NaN-boxed value.
///
/// Returns `None` if the value is not an array object. Returns the resolved
/// element list (handles Dense, Holey, and Dictionary storage).
fn extract_array_elements(bits: u64) -> Option<Vec<JsValue>> {
    let uni = unsafe {
        // SAFETY: caller ensures bits is a valid tagged pointer.
        deref_tagged::<UnifiedObject>(bits)
    }?;
    if uni.kind != InternalKind::Array {
        return None;
    }
    Some(uni.array_elements_resolved())
}

/// Helper: check if a NaN-boxed value is a Set, and extract its values.
///
/// Returns `None` if the value is not a Set object.
fn extract_set_values(bits: u64) -> Option<Vec<JsValue>> {
    let uni = unsafe {
        // SAFETY: caller ensures bits is a valid tagged pointer.
        deref_tagged::<UnifiedObject>(bits)
    }?;
    if uni.kind != InternalKind::SetObj {
        return None;
    }
    if let Some(InternalData::Set { values }) = uni.internal_data() {
        Some(values.clone())
    } else {
        None
    }
}

/// Helper: check if a NaN-boxed value is a Map, and extract its entries.
///
/// Returns `None` if the value is not a Map object.
fn extract_map_entries(bits: u64) -> Option<Vec<(JsValue, JsValue)>> {
    let uni = unsafe {
        // SAFETY: caller ensures bits is a valid tagged pointer.
        deref_tagged::<UnifiedObject>(bits)
    }?;
    if uni.kind != InternalKind::MapObj {
        return None;
    }
    if let Some(InternalData::Map { entries }) = uni.internal_data() {
        Some(entries.clone())
    } else {
        None
    }
}

/// Helper: throw a TypeError with the given message.
///
/// Sets the pending exception and returns `undefined`.
fn throw_type_error(msg: String) -> u64 {
    let err_msg = make_rt_string(msg);
    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, err_msg);
    __esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// `Map ( [ iterable ] )`
///
/// Implements the Map constructor per ES2024 §24.1.1.
///
/// - `new Map()` returns an empty Map.
/// - `new Map(iterable)` populates the Map from `[key, value]` pairs.
///   If the iterable is an array of arrays, each sub-array is destructured.
///   If it's another Map, entries are copied.
///
/// [spec]: https://tc39.es/ecma262/#sec-map-iterable
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_map(argc: u32, argv: *const u64) -> u64 {
    let map = __esc_rt_create_map();
    if argc == 0 {
        return map;
    }
    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let arg = JsValue::from_raw_bits(unsafe { *argv });
    if arg.is_null() || arg.is_undefined() {
        return map;
    }

    // If the argument is a Map, copy entries
    if let Some(entries) = extract_map_entries(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: map was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(map)
        };
        if let Some(u) = uni
            && let Some(InternalData::Map {
                entries: map_entries,
            }) = u.internal_data_mut()
        {
            *map_entries = entries;
        }
        return map;
    }

    // If the argument is an array of [key, value] pairs, iterate and add
    if let Some(elements) = extract_array_elements(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: map was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(map)
        };
        if let Some(u) = uni
            && let Some(InternalData::Map {
                entries: map_entries,
            }) = u.internal_data_mut()
        {
            for elem in &elements {
                if let Some(pair) = extract_array_elements(elem.raw_bits()) {
                    let key = pair.first().copied().unwrap_or(JsValue::undefined());
                    let val = pair.get(1).copied().unwrap_or(JsValue::undefined());
                    // Update existing or insert
                    let mut found = false;
                    for entry in map_entries.iter_mut() {
                        if value_ops::strict_eq(entry.0, key) {
                            entry.1 = val;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        map_entries.push((key, val));
                    }
                }
            }
        }
    }

    map
}

/// `Set ( [ iterable ] )`
///
/// Implements the Set constructor per ES2024 §24.2.1.
///
/// - `new Set()` returns an empty Set.
/// - `new Set(iterable)` populates the Set from values.
///   If the iterable is an array, each element is added.
///   If it's another Set, values are copied.
///
/// [spec]: https://tc39.es/ecma262/#sec-set-iterable
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_set(argc: u32, argv: *const u64) -> u64 {
    let set = __esc_rt_create_set();
    if argc == 0 {
        return set;
    }
    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let arg = JsValue::from_raw_bits(unsafe { *argv });
    if arg.is_null() || arg.is_undefined() {
        return set;
    }

    // If the argument is a Set, copy values
    if let Some(src_values) = extract_set_values(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: set was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(set)
        };
        if let Some(u) = uni
            && let Some(InternalData::Set { values }) = u.internal_data_mut()
        {
            *values = src_values;
        }
        return set;
    }

    // If the argument is an array, add each element
    if let Some(elements) = extract_array_elements(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: set was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(set)
        };
        if let Some(u) = uni
            && let Some(InternalData::Set { values }) = u.internal_data_mut()
        {
            for elem in &elements {
                if !values.iter().any(|v| value_ops::strict_eq(*v, *elem)) {
                    values.push(*elem);
                }
            }
        }
    }

    set
}

/// `WeakMap ( [ iterable ] )`
///
/// Implements the WeakMap constructor per ES2024 §24.3.1.
///
/// - `new WeakMap()` returns an empty WeakMap.
/// - `new WeakMap(iterable)` populates from `[key, value]` pairs.
///   Keys must be objects; non-object keys throw TypeError.
///
/// [spec]: https://tc39.es/ecma262/#sec-weakmap-iterable
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_weakmap(argc: u32, argv: *const u64) -> u64 {
    let wm = __esc_rt_create_weakmap();
    if argc == 0 {
        return wm;
    }
    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let arg = JsValue::from_raw_bits(unsafe { *argv });
    if arg.is_null() || arg.is_undefined() {
        return wm;
    }

    // If the argument is an array of [key, value] pairs, iterate and add
    if let Some(elements) = extract_array_elements(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: wm was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(wm)
        };
        if let Some(u) = uni
            && let Some(InternalData::Map {
                entries: map_entries,
            }) = u.internal_data_mut()
        {
            for elem in &elements {
                if let Some(pair) = extract_array_elements(elem.raw_bits()) {
                    let key = pair.first().copied().unwrap_or(JsValue::undefined());
                    let val = pair.get(1).copied().unwrap_or(JsValue::undefined());
                    // WeakMap keys must be objects
                    if !key.is_object() {
                        return throw_type_error("Invalid value used as weak map key".to_string());
                    }
                    map_entries.push((key, val));
                }
            }
        }
    }

    wm
}

/// `WeakSet ( [ iterable ] )`
///
/// Implements the WeakSet constructor per ES2024 §24.4.1.
///
/// - `new WeakSet()` returns an empty WeakSet.
/// - `new WeakSet(iterable)` populates from values.
///   Values must be objects; non-object values throw TypeError.
///
/// [spec]: https://tc39.es/ecma262/#sec-weakset-iterable
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_weakset(argc: u32, argv: *const u64) -> u64 {
    let ws = __esc_rt_create_weakset();
    if argc == 0 {
        return ws;
    }
    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let arg = JsValue::from_raw_bits(unsafe { *argv });
    if arg.is_null() || arg.is_undefined() {
        return ws;
    }

    // If the argument is an array, add each element (must be objects)
    if let Some(elements) = extract_array_elements(arg.raw_bits()) {
        let uni = unsafe {
            // SAFETY: ws was just created and is a valid tagged pointer.
            deref_tagged_mut::<UnifiedObject>(ws)
        };
        if let Some(u) = uni
            && let Some(InternalData::Set { values }) = u.internal_data_mut()
        {
            for elem in &elements {
                if !elem.is_object() {
                    return throw_type_error("Invalid value used in weak set".to_string());
                }
                if !values.iter().any(|v| value_ops::strict_eq(*v, *elem)) {
                    values.push(*elem);
                }
            }
        }
    }

    ws
}

/// `RegExp ( pattern, flags )`
///
/// Implements the RegExp constructor per ES2024 §22.2.3.1.
///
/// Supports:
/// - `new RegExp(pattern, flags)` — creates from string pattern and flags
/// - `new RegExp(existingRegExp)` — copies pattern and flags from existing
/// - `new RegExp(existingRegExp, newFlags)` — copies pattern, uses new flags
///
/// [spec]: https://tc39.es/ecma262/#sec-regexp-pattern-flags
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_regexp(argc: u32, argv: *const u64) -> u64 {
    if argc == 0 {
        return __esc_rt_create_regexp(
            JsValue::undefined().raw_bits(),
            JsValue::undefined().raw_bits(),
        );
    }

    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let first_arg = unsafe { *argv };
    let first_val = JsValue::from_raw_bits(first_arg);

    // Check if the first argument is a RegExp object
    let regexp_data = if first_val.is_object() {
        let tag = read_obj_tag(first_arg);
        if tag == Some(ObjTag::Unified as u8) {
            let uni = unsafe {
                // SAFETY: tag check confirms this is a unified object.
                deref_tagged::<UnifiedObject>(first_arg)
            };
            if let Some(u) = uni
                && u.kind == InternalKind::RegExpObj
                && let Some(InternalData::RegExp { inner }) = u.internal_data()
                && let Some(re) = inner.downcast_ref::<crate::regexp_bridge::JsRegExpData>()
            {
                Some((re.inner.pattern.clone(), re.flags_string()))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some((pattern, existing_flags)) = regexp_data {
        // First arg is a RegExp object
        let flags_str = if argc >= 2 {
            // SAFETY: argc >= 2, so argv[1] is valid per caller's contract.
            let flags_arg = unsafe { *argv.add(1) };
            let flags_val = JsValue::from_raw_bits(flags_arg);
            if flags_val.is_undefined() {
                existing_flags
            } else {
                extract_key_string(flags_arg).unwrap_or(existing_flags)
            }
        } else {
            existing_flags
        };
        let pattern_bits = make_rt_string(pattern);
        let flags_bits = make_rt_string(flags_str);
        __esc_rt_create_regexp(pattern_bits, flags_bits)
    } else {
        // First arg is a string (or will be coerced)
        let flags = if argc >= 2 {
            // SAFETY: argc >= 2, so argv[1] is valid per caller's contract.
            unsafe { *argv.add(1) }
        } else {
            make_rt_string(String::new())
        };
        __esc_rt_create_regexp(first_arg, flags)
    }
}

/// `WeakRef ( target )`
///
/// Implements the WeakRef constructor per ES2024 §26.1.1.
///
/// - `new WeakRef(target)` creates a weak reference to `target`.
/// - `target` must be an object; non-object throws TypeError.
/// - Missing target throws TypeError.
///
/// [spec]: https://tc39.es/ecma262/#sec-weak-ref-target
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_weakref(argc: u32, argv: *const u64) -> u64 {
    if argc == 0 {
        return throw_type_error("WeakRef requires a target argument".to_string());
    }
    // SAFETY: argc > 0, so argv[0] is valid per caller's contract.
    let target = unsafe { *argv };
    let target_val = JsValue::from_raw_bits(target);
    if !target_val.is_object() {
        return throw_type_error("WeakRef target must be an object".to_string());
    }
    __esc_rt_create_weakref(target)
}

// =========================================================================
// Date constructor
// =========================================================================

/// `Date ( ...values )`
///
/// Implements the Date constructor per ES2024 §21.4.2.
///
/// - `new Date()` — current time (§21.4.2.1)
/// - `new Date(value)` — from timestamp or string (§21.4.2.2)
/// - `new Date(year, month [, date [, hours [, minutes [, seconds [, ms]]]]])` — from components (§21.4.2.3)
///
/// [spec]: https://tc39.es/ecma262/#sec-date-constructor
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
unsafe fn construct_date(argc: u32, argv: *const u64) -> u64 {
    let timestamp = if argc == 0 {
        // §21.4.2.1 Date ( )
        // 1. Let dv be the time value (UTC) identifying the current time.
        host::abi::__esc_host_now_ms()
    } else if argc == 1 {
        // §21.4.2.2 Date ( value )
        // SAFETY: argc == 1, so argv[0] is valid per caller's contract.
        let arg = JsValue::from_raw_bits(unsafe { *argv });
        if arg.is_string() {
            // 2. If value is a String, then
            //    a. Let tv be the result of parsing value as a date-time string.
            let s = crate::string_ops::get_string_data(arg);
            super::parse_date_string(&s)
        } else {
            // 3. Else, let tv be ? ToNumber(value).
            if let Some(n) = arg.as_number() {
                n
            } else if let Some(i) = arg.as_int() {
                i as f64
            } else {
                f64::NAN
            }
        }
    } else {
        // §21.4.2.3 Date ( year, month [, date [, hours [, minutes [, seconds [, ms]]]]] )
        // 1-8. Compute MakeDay + MakeTime + MakeDate from components.
        let args = read_argv(argc, argv);
        let utc_ms = super::make_date_from_components(&args);
        // 9. Let u be TimeClip(UTC(MakeDate(MakeDay(yr, m, dt), MakeTime(h, min, s, milli)))).
        // Per spec, multi-arg Date constructor interprets as LOCAL time,
        // then converts to UTC.
        super::local_to_utc(utc_ms)
    };

    // 10. Set O.[[DateValue]] to dv / tv / u.
    super::__esc_rt_create_date(timestamp)
}

// =========================================================================
// Function.prototype.call / apply / bind
// =========================================================================

/// `Function.prototype.call ( thisArg, ...args )`
///
/// Invokes `func` with `thisArg` as the `this` value, passing the remaining
/// arguments. If no `thisArg` is provided, `undefined` is used.
///
/// [spec]: https://tc39.es/ecma262/#sec-function.prototype.call
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn dispatch_function_call(func: u64, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);

    // 1. If IsCallable(func) is false, throw a TypeError exception.
    if !is_callable(func) {
        let msg = super::make_rt_string(
            "TypeError: Function.prototype.call called on non-callable".to_string(),
        );
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 2. Perform PrepareForTailCall(). (Not applicable in AOT.)

    // 3. Return ? Call(func, thisArg, args).
    // First argument is thisArg, rest are call arguments.
    let this_arg = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let call_args: Vec<u64> = args.iter().skip(1).map(|v| v.raw_bits()).collect();
    let call_argc = call_args.len() as u32;
    let call_argv = if call_args.is_empty() {
        std::ptr::null()
    } else {
        call_args.as_ptr()
    };

    super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
    let prev_this = CURRENT_THIS.with(|cell| cell.replace(this_arg));
    let result = unsafe {
        // SAFETY: call_argv points to call_argc valid u64 values from the Vec.
        __esc_rt_call_indirect(func, call_argc as i32, call_argv)
    };
    CURRENT_THIS.with(|cell| cell.set(prev_this));
    result
}

/// `Function.prototype.apply ( thisArg, argArray )`
///
/// Invokes `func` with `thisArg` as the `this` value, spreading `argArray`
/// as the argument list. If `argArray` is null/undefined, no arguments are passed.
///
/// [spec]: https://tc39.es/ecma262/#sec-function.prototype.apply
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
pub(crate) unsafe fn dispatch_function_apply(func: u64, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);

    // 1. If IsCallable(func) is false, throw a TypeError exception.
    if !is_callable(func) {
        let msg = super::make_rt_string(
            "TypeError: Function.prototype.apply called on non-callable".to_string(),
        );
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 2. If argArray is undefined or null, then
    //    a. Perform PrepareForTailCall(). (Not applicable in AOT.)
    //    b. Return ? Call(func, thisArg).
    let this_arg = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

    // 3. Let argList be ? CreateListFromArrayLike(argArray).
    let args_array = args.get(1).map_or(JsValue::undefined(), |v| *v);
    let call_args: Vec<u64>;

    let args_tag = read_obj_tag(args_array.raw_bits());
    if args_array.is_object() && args_tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(args_array.raw_bits())
        };
        if let Some(u) = uni
            && u.kind == crate::internal_data::InternalKind::Array
        {
            call_args = u
                .array_elements_resolved()
                .iter()
                .map(|v| v.raw_bits())
                .collect();
        } else {
            // TODO: CreateListFromArrayLike should work on any array-like object,
            // not just Array kind. Currently only handles true arrays.
            call_args = Vec::new();
        }
    } else {
        // null, undefined, or non-object → no arguments (step 2)
        call_args = Vec::new();
    }
    let call_argc = call_args.len() as u32;
    let call_argv = if call_args.is_empty() {
        std::ptr::null()
    } else {
        call_args.as_ptr()
    };

    // 4. Perform PrepareForTailCall(). (Not applicable in AOT.)
    // 5. Return ? Call(func, thisArg, argList).
    super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
    let prev_this = CURRENT_THIS.with(|cell| cell.replace(this_arg));
    let result = unsafe {
        // SAFETY: call_argv points to call_argc valid u64 values from the Vec.
        __esc_rt_call_indirect(func, call_argc as i32, call_argv)
    };
    CURRENT_THIS.with(|cell| cell.set(prev_this));
    result
}

/// Extract the name of a function/closure from its `InternalData`.
///
/// Returns the function name as a string, or an empty string if the name
/// cannot be determined. Handles `Function`, `Closure`, and `NativeFunc` kinds.
/// For bound functions, checks the OBJECT_PROPS side-table first.
pub(crate) fn get_function_name(func_bits: u64) -> String {
    // Check OBJECT_PROPS first (handles bound-of-bound, user-set names)
    let side_table_name = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&func_bits).and_then(|m| m.get("name").copied())
    });
    if let Some(name_bits) = side_table_name {
        let name_val = JsValue::from_raw_bits(name_bits);
        if name_val.is_string() {
            return string_ops::get_string_data(name_val);
        }
    }
    // Fall back to InternalData
    let Some(tag) = read_obj_tag(func_bits) else {
        return String::new();
    };
    if tag != ObjTag::Unified as u8 {
        return String::new();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(func_bits) };
    let Some(u) = uni else {
        return String::new();
    };
    if let Some(InternalData::Function { name, .. }) = u.internal_data() {
        let n = JsValue::from_raw_bits(*name);
        if n.is_string() {
            return string_ops::get_string_data(n);
        }
    }
    String::new()
}

/// Extract the `length` (formal parameter count) of a function/closure.
///
/// Returns the parameter count from `InternalData::Function`, or 0 if the
/// value is not a function or the length cannot be determined. Checks the
/// OBJECT_PROPS side-table first for user-set or bound function lengths.
fn get_function_length(func_bits: u64) -> u32 {
    // Check OBJECT_PROPS first (handles bound-of-bound, user-set lengths)
    let side_table_length = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&func_bits).and_then(|m| m.get("length").copied())
    });
    if let Some(length_bits) = side_table_length {
        let length_val = JsValue::from_raw_bits(length_bits);
        if let Some(n) = length_val.as_number() {
            return n as u32;
        }
        if let Some(n) = length_val.as_int() {
            return n.max(0) as u32;
        }
    }
    // Fall back to InternalData
    let Some(tag) = read_obj_tag(func_bits) else {
        return 0;
    };
    if tag != ObjTag::Unified as u8 {
        return 0;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(func_bits) };
    let Some(u) = uni else {
        return 0;
    };
    if let Some(InternalData::Function { param_count, .. }) = u.internal_data() {
        return *param_count;
    }
    0
}

/// `Function.prototype.bind ( thisArg, ...args )`
///
/// Creates a new bound function exotic object that wraps the original callable.
/// When called, the bound function invokes the target with `thisArg` as `this`
/// and prepends `args` before the call-time arguments.
///
/// The bound closure is implemented as a `NativeFunc` (since it does not
/// correspond to a compiled function index) wrapping all captured state in
/// a heap-allocated `BoundFunctionData`.
///
/// [spec]: https://tc39.es/ecma262/#sec-function.prototype.bind
pub(crate) fn dispatch_function_bind(func: u64, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);

    // 1. Let Target be the this value (func).
    // 2. If IsCallable(Target) is false, throw a TypeError exception.
    if !is_callable(func) {
        let msg = super::make_rt_string("TypeError: Bind must be called on a function".to_string());
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 3. Let F be ? BoundFunctionCreate(Target, thisArg, args).
    let this_arg = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let partial_args: Vec<u64> = args.iter().skip(1).map(|v| v.raw_bits()).collect();
    let partial_args_len = partial_args.len();

    // BoundFunctionCreate: Heap-allocate the bound function data
    let bound = Box::new(BoundFunctionData {
        target: func,
        this_arg,
        partial_args,
    });
    let bound_ptr = Box::into_raw(bound) as u64;

    // Create a unified NativeFunc that calls the bound function trampoline
    let bound_bits = TaggedObj::boxed(
        ObjTag::Unified,
        crate::internal_data::UnifiedObject::native_func(bound_function_trampoline, bound_ptr),
    );

    // 4. Let L be 0.
    // 5. Let targetHasLength be ? HasOwnProperty(Target, "length").
    // 6. If targetHasLength is true, then
    //    a. Let targetLen be ? Get(Target, "length").
    //    b. If targetLen is a Number, then
    //       i. Let targetLenAsInt be ! ToIntegerOrInfinity(targetLen).
    //       ii. Let L be max(targetLenAsInt - argCount, 0).
    let target_length = get_function_length(func);
    let bound_length = target_length.saturating_sub(partial_args_len as u32);

    // 7. Perform SetFunctionLength(F, L).
    let length_bits = JsValue::number(bound_length as f64).raw_bits();

    // 8. Let targetName be ? Get(Target, "name").
    // 9. If targetName is not a String, set targetName to "".
    // 10. Perform SetFunctionName(F, targetName, "bound").
    let target_name = get_function_name(func);
    let bound_name = format!("bound {target_name}");
    let name_bits = make_rt_string(bound_name);

    // Store name and length in OBJECT_PROPS side-table so property access finds them
    super::OBJECT_PROPS.with(|props| {
        let mut props = props.borrow_mut();
        let map = props.entry(bound_bits).or_default();
        map.insert("name".to_string(), name_bits);
        map.insert("length".to_string(), length_bits);
    });

    // 11. Return F.
    bound_bits
}

/// Heap-allocated data for a bound function created by `.bind()`.
///
/// Stores the original callable, the bound `this` value, and any
/// partially applied arguments.
pub(crate) struct BoundFunctionData {
    /// The original function/closure to call.
    pub(crate) target: u64,
    /// The bound `this` value.
    pub(crate) this_arg: u64,
    /// Partially applied arguments (prepended before call-time arguments).
    pub(crate) partial_args: Vec<u64>,
}

/// Implements `[[Call]]` for bound function exotic objects.
///
/// When a bound function is called, this retrieves the `BoundFunctionData`,
/// prepends the bound arguments before the call-time arguments, sets the
/// bound `this` value, and delegates to the target function.
///
/// [spec]: https://tc39.es/ecma262/#sec-bound-function-exotic-objects-call-thisargument-argumentslist
fn bound_function_trampoline(context: u64) -> u64 {
    let bound = unsafe {
        // SAFETY: context is a pointer from Box::into_raw in dispatch_function_bind.
        &*(context as *const BoundFunctionData)
    };

    // 1. Let target be F.[[BoundTargetFunction]].
    // 2. Let boundThis be F.[[BoundThis]].
    // 3. Let boundArgs be F.[[BoundArguments]].

    // 4. Let args be the list-concatenation of boundArgs and argumentsList.
    let call_argc = CURRENT_ARGC.with(|cell| cell.get());
    let call_argv = CURRENT_ARGV.with(|cell| cell.get());
    let call_args = read_argv(call_argc, call_argv);

    let mut combined: Vec<u64> = bound.partial_args.clone();
    combined.extend(call_args.iter().map(|v| v.raw_bits()));
    let total_argc = combined.len() as u32;
    let total_argv = if combined.is_empty() {
        std::ptr::null()
    } else {
        combined.as_ptr()
    };

    // 5. Return ? Call(target, boundThis, args).
    super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
    let prev_this = CURRENT_THIS.with(|cell| cell.replace(bound.this_arg));
    let result = if extract_closure_data(bound.target).is_some() {
        unsafe {
            // SAFETY: total_argv points to total_argc valid u64 values from the Vec.
            __esc_rt_call_closure(bound.target, total_argc, total_argv)
        }
    } else {
        unsafe {
            // SAFETY: total_argv points to total_argc valid u64 values from the Vec.
            __esc_rt_call_indirect(bound.target, total_argc as i32, total_argv)
        }
    };
    CURRENT_THIS.with(|cell| cell.set(prev_this));
    result
}

// =========================================================================
// Method call dispatch
// =========================================================================

/// Implements the `EvaluateCall` path for method calls: `obj.key(args)`.
///
/// Dispatches based on the receiver's type and the method name to the
/// appropriate built-in or user-defined method implementation.
///
/// For built-in types (arrays, strings), dispatches to inline implementations.
/// For user objects (plain, closure, function), gets the property from the
/// object's shape table (including prototype chain walk) and, if it resolves
/// to a closure, calls it with `this` bound to the receiver. This enables
/// patterns like `assert.sameValue(...)` where `assert` is a closure with
/// method properties set via `assert.sameValue = function(...) { ... }`.
///
/// [spec]: https://tc39.es/ecma262/#sec-evaluatecall
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_call_method(
    obj: u64,
    key: u64,
    argc: u32,
    argv: *const u64,
) -> u64 {
    // Extract the method name string
    let key_str = extract_key_string(key);
    let method_name = key_str.as_deref().unwrap_or("");

    let obj_val = JsValue::from_raw_bits(obj);

    // String-valued obj may be a built-in global namespace (e.g., Math, Object, Array).
    // Check for global namespace dispatch before string instance methods.
    if obj_val.is_string() {
        let obj_name = string_ops::get_string_data(obj_val);
        if let Some(result) =
            super::dispatch_global_namespace_method(&obj_name, method_name, argc, argv)
        {
            return result;
        }
        // Otherwise treat as a normal string value's methods (e.g., "hello".length)
        return dispatch_string_method(obj, method_name, argc, argv);
    }

    // Number instance methods on numeric values
    if (obj_val.is_number() || obj_val.is_int())
        && let Some(result) = dispatch_number_instance_method(obj_val, method_name, argc, argv)
    {
        return result;
    }

    // Boolean instance methods (true.toString(), false.valueOf())
    if obj_val.is_bool()
        && let Some(result) = super::dispatch_boolean_method(obj_val, method_name)
    {
        return result;
    }

    // Symbol instance methods (Symbol.prototype.toString, valueOf, description)
    if let Some(sym_id) = obj_val.as_symbol()
        && let Some(result) = super::dispatch_symbol_instance_method(sym_id, method_name)
    {
        return result;
    }

    // Object/Array methods: check tag
    if let Some(tag) = read_obj_tag(obj)
        && tag == ObjTag::Unified as u8
    {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<crate::internal_data::UnifiedObject>(obj)
        };
        if let Some(u) = uni {
            // Object.prototype methods apply to ALL object kinds as a fallback.
            // Check them first for kinds that return early from their specific
            // dispatchers (Array, Map, Set, etc.) so hasOwnProperty/valueOf/etc.
            // are always available.
            if matches!(
                method_name,
                "hasOwnProperty" | "propertyIsEnumerable" | "isPrototypeOf" | "toLocaleString"
            ) && let Some(result) =
                super::dispatch_object_proto_method(obj, method_name, argc, argv)
            {
                return result;
            }
            // For toString/valueOf: let kind-specific handlers override first,
            // but Error/Array have their own. Other kinds fall through to the
            // Object.prototype dispatch below.
            match u.kind {
                InternalKind::Array => {
                    // Array.prototype.toString is custom (joins elements)
                    // but valueOf, hasOwnProperty etc. are handled above.
                    return dispatch_array_method(obj, method_name, argc, argv);
                }
                InternalKind::MapObj | InternalKind::WeakMapObj => {
                    return super::dispatch_map_method(obj, method_name, argc, argv);
                }
                InternalKind::SetObj | InternalKind::WeakSetObj => {
                    return super::dispatch_set_method(obj, method_name, argc, argv);
                }
                InternalKind::RegExpObj => {
                    return super::dispatch_regexp_method(obj, method_name, argc, argv);
                }
                InternalKind::DateObj => {
                    return super::dispatch_date_method(obj, method_name, argc, argv);
                }
                InternalKind::WeakRefObj if method_name == "deref" => {
                    if let Some(InternalData::WeakRef { target }) = u.internal_data() {
                        return *target;
                    }
                    return JsValue::undefined().raw_bits();
                }
                InternalKind::Promise => {
                    return super::dispatch_promise_instance_method(obj, method_name, argc, argv);
                }
                InternalKind::Generator => {
                    let args = read_argv(argc, argv);
                    let arg = args
                        .first()
                        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
                    return super::dispatch_generator_method_with_arg(obj, method_name, arg);
                }
                InternalKind::Iterator => {
                    // ES2025 Iterator Helpers: route method calls to iterator_helpers
                    if let Some(result) = unsafe {
                        // SAFETY: argc/argv are valid per the caller's contract
                        // on __esc_rt_call_method.
                        crate::iterator_helpers::dispatch_iterator_method(
                            obj,
                            method_name,
                            argc,
                            argv,
                        )
                    } {
                        return result;
                    }
                }
                InternalKind::AsyncIterator => {
                    // Async Iterator Helpers: route method calls
                    if let Some(result) = unsafe {
                        // SAFETY: argc/argv are valid per the caller's contract
                        // on __esc_rt_call_method.
                        crate::async_iterator_helpers::dispatch_async_iterator_method(
                            obj,
                            method_name,
                            argc,
                            argv,
                        )
                    } {
                        return result;
                    }
                }
                InternalKind::ErrorObj => {
                    if let Some(InternalData::Error {
                        error_tag,
                        raw_message,
                        ..
                    }) = u.internal_data()
                        && method_name == "toString"
                    {
                        let name = crate::exceptions::error_name(*error_tag).to_string();
                        let msg = string_ops::get_string_data(JsValue::from_raw_bits(*raw_message));
                        let result = if msg.is_empty() {
                            name
                        } else {
                            format!("{name}: {msg}")
                        };
                        return make_rt_string(result);
                    }
                }
                // Wrapper objects: dispatch to their prototype methods
                InternalKind::BooleanObj => {
                    let unwrapped = JsValue::from_raw_bits(super::unwrap_wrapper_object(obj));
                    if let Some(result) = super::dispatch_boolean_method(unwrapped, method_name) {
                        return result;
                    }
                }
                InternalKind::NumberObj => {
                    let unwrapped = JsValue::from_raw_bits(super::unwrap_wrapper_object(obj));
                    if let Some(result) =
                        dispatch_number_instance_method(unwrapped, method_name, argc, argv)
                    {
                        return result;
                    }
                }
                InternalKind::StringObj => {
                    let unwrapped = super::unwrap_wrapper_object(obj);
                    return dispatch_string_method(unwrapped, method_name, argc, argv);
                }
                _ => {}
            }
            // Array.prototype methods on non-array objects (generic this via .call())
            // ES spec requires Array.prototype methods to work on any array-like object.
            if is_array_prototype_method(method_name) {
                return dispatch_array_method(obj, method_name, argc, argv);
            }
        }
        // Check if this is a Math-like object by method name
        if let Some(result) = dispatch_math_method(method_name, argc, argv) {
            return result;
        }
        // Check if this is a Number-like object by static method name
        if let Some(result) = dispatch_number_static_method(method_name, argc, argv) {
            return result;
        }
        // Try to get the property as a callable (check own properties + proto chain)
        let prop = __esc_rt_get_prop(obj, key);
        if extract_closure_data(prop).is_some() {
            super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
            let prev_this = CURRENT_THIS.with(|cell| cell.replace(obj));
            // SAFETY: prop is a closure (verified above), argc/argv are valid
            // per the caller's contract on __esc_rt_call_method.
            let result = unsafe { __esc_rt_call_closure(prop, argc, argv) };
            CURRENT_THIS.with(|cell| cell.set(prev_this));
            return result;
        }
        // Also check if the prop is a native func (NativeFunc dispatch)
        // Set CURRENT_THIS so the trampoline sees the correct receiver.
        if extract_native_func(prop).is_some() {
            super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
            let prev_this = CURRENT_THIS.with(|cell| cell.replace(obj));
            let result = super::try_call_native_func_prop(prop, argc, argv);
            CURRENT_THIS.with(|cell| cell.set(prev_this));
            if let Some(r) = result {
                return r;
            }
        }
        // Callable objects: Function.prototype.call/apply/bind/toString
        if is_callable(obj) {
            if method_name == "call" {
                // SAFETY: argc/argv are valid per the caller's contract on __esc_rt_call_method.
                return unsafe { dispatch_function_call(obj, argc, argv) };
            }
            if method_name == "apply" {
                // SAFETY: argc/argv are valid per the caller's contract on __esc_rt_call_method.
                return unsafe { dispatch_function_apply(obj, argc, argv) };
            }
            if method_name == "bind" {
                return dispatch_function_bind(obj, argc, argv);
            }
            // Function.prototype.toString — returns "function name() { [native code] }"
            if method_name == "toString" {
                return super::dispatch_function_to_string(obj);
            }
            // Property-based method dispatch on closures/functions
            if extract_closure_data(obj).is_some() {
                let prop = __esc_rt_get_prop(obj, key);
                if extract_closure_data(prop).is_some() {
                    super::THIS_EXPLICITLY_SET.with(|cell| cell.set(true));
                    let prev_this = CURRENT_THIS.with(|cell| cell.replace(obj));
                    // SAFETY: prop is a closure (verified above), argc/argv are valid
                    // per the caller's contract on __esc_rt_call_method.
                    let result = unsafe { __esc_rt_call_closure(prop, argc, argv) };
                    CURRENT_THIS.with(|cell| cell.set(prev_this));
                    return result;
                }
            }
        }
        // Object.prototype methods: toString, valueOf, hasOwnProperty, etc.
        // These serve as fallback for ALL object types.
        if let Some(result) = super::dispatch_object_proto_method(obj, method_name, argc, argv) {
            return result;
        }
    }

    // Global object methods: Math.max, Number.isInteger, Promise.resolve, etc.
    // arrive with obj=undefined because the global identifier isn't resolved to
    // an object at compile time.
    if obj_val.is_undefined() {
        if let Some(result) = dispatch_math_method(method_name, argc, argv) {
            return result;
        }
        if let Some(result) = dispatch_number_static_method(method_name, argc, argv) {
            return result;
        }
        if let Some(result) = super::dispatch_promise_static_method(method_name, argc, argv) {
            return result;
        }
        if let Some(result) = dispatch_object_static_method(method_name, argc, argv) {
            return result;
        }
    }

    // Fallback: method not found — throw TypeError per ES spec.
    // `obj.method()` where `method` is not a function throws
    // "TypeError: obj.method is not a function".
    if !obj_val.is_undefined() && !obj_val.is_null() {
        let msg = make_rt_string(format!("TypeError: {method_name} is not a function"));
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        __esc_rt_throw(err);
    } else {
        // Cannot read properties of null/undefined
        let type_name = if obj_val.is_null() {
            "null"
        } else {
            "undefined"
        };
        let msg = make_rt_string(format!(
            "TypeError: Cannot read properties of {type_name} (reading '{method_name}')"
        ));
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        __esc_rt_throw(err);
    }
    JsValue::undefined().raw_bits()
}

/// Implements `SuperCall : super Arguments` evaluation.
///
/// `super(args)` in a class constructor. Currently delegates to `[[Construct]]`
/// on the parent class. A full implementation would also bind `this` in the
/// derived constructor's environment record.
///
/// [spec]: https://tc39.es/ecma262/#sec-super-keyword-runtime-semantics-evaluation
///
/// # Safety
///
/// `argv` must point to `argc` valid u64 values, or be null when `argc` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __esc_rt_super_call(callee: u64, argc: u32, argv: *const u64) -> u64 {
    // §13.3.7.1 Runtime Semantics: Evaluation — SuperCall : super Arguments
    // 1. Let newTarget be GetNewTarget().
    // 2. Assert: newTarget is an Object.
    // 3. Let func be GetSuperConstructor().
    // 4. Let argList be ? ArgumentListEvaluation of Arguments.
    // 5. If IsConstructor(func) is false, throw a TypeError exception.
    // 6. Let result be ? Construct(func, argList, newTarget).
    // 7. Let thisER be GetThisEnvironment().
    // 8. Perform ? thisER.BindThisValue(result).
    // TODO: Steps 7-8 — BindThisValue for derived constructors not yet implemented.
    // 9. ... (initialize fields, etc.)
    // 10. Return result.

    // Delegate to call_new for now — super() is essentially new ParentClass()
    // SAFETY: argc/argv validity guaranteed by the caller's contract on __esc_rt_super_call.
    unsafe { __esc_rt_call_new(callee, argc, argv) }
}
