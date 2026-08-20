//! Dispatch for Promise, Generator, Boolean, Symbol, Date, process, and Object.prototype methods.
//!
//! Contains dispatch helpers for built-in type instance methods,
//! global namespace routing, and `Object.prototype` fallback methods.

use nanbox::JsValue;

use crate::generator;
use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, read_obj_tag};
use crate::{display, exceptions, string_ops};

/// `IsPromise ( x )` — Check if a NaN-boxed value is a Promise object.
///
/// Returns `true` if the value is a unified object with `InternalKind::Promise`.
///
/// [spec]: https://tc39.es/ecma262/#sec-ispromise
fn is_promise(bits: u64) -> bool {
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    uni.is_some_and(|u| u.kind == InternalKind::Promise)
}

/// Throw a TypeError for a non-Promise `this` value.
///
/// Used by Promise.prototype methods that require `this` to be a Promise.
fn throw_not_promise(method_name: &str) -> u64 {
    let msg = make_rt_string(format!(
        "TypeError: {method_name} called on non-Promise object"
    ));
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

use super::{
    __esc_rt_create_object, __esc_rt_create_proxy, __esc_rt_get_prop, __esc_rt_promise_create,
    __esc_rt_promise_reject, __esc_rt_promise_resolve, __esc_rt_promise_then, __esc_rt_set_prop,
    create_array_from_elements, dispatch_array_static_method, dispatch_json_method,
    dispatch_math_method, dispatch_number_static_method, dispatch_object_static_method,
    dispatch_reflect_method, dispatch_string_static_method, make_rt_string, native_proxy_revoke,
    read_argv,
};

// =========================================================================
// Promise dispatch
// =========================================================================

/// `Promise.prototype.then ( onFulfilled, onRejected )`
///
/// Dispatch a Promise instance method (`.then`, `.catch`, `.finally`).
///
/// - `.then` — [spec]: <https://tc39.es/ecma262/#sec-promise.prototype.then>
/// - `.catch` — [spec]: <https://tc39.es/ecma262/#sec-promise.prototype.catch>
/// - `.finally` — [spec]: <https://tc39.es/ecma262/#sec-promise.prototype.finally>
pub(crate) fn dispatch_promise_instance_method(
    obj: u64,
    method: &str,
    argc: u32,
    argv: *const u64,
) -> u64 {
    let args = read_argv(argc, argv);
    match method {
        // === Promise.prototype.then ( onFulfilled, onRejected ) ===
        // [spec]: https://tc39.es/ecma262/#sec-promise.prototype.then
        "then" => {
            // 1. Let promise be the this value.
            // 2. If IsPromise(promise) is false, throw a TypeError exception.
            if !is_promise(obj) {
                return throw_not_promise("Promise.prototype.then");
            }
            // 3. Let C be ? SpeciesConstructor(promise, %Promise%).
            // TODO: Step 3 — SpeciesConstructor not implemented, uses default Promise
            // 4. Let resultCapability be ? NewPromiseCapability(C).
            // 5. Return PerformPromiseThen(promise, onFulfilled, onRejected, resultCapability).
            let on_fulfill = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let on_reject = args
                .get(1)
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            __esc_rt_promise_then(obj, on_fulfill, on_reject)
        }
        // === Promise.prototype.catch ( onRejected ) ===
        // [spec]: https://tc39.es/ecma262/#sec-promise.prototype.catch
        "catch" => {
            // 1. Let promise be the this value.
            // 2. Return ? Invoke(promise, "then", « undefined, onRejected »).
            // NOTE: catch delegates to then, which checks IsPromise.
            if !is_promise(obj) {
                return throw_not_promise("Promise.prototype.catch");
            }
            let on_reject = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            __esc_rt_promise_then(obj, JsValue::undefined().raw_bits(), on_reject)
        }
        // === Promise.prototype.finally ( onFinally ) ===
        // [spec]: https://tc39.es/ecma262/#sec-promise.prototype.finally
        "finally" => {
            // 1. Let promise be the this value.
            // 2. If IsPromise(promise) is false, throw a TypeError.
            if !is_promise(obj) {
                return throw_not_promise("Promise.prototype.finally");
            }
            // TODO: Step 3 — Let C be ? SpeciesConstructor(promise, %Promise%).
            // TODO: Steps 4-10 — Create thenFinally/catchFinally closures, invoke .then
            // Simplified: just return the promise as-is for now
            obj
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// `Promise.resolve ( x )` / `Promise.reject ( r )`
///
/// Dispatch a Promise static method (`Promise.resolve`, `Promise.reject`).
///
/// - `Promise.resolve` — [spec]: <https://tc39.es/ecma262/#sec-promise.resolve>
/// - `Promise.reject` — [spec]: <https://tc39.es/ecma262/#sec-promise.reject>
///
/// Returns `Some(bits)` if the method is a known Promise static method, `None` otherwise.
pub(crate) fn dispatch_promise_static_method(
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // === Promise.resolve ( x ) ===
        // [spec]: https://tc39.es/ecma262/#sec-promise.resolve
        "resolve" => {
            // 1. Let C be the this value.
            // 2. If C is not an Object, throw a TypeError exception.
            // TODO: Step 2 — no TypeError check for non-object this
            // 3. Return ? PromiseResolve(C, x).
            //   PromiseResolve (§27.2.4.7):
            //   1. If IsPromise(x) is true, and x.[[PromiseConstructor]] === C, return x.
            //   TODO: Step 1 — not checking if x is already a Promise of same constructor
            //   2. Let promiseCapability be ? NewPromiseCapability(C).
            //   3. Perform ? Call(promiseCapability.[[Resolve]], undefined, « x »).
            //   4. Return promiseCapability.[[Promise]].
            let val = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let prom = __esc_rt_promise_create();
            __esc_rt_promise_resolve(prom, val);
            Some(prom)
        }
        // === Promise.reject ( r ) ===
        // [spec]: https://tc39.es/ecma262/#sec-promise.reject
        "reject" => {
            // 1. Let C be the this value.
            // 2. Let promiseCapability be ? NewPromiseCapability(C).
            // 3. Perform ? Call(promiseCapability.[[Reject]], undefined, « r »).
            // 4. Return promiseCapability.[[Promise]].
            let val = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let prom = __esc_rt_promise_create();
            __esc_rt_promise_reject(prom, val);
            Some(prom)
        }
        _ => None,
    }
}

// =========================================================================
// Generator dispatch (state machine protocol)
// =========================================================================

/// `Generator.prototype.next ( value )` / `.return ( value )` / `.throw ( exception )`
///
/// Dispatch a method call on a generator object (no-argument form).
///
/// - `.next` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.next>
/// - `.return` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.return>
/// - `.throw` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.throw>
///
/// Supports `.next(value)`, `.throw(error)`, and `.return(value)` which call
/// the compiled resume function (state machine) with the appropriate resume mode.
pub(crate) fn dispatch_generator_method(obj: u64, method: &str) -> u64 {
    match method {
        // GeneratorResume(generator, undefined) — §27.5.3.3
        "next" => generator_resume(obj, JsValue::undefined().raw_bits(), generator::RESUME_NEXT),
        // GeneratorResumeAbrupt(generator, Completion{[[Type]]: return, [[Value]]: undefined}) — §27.5.3.4
        "return" => generator_resume(
            obj,
            JsValue::undefined().raw_bits(),
            generator::RESUME_RETURN,
        ),
        // GeneratorResumeAbrupt(generator, Completion{[[Type]]: throw, [[Value]]: undefined}) — §27.5.3.4
        "throw" => generator_resume(
            obj,
            JsValue::undefined().raw_bits(),
            generator::RESUME_THROW,
        ),
        _ => JsValue::undefined().raw_bits(),
    }
}

/// `Generator.prototype.next ( value )` / `.return ( value )` / `.throw ( exception )`
///
/// Dispatch a generator method call with an argument value.
///
/// - `.next` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.next>
/// - `.return` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.return>
/// - `.throw` — [spec]: <https://tc39.es/ecma262/#sec-generator.prototype.throw>
///
/// Used by `.next(val)`, `.throw(err)`, `.return(val)` when called with an argument.
pub(crate) fn dispatch_generator_method_with_arg(obj: u64, method: &str, arg: u64) -> u64 {
    match method {
        // GeneratorResume(generator, value) — §27.5.3.3
        "next" => generator_resume(obj, arg, generator::RESUME_NEXT),
        // GeneratorResumeAbrupt(generator, Completion{[[Type]]: return, [[Value]]: value}) — §27.5.3.4
        "return" => generator_resume(obj, arg, generator::RESUME_RETURN),
        // GeneratorResumeAbrupt(generator, Completion{[[Type]]: throw, [[Value]]: exception}) — §27.5.3.4
        "throw" => generator_resume(obj, arg, generator::RESUME_THROW),
        _ => JsValue::undefined().raw_bits(),
    }
}

/// `GeneratorResume ( generator, value, generatorBrand )`
///
/// Resume a generator by calling its compiled resume function.
///
/// [spec]: <https://tc39.es/ecma262/#sec-generatorresume>
///
/// Extracts the state object and resume function index from the generator's
/// internal data, then calls `__esc_dispatch(resume_func_idx, 3, [state, sent_value, resume_mode])`.
///
/// Returns a `{value, done}` iterator result object.
fn generator_resume(gen_obj: u64, sent_value: u64, resume_mode: i32) -> u64 {
    // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
    let uni = unsafe {
        // SAFETY: caller ensures gen_obj is a unified Generator object.
        deref_tagged::<UnifiedObject>(gen_obj)
    };
    let Some(u) = uni else {
        // Generator is invalid — return {value: undefined, done: true}
        return create_done_result();
    };
    let Some(InternalData::Generator {
        state_obj,
        resume_func_idx,
    }) = u.internal_data()
    else {
        // Not a generator — return {value: undefined, done: true}
        return create_done_result();
    };

    let state = *state_obj;
    let func_idx = *resume_func_idx;

    // 2. Read state_index to check generator state.
    let state_index = {
        let key = make_rt_string("state_index".to_string());
        let val = __esc_rt_get_prop(state, key);
        let jv = JsValue::from_raw_bits(val);
        if let Some(i) = jv.as_int() {
            i
        } else if let Some(n) = jv.as_number() {
            n as i32
        } else {
            -1 // default: not started
        }
    };

    // §27.5.3.3 Step 2: If state is completed, handle based on resume_mode.
    if state_index == -2 {
        // Generator is completed (done).
        return match resume_mode {
            // .return(val) on completed generator → {val, true}
            2 => create_iterator_result(sent_value, true),
            // .next() on completed generator → {undefined, true}
            _ => create_done_result(),
        };
    }

    // §27.5.3.4: GeneratorResumeAbrupt — handle .return() and .throw()
    // before entering the compiled state machine.
    if resume_mode == generator::RESUME_RETURN {
        // .return(val) — mark generator as done and return {val, true}.
        // NOTE: This skips finally blocks (TODO: support try/finally).
        let key = make_rt_string("state_index".to_string());
        let done_val = JsValue::int(-2).raw_bits();
        __esc_rt_set_prop(state, key, done_val);
        return create_iterator_result(sent_value, true);
    }

    if resume_mode == generator::RESUME_THROW {
        // .throw(err) — mark generator as done and throw the error.
        // NOTE: This doesn't throw into the generator's try/catch context
        // (TODO: support try/catch around yield points). The throw
        // propagates to the caller's try/catch via the exception system.
        let key = make_rt_string("state_index".to_string());
        let done_val = JsValue::int(-2).raw_bits();
        __esc_rt_set_prop(state, key, done_val);
        // Use the exception system to throw — this will be caught by
        // the caller's try/catch if present.
        crate::exceptions::throw(sent_value);
        return create_done_result();
    }

    // RESUME_NEXT: call the compiled resume function.
    let boxed_mode = JsValue::int(resume_mode).raw_bits();
    let args = [state, sent_value, boxed_mode];
    // SAFETY: args is a stack-local array of 3 u64 values; __esc_dispatch reads
    // exactly `argc` values from the pointer. func_idx is the index of the
    // compiled resume function in the module's function table.
    let result = unsafe { super::__esc_dispatch(func_idx as i32, 3, args.as_ptr()) };

    // 10. Assert: When we return here, genContext has already been removed from
    //     the execution context stack and methodContext is the currently running
    //     execution context.
    // 11. Return Completion(result).

    // If the result is a valid iterator result object, return it.
    // Otherwise wrap it in one (safety fallback).
    let tag = read_obj_tag(result);
    if tag == Some(ObjTag::Unified as u8) {
        return result;
    }

    // Fallback: wrap non-object results
    create_iterator_result(result, true)
}

/// Create a generator object from a closure using the state machine protocol.
///
/// For generator functions that still use the old is_generator flag, this
/// creates a generator by calling the ramp function (which is the closure body).
/// The ramp function allocates state and calls `__esc_rt_create_generator`.
///
/// # Safety
///
/// `closure` must be a valid NaN-boxed closure value. `argc` and `argv` follow
/// the standard C ABI contract for runtime calls.
pub(crate) unsafe fn create_generator_from_closure(
    closure: u64,
    argc: u32,
    argv: *const u64,
) -> u64 {
    // The closure IS the ramp function — it allocates state, saves params,
    // and calls __esc_rt_create_generator(state, resume_func_idx).
    // SAFETY: closure is a valid closure (caller guarantees), argc/argv follow the
    // standard C ABI contract per the function's safety documentation.
    unsafe { super::__esc_rt_call_closure(closure, argc, argv) }
}

/// `CreateIterResultObject ( value, done )`
///
/// Create a `{value, done}` result object.
///
/// [spec]: <https://tc39.es/ecma262/#sec-createiterresultobject>
pub(crate) fn create_iterator_result(value: u64, done: bool) -> u64 {
    // 1. Let obj be OrdinaryObjectCreate(%Object.prototype%).
    let result = __esc_rt_create_object();
    // 2. Perform ! CreateDataPropertyOrThrow(obj, "value", value).
    let value_key = make_rt_string("value".to_string());
    __esc_rt_set_prop(result, value_key, value);
    // 3. Perform ! CreateDataPropertyOrThrow(obj, "done", done).
    let done_key = make_rt_string("done".to_string());
    __esc_rt_set_prop(result, done_key, JsValue::bool(done).raw_bits());
    // 4. Return obj.
    result
}

/// `CreateIterResultObject ( undefined, true )`
///
/// Create a `{value: undefined, done: true}` result object.
///
/// [spec]: <https://tc39.es/ecma262/#sec-createiterresultobject>
pub(crate) fn create_done_result() -> u64 {
    create_iterator_result(JsValue::undefined().raw_bits(), true)
}

// =========================================================================
// Boolean method dispatch
// =========================================================================

/// `Boolean.prototype.toString ( )` / `Boolean.prototype.valueOf ( )`
///
/// Dispatch a boolean instance method.
///
/// - `toString` — [spec]: <https://tc39.es/ecma262/#sec-boolean.prototype.tostring>
/// - `valueOf` — [spec]: <https://tc39.es/ecma262/#sec-boolean.prototype.valueof>
///
/// Returns `Some(result)` if the method is recognized, `None` otherwise.
pub(crate) fn dispatch_boolean_method(val: JsValue, method: &str) -> Option<u64> {
    // thisBooleanValue (ES2024 §20.3.3):
    // 1. If val is a Boolean primitive, return val.
    // 2. If val is a Boolean wrapper object, return [[BooleanData]].
    // 3. Throw TypeError.
    let b = if let Some(b) = val.as_bool() {
        // Primitive boolean
        b
    } else {
        // Try unwrapping BooleanObj
        let unwrapped_bits = super::unwrap_wrapper_object(val.raw_bits());
        if unwrapped_bits != val.raw_bits() {
            // Successfully unwrapped — get the boolean value
            JsValue::from_raw_bits(unwrapped_bits)
                .as_bool()
                .unwrap_or(false)
        } else {
            // Not a boolean and not a BooleanObj — throw TypeError
            match method {
                "toString" | "valueOf" => {
                    let msg =
                        format!("Boolean.prototype.{method} requires that 'this' be a Boolean");
                    let msg_bits = make_rt_string(msg);
                    let err = super::__esc_rt_create_error(
                        crate::exceptions::error_tag::TYPE_ERROR,
                        msg_bits,
                    );
                    super::__esc_rt_throw(err);
                    return Some(JsValue::undefined().raw_bits());
                }
                _ => return None,
            }
        }
    };

    match method {
        // === Boolean.prototype.toString ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-boolean.prototype.tostring
        "toString" => {
            // 1. Let b be ? thisBooleanValue(this value).
            // 2. If b is true, return "true"; else return "false".
            let s = if b { "true" } else { "false" };
            Some(make_rt_string(s.to_string()))
        }
        // === Boolean.prototype.valueOf ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-boolean.prototype.valueof
        "valueOf" => {
            // 1. Return ? thisBooleanValue(this value).
            Some(JsValue::bool(b).raw_bits())
        }
        _ => None,
    }
}

// =========================================================================
// Global namespace dispatch
// =========================================================================

/// Dispatch a method call on a global namespace object (Math, Object, JSON, etc.).
///
/// Routes to the appropriate static method dispatcher for each global constructor
/// or namespace object. This is an internal routing function with no direct spec
/// equivalent — each dispatched method implements its own spec algorithm.
///
/// Returns `Some(result)` if the object name is a recognized global and the
/// method is handled; returns `None` otherwise.
pub(crate) fn dispatch_global_namespace_method(
    obj_name: &str,
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    match obj_name {
        "Math" => dispatch_math_method(method, argc, argv),
        "Number" => dispatch_number_static_method(method, argc, argv),
        "Object" => dispatch_object_static_method(method, argc, argv),
        "Promise" => dispatch_promise_static_method(method, argc, argv),
        "JSON" => dispatch_json_method(method, argc, argv),
        "String" => dispatch_string_static_method(method, argc, argv),
        "Array" => dispatch_array_static_method(method, argc, argv),
        // === Proxy.revocable ( target, handler ) ===
        // [spec]: https://tc39.es/ecma262/#sec-proxy.revocable
        "Proxy" => {
            if method == "revocable" && argc >= 2 {
                let args = read_argv(argc, argv);
                let target = args[0].raw_bits();
                let handler = args[1].raw_bits();
                // 1. Let p be ? ProxyCreate(target, handler).
                let proxy = __esc_rt_create_proxy(target, handler);
                // 2. Let revokerClosure be a new Abstract Closure ... that revokes the proxy.
                // 3. Let revoker be CreateBuiltinFunction(revokerClosure, 0, "", ...).
                let revoke_fn = TaggedObj::boxed(
                    ObjTag::Unified,
                    crate::internal_data::UnifiedObject::native_func(native_proxy_revoke, proxy),
                );
                // 4. Let result be OrdinaryObjectCreate(%Object.prototype%).
                let result = __esc_rt_create_object();
                // 5. Perform ! CreateDataPropertyOrThrow(result, "proxy", p).
                let proxy_key = make_rt_string("proxy".to_string());
                __esc_rt_set_prop(result, proxy_key, proxy);
                // 6. Perform ! CreateDataPropertyOrThrow(result, "revoke", revoker).
                let revoke_key = make_rt_string("revoke".to_string());
                __esc_rt_set_prop(result, revoke_key, revoke_fn);
                // 7. Return result.
                Some(result)
            } else {
                None
            }
        }
        "Reflect" => dispatch_reflect_method(method, argc, argv),
        "Symbol" => dispatch_symbol_static_method(method, argc, argv),
        "Date" => dispatch_date_static_method(method, argc, argv),
        "process" => dispatch_process_method(method, argc, argv),
        _ => None,
    }
}

/// `Date.now ( )` / `Date.parse ( string )` / `Date.UTC ( year [ , month [ , ... ] ] )`
///
/// Dispatch a Date static method.
///
/// - `Date.now` — [spec]: <https://tc39.es/ecma262/#sec-date.now>
/// - `Date.parse` — [spec]: <https://tc39.es/ecma262/#sec-date.parse>
/// - `Date.UTC` — [spec]: <https://tc39.es/ecma262/#sec-date.utc>
///
/// Returns `Some(bits)` if the method is a known Date static method, `None` otherwise.
fn dispatch_date_static_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    match method {
        // === Date.now ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-date.now
        "now" => {
            // 1. Return the time value (UTC) identifying the current time.
            let ms = host::abi::__esc_host_now_ms();
            Some(JsValue::number(ms).raw_bits())
        }
        // === Date.parse ( string ) ===
        // [spec]: https://tc39.es/ecma262/#sec-date.parse
        "parse" => {
            // 1. Let string be ? ToString(value).
            let args = read_argv(argc, argv);
            let s = args
                .first()
                .map(|v| crate::string_ops::get_string_data(*v))
                .unwrap_or_default();
            // 2. Parse string as a date, in exactly the same manner as for the
            //    Date constructor (§21.4.2). Return the time value (UTC) for that date.
            //    If string is not parseable, return NaN.
            let ms = super::parse_date_string(&s);
            Some(JsValue::number(ms).raw_bits())
        }
        // === Date.UTC ( year [ , month [ , date [ , hours [ , minutes [ , seconds [ , ms ] ] ] ] ] ] ) ===
        // [spec]: https://tc39.es/ecma262/#sec-date.utc
        "UTC" => {
            // 1-8. Let y, m, dt, h, min, sec, milli be the respective arguments (or defaults).
            // 9. Let yr be MakeFullYear(y).
            // 10. Return TimeClip(MakeDate(MakeDay(yr, m, dt), MakeTime(h, min, sec, milli))).
            let args = read_argv(argc, argv);
            let ms = super::make_utc_from_components(&args);
            Some(JsValue::number(ms).raw_bits())
        }
        _ => None,
    }
}

/// `Symbol.for ( key )` / `Symbol.keyFor ( sym )`
///
/// Dispatch a Symbol static method.
///
/// - `Symbol.for` — [spec]: <https://tc39.es/ecma262/#sec-symbol.for>
/// - `Symbol.keyFor` — [spec]: <https://tc39.es/ecma262/#sec-symbol.keyfor>
///
/// Returns `Some(bits)` if the method is a known Symbol static method, `None` otherwise.
fn dispatch_symbol_static_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // === Symbol.for ( key ) ===
        // [spec]: https://tc39.es/ecma262/#sec-symbol.for
        "for" => {
            // 1. Let stringKey be ? ToString(key).
            let key = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // 2. For each element e of the GlobalSymbolRegistry List, do
            //    a. If SameValue(e.[[Key]], stringKey) is true, return e.[[Symbol]].
            // 3. Assert: GlobalSymbolRegistry does not currently contain an entry for stringKey.
            // 4. Let newSymbol be a new unique Symbol value whose [[Description]] is stringKey.
            // 5. Append the Record { [[Key]]: stringKey, [[Symbol]]: newSymbol } to GlobalSymbolRegistry.
            // 6. Return newSymbol.
            Some(super::__esc_rt_symbol_for(key))
        }
        // === Symbol.keyFor ( sym ) ===
        // [spec]: https://tc39.es/ecma262/#sec-symbol.keyfor
        "keyFor" => {
            // 1. If sym is not a Symbol, throw a TypeError exception.
            // TODO: Step 1 — no TypeError for non-symbol argument
            // 2. Return KeyForSymbol(sym).
            //    (Returns the key if sym is in GlobalSymbolRegistry, undefined otherwise.)
            let sym = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            Some(super::__esc_rt_symbol_key_for(sym))
        }
        _ => None,
    }
}

