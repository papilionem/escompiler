//! Type conversion and coercion runtime functions.
//!
//! Contains ECMAScript abstract operations like `ToBoolean`, `ToNumber`,
//! `ToString`, `typeof`, `instanceof`, and related coercions.

use nanbox::JsValue;

use crate::exceptions;
use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::symbol;
use crate::tagged_obj::{ObjTag, deref_tagged, read_obj_tag};
use crate::{display, string_ops, value_ops};

use super::{
    __esc_rt_get_prop, is_builtin_callable_name, is_builtin_namespace_name, make_rt_string,
};

// =========================================================================
// Error helper
// =========================================================================

/// Extract the error tag from a `UnifiedObject` with `InternalKind::ErrorObj`.
///
/// Returns `None` if the value is not an error object.
fn extract_error_tag(bits: u64) -> Option<u32> {
    let tag = read_obj_tag(bits)?;
    if tag != ObjTag::Unified as u8 {
        return None;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) }?;
    if uni.kind == InternalKind::ErrorObj
        && let Some(InternalData::Error { error_tag, .. }) = uni.internal_data()
    {
        return Some(*error_tag);
    }
    None
}

// =========================================================================
// Conversion
// =========================================================================

/// `ToBoolean ( argument )`
///
/// Converts a value to a boolean.
///
/// [spec]: https://tc39.es/ecma262/#sec-toboolean
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_boolean(val: u64) -> u8 {
    // 1. If argument is a Boolean, return argument.
    // 2. If argument is one of undefined, null, +0, -0, NaN, 0n, or
    //    the empty String, return false.
    // 3. NOTE: This step is replaced in section B.3.6.1.
    // 4. Return true.
    // (All steps are handled by value_ops::to_boolean.)
    let result = value_ops::to_boolean(JsValue::from_raw_bits(val));
    result as u8
}

/// `ToNumber ( argument )`
///
/// Converts a value to a Number.
///
/// [spec]: https://tc39.es/ecma262/#sec-tonumber
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_number(val: u64) -> u64 {
    // 1. If argument is a Number, return argument.
    // 2. If argument is either a Symbol or a BigInt, throw a TypeError exception.
    // 3. If argument is undefined, return NaN.
    // 4. If argument is either null or false, return +0.
    // 5. If argument is true, return 1.
    // 6. If argument is a String, return StringToNumber(argument).
    // 7. Assert: argument is an Object.
    // 8. Let primValue be ? ToPrimitive(argument, number).
    // 9. Assert: primValue is not an Object.
    // 10. Return ? ToNumber(primValue).
    // (All steps are handled by value_ops::to_number.)
    let result = value_ops::to_number(JsValue::from_raw_bits(val));
    JsValue::number(result).raw_bits()
}