/// `Symbol.prototype.toString ( )` / `Symbol.prototype.valueOf ( )` /
/// `Symbol.prototype.description` (getter)
///
/// Dispatch a Symbol instance method.
///
/// - `toString` — [spec]: <https://tc39.es/ecma262/#sec-symbol.prototype.tostring>
/// - `valueOf` — [spec]: <https://tc39.es/ecma262/#sec-symbol.prototype.valueof>
/// - `description` — [spec]: <https://tc39.es/ecma262/#sec-symbol.prototype.description>
///
/// Returns `Some(bits)` if the method is a known Symbol prototype method, `None` otherwise.
pub(crate) fn dispatch_symbol_instance_method(sym_id: u32, method: &str) -> Option<u64> {
    match method {
        // === Symbol.prototype.toString ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-symbol.prototype.tostring
        "toString" => {
            // 1. Let sym be ? thisSymbolValue(this value).
            // TODO: Step 1 — no thisSymbolValue check; caller passes sym_id directly
            // 2. Return SymbolDescriptiveString(sym).
            //    SymbolDescriptiveString (§20.4.3.3.1):
            //    1. Let desc be sym's [[Description]] value.
            //    2. If desc is undefined, set desc to the empty String.
            //    3. Return the string-concatenation of "Symbol(", desc, and ")".
            let s = crate::symbol::symbol_to_string(sym_id);
            Some(make_rt_string(s))
        }
        // === Symbol.prototype.valueOf ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-symbol.prototype.valueof
        "valueOf" => {
            // 1. Return ? thisSymbolValue(this value).
            // TODO: Step 1 — no thisSymbolValue check; directly returns the symbol
            Some(JsValue::symbol(sym_id).raw_bits())
        }
        // === get Symbol.prototype.description ===
        // [spec]: https://tc39.es/ecma262/#sec-symbol.prototype.description
        "description" => {
            // 1. Let s be the this value.
            // 2. Let sym be ? thisSymbolValue(s).
            // TODO: Step 2 — no thisSymbolValue check
            // 3. Return sym.[[Description]].
            match crate::symbol::symbol_description(sym_id) {
                Some(desc) => Some(make_rt_string(desc)),
                None => Some(JsValue::undefined().raw_bits()),
            }
        }
        _ => None,
    }
}

// =========================================================================
// process method dispatch
// =========================================================================

/// Dispatch a method call on the `process` global namespace.
///
/// These are Node.js-compatible runtime methods, not part of the ECMAScript
/// specification. Implemented for Node.js API compatibility.
///
/// Returns `Some(result)` if the method is recognized, `None` otherwise.
fn dispatch_process_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    match method {
        // process.exit([code]) — Node.js API
        "exit" => {
            let args = read_argv(argc, argv);
            let code = args
                .first()
                .and_then(|v| v.as_int().or_else(|| v.as_number().map(|n| n as i32)))
                .unwrap_or(0);
            host::abi::__esc_host_exit(code);
        }
        // process.cwd() — Node.js API
        "cwd" => {
            let mut buf = vec![0u8; 4096];
            // SAFETY: buf is a valid mutable slice of known length.
            let len = unsafe { host::abi::__esc_host_cwd(buf.as_mut_ptr(), buf.len() as u32) };
            if len < 0 {
                return Some(make_rt_string(String::new()));
            }
            let actual_len = (len as usize).min(buf.len());
            let s = String::from_utf8_lossy(&buf[..actual_len]).into_owned();
            Some(make_rt_string(s))
        }
        // process.hrtime() — Node.js API
        // Returns [seconds, nanoseconds] as a two-element array.
        "hrtime" => {
            let ns = host::abi::__esc_host_hrtime_ns();
            let seconds = ns / 1_000_000_000;
            let remaining_ns = ns % 1_000_000_000;
            let elements = vec![
                JsValue::number(seconds as f64),
                JsValue::number(remaining_ns as f64),
            ];
            Some(create_array_from_elements(elements))
        }
        _ => None,
    }
}