/// `ToString ( argument )`
///
/// Converts a value to a String.
///
/// For objects, calls `ToPrimitive(hint: String)` first to obtain a
/// primitive, then converts that primitive to its string representation.
/// This ensures custom `toString()` / `valueOf()` methods are invoked.
///
/// Per ES2024 `ToString(Symbol)` throws a TypeError. Use
/// `display::display_value` for non-throwing display (e.g., `console.log`).
///
/// [spec]: https://tc39.es/ecma262/#sec-tostring
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_string(val: u64) -> u64 {
    let v = JsValue::from_raw_bits(val);
    // 1. If argument is a String, return argument.
    //    (Handled implicitly — strings pass through display_value below.)
    // 2. If argument is a Symbol, throw a TypeError exception.
    if v.is_symbol() {
        let msg =
            make_rt_string("TypeError: Cannot convert a Symbol value to a string".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        // Return empty string as fallback after throw (exception is pending)
        let rt_str = Box::new(string_ops::RtString::new(String::new()));
        let raw_ptr = Box::into_raw(rt_str) as *const ();
        return JsValue::string(raw_ptr).raw_bits();
    }
    // 3. If argument is undefined, return "undefined".
    // 4. If argument is null, return "null".
    // 5. If argument is true, return "true".
    // 6. If argument is false, return "false".
    // 7. If argument is a Number, return Number::toString(argument, 10).
    //    (Steps 3-7 handled by display::display_value below.)
    // 8. If argument is a BigInt, return BigInt::toString(argument, 10).
    //    TODO: Step 8 — BigInt not yet supported.
    // 9. Assert: argument is an Object.
    // 10. Let primValue be ? ToPrimitive(argument, string).
    let prim = if v.is_object() {
        value_ops::to_primitive(v, value_ops::ToPrimitiveHint::String)
    } else {
        v
    };
    // 11. Assert: primValue is not an Object.
    // 12. Return ? ToString(primValue).
    //     (Recursive call — if ToPrimitive returned a symbol, throw TypeError.)
    if prim.is_symbol() {
        let msg =
            make_rt_string("TypeError: Cannot convert a Symbol value to a string".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        let rt_str = Box::new(string_ops::RtString::new(String::new()));
        let raw_ptr = Box::into_raw(rt_str) as *const ();
        return JsValue::string(raw_ptr).raw_bits();
    }
    let s = display::display_value(prim);
    let rt_str = Box::new(string_ops::RtString::new(s));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr).raw_bits()
}

/// `typeof` Operator — Runtime Semantics: Evaluation
///
/// Returns a NaN-boxed string `JsValue` with the type name.
///
/// Handles tagged objects (closures, functions, native funcs) as well as
/// string-valued built-in globals that represent callable constructors/functions
/// (like `Object`, `parseInt`) or namespace objects (like `Math`, `JSON`).
///
/// [spec]: https://tc39.es/ecma262/#sec-typeof-operator
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_typeof(val: u64) -> u64 {
    let v = JsValue::from_raw_bits(val);
    // Table 41 — typeof Operator Results:
    //   Undefined   -> "undefined"
    //   Null        -> "object"
    //   Boolean     -> "boolean"
    //   Number      -> "number"
    //   String      -> "string"
    //   Symbol      -> "symbol"
    //   BigInt      -> "bigint"        (TODO: BigInt not yet supported)
    //   Object (not callable) -> "object"
    //   Object (callable / [[Call]]) -> "function"
    //
    // The implementation below extends the spec table with AOT-specific
    // handling for unified objects and string-valued builtin globals.
    let type_str = if v.is_object() {
        let tag = read_obj_tag(val);
        if tag == Some(ObjTag::Unified as u8) {
            // SAFETY: tag check confirms this is a unified object.
            let uni = unsafe { deref_tagged::<UnifiedObject>(val) };
            if let Some(u) = uni {
                if u.kind == crate::internal_data::InternalKind::SymbolObj {
                    // Symbol wrapper objects report "symbol" (not "object")
                    "symbol"
                } else if u.is_callable() {
                    // Object implements [[Call]] -> "function"
                    "function"
                } else {
                    // Object without [[Call]] -> "object"
                    value_ops::js_typeof(v)
                }
            } else {
                value_ops::js_typeof(v)
            }
        } else {
            value_ops::js_typeof(v)
        }
    } else if v.is_string() {
        // Built-in globals are represented as string constants at runtime.
        // Check if the string names a callable constructor/function or a namespace.
        let name = string_ops::get_string_data(v);
        if is_builtin_callable_name(&name) {
            "function"
        } else if is_builtin_namespace_name(&name) {
            "object"
        } else {
            "string"
        }
    } else {
        value_ops::js_typeof(v)
    };
    let rt_str = Box::new(string_ops::RtString::new(type_str.to_string()));
    let raw_ptr = Box::into_raw(rt_str) as *const ();
    JsValue::string(raw_ptr).raw_bits()
}

/// `typeof` on a boxed value — same as `__esc_rt_typeof` but used by
/// opcodes that already have a boxed value (e.g., `TypeofBoxed`).
///
/// [spec]: https://tc39.es/ecma262/#sec-typeof-operator
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_typeof_boxed(val: u64) -> u64 {
    __esc_rt_typeof(val)
}

/// Check if a NaN-boxed value is nullish (`null` or `undefined`).
///
/// Returns a NaN-boxed boolean. A value is nullish if it is `null` or
/// `undefined`. This is used to implement the `?.` (optional chaining)
/// and `??` (nullish coalescing) operators.
///
/// Note: This is not a standalone spec abstract operation, but used
/// internally to implement `?.` and `??` operators.
///
/// [spec-optional-chaining]: https://tc39.es/ecma262/#sec-optional-chaining-evaluation
/// [spec-nullish-coalescing]: https://tc39.es/ecma262/#sec-binary-logical-operators
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_is_nullish(val: u64) -> u64 {
    let v = JsValue::from_raw_bits(val);
    JsValue::bool(v.is_nullish()).raw_bits()
}

/// Check if a NaN-boxed value is falsy, returning a NaN-boxed boolean.
///
/// Equivalent to `!ToBoolean(argument)`. Uses `value_ops::to_boolean`
/// for full ECMAScript compliance, including empty-string falsiness that
/// the nanbox-level `is_falsy()` cannot detect.
///
/// [spec]: https://tc39.es/ecma262/#sec-toboolean
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_is_falsy(val: u64) -> u64 {
    // Returns the logical NOT of ToBoolean(argument).
    // See __esc_rt_to_boolean for the full ToBoolean algorithm steps.
    let v = JsValue::from_raw_bits(val);
    JsValue::bool(!value_ops::to_boolean(v)).raw_bits()
}

/// `IsCallable ( argument )`
///
/// Determines whether a value has a `[[Call]]` internal method.
/// Returns `true` for closures, functions, native functions, and other
/// callable unified objects. Used by `instanceof` to guard against
/// non-callable RHS values.
///
/// [spec]: https://tc39.es/ecma262/#sec-iscallable
fn is_ctor_callable(bits: u64) -> bool {
    // 1. If argument is not an Object, return false.
    let Some(tag) = read_obj_tag(bits) else {
        return false;
    };
    // 2. If argument has a [[Call]] internal method, return true.
    if tag == ObjTag::Unified as u8 {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
        if let Some(u) = uni {
            return u.is_callable();
        }
    }
    // 3. Return false.
    false
}

/// Resolve the effective constructor for `instanceof` by unwrapping bound functions.
///
/// Per ES2024 `OrdinaryHasInstance` (step 2), if the constructor is a bound
/// function, we must follow the `[[BoundTargetFunction]]` chain to find the original
/// constructor whose `prototype` property to check.
///
/// Returns the resolved constructor's bits, or the original bits if not a bound function.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryhasinstance (step 2)
fn resolve_bound_target(ctor: u64) -> u64 {
    let mut current = ctor;
    // Max 100 hops to prevent infinite loops (same as prototype chain limit)
    for _ in 0..100 {
        let tag = read_obj_tag(current);
        if tag != Some(ObjTag::Unified as u8) {
            break;
        }
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(current) };
        let Some(u) = uni else { break };
        // Check if this is a NativeFunc with bound function data
        if u.kind != InternalKind::NativeFunc {
            break;
        }
        if let Some(InternalData::NativeFunc { context, .. }) = u.internal_data() {
            // The context pointer for bound functions points to BoundFunctionData.
            // We can detect a bound function because its context is a heap pointer
            // to BoundFunctionData. The bound function trampoline is the `func` field.
            // To safely detect bound functions, check if the target field at that
            // context address is a valid object. Only follow if it looks like a bound fn.
            if *context == 0 {
                break;
            }
            // Try to read the target from what might be BoundFunctionData.
            // The first field is `target: u64`.
            let potential_target = unsafe {
                // SAFETY: context was created by Box::into_raw in dispatch_function_bind.
                // We read only the first u64 field (target) which is always at offset 0.
                let ptr = *context as *const u64;
                if ptr.is_null() {
                    break;
                }
                *ptr
            };
            // Verify the potential target is a valid object
            let target_val = JsValue::from_raw_bits(potential_target);
            if target_val.is_object() {
                // Check if the target has a `prototype` property (functions do, bound data doesn't)
                let proto_key = make_rt_string("prototype".to_string());
                let target_proto = __esc_rt_get_prop(potential_target, proto_key);
                if !JsValue::from_raw_bits(target_proto).is_undefined() {
                    current = potential_target;
                    continue;
                }
            }
            break;
        }
        break;
    }
    current
}