// =========================================================================
// Object.prototype method dispatch
// =========================================================================

/// Try calling a native function property value.
///
/// If `prop_bits` is a `UnifiedObject` with `InternalKind::NativeFunc`
/// internal data, sets `CURRENT_ARGC`/`CURRENT_ARGV` thread-locals and
/// calls the function. Returns `Some(result)` on success, `None` otherwise.
///
/// This is an internal dispatch helper with no direct spec equivalent.
pub(crate) fn try_call_native_func_prop(
    prop_bits: u64,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let tag = read_obj_tag(prop_bits)?;
    if tag != ObjTag::Unified as u8 {
        return None;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(prop_bits) }?;
    if let Some(InternalData::NativeFunc { func, context }) = uni.internal_data() {
        // Set thread-locals so the NativeFunc can read its arguments.
        let prev_argc = super::CURRENT_ARGC.with(|cell| cell.replace(argc));
        let prev_argv = super::CURRENT_ARGV.with(|cell| cell.replace(argv));
        let result = func(*context);
        super::CURRENT_ARGC.with(|cell| cell.set(prev_argc));
        super::CURRENT_ARGV.with(|cell| cell.set(prev_argv));
        return Some(result);
    }
    None
}

/// `Function.prototype.toString ( )`
///
/// Dispatch `Function.prototype.toString()` for callable objects.
///
/// [spec]: <https://tc39.es/ecma262/#sec-function.prototype.tostring>
///
/// Returns a string like `"function name() { [native code] }"` following
/// the ES spec requirement that `Function.prototype.toString` returns a
/// string representation of the function.
pub(crate) fn dispatch_function_to_string(func_bits: u64) -> u64 {
    // 1. Let func be the this value.
    // 2. If func is a built-in function object, return an implementation-defined
    //    String source code representation of func. The representation must have
    //    the syntax of a NativeFunction.
    //    NativeFunction: `function PropertyName[~Yield, ~Await]opt ( FormalParameters[~Yield, ~Await] ) { [ native code ] }`
    // 3. If func is an Object with a [[SourceText]] internal slot, ...
    // TODO: Step 3 — no [[SourceText]] support; all functions show [native code]
    // 4. If func is an Object and IsCallable(func) is true, return an implementation-defined
    //    String source code representation of func (same NativeFunction syntax).
    // 5. Throw a TypeError exception.
    // TODO: Step 5 — no TypeError for non-callable this values

    // Use get_function_name which checks OBJECT_PROPS side-table first,
    // then falls back to InternalData. This handles NativeFunc objects
    // whose names are stored in the side-table (e.g., bound functions,
    // builtin method wrappers from G1/G3).
    let fn_name = super::dispatch_core::get_function_name(func_bits);
    let result = if fn_name.is_empty() {
        "function () { [native code] }".to_string()
    } else {
        format!("function {fn_name}() {{ [native code] }}")
    };
    make_rt_string(result)
}

/// `Object.prototype.toString ( )` / `.valueOf ( )` / `.hasOwnProperty ( V )` /
/// `.propertyIsEnumerable ( V )` / `.isPrototypeOf ( V )` / `.toLocaleString ( )`
///
/// Dispatch `Object.prototype` methods on any object.
///
/// - `toString` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.tostring>
/// - `valueOf` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.valueof>
/// - `hasOwnProperty` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.hasownproperty>
/// - `propertyIsEnumerable` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.propertyisenumerable>
/// - `isPrototypeOf` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.isprototypeof>
/// - `toLocaleString` — [spec]: <https://tc39.es/ecma262/#sec-object.prototype.tolocalestring>
///
/// Returns `Some(result)` if the method is a known `Object.prototype` method,
/// `None` otherwise.
pub(crate) fn dispatch_object_proto_method(
    obj: u64,
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    match method {
        // === Object.prototype.toString ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.tostring
        "toString" => {
            let val = JsValue::from_raw_bits(obj);
            // 1. If the this value is undefined, return "[object Undefined]".
            if val.is_undefined() {
                return Some(make_rt_string("[object Undefined]".to_string()));
            }
            // 2. If the this value is null, return "[object Null]".
            if val.is_null() {
                return Some(make_rt_string("[object Null]".to_string()));
            }
            // 3. Let O be ! ToObject(this value).
            // For primitives, determine the class without actually boxing.
            let tag_label = if val.is_bool() {
                "Boolean".to_string()
            } else if val.is_number() || val.is_int() {
                "Number".to_string()
            } else if val.is_string() {
                "String".to_string()
            } else if val.is_symbol() {
                "Symbol".to_string()
            } else {
                // 4-16. For objects, determine builtinTag from InternalKind / @@toStringTag.
                get_object_class_name(obj)
            };
            // 17. Return the string-concatenation of "[object ", tag, and "]".
            let result = format!("[object {tag_label}]");
            Some(make_rt_string(result))
        }
        // === Object.prototype.valueOf ( ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.valueof
        "valueOf" => {
            // 1. Return ? ToObject(this value).
            Some(obj)
        }
        // === Object.prototype.hasOwnProperty ( V ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.hasownproperty
        "hasOwnProperty" => {
            // 1. Let P be ? ToPropertyKey(V).
            let args = read_argv(argc, argv);
            let prop_name = args.first().map_or(String::new(), |v| {
                if v.is_string() {
                    string_ops::get_string_data(*v)
                } else {
                    display::display_value(*v)
                }
            });
            // 2. Let O be ? ToObject(this value).
            // 3. Return ? HasOwnProperty(O, P).
            let result = has_own_property_check(obj, &prop_name);
            Some(JsValue::bool(result).raw_bits())
        }
        // === Object.prototype.propertyIsEnumerable ( V ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.propertyisenumerable
        "propertyIsEnumerable" => {
            // 1. Let P be ? ToPropertyKey(V).
            let args = read_argv(argc, argv);
            let prop_name = args.first().map_or(String::new(), |v| {
                if v.is_string() {
                    string_ops::get_string_data(*v)
                } else {
                    display::display_value(*v)
                }
            });
            // 2. Let O be ? ToObject(this value).
            // 3. Let desc be ? O.[[GetOwnProperty]](P).
            // 4. If desc is undefined, return false.
            // 5. Return desc.[[Enumerable]].
            let result = property_is_enumerable_check(obj, &prop_name);
            Some(JsValue::bool(result).raw_bits())
        }
        // === Object.prototype.isPrototypeOf ( V ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.isprototypeof
        "isPrototypeOf" => {
            // 1. If V is not an Object, return false.
            let args = read_argv(argc, argv);
            let target = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // 2. Let O be ? ToObject(this value).
            // 3. Repeat,
            //    a. Set V to ? V.[[GetPrototypeOf]]().
            //    b. If V is null, return false.
            //    c. If SameValue(O, V) is true, return true.
            let result = is_prototype_of_check(obj, target);
            Some(JsValue::bool(result).raw_bits())
        }
        // === Object.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ) ===
        // [spec]: https://tc39.es/ecma262/#sec-object.prototype.tolocalestring
        "toLocaleString" => {
            // 1. Let O be the this value.
            // 2. Return ? Invoke(O, "toString").
            dispatch_object_proto_method(obj, "toString", argc, argv)
        }
        _ => None,
    }
}