/// `InstanceofOperator ( V, target )`
///
/// Implements the `instanceof` operator and delegates to `OrdinaryHasInstance`.
///
/// [spec-instanceof]: https://tc39.es/ecma262/#sec-instanceofoperator
/// [spec-ordinaryhasinstance]: https://tc39.es/ecma262/#sec-ordinaryhasinstance
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_instanceof(obj: u64, ctor: u64) -> u64 {
    let ctor_val = JsValue::from_raw_bits(ctor);

    // InstanceofOperator step 1: If target is not an Object, throw a TypeError.
    // (String-valued builtins are our internal representation for globals, so
    // they are allowed through.)
    if !ctor_val.is_object() && !ctor_val.is_string() {
        let msg =
            make_rt_string("TypeError: Right-hand side of instanceof is not an object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // InstanceofOperator step 2: Let instOfHandler be ? GetMethod(target, @@hasInstance).
    // InstanceofOperator step 3: If instOfHandler is not undefined, then
    //   a. Return ! ToBoolean(? Call(instOfHandler, target, << V >>)).
    if ctor_val.is_object() {
        let sym_key = JsValue::symbol(symbol::SYMBOL_HAS_INSTANCE).raw_bits();
        let has_instance_prop = __esc_rt_get_prop(ctor, sym_key);
        let has_instance_val = JsValue::from_raw_bits(has_instance_prop);
        if !has_instance_val.is_undefined()
            && let Some(result) =
                value_ops::try_call_symbol_method(ctor, symbol::SYMBOL_HAS_INSTANCE, &[obj])
        {
            return JsValue::bool(value_ops::to_boolean(result)).raw_bits();
        }

        // InstanceofOperator step 4: If IsCallable(target) is false, throw a TypeError.
        if !is_ctor_callable(ctor) {
            let msg = make_rt_string(
                "TypeError: Right-hand side of instanceof is not callable".to_string(),
            );
            let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
            super::__esc_rt_throw(err);
            return JsValue::undefined().raw_bits();
        }
    }

    let v = JsValue::from_raw_bits(obj);

    // (ESCompiler-specific) Handle built-in error types.
    // Error objects use InternalKind::ErrorObj with an error_tag, so we check
    // the tag against the constructor name. This works for both string-valued
    // constructors (from desugar) and object-valued constructors (from LoadGlobal).
    if v.is_object()
        && let Some(error_tag) = extract_error_tag(obj)
    {
        // Resolve the constructor name from either a string or a NativeFunc object.
        let ctor_name = if ctor_val.is_string() {
            Some(string_ops::get_string_data(ctor_val))
        } else {
            // NativeFunc constructor — look up its "name" from OBJECT_PROPS
            super::OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&ctor).and_then(|m| {
                    m.get("name")
                        .map(|&bits| string_ops::get_string_data(JsValue::from_raw_bits(bits)))
                })
            })
        };
        if let Some(name) = ctor_name {
            let matches = match name.as_str() {
                "Error" => true, // all errors are instanceof Error
                "TypeError" => error_tag == exceptions::error_tag::TYPE_ERROR,
                "RangeError" => error_tag == exceptions::error_tag::RANGE_ERROR,
                "ReferenceError" => error_tag == exceptions::error_tag::REFERENCE_ERROR,
                "SyntaxError" => error_tag == exceptions::error_tag::SYNTAX_ERROR,
                "URIError" => error_tag == exceptions::error_tag::URI_ERROR,
                "EvalError" => error_tag == exceptions::error_tag::EVAL_ERROR,
                _ => false,
            };
            if matches {
                return JsValue::bool(true).raw_bits();
            }
        }
    }

    // InstanceofOperator step 5: Return ? OrdinaryHasInstance(target, V).
    // For string-valued builtins (e.g., "Object", "Array"), resolve to the
    // actual constructor object so we can walk the prototype chain correctly.
    let effective_ctor_bits = if ctor_val.is_string() {
        let name = string_ops::get_string_data(ctor_val);
        let global_bits = super::get_global_object(&name);
        if global_bits == JsValue::undefined().raw_bits() {
            return JsValue::bool(false).raw_bits();
        }
        global_bits
    } else if !ctor_val.is_object() {
        return JsValue::bool(false).raw_bits();
    } else {
        ctor
    };

    // OrdinaryHasInstance step 2: If F is a BoundFunctionExoticObject, then
    //   a. Let BC be F.[[BoundTargetFunction]].
    //   b. Return ? InstanceofOperator(O, BC).
    let effective_ctor = resolve_bound_target(effective_ctor_bits);

    // OrdinaryHasInstance step 3: Let P be ? Get(F, "prototype").
    let proto_key = make_rt_string("prototype".to_string());
    let ctor_proto = __esc_rt_get_prop(effective_ctor, proto_key);
    let ctor_proto_val = JsValue::from_raw_bits(ctor_proto);
    // OrdinaryHasInstance step 4: If P is not an Object, throw a TypeError.
    // (We return false instead — matches engine behavior for missing prototype.)
    if !ctor_proto_val.is_object() {
        return JsValue::bool(false).raw_bits();
    }

    // OrdinaryHasInstance step 5: Repeat,
    //   a. Set O to ? O.[[GetPrototypeOf]]().
    //   b. If O is null, return false.
    //   c. If SameValue(P, O) is true, return true.
    ordinary_has_instance(obj, ctor_proto)
}