/// `Object.prototype.toString ( )` — `[[Class]]` tag resolution.
///
/// Get the `[[Class]]` / `@@toStringTag` name for an object.
///
/// [spec]: <https://tc39.es/ecma262/#sec-object.prototype.tostring> (steps 4-16)
///
/// Returns the appropriate tag like `"Object"`, `"Array"`, `"Function"`, etc.
fn get_object_class_name(bits: u64) -> String {
    let Some(tag) = read_obj_tag(bits) else {
        return "Object".to_string();
    };
    if tag != ObjTag::Unified as u8 {
        return "Object".to_string();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    let Some(u) = uni else {
        return "Object".to_string();
    };
    // Step 15. Let tag be ? Get(O, @@toStringTag).
    // Step 16. If tag is not a String, set tag to builtinTag.
    let sym_key = JsValue::symbol(crate::symbol::SYMBOL_TO_STRING_TAG).raw_bits();
    let tag_val_bits = __esc_rt_get_prop(bits, sym_key);
    let tag_val = JsValue::from_raw_bits(tag_val_bits);
    if tag_val.is_string() {
        return string_ops::get_string_data(tag_val);
    }
    // Steps 4-14: Determine builtinTag based on internal slots / object kind.
    match u.kind {
        InternalKind::Array => "Array".to_string(),
        InternalKind::Function | InternalKind::Closure => "Function".to_string(),
        InternalKind::NativeFunc => "Function".to_string(),
        InternalKind::ErrorObj => "Error".to_string(),
        InternalKind::RegExpObj => "RegExp".to_string(),
        InternalKind::DateObj => "Date".to_string(),
        InternalKind::Promise => "Promise".to_string(),
        InternalKind::Generator => "Generator".to_string(),
        InternalKind::MapObj => "Map".to_string(),
        InternalKind::SetObj => "Set".to_string(),
        InternalKind::WeakMapObj => "WeakMap".to_string(),
        InternalKind::WeakSetObj => "WeakSet".to_string(),
        InternalKind::WeakRefObj => "WeakRef".to_string(),
        InternalKind::BooleanObj => "Boolean".to_string(),
        InternalKind::NumberObj => "Number".to_string(),
        InternalKind::StringObj => "String".to_string(),
        InternalKind::SymbolObj => "Symbol".to_string(),
        InternalKind::Proxy => "Object".to_string(),
        _ => "Object".to_string(),
    }
}

/// `HasOwnProperty ( O, P )` — check for own (non-inherited) property.
///
/// [spec]: <https://tc39.es/ecma262/#sec-hasownproperty>
///
/// Used by `Object.prototype.hasOwnProperty`.
pub(crate) fn has_own_property_check(obj_bits: u64, prop_name: &str) -> bool {
    // 1. Let desc be ? O.[[GetOwnProperty]](P).
    // 2. If desc is undefined, return false.
    // 3. Return true.
    let Some(tag) = read_obj_tag(obj_bits) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(obj_bits) };
    let Some(u) = uni else {
        return false;
    };
    match u.kind {
        InternalKind::Array => {
            if prop_name == "length" {
                return true;
            }
            // Check if this property was deleted (tombstone workaround).
            // Must be checked before shape/element lookup to respect OrdinaryDelete.
            let is_deleted = super::DELETED_PROPS.with(|dp| {
                dp.borrow()
                    .get(&obj_bits)
                    .is_some_and(|s| s.contains(prop_name))
            });
            if is_deleted {
                return false;
            }
            // Check integer indices: either in dense element storage OR in shape table
            // (Object.defineProperty stores in shape, not dense elements).
            if let Ok(idx) = prop_name.parse::<usize>() {
                let in_dense = u.as_array_length().is_some_and(|len| idx < len as usize);
                if in_dense {
                    return true;
                }
                // Also check shape table — defineProperty may have stored it there
                return super::SHAPES.with(|shapes| {
                    super::INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.has_own_property(prop_name, &shapes, &interner)
                    })
                });
            }
            // Check shape properties (for non-index named properties on arrays)
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(prop_name, &shapes, &interner)
                })
            })
        }
        InternalKind::Function | InternalKind::Closure | InternalKind::NativeFunc => {
            // Check OBJECT_PROPS first
            let in_obj_props = super::OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props
                    .get(&obj_bits)
                    .is_some_and(|m| m.contains_key(prop_name))
            });
            if in_obj_props {
                return true;
            }
            // Well-known function properties
            if prop_name == "name" || prop_name == "length" {
                return true;
            }
            if prop_name == "prototype" {
                // Check if prototype was set in OBJECT_PROPS
                return super::OBJECT_PROPS.with(|props| {
                    let props = props.borrow();
                    props
                        .get(&obj_bits)
                        .is_some_and(|m| m.contains_key("prototype"))
                });
            }
            // Check shape properties
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(prop_name, &shapes, &interner)
                })
            })
        }
        InternalKind::ErrorObj => {
            // "name", "message", "stack" are stored in InternalData::Error.
            // All other properties may be in shape slots (set via set_prop).
            if matches!(prop_name, "name" | "message" | "stack") {
                return true;
            }
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(prop_name, &shapes, &interner)
                })
            })
        }
        InternalKind::IterResult => {
            matches!(prop_name, "value" | "done")
        }
        _ => {
            // Ordinary objects (and all others): check DELETED_PROPS first,
            // then fall back to shape table lookup.
            let is_deleted = super::DELETED_PROPS.with(|dp| {
                dp.borrow()
                    .get(&obj_bits)
                    .is_some_and(|s| s.contains(prop_name))
            });
            if is_deleted {
                return false;
            }
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(prop_name, &shapes, &interner)
                })
            })
        }
    }
}

/// `Object.prototype.propertyIsEnumerable ( V )` — enumerable own property check.
///
/// [spec]: <https://tc39.es/ecma262/#sec-object.prototype.propertyisenumerable>
///
/// Used by `Object.prototype.propertyIsEnumerable`.
fn property_is_enumerable_check(obj_bits: u64, prop_name: &str) -> bool {
    // 1. Let P be ? ToPropertyKey(V).
    // 2. Let O be ? ToObject(this value).
    // 3. Let desc be ? O.[[GetOwnProperty]](P).
    // 4. If desc is undefined, return false.
    // 5. Return desc.[[Enumerable]].
    let Some(tag) = read_obj_tag(obj_bits) else {
        return false;
    };
    if tag != ObjTag::Unified as u8 {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(obj_bits) };
    let Some(u) = uni else {
        return false;
    };
    match u.kind {
        InternalKind::Array => {
            // Array indices are enumerable, "length" is not
            if prop_name == "length" {
                return false;
            }
            if let Ok(idx) = prop_name.parse::<usize>()
                && let Some(len) = u.as_array_length()
            {
                return idx < len as usize;
            }
            // Check shape properties for enumerability
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_property_descriptor(prop_name, &shapes, &interner)
                        .is_some_and(|d| d.is_enumerable())
                })
            })
        }
        InternalKind::Function | InternalKind::Closure | InternalKind::NativeFunc => {
            // name, length, prototype are not enumerable per spec
            if matches!(prop_name, "name" | "length" | "prototype") {
                return false;
            }
            // Check shape/OBJECT_PROPS
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_property_descriptor(prop_name, &shapes, &interner)
                        .is_some_and(|d| d.is_enumerable())
                })
            })
        }
        _ => {
            // For ordinary objects, check the property descriptor
            super::SHAPES.with(|shapes| {
                super::INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_property_descriptor(prop_name, &shapes, &interner)
                        .is_some_and(|d| d.is_enumerable())
                })
            })
        }
    }
}

/// `Object.prototype.isPrototypeOf ( V )`
///
/// Check if `proto_obj` is in the prototype chain of `target_obj`.
///
/// [spec]: <https://tc39.es/ecma262/#sec-object.prototype.isprototypeof>
fn is_prototype_of_check(proto_obj: u64, target_obj: u64) -> bool {
    // 1. If V is not an Object, return false.
    let target_val = JsValue::from_raw_bits(target_obj);
    if !target_val.is_object() {
        return false;
    }
    // 2. Let O be ? ToObject(this value).
    // (proto_obj is already an object — it's the `this` value)
    // 3. Repeat,
    //    a. Set V to ? V.[[GetPrototypeOf]]().
    //    b. If V is null, return false.
    //    c. If SameValue(O, V) is true, return true.
    let mut current = super::object::get_prototype_of(target_obj);
    for _ in 0..100 {
        let current_val = JsValue::from_raw_bits(current);
        // 3b. If V is null, return false.
        if current_val.is_null() || current_val.is_undefined() {
            break;
        }
        // 3c. If SameValue(O, V) is true, return true.
        if current == proto_obj {
            return true;
        }
        // 3a. Set V to ? V.[[GetPrototypeOf]]().
        current = super::object::get_prototype_of(current);
    }
    false
}