/// `OrdinaryHasInstance ( C, O )` — prototype chain walk (step 5).
///
/// Walks the prototype chain of `obj` looking for `target_proto`.
///
/// Uses both the shape-based prototype mechanism (via `PROTO_OBJECTS` registry)
/// and legacy `__proto__` property lookup. Compares prototype objects by raw
/// NaN-boxed bits (pointer identity). Limited to 100 hops to prevent infinite loops.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryhasinstance
fn ordinary_has_instance(obj: u64, target_proto: u64) -> u64 {
    // OrdinaryHasInstance step 1: If IsCallable(C) is false, return false.
    //   (Already checked by the caller before reaching here.)

    // OrdinaryHasInstance step 5 precondition: If O is not an Object, return false.
    let obj_val = JsValue::from_raw_bits(obj);
    if !obj_val.is_object() {
        return JsValue::bool(false).raw_bits();
    }

    // OrdinaryHasInstance step 5: Repeat,
    //   a. Set O to ? O.[[GetPrototypeOf]]().
    //   b. If O is null, return false.
    //   c. If SameValue(P, O) is true, return true.
    let mut current = obj;
    for _ in 0..100 {
        let tag = read_obj_tag(current);
        if tag != Some(ObjTag::Unified as u8) {
            break;
        }
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(current) };
        let Some(u) = uni else { break };

        // Step 5a: Set O to ? O.[[GetPrototypeOf]]().
        let proto_bits = super::get_prototype_object(u);
        let Some(proto) = proto_bits else { break };

        // Step 5b: If O is null, return false.
        let proto_val = JsValue::from_raw_bits(proto);
        if proto_val.is_null() || proto_val.is_undefined() {
            break;
        }
        // Step 5c: If SameValue(P, O) is true, return true.
        if proto == target_proto {
            return JsValue::bool(true).raw_bits();
        }
        current = proto;
    }

    // Fallback: also try the __proto__ string property chain walk.
    // This handles cases where shape-based prototype is not set up
    // (e.g., objects created by non-standard paths).
    let proto_link_key = make_rt_string("__proto__".to_string());
    let mut current = __esc_rt_get_prop(obj, proto_link_key);
    for _ in 0..100 {
        let current_val = JsValue::from_raw_bits(current);
        // Step 5b: If O is null, return false.
        if current_val.is_null() || current_val.is_undefined() {
            break;
        }
        // Step 5c: If SameValue(P, O) is true, return true.
        if current == target_proto {
            return JsValue::bool(true).raw_bits();
        }
        // Step 5a: Set O to ? O.[[GetPrototypeOf]]().
        current = __esc_rt_get_prop(current, make_rt_string("__proto__".to_string()));
    }

    JsValue::bool(false).raw_bits()
}

// =========================================================================
// Error type helpers
// =========================================================================

/// Throw a `TypeError` for a non-callable invocation attempt.
///
/// Per ES2024 `EvaluateCall`, calling a non-callable value throws a TypeError
/// of the form `"X is not a function"`. The `callee_desc` argument is a
/// human-readable description of what was called (e.g., `"undefined"`,
/// `"obj.foo"`).
///
/// [spec]: https://tc39.es/ecma262/#sec-evaluatecall (step 2)
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_throw_not_callable(callee_desc: u64) -> u64 {
    let desc_val = JsValue::from_raw_bits(callee_desc);
    let desc_str = if desc_val.is_string() {
        string_ops::get_string_data(desc_val)
    } else {
        display::display_value(desc_val)
    };
    let msg = make_rt_string(format!("{desc_str} is not a function"));
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Throw a `ReferenceError` for a TDZ (Temporal Dead Zone) violation.
///
/// Per ES2024 `GetBindingValue` for declarative environment records,
/// accessing a `let`/`const`/`class` binding before its declaration in
/// the same scope throws a `ReferenceError`.
///
/// `var_name` is the NaN-boxed string name of the variable.
///
/// [spec]: https://tc39.es/ecma262/#sec-declarative-environment-records-getbindingvalue-n-s
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_throw_tdz(var_name: u64) -> u64 {
    let name_val = JsValue::from_raw_bits(var_name);
    let name_str = if name_val.is_string() {
        string_ops::get_string_data(name_val)
    } else {
        "unknown".to_string()
    };
    let msg = make_rt_string(format!("Cannot access '{name_str}' before initialization"));
    let err = super::__esc_rt_create_error(exceptions::error_tag::REFERENCE_ERROR, msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Throw a `TypeError` for strict-mode assignment to an immutable binding.
///
/// Per ES2024 `SetMutableBinding` for declarative environment records,
/// assigning to a `const` variable or an otherwise immutable binding
/// throws a TypeError.
///
/// `var_name` is the NaN-boxed string name of the binding.
///
/// [spec]: https://tc39.es/ecma262/#sec-declarative-environment-records-setmutablebinding-n-v-s
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_throw_const_assign(var_name: u64) -> u64 {
    let name_val = JsValue::from_raw_bits(var_name);
    let name_str = if name_val.is_string() {
        string_ops::get_string_data(name_val)
    } else {
        "unknown".to_string()
    };
    let msg = make_rt_string(format!("Assignment to constant variable '{name_str}'"));
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Throw a `TypeError` for mutation of a frozen or sealed object property.
///
/// Per ES2024 `[[Set]]` on ordinary objects, if the property is not writable
/// (e.g., the object is frozen or sealed), a TypeError is thrown in strict mode.
///
/// `prop_name` is the NaN-boxed string property name, `obj_desc` describes
/// the object state (`"frozen"` or `"sealed"`).
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinarysetwithowndescriptor (step 2.b.i)
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_throw_frozen_sealed(prop_name: u64, obj_desc: u64) -> u64 {
    let name_val = JsValue::from_raw_bits(prop_name);
    let name_str = if name_val.is_string() {
        string_ops::get_string_data(name_val)
    } else {
        display::display_value(name_val)
    };
    let desc_val = JsValue::from_raw_bits(obj_desc);
    let desc_str = if desc_val.is_string() {
        string_ops::get_string_data(desc_val)
    } else {
        "frozen".to_string()
    };
    let msg = make_rt_string(format!(
        "Cannot assign to read only property '{name_str}' of object '#<Object>' ({desc_str})"
    ));
    let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

// =========================================================================
// Numeric conversions
// =========================================================================

/// `ToNumeric ( value )`
///
/// Returns a numeric value (Number or BigInt). Currently delegates to
/// `ToNumber` since BigInt is not yet supported.
///
/// [spec]: https://tc39.es/ecma262/#sec-tonumeric
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_numeric(val: u64) -> u64 {
    // 1. Let primValue be ? ToPrimitive(value, number).
    // 2. If primValue is a BigInt, return primValue.
    //    TODO: Step 2 — BigInt not yet supported.
    // 3. Return ? ToNumber(primValue).
    __esc_rt_to_number(val)
}

/// `ToObject ( argument )`
///
/// Converts a value to an Object per ES2024 Table 16.
///
/// - **Undefined / Null** — throws a TypeError.
/// - **Boolean / Number / String / Symbol** — returns a new wrapper object.
/// - **Object** — returns the argument unchanged.
///
/// [spec]: https://tc39.es/ecma262/#sec-toobject
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_object(val: u64) -> u64 {
    use crate::tagged_obj::TaggedObj;
    use shapes::ShapeTable;

    let v = JsValue::from_raw_bits(val);

    // Table 16 — ToObject Conversions:

    // 1. If argument is undefined, throw a TypeError exception.
    if v.is_undefined() {
        let msg = make_rt_string("Cannot convert undefined to object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 2. If argument is null, throw a TypeError exception.
    if v.is_null() {
        let msg = make_rt_string("Cannot convert null to object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 3. If argument is a Boolean, return a new Boolean object whose
    //    [[BooleanData]] internal slot is set to argument.
    if v.is_bool() {
        let obj = UnifiedObject::boolean_wrapper(ShapeTable::EMPTY_SHAPE, val);
        let bits = TaggedObj::boxed(ObjTag::Unified, obj);
        let proto = super::get_or_create_builtin_prototype("Boolean");
        super::dispatch_core::set_prototype_on_new_object(bits, proto);
        return bits;
    }

    // 4. If argument is a Number, return a new Number object whose
    //    [[NumberData]] internal slot is set to argument.
    if v.is_number() || v.is_int() {
        let obj = UnifiedObject::number_wrapper(ShapeTable::EMPTY_SHAPE, val);
        let bits = TaggedObj::boxed(ObjTag::Unified, obj);
        let proto = super::get_or_create_builtin_prototype("Number");
        super::dispatch_core::set_prototype_on_new_object(bits, proto);
        return bits;
    }

    // 5. If argument is a String, return a new String object whose
    //    [[StringData]] internal slot is set to argument.
    if v.is_string() {
        let obj = UnifiedObject::string_wrapper(ShapeTable::EMPTY_SHAPE, val);
        let bits = TaggedObj::boxed(ObjTag::Unified, obj);
        let proto = super::get_or_create_builtin_prototype("String");
        super::dispatch_core::set_prototype_on_new_object(bits, proto);
        return bits;
    }

    // 6. If argument is a Symbol, return a new Symbol object whose
    //    [[SymbolData]] internal slot is set to argument.
    if let Some(sym_id) = v.as_symbol() {
        let obj = UnifiedObject::symbol(ShapeTable::EMPTY_SHAPE, sym_id as u64);
        return TaggedObj::boxed(ObjTag::Unified, obj);
    }

    // 7. If argument is a BigInt, return a new BigInt object whose
    //    [[BigIntData]] internal slot is set to argument.
    //    TODO: BigInt not yet supported.

    // 8. Assert: argument is an Object. Return argument.
    val
}

/// `RequireObjectCoercible ( argument )`
///
/// Throws a TypeError if the argument is undefined or null.
/// Returns the argument unchanged for all other types.
///
/// [spec]: https://tc39.es/ecma262/#sec-requireobjectcoercible
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_require_object_coercible(val: u64) -> u64 {
    let v = JsValue::from_raw_bits(val);

    // 1. If argument is undefined, throw a TypeError exception.
    if v.is_undefined() {
        let msg = make_rt_string("Cannot convert undefined or null to object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 2. If argument is null, throw a TypeError exception.
    if v.is_null() {
        let msg = make_rt_string("Cannot convert undefined or null to object".to_string());
        let err = super::__esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // 3. Return argument.
    val
}

/// `ToPrimitive ( input [ , preferredType ] )`
///
/// Converts a value to a primitive. Uses `Default` hint, which for ordinary
/// objects follows the `number` hint order: `valueOf` then `toString`.
///
/// [spec]: https://tc39.es/ecma262/#sec-toprimitive
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_primitive(val: u64) -> u64 {
    // 1. If input is an Object, then
    //   a. Let exoticToPrim be ? GetMethod(input, @@toPrimitive).
    //   b. If exoticToPrim is not undefined, then
    //      i. If preferredType is not present, let hint be "default".
    //      ii. Else if preferredType is string, let hint be "string".
    //      iii. Else, let hint be "number".
    //      iv. Let result be ? Call(exoticToPrim, input, << hint >>).
    //      v. If result is not an Object, return result.
    //      vi. Throw a TypeError exception.
    //   c. If preferredType is not present, let preferredType be number.
    //   d. Return ? OrdinaryToPrimitive(input, preferredType).
    // 2. Return input.
    // (All steps are handled by value_ops::to_primitive.)
    let v = JsValue::from_raw_bits(val);
    let prim = value_ops::to_primitive(v, value_ops::ToPrimitiveHint::Default);
    prim.raw_bits()
}

/// `ToPropertyKey ( argument )`
///
/// Converts a value to a property key (String or Symbol).
///
/// [spec]: https://tc39.es/ecma262/#sec-topropertykey
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_property_key(val: u64) -> u64 {
    let v = JsValue::from_raw_bits(val);
    // 1. Let key be ? ToPrimitive(argument, string).
    // 2. If key is a Symbol, then
    //   a. Return key.
    if v.is_symbol() {
        return val;
    }
    if v.is_object() {
        // Step 1: ToPrimitive with string hint for objects.
        let prim = value_ops::to_primitive(v, value_ops::ToPrimitiveHint::String);
        // Step 2: If ToPrimitive returned a symbol, use it directly.
        if prim.is_symbol() {
            return prim.raw_bits();
        }
        // 3. Return ! ToString(key).
        let s = display::display_value(prim);
        let rt_str = Box::new(string_ops::RtString::new(s));
        let raw_ptr = Box::into_raw(rt_str) as *const ();
        JsValue::string(raw_ptr).raw_bits()
    } else {
        // 3. Return ! ToString(key).
        __esc_rt_to_string(val)
    }
}

/// `ToInt32 ( argument )`
///
/// Converts a value to a 32-bit signed integer, then re-boxes as a NaN-boxed int.
///
/// [spec]: https://tc39.es/ecma262/#sec-toint32
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_int32(val: u64) -> u64 {
    // 1. Let number be ? ToNumber(argument).
    let n = value_ops::to_number(JsValue::from_raw_bits(val));
    // 2. If number is not finite or number is either +0 or -0, return +0.
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return JsValue::int(0).raw_bits();
    }
    // 3. Let int be truncate(R(number)).
    let int_val = (n.signum() * n.abs().floor()) as i64;
    // 4. Let int32bit be int modulo 2^32.
    let int32 = ((int_val as u64) % (1u64 << 32)) as u32;
    // 5. If int32bit >= 2^31, return F(int32bit - 2^32); otherwise return F(int32bit).
    JsValue::int(int32 as i32).raw_bits()
}

/// `ToUint32 ( argument )`
///
/// Converts a value to an unsigned 32-bit integer in the range `[0, 2^32 - 1]`.
/// Used by the `>>>` operator for the right operand.
/// The result is re-boxed as an int32 with the unsigned bit pattern preserved.
///
/// [spec]: https://tc39.es/ecma262/#sec-touint32
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_to_uint32(val: u64) -> u64 {
    // 1. Let number be ? ToNumber(argument).
    let n = value_ops::to_number(JsValue::from_raw_bits(val));
    // 2. If number is not finite or number is either +0 or -0, return +0.
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return JsValue::int(0).raw_bits();
    }
    // 3. Let int be truncate(R(number)).
    let int_val = (n.signum() * n.abs().floor()) as i64;
    // 4. Let int32bit be int modulo 2^32.
    let uint32 = ((int_val as u64) % (1u64 << 32)) as u32;
    // 5. Return F(int32bit).
    // Store the u32 bit pattern as i32 in the NaN-box — the caller
    // interprets the low 32 bits as unsigned where needed.
    JsValue::int(uint32 as i32).raw_bits()
}

/// Unbox a symbol value — identity for now.
///
/// In the full implementation this would extract the symbol ID from
/// the NaN-boxed representation. Symbols are primitive values per the spec.
///
/// Note: This is an internal helper with no direct spec equivalent.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_unbox_symbol(val: u64) -> u64 {
    val
}
