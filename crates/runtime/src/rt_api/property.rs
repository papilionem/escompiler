//! Property access runtime functions.
//!
//! Contains `__esc_rt_get_prop`, `__esc_rt_set_prop`, `__esc_rt_delete_prop`,
//! `__esc_rt_has_prop`, element access, and related object property operations.

use std::cell::RefCell;
use std::collections::HashMap;

use nanbox::JsValue;

use crate::display;
use crate::exceptions;
use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut, read_obj_tag};

use super::{
    __esc_rt_call_indirect, __esc_rt_create_error, __esc_rt_create_object, __esc_rt_throw,
    CURRENT_ARGC, CURRENT_ARGV, DELETED_PROPS, INTERNER, OBJECT_PROPS, PROTO_OBJECTS, SHAPES,
    as_array_index, create_array_from_elements, create_empty_array,
    dispatch_global_namespace_method, get_regexp_property, has_property_in_chain, key_to_string,
    lookup_property_chain, lookup_property_chain_get, lookup_proto_chain_setter, make_rt_string,
};

// =========================================================================
// Built-in constructor property caches
// =========================================================================

thread_local! {
    /// Cache for NativeFunc wrappers of built-in static methods.
    ///
    /// Keyed by `(builtin_name, method_name)` to ensure identity semantics
    /// (e.g., `Object.keys === Object.keys`).
    static BUILTIN_METHOD_CACHE: RefCell<HashMap<(String, String), u64>>
        = RefCell::new(HashMap::new());

    /// Cache for built-in constructor prototype objects.
    ///
    /// Keyed by constructor name to ensure identity semantics
    /// (e.g., `Array.prototype === Array.prototype`).
    static BUILTIN_PROTO_CACHE: RefCell<HashMap<String, u64>>
        = RefCell::new(HashMap::new());
}

// =========================================================================
// Object.create / Object.keys
// =========================================================================

/// `Object.create ( O, Properties )`
///
/// Creates a new object with the specified prototype object and properties.
///
/// [spec]: https://tc39.es/ecma262/#sec-object.create
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_object_create(proto: u64) -> u64 {
    // 1. If O is not an Object and O is not null, throw a TypeError exception.
    // TODO: Step 1 — should throw TypeError if proto is not object/null (e.g., number, string)
    let proto_val = JsValue::from_raw_bits(proto);

    // 2. Let obj be OrdinaryObjectCreate(O).
    let obj = UnifiedObject::ordinary(shapes::ShapeTable::EMPTY_SHAPE);
    let obj_bits = TaggedObj::boxed(ObjTag::Unified, obj);

    // Set prototype if not null/undefined (step 2 of OrdinaryObjectCreate sets [[Prototype]])
    if !proto_val.is_null() && !proto_val.is_undefined() {
        // Set legacy __proto__ property first (this may cause shape transitions)
        let key_bits = make_rt_string("__proto__".to_string());
        __esc_rt_set_prop(obj_bits, key_bits, proto);

        // Register shape-based prototype on the final shape
        register_prototype_on_object(obj_bits, proto);
    } else if proto_val.is_null() {
        // Explicitly register null prototype so that the implicit
        // Object.prototype fallback in get_prototype_object() is skipped.
        register_prototype_on_object(obj_bits, JsValue::null().raw_bits());
    }

    // 3. If Properties is not undefined, then
    //   a. Return ? ObjectDefineProperties(obj, Properties).
    // TODO: Step 3 — second argument (Properties) not yet supported

    // 4. Return obj.
    obj_bits
}

/// Register a prototype on an object using the shape-based mechanism.
///
/// This sets the prototype shape on the object's current shape and registers
/// the actual prototype object bits in the PROTO_OBJECTS registry.
pub(crate) fn register_prototype_on_object(obj_bits: u64, proto_bits: u64) {
    let tag = read_obj_tag(obj_bits);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj_bits)
    };
    let Some(u) = uni else { return };

    SHAPES.with(|shapes| {
        let mut shapes = shapes.borrow_mut();
        // Create a new shape ID for the prototype object
        let proto_shape_id = shapes::ShapeId(shapes.shape_count() as u32);
        let new_shape_id = shapes.set_prototype(u.shape_id, proto_shape_id);
        u.shape_id = new_shape_id;

        // The set_prototype call created the proto_shape_id implicitly as part of
        // the new shape. We need to get the actual prototype shape ID from the new shape.
        if let Some(sid) = shapes.get_prototype(new_shape_id) {
            PROTO_OBJECTS.with(|protos| {
                protos.borrow_mut().insert(sid, proto_bits);
            });
        }
    });
}

/// `Object.keys ( O )`
///
/// Returns an array of the object's own enumerable string-keyed property names.
///
/// [spec]: https://tc39.es/ecma262/#sec-object.keys
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_object_keys(obj: u64) -> u64 {
    // 1. Let obj be ? ToObject(O).
    // TODO: Step 1 — should call ToObject on non-object values (e.g., strings → String wrapper)
    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return create_empty_array();
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return create_empty_array();
    };

    // Proxy: intercept with ownKeys trap (§10.5.11 [[OwnPropertyKeys]])
    if u.kind == InternalKind::Proxy {
        match crate::proxy::proxy_own_keys(obj) {
            Ok(result) => {
                // If the trap returned a value, use it directly (should be an array)
                let rv = JsValue::from_raw_bits(result);
                if !rv.is_undefined() {
                    return result;
                }
                // Fallthrough returned undefined: get keys from target
                if let Some(InternalData::Proxy { target, .. }) = u.internal_data() {
                    return __esc_rt_object_keys(*target);
                }
                return create_empty_array();
            }
            Err(e) => {
                let msg = make_rt_string(e.to_string());
                let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                __esc_rt_throw(err);
                return create_empty_array();
            }
        }
    }

    // 2. Let nameList be ? EnumerableOwnProperties(obj, key).
    // 3. Return CreateArrayFromList(nameList).
    SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            let keys = u.enumerable_keys(&shapes, &interner);
            // Filter out deleted properties
            let deleted = DELETED_PROPS.with(|dp| dp.borrow().get(&obj).cloned());
            let values: Vec<JsValue> = keys
                .into_iter()
                .filter(|k| deleted.as_ref().is_none_or(|d| !d.contains(k)))
                .map(|k| JsValue::from_raw_bits(make_rt_string(k)))
                .collect();
            create_array_from_elements(values)
        })
    })
}

// =========================================================================
// Symbol-keyed property helpers
// =========================================================================

/// `OrdinaryGet ( O, P, Receiver )` — symbol-keyed variant
///
/// Gets a property from an object using a `PropertyKey::Symbol(id)`.
/// Walks the object's shape table (and prototype chain) looking for a
/// symbol-keyed property. Returns `undefined` if not found or if the
/// object is not a valid unified object.
///
/// Implements the symbol-key path of §10.1.8 OrdinaryGet.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryget
pub(crate) fn get_prop_by_symbol_key(obj: u64, sym_id: u32) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    // Strings have a built-in [Symbol.iterator] that iterates code points.
    if v.is_string() && sym_id == crate::symbol::SYMBOL_ITERATOR {
        let method = crate::tagged_obj::TaggedObj::boxed(
            ObjTag::Unified,
            UnifiedObject::native_func(native_return_this_for_iter, 0),
        );
        return method;
    }
    if !v.is_object() {
        return JsValue::undefined().raw_bits();
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::undefined().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::undefined().raw_bits();
    };
    let key = shapes::PropertyKey::Symbol(sym_id);
    let found = SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        u.get_slot_by_key(&key, &shapes)
    });
    if let Some(val) = found {
        return val.raw_bits();
    }
    // Also check OBJECT_PROPS — symbol-keyed properties set via set_prop
    // are stored under the symbol's string representation (e.g.,
    // "Symbol(Symbol.iterator)") because key_to_string converts symbol keys.
    let sym_str = crate::symbol::symbol_to_string(sym_id);
    let obj_prop_found = OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&obj).and_then(|m| m.get(&sym_str).copied())
    });
    if let Some(val) = obj_prop_found {
        return val;
    }
    // Walk prototype chain for symbol keys
    let chain_result = lookup_symbol_property_chain(obj, sym_id);
    if !JsValue::from_raw_bits(chain_result).is_undefined() {
        return chain_result;
    }
    // Built-in [Symbol.iterator] for known iterable types.
    // Returns a native function that creates the appropriate iterator when called.
    if sym_id == crate::symbol::SYMBOL_ITERATOR
        && let Some(method) = builtin_symbol_iterator_for(u.kind)
    {
        return method;
    }
    JsValue::undefined().raw_bits()
}

/// Return a native `[Symbol.iterator]` method for built-in iterable types.
///
/// For Array, Map, Set, and Generator objects, synthesizes a `NativeFunc`
/// that returns `this` (since `iter_init` already handles iteration via
/// the fast-path kind check). This lets user code call
/// `arr[Symbol.iterator]()` and get back the array itself, which is then
/// passed to `iter_init`.
fn builtin_symbol_iterator_for(kind: InternalKind) -> Option<u64> {
    match kind {
        InternalKind::Array
        | InternalKind::MapObj
        | InternalKind::WeakMapObj
        | InternalKind::SetObj
        | InternalKind::WeakSetObj
        | InternalKind::Generator => {
            let method = crate::tagged_obj::TaggedObj::boxed(
                ObjTag::Unified,
                UnifiedObject::native_func(native_return_this_for_iter, 0),
            );
            Some(method)
        }
        _ => None,
    }
}

/// Native function for built-in `[Symbol.iterator]()` — returns `this`.
///
/// When `arr[Symbol.iterator]()` is called, returns the current `this` value
/// so it can be passed to `iter_init` for actual iteration.
fn native_return_this_for_iter(_context: u64) -> u64 {
    super::CURRENT_THIS.with(|cell| {
        let v = cell.get();
        if v == 0 {
            JsValue::undefined().raw_bits()
        } else {
            v
        }
    })
}

/// `OrdinarySet ( O, P, V, Receiver )` — symbol-keyed variant
///
/// Sets a property on an object using a `PropertyKey::Symbol(id)`.
/// Transitions the shape table to add the symbol key if not already present.
/// Returns the value that was set.
///
/// Implements the symbol-key path of §10.1.9 OrdinarySet.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryset
pub(super) fn set_prop_by_symbol_key(obj: u64, sym_id: u32, val: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        return val;
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return val;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    if let Some(u) = uni {
        let key = shapes::PropertyKey::Symbol(sym_id);
        SHAPES.with(|shapes| {
            let mut shapes = shapes.borrow_mut();
            u.set_slot_by_key(key, JsValue::from_raw_bits(val), &mut shapes);
        });
    }
    val
}

/// Walk the prototype chain looking for a symbol-keyed property.
///
/// Similar to [`lookup_property_chain`] but uses `PropertyKey::Symbol` instead
/// of string-based lookup.
fn lookup_symbol_property_chain(obj: u64, sym_id: u32) -> u64 {
    let key = shapes::PropertyKey::Symbol(sym_id);
    let mut current = obj;
    for _ in 0..100 {
        let tag = read_obj_tag(current);
        if tag != Some(ObjTag::Unified as u8) {
            break;
        }
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(current)
        };
        let Some(u) = uni else { break };

        // Check own symbol-keyed properties
        let found = SHAPES.with(|shapes| {
            let shapes = shapes.borrow();
            u.get_slot_by_key(&key, &shapes)
        });
        if let Some(val) = found {
            return val.raw_bits();
        }

        // Follow prototype chain
        match super::get_prototype_object(u) {
            Some(proto_bits) => current = proto_bits,
            None => break,
        }
    }
    JsValue::undefined().raw_bits()
}

// =========================================================================
// Property access (B1)
// =========================================================================

/// `OrdinaryGet ( O, P, Receiver )`
///
/// Gets the value of property P from object O. This is the runtime
/// implementation of the `[[Get]]` internal method (§10.1.8).
///
/// Extracts the property name from the NaN-boxed key, looks up the property
/// in the object's shape-based storage, and returns the value or `undefined`.
/// When the key is a NaN-boxed symbol, uses `PropertyKey::Symbol(id)` for
/// shape-based lookup instead of string interning.
///
/// Throws a `TypeError` if `obj` is `null` or `undefined`, matching the
/// ECMAScript spec: "Cannot read properties of null/undefined".
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryget
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_get_prop(obj: u64, key: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Null/undefined receiver: throw TypeError per §7.3.2 GetV step 3
    if v.is_null() || v.is_undefined() {
        let name = key_to_string(key);
        let desc = if v.is_null() { "null" } else { "undefined" };
        let msg = format!("Cannot read properties of {desc} (reading '{name}')");
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // §10.1.8 OrdinaryGet — dispatch based on key type:
    // Symbol key: use PropertyKey::Symbol for shape-based lookup
    let key_val = JsValue::from_raw_bits(key);
    if let Some(sym_id) = key_val.as_symbol() {
        return get_prop_by_symbol_key(obj, sym_id);
    }

    // Symbol receiver: property access on a symbol value (e.g., sym.description)
    // Per §20.4.3 Properties of Symbol Instances
    if let Some(sym_id) = v.as_symbol() {
        let name = key_to_string(key);
        return match name.as_str() {
            "description" => match crate::symbol::symbol_description(sym_id) {
                Some(desc) => make_rt_string(desc),
                None => JsValue::undefined().raw_bits(),
            },
            _ => JsValue::undefined().raw_bits(),
        };
    }

    // String .length property access and well-known global namespaces
    if v.as_string().is_some() {
        let str_data = crate::string_ops::get_string_data(v);
        let name = key_to_string(key);

        // Well-known Symbol namespace properties (e.g., Symbol.iterator)
        if str_data == "Symbol" {
            return match name.as_str() {
                "iterator" => JsValue::symbol(crate::symbol::SYMBOL_ITERATOR).raw_bits(),
                "toPrimitive" => JsValue::symbol(crate::symbol::SYMBOL_TO_PRIMITIVE).raw_bits(),
                "hasInstance" => JsValue::symbol(crate::symbol::SYMBOL_HAS_INSTANCE).raw_bits(),
                "toStringTag" => JsValue::symbol(crate::symbol::SYMBOL_TO_STRING_TAG).raw_bits(),
                "asyncIterator" => JsValue::symbol(crate::symbol::SYMBOL_ASYNC_ITERATOR).raw_bits(),
                "species" => JsValue::symbol(crate::symbol::SYMBOL_SPECIES).raw_bits(),
                _ => {
                    // Fall through to generic builtin dispatch for Symbol.for, Symbol.keyFor, etc.
                    if let Some(result) = dispatch_builtin_property("Symbol", &name) {
                        return result;
                    }
                    JsValue::undefined().raw_bits()
                }
            };
        }

        // Math namespace constants and methods (e.g., Math.PI, Math.floor)
        if str_data == "Math" {
            // Constants first (fast path — no allocation needed)
            let constant = match name.as_str() {
                "E" => Some(JsValue::number(std::f64::consts::E).raw_bits()),
                "LN2" => Some(JsValue::number(std::f64::consts::LN_2).raw_bits()),
                "LN10" => Some(JsValue::number(std::f64::consts::LN_10).raw_bits()),
                "LOG2E" => Some(JsValue::number(std::f64::consts::LOG2_E).raw_bits()),
                "LOG10E" => Some(JsValue::number(std::f64::consts::LOG10_E).raw_bits()),
                "PI" => Some(JsValue::number(std::f64::consts::PI).raw_bits()),
                "SQRT2" => Some(JsValue::number(std::f64::consts::SQRT_2).raw_bits()),
                "SQRT1_2" => Some(JsValue::number(1.0 / std::f64::consts::SQRT_2).raw_bits()),
                _ => None,
            };
            if let Some(val) = constant {
                return val;
            }
            // Fall through to generic builtin dispatch for method NativeFunc wrappers
            if let Some(result) = dispatch_builtin_property("Math", &name) {
                return result;
            }
            return JsValue::undefined().raw_bits();
        }

        // Number namespace constants and static methods (e.g., Number.MAX_VALUE, Number.isNaN)
        if str_data == "Number" {
            // Constants first (fast path)
            let constant = match name.as_str() {
                "MAX_VALUE" => Some(JsValue::number(f64::MAX).raw_bits()),
                "MIN_VALUE" => Some(JsValue::number(5e-324).raw_bits()),
                "POSITIVE_INFINITY" => Some(JsValue::number(f64::INFINITY).raw_bits()),
                "NEGATIVE_INFINITY" => Some(JsValue::number(f64::NEG_INFINITY).raw_bits()),
                "NaN" => Some(JsValue::number(f64::NAN).raw_bits()),
                "EPSILON" => Some(JsValue::number(f64::EPSILON).raw_bits()),
                "MAX_SAFE_INTEGER" => Some(JsValue::number(9_007_199_254_740_991.0).raw_bits()),
                "MIN_SAFE_INTEGER" => Some(JsValue::number(-9_007_199_254_740_991.0).raw_bits()),
                _ => None,
            };
            if let Some(val) = constant {
                return val;
            }
            // Fall through to generic builtin dispatch for static method wrappers
            if let Some(result) = dispatch_builtin_property("Number", &name) {
                return result;
            }
            return JsValue::undefined().raw_bits();
        }

        // process namespace properties (e.g., process.argv, process.platform)
        if str_data == "process" {
            return dispatch_process_property(&name);
        }

        // Generic built-in constructor/namespace property dispatch
        // Handles Array, Object, String, Boolean, Function, Error subclasses,
        // Promise, RegExp, Date, Map, Set, JSON, Reflect, Proxy, etc.
        if is_builtin_constructor(&str_data) {
            if let Some(result) = dispatch_builtin_property(&str_data, &name) {
                return result;
            }
            return JsValue::undefined().raw_bits();
        }

        if name == "length" {
            return JsValue::int(str_data.chars().count() as i32).raw_bits();
        }
        // String index access: "hello"[0] → "h"
        if let Ok(idx) = name.parse::<usize>() {
            if let Some(ch) = str_data.chars().nth(idx) {
                return make_rt_string(ch.to_string());
            }
            return JsValue::undefined().raw_bits();
        }

        // Well-known global namespace dispatch was already handled above.
        // For actual string primitives, look up String.prototype methods
        // (auto-boxing per ES2024 §7.3.2 GetV step 3).
        if !is_builtin_constructor(&str_data) && str_data != "process" {
            // Look up the property on String.prototype
            let proto = get_or_create_builtin_prototype("String");
            let proto_val = lookup_property_chain_get(proto, &name, obj);
            if proto_val != JsValue::undefined().raw_bits() {
                return proto_val;
            }
        }
        return JsValue::undefined().raw_bits();
    }

    // Auto-boxing for number/boolean/symbol primitives:
    // Property access on primitives looks up the corresponding prototype
    // (ES2024 §7.3.2 GetV — "Let base be ? ToObject(V)").
    if v.is_number() || v.is_int() {
        let name = key_to_string(key);
        let proto = get_or_create_builtin_prototype("Number");
        let proto_val = lookup_property_chain_get(proto, &name, obj);
        if proto_val != JsValue::undefined().raw_bits() {
            return proto_val;
        }
        return JsValue::undefined().raw_bits();
    }
    if v.is_bool() {
        let name = key_to_string(key);
        let proto = get_or_create_builtin_prototype("Boolean");
        let proto_val = lookup_property_chain_get(proto, &name, obj);
        if proto_val != JsValue::undefined().raw_bits() {
            return proto_val;
        }
        return JsValue::undefined().raw_bits();
    }

    if !v.is_object() {
        return JsValue::undefined().raw_bits();
    }

    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::undefined().raw_bits();
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::undefined().raw_bits();
    };

    // §10.1.8 OrdinaryGet ( O, P, Receiver )
    // 1. Return ? OrdinaryGet(O, P, Receiver).
    match u.kind {
        InternalKind::Ordinary => {
            let name = key_to_string(key);
            // Check if this property was deleted (tombstone workaround)
            let is_deleted =
                DELETED_PROPS.with(|dp| dp.borrow().get(&obj).is_some_and(|s| s.contains(&name)));
            if is_deleted {
                // Property was deleted — skip own property lookup, fall through
                // to prototype chain
                return lookup_property_chain_get(obj, &name, obj);
            }
            // 1. Let desc be ? O.[[GetOwnProperty]](P).
            // Check for shape-based accessor property (getter)
            let accessor_getter = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_accessor_getter(&name, &shapes, &interner)
                })
            });
            // 4. If IsAccessorDescriptor(desc) is true, then
            //   a. Let getter be desc.[[Get]].
            //   b. If getter is undefined, return undefined.
            //   c. Return ? Call(getter, Receiver).
            if let Some(getter) = accessor_getter {
                if !getter.is_undefined() {
                    let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                    let result = unsafe {
                        // SAFETY: getter was found by shape-based accessor lookup.
                        __esc_rt_call_indirect(getter.raw_bits(), 0, std::ptr::null())
                    };
                    super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                    return result;
                }
                // Accessor exists but no getter: return undefined (step 4b)
                return JsValue::undefined().raw_bits();
            }
            // Also check legacy __get_<name> convention for backward compatibility
            let getter_key = format!("__get_{name}");
            let getter = lookup_property_chain(obj, &getter_key);
            if getter != JsValue::undefined().raw_bits() {
                let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                let result = unsafe {
                    // SAFETY: getter was found by property lookup.
                    __esc_rt_call_indirect(getter, 0, std::ptr::null())
                };
                super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                return result;
            }
            // 2. If desc is undefined, then
            //   a. Let parent be ? O.[[GetPrototypeOf]]().
            //   b. If parent is null, return undefined.
            //   c. Return ? parent.[[Get]](P, Receiver).
            // 3. If IsDataDescriptor(desc) is true, return desc.[[Value]].
            let result = lookup_property_chain_get(obj, &name, obj);
            if result != JsValue::undefined().raw_bits() {
                return result;
            }
            // §19.3 SetDefaultGlobalBindings: if this is the globalThis object,
            // fall back to the global object registry for built-in global names
            // (parseInt, parseFloat, isNaN, isFinite, Array, Math, etc.).
            // This allows tests that access `this.parseInt` to find the callable.
            if obj == super::__esc_rt_get_global_this() {
                let global_val = super::get_global_object(&name);
                if global_val != JsValue::undefined().raw_bits() {
                    return global_val;
                }
            }
            // Check if this is a namespace object (Math, JSON, Reflect) and
            // dispatch property access to the builtin method/constant system.
            let ns_name = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| {
                    m.get("__namespace__").map(|&bits| {
                        crate::string_ops::get_string_data(JsValue::from_raw_bits(bits))
                    })
                })
            });
            if let Some(ref ns) = ns_name {
                // Try constants first (Math.PI, Math.E, etc.)
                if let Some(val) = builtin_constant(ns, &name) {
                    return val;
                }
                // Then methods (Math.exp, JSON.parse, etc.)
                if let Some(val) = dispatch_builtin_property(ns, &name) {
                    return val;
                }
            }
            // Check if this is a built-in prototype object
            if let Some(builtin) = detect_builtin_prototype(obj)
                && is_builtin_instance_method(&builtin, &name)
            {
                return get_or_create_builtin_instance_method(&builtin, &name);
            }
            JsValue::undefined().raw_bits()
        }
        InternalKind::Array => {
            let name = key_to_string(key);
            if name == "length"
                && let Some(len) = u.as_array_length()
            {
                return JsValue::number(len as f64).raw_bits();
            }
            let kv = JsValue::from_raw_bits(key);
            if let Some(_idx) = as_array_index(kv) {
                // Per ES spec §10.4.2.1: Array integer indices are looked up via
                // [[GetOwnProperty]], which checks the shape table FIRST (for
                // properties installed by Object.defineProperty) and falls back to
                // dense element storage only when no shape entry exists.
                // get_property_descriptor implements this shape-first ordering.
                let shape_result = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.get_property_descriptor(&name, &shapes, &interner)
                    })
                });
                match shape_result {
                    Some(crate::property::OwnPropertyDescriptor::Data { value, .. }) => {
                        return value.raw_bits();
                    }
                    Some(crate::property::OwnPropertyDescriptor::Accessor { getter, .. }) => {
                        if !getter.is_undefined() {
                            let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                            let result = unsafe {
                                // SAFETY: getter found by shape accessor lookup.
                                __esc_rt_call_indirect(getter.raw_bits(), 0, std::ptr::null())
                            };
                            super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                            return result;
                        }
                        return JsValue::undefined().raw_bits();
                    }
                    None => {}
                }
                // Not found in own properties (shape or dense) — check prototype chain.
                return lookup_property_chain_get(obj, &name, obj);
            }
            // Handle numeric string keys like "0", "1", "2" (Gap B fix)
            if let Ok(_idx) = name.parse::<u32>() {
                // Same shape-first approach as the as_array_index path above.
                let shape_result = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.get_property_descriptor(&name, &shapes, &interner)
                    })
                });
                match shape_result {
                    Some(crate::property::OwnPropertyDescriptor::Data { value, .. }) => {
                        return value.raw_bits();
                    }
                    Some(crate::property::OwnPropertyDescriptor::Accessor { getter, .. }) => {
                        if !getter.is_undefined() {
                            let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                            let result = unsafe {
                                // SAFETY: getter found by shape accessor lookup.
                                __esc_rt_call_indirect(getter.raw_bits(), 0, std::ptr::null())
                            };
                            super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                            return result;
                        }
                        return JsValue::undefined().raw_bits();
                    }
                    None => {}
                }
                return lookup_property_chain_get(obj, &name, obj);
            }
            // Fall through to prototype chain for non-index, non-length properties
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::ErrorObj => {
            if let Some(InternalData::Error {
                error_tag,
                raw_message,
                stack,
                ..
            }) = u.internal_data()
            {
                let name = key_to_string(key);
                return match name.as_str() {
                    "name" => make_rt_string(exceptions::error_name(*error_tag).to_string()),
                    "message" => *raw_message,
                    "stack" => *stack,
                    _ => lookup_property_chain_get(obj, &name, obj),
                };
            }
            JsValue::undefined().raw_bits()
        }
        InternalKind::IterResult => {
            if let Some(InternalData::IterResult { value, done }) = u.internal_data() {
                let name = key_to_string(key);
                return match name.as_str() {
                    "value" => *value,
                    "done" => *done,
                    _ => JsValue::undefined().raw_bits(),
                };
            }
            JsValue::undefined().raw_bits()
        }
        InternalKind::Proxy => {
            let key_name = key_to_string(key);
            match crate::proxy::proxy_get(obj, key, &key_name) {
                Ok(result) => result,
                Err(e) => {
                    let msg = make_rt_string(e.to_string());
                    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    __esc_rt_throw(err);
                    JsValue::undefined().raw_bits()
                }
            }
        }
        InternalKind::Function | InternalKind::Closure => {
            let name = key_to_string(key);
            // §10.2.4 AddRestrictedFunctionProperties — .caller and .arguments
            // are accessor properties that always throw TypeError (ES2024).
            if name == "caller" || name == "arguments" {
                let msg = make_rt_string(
                    "'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them".to_string(),
                );
                let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                __esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }
            // Check OBJECT_PROPS first (user-set or desugar-set properties)
            let found = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| m.get(&name).copied())
            });
            if let Some(val) = found {
                return val;
            }
            // Fall back to InternalData defaults for well-known properties
            if let Some(InternalData::Function {
                name: fn_name,
                param_count,
                ..
            }) = u.internal_data()
            {
                match name.as_str() {
                    "name" => {
                        // If name is 0 (unset), return empty string
                        let n = JsValue::from_raw_bits(*fn_name);
                        if n.is_undefined() || *fn_name == 0 {
                            return make_rt_string(String::new());
                        }
                        return *fn_name;
                    }
                    "length" => {
                        return JsValue::number(*param_count as f64).raw_bits();
                    }
                    _ => {}
                }
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::MapObj | InternalKind::WeakMapObj => {
            let name = key_to_string(key);
            if name == "size"
                && let Some(InternalData::Map { entries }) = u.internal_data()
            {
                return JsValue::number(entries.len() as f64).raw_bits();
            }
            // Check OBJECT_PROPS (user-set own properties, e.g., mapObj.foo = "bar")
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::SetObj | InternalKind::WeakSetObj => {
            let name = key_to_string(key);
            if name == "size"
                && let Some(InternalData::Set { values }) = u.internal_data()
            {
                return JsValue::number(values.len() as f64).raw_bits();
            }
            // Check OBJECT_PROPS (user-set own properties)
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::RegExpObj => {
            let name = key_to_string(key);
            let result = get_regexp_property(obj, &name);
            if result != JsValue::undefined().raw_bits() {
                return result;
            }
            // Check OBJECT_PROPS (user-set own properties, e.g., regObj.foo = "bar")
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::SymbolObj => {
            let name = key_to_string(key);
            // Check OBJECT_PROPS (user-set own properties)
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::WeakRefObj => {
            let name = key_to_string(key);
            // Check OBJECT_PROPS (user-set own properties)
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::NativeFunc => {
            let name = key_to_string(key);
            // §10.2.4 AddRestrictedFunctionProperties — .caller and .arguments throw TypeError.
            if name == "caller" || name == "arguments" {
                let msg = make_rt_string(
                    "'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them".to_string(),
                );
                let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                __esc_rt_throw(err);
                return JsValue::undefined().raw_bits();
            }
            // Check OBJECT_PROPS first (name, length, prototype)
            let found = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| m.get(&name).copied())
            });
            if let Some(val) = found {
                return val;
            }
            // For built-in constructors, lazily create prototype and handle
            // well-known properties via dispatch_builtin_property.
            // Detect builtin name from OBJECT_PROPS "name" entry.
            let builtin_name = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| {
                    m.get("name").map(|&bits| {
                        let v = JsValue::from_raw_bits(bits);
                        if v.is_string() || v.as_string().is_some() {
                            crate::string_ops::get_string_data(v)
                        } else {
                            String::new()
                        }
                    })
                })
            });
            if let Some(ref bname) = builtin_name
                && is_builtin_constructor(bname)
                && let Some(result) = dispatch_builtin_property(bname, &name)
            {
                return result;
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::BooleanObj => {
            let name = key_to_string(key);
            // Check own shape-based properties (e.g., constructor on Boolean.prototype)
            let own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_slot_by_name(&name, &shapes, &interner)
                })
            });
            if let Some(val) = own {
                return val.raw_bits();
            }
            // Check OBJECT_PROPS (user-set properties)
            let found = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| m.get(&name).copied())
            });
            if let Some(val) = found {
                return val;
            }
            // Dispatch to Boolean.prototype methods
            if is_builtin_instance_method("Boolean", &name) {
                return get_or_create_builtin_instance_method("Boolean", &name);
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::NumberObj => {
            let name = key_to_string(key);
            // Check own shape-based properties
            let own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_slot_by_name(&name, &shapes, &interner)
                })
            });
            if let Some(val) = own {
                return val.raw_bits();
            }
            // Check OBJECT_PROPS
            let found = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| m.get(&name).copied())
            });
            if let Some(val) = found {
                return val;
            }
            // Dispatch to Number.prototype methods
            if is_builtin_instance_method("Number", &name) {
                return get_or_create_builtin_instance_method("Number", &name);
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        InternalKind::StringObj => {
            let name = key_to_string(key);
            // Check "length" — return the string length
            if name == "length"
                && let Some(InternalData::StringWrapper { value }) = u.internal_data()
            {
                let sv = JsValue::from_raw_bits(*value);
                if let Some(ptr) = sv.as_string()
                    && !ptr.is_null()
                {
                    let s = unsafe {
                        // SAFETY: string pointer was created by runtime.
                        &*(ptr as *const crate::string_ops::RtString)
                    };
                    return JsValue::number(s.as_str().chars().count() as f64).raw_bits();
                }
            }
            // Check indexed character access (e.g., s[0])
            let kv = JsValue::from_raw_bits(key);
            if let Some(idx) = as_array_index(kv) {
                if let Some(InternalData::StringWrapper { value }) = u.internal_data() {
                    let sv = JsValue::from_raw_bits(*value);
                    if let Some(ptr) = sv.as_string()
                        && !ptr.is_null()
                    {
                        let s = unsafe {
                            // SAFETY: string pointer was created by runtime.
                            &*(ptr as *const crate::string_ops::RtString)
                        };
                        if let Some(ch) = s.as_str().chars().nth(idx as usize) {
                            return make_rt_string(ch.to_string());
                        }
                    }
                }
                return JsValue::undefined().raw_bits();
            }
            // Check own shape-based properties (e.g., constructor on String.prototype)
            let own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.get_slot_by_name(&name, &shapes, &interner)
                })
            });
            if let Some(val) = own {
                return val.raw_bits();
            }
            // Check OBJECT_PROPS
            let found = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).and_then(|m| m.get(&name).copied())
            });
            if let Some(val) = found {
                return val;
            }
            // Dispatch to String.prototype methods
            if is_builtin_instance_method("String", &name) {
                return get_or_create_builtin_instance_method("String", &name);
            }
            // Fall through to prototype chain
            lookup_property_chain_get(obj, &name, obj)
        }
        _ => {
            // All remaining kinds (DateObj, Generator, Promise, Iterator, etc.)
            // are ordinary objects for [[Get]] per ES spec.
            // Check OBJECT_PROPS (user-set own properties) before prototype chain.
            let name = key_to_string(key);
            let found = OBJECT_PROPS
                .with(|props| props.borrow().get(&obj).and_then(|m| m.get(&name).copied()));
            if let Some(val) = found {
                return val;
            }
            lookup_property_chain_get(obj, &name, obj)
        }
    }
}

/// `OrdinarySet ( O, P, V, Receiver )`
///
/// Sets the value of property P on object O. This is the runtime
/// implementation of the `[[Set]]` internal method (§10.1.9) in sloppy mode.
///
/// Extracts the property name from the NaN-boxed key, sets the value
/// in the object's shape-based storage, and returns the value.
/// When the key is a NaN-boxed symbol, uses `PropertyKey::Symbol(id)` for
/// shape-based transitions instead of string interning.
///
/// Define a method property with `{ writable: true, enumerable: false, configurable: true }`.
///
/// Used by class method definitions per ES2024 §14.3.7 `DefineMethodProperty`.
/// Class methods must be non-enumerable (unlike normal data properties).
///
/// [spec]: https://tc39.es/ecma262/#sec-definemethodproperty
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_define_method(obj: u64, key: u64, val: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        return val;
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return val;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    if let Some(u) = uni {
        let name = key_to_string(key);
        SHAPES.with(|shapes| {
            INTERNER.with(|interner| {
                let mut shapes = shapes.borrow_mut();
                let interner = interner.borrow();
                u.set_slot_by_name_with_flags(
                    &name,
                    JsValue::from_raw_bits(val),
                    true,  // writable
                    false, // enumerable (class methods are NOT enumerable)
                    true,  // configurable
                    &mut shapes,
                    &interner,
                );
            });
        });
    }
    val
}

/// Throws a `TypeError` if `obj` is `null` or `undefined`.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryset
/// Returns `true` if `name` was made non-writable on `u` via
/// `Object.defineProperty` (descriptor stored in the shape table).
///
/// Assignment on OBJECT_PROPS-backed kinds (Function, wrappers, exotic objects)
/// must respect the flag even though their user properties live in the
/// side-table: `Object.defineProperty` stores descriptors in the shape table,
/// while plain assignment writes the side-table — without this check the
/// side-table write would silently override a non-writable definition.
fn is_shape_non_writable(u: &crate::internal_data::UnifiedObject, name: &str) -> bool {
    SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            matches!(
                u.get_property_descriptor(name, &shapes, &interner),
                Some(crate::property::OwnPropertyDescriptor::Data {
                    writable: false,
                    ..
                })
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_set_prop(obj: u64, key: u64, val: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Null/undefined receiver: throw TypeError per §7.3.3 Set step 4
    if v.is_null() || v.is_undefined() {
        let name = key_to_string(key);
        let desc = if v.is_null() { "null" } else { "undefined" };
        let msg = format!("Cannot set properties of {desc} (setting '{name}')");
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // Symbol key: use PropertyKey::Symbol for shape-based storage
    let key_val = JsValue::from_raw_bits(key);
    if let Some(sym_id) = key_val.as_symbol() {
        return set_prop_by_symbol_key(obj, sym_id, val);
    }

    if !v.is_object() {
        return val;
    }

    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return val;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    // §10.1.9 OrdinarySet ( O, P, V, Receiver )
    if let Some(u) = uni {
        match u.kind {
            InternalKind::Ordinary => {
                let name = key_to_string(key);
                // 1. Let ownDesc be ? O.[[GetOwnProperty]](P).
                // Check for shape-based accessor property (setter)
                let accessor_setter = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.get_accessor_setter(&name, &shapes, &interner)
                    })
                });
                // 3. If IsAccessorDescriptor(ownDesc) is true, then
                //   a. Let setter be ownDesc.[[Set]].
                //   b. If setter is undefined, return false.
                //   c. Perform ? Call(setter, Receiver, « V »).
                //   d. Return true.
                if let Some(setter) = accessor_setter {
                    if !setter.is_undefined() {
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter was found by shape-based accessor lookup.
                            __esc_rt_call_indirect(setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    // Accessor exists but no setter: silent fail in sloppy mode (step 3b)
                    return val;
                }
                // Also check legacy __set_<name> convention for backward compatibility
                let setter_key = format!("__set_{name}");
                let setter = lookup_property_chain(obj, &setter_key);
                if setter != JsValue::undefined().raw_bits() {
                    let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                    let call_args = [val];
                    unsafe {
                        // SAFETY: setter was found by property lookup.
                        __esc_rt_call_indirect(setter, 1, call_args.as_ptr());
                    }
                    super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                    return val;
                }
                // §10.1.9 step 2: If ownDesc is undefined, walk the prototype
                // chain looking for an inherited accessor setter before falling
                // through to create a new own data property.
                let has_own = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.has_own_property(&name, &shapes, &interner)
                    })
                });
                if !has_own && let Some(proto_setter) = lookup_proto_chain_setter(obj, &name) {
                    if !proto_setter.is_undefined() {
                        // Invoke the inherited setter with the original receiver as this
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter was found by prototype chain walk.
                            __esc_rt_call_indirect(proto_setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    // Accessor exists on prototype but no setter: silent fail
                    // in sloppy mode (§10.1.9 step 3b)
                    return val;
                }
                // 4. If ownDesc is undefined, then create a new data property.
                // 5. Else, ownDesc is a data descriptor — update desc.[[Value]].
                // Clear deleted-property marker if re-creating a deleted property
                DELETED_PROPS.with(|dp| {
                    let mut dp = dp.borrow_mut();
                    if let Some(s) = dp.get_mut(&obj) {
                        s.remove(&name);
                    }
                });
                SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let mut shapes = shapes.borrow_mut();
                        let interner = interner.borrow();
                        u.set_slot_by_name(
                            &name,
                            JsValue::from_raw_bits(val),
                            &mut shapes,
                            &interner,
                        );
                    });
                });
            }
            InternalKind::Array => {
                let name = key_to_string(key);
                if name == "length" {
                    let val_js = JsValue::from_raw_bits(val);
                    let num = if let Some(i) = val_js.as_int() {
                        i as f64
                    } else {
                        val_js.as_number().unwrap_or(f64::NAN)
                    };
                    let as_u32 = num as u32;
                    if num.is_nan() || num < 0.0 || (as_u32 as f64) != num {
                        let msg = make_rt_string("Invalid array length".to_string());
                        let err = __esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                        __esc_rt_throw(err);
                        return JsValue::undefined().raw_bits();
                    }
                    // Truncate elements and update length atomically
                    u.array_set_length(as_u32);
                    return val;
                }
                let kv = JsValue::from_raw_bits(key);
                if let Some(idx) = as_array_index(kv) {
                    // Check if the shape table has a property at this index key.
                    // Object.defineProperty(arr, "0", ...) stores in shape, not dense.
                    // Dense element writes (arr[0]=val, push) do NOT create shape entries.
                    let in_shape = SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let shapes = shapes.borrow();
                            let interner = interner.borrow();
                            u.has_own_property(&name, &shapes, &interner)
                        })
                    });
                    if in_shape {
                        // Check if this is an accessor property — invoke the setter.
                        // Per ES spec §10.1.9 step 3: if ownDesc is accessor, call setter.
                        let accessor_setter = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let shapes = shapes.borrow();
                                let interner = interner.borrow();
                                u.get_accessor_setter(&name, &shapes, &interner)
                            })
                        });
                        if let Some(setter) = accessor_setter {
                            if !setter.is_undefined() {
                                let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                                let call_args = [val];
                                unsafe {
                                    // SAFETY: setter found via shape accessor lookup.
                                    __esc_rt_call_indirect(
                                        setter.raw_bits(),
                                        1,
                                        call_args.as_ptr(),
                                    );
                                }
                                super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                                return val;
                            }
                            // Accessor with no setter: silent fail in sloppy mode
                            return val;
                        }
                        // Data property (from defineProperty): check writable
                        let result = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let mut shapes = shapes.borrow_mut();
                                let interner = interner.borrow();
                                // set_slot_by_name respects writable flag
                                u.set_slot_by_name(
                                    &name,
                                    JsValue::from_raw_bits(val),
                                    &mut shapes,
                                    &interner,
                                )
                            })
                        });
                        if result {
                            return val;
                        }
                        // Non-writable: sloppy mode ignores the write
                        return val;
                    }
                    // No shape property — use dense element storage (normal array path)
                    u.set_element(idx, JsValue::from_raw_bits(val));
                    // Update length if needed
                    if let Some(InternalData::Array { length, .. }) = u.internal_data_mut()
                        && idx >= *length
                    {
                        *length = idx + 1;
                    }
                    return val;
                }
                // Handle numeric string keys like "0", "1", "2" (Gap B fix)
                if let Ok(idx) = name.parse::<u32>() {
                    let in_shape = SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let shapes = shapes.borrow();
                            let interner = interner.borrow();
                            u.has_own_property(&name, &shapes, &interner)
                        })
                    });
                    if in_shape {
                        // Check for accessor property first
                        let accessor_setter = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let shapes = shapes.borrow();
                                let interner = interner.borrow();
                                u.get_accessor_setter(&name, &shapes, &interner)
                            })
                        });
                        if let Some(setter) = accessor_setter {
                            if !setter.is_undefined() {
                                let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                                let call_args = [val];
                                unsafe {
                                    // SAFETY: setter found via shape accessor lookup.
                                    __esc_rt_call_indirect(
                                        setter.raw_bits(),
                                        1,
                                        call_args.as_ptr(),
                                    );
                                }
                                super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                                return val;
                            }
                            return val; // Accessor with no setter: silent fail
                        }
                        SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let mut shapes = shapes.borrow_mut();
                                let interner = interner.borrow();
                                u.set_slot_by_name(
                                    &name,
                                    JsValue::from_raw_bits(val),
                                    &mut shapes,
                                    &interner,
                                );
                            });
                        });
                        // Non-writable: sloppy mode ignores the write
                        return val;
                    }
                    u.set_element(idx, JsValue::from_raw_bits(val));
                    if let Some(InternalData::Array { length, .. }) = u.internal_data_mut()
                        && idx >= *length
                    {
                        *length = idx + 1;
                    }
                    return val;
                }
                // Non-index named properties on arrays (e.g., arr.foo = "bar").
                // Per ES spec §10.1.9 OrdinarySet: check own accessor first, then
                // inherited accessor, then store as a new data property.
                let accessor_setter = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.get_accessor_setter(&name, &shapes, &interner)
                    })
                });
                if let Some(setter) = accessor_setter {
                    if !setter.is_undefined() {
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter found via shape accessor lookup.
                            __esc_rt_call_indirect(setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    return val; // Accessor with no setter: silent fail
                }
                // Check inherited (prototype chain) accessor setter.
                let has_own = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.has_own_property(&name, &shapes, &interner)
                    })
                });
                if !has_own && let Some(proto_setter) = lookup_proto_chain_setter(obj, &name) {
                    if !proto_setter.is_undefined() {
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter found by prototype chain walk.
                            __esc_rt_call_indirect(proto_setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    return val; // Prototype accessor with no setter: silent fail
                }
                // No accessor found — store as own data property in shape slots.
                // set_slot_by_name checks writable; non-writable → false → value not written.
                // In sloppy mode, non-writable writes are silently ignored per spec.
                SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let mut shapes = shapes.borrow_mut();
                        let interner = interner.borrow();
                        u.set_slot_by_name(
                            &name,
                            JsValue::from_raw_bits(val),
                            &mut shapes,
                            &interner,
                        );
                    });
                });
            }
            InternalKind::Proxy => {
                let key_name = key_to_string(key);
                match crate::proxy::proxy_set(obj, key, val, &key_name) {
                    Ok(_) => return val,
                    Err(e) => {
                        let msg = make_rt_string(e.to_string());
                        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                        __esc_rt_throw(err);
                        return JsValue::undefined().raw_bits();
                    }
                }
            }
            InternalKind::Function | InternalKind::Closure => {
                let name = key_to_string(key);
                // Per ES spec, Function.name and Function.length are non-writable
                // but configurable. Simple assignment should not overwrite them
                // after they've been initially set by the compiler.
                if name == "name" || name == "length" {
                    let already_set = OBJECT_PROPS.with(|props| {
                        let props = props.borrow();
                        props.get(&obj).is_some_and(|m| m.contains_key(&name))
                    });
                    if already_set {
                        // Non-writable: silently ignore in sloppy mode
                        return val;
                    }
                }
                if is_shape_non_writable(u, &name) {
                    // Sloppy mode: non-writable write silently ignored (ES2024 §10.1.9).
                    return val;
                }
                OBJECT_PROPS.with(|props| {
                    let mut props = props.borrow_mut();
                    props.entry(obj).or_default().insert(name, val);
                });
            }
            InternalKind::ErrorObj => {
                let name = key_to_string(key);
                // "name", "message", "stack" are stored in InternalData::Error and
                // are read-only (overriding them via assignment is a no-op in sloppy mode).
                // All other properties can be stored in the shape slot table, allowing
                // Error objects to be used as property descriptor bags.
                if name != "name" && name != "message" && name != "stack" {
                    SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let mut shapes = shapes.borrow_mut();
                            let interner = interner.borrow();
                            u.set_slot_by_name(
                                &name,
                                JsValue::from_raw_bits(val),
                                &mut shapes,
                                &interner,
                            );
                        });
                    });
                }
            }
            InternalKind::BooleanObj | InternalKind::NumberObj | InternalKind::StringObj => {
                // Wrapper objects (new Boolean(), new Number(), new String()) support
                // user-set own properties. These are stored in the OBJECT_PROPS side-table
                // (same as Function/Closure). This allows them to be used as property
                // descriptor bags (e.g., boolObj.value = "foo"; Object.defineProperty(o, "p", boolObj)).
                // Per ES spec: primitive wrapper objects are ordinary objects for property access.
                let name = key_to_string(key);
                if is_shape_non_writable(u, &name) {
                    // Sloppy mode: non-writable write silently ignored (ES2024 §10.1.9).
                    return val;
                }
                OBJECT_PROPS.with(|props| {
                    props.borrow_mut().entry(obj).or_default().insert(name, val);
                });
            }
            _ => {
                // All remaining exotic kinds (DateObj, RegExpObj, MapObj, SetObj,
                // WeakMapObj, WeakSetObj, WeakRefObj, NativeFunc, SymbolObj, Generator,
                // Promise, etc.) are ordinary objects for [[Set]] per ES spec.
                // User-set properties are stored in the OBJECT_PROPS side-table.
                let name = key_to_string(key);
                if is_shape_non_writable(u, &name) {
                    // Sloppy mode: non-writable write silently ignored (ES2024 §10.1.9).
                    return val;
                }
                OBJECT_PROPS.with(|props| {
                    props.borrow_mut().entry(obj).or_default().insert(name, val);
                });
            }
        }
    }

    val
}

/// `OrdinarySet ( O, P, V, Receiver )` — strict mode variant
///
/// Sets the value of property P on object O in strict mode. This is the
/// runtime implementation of the `[[Set]]` internal method (§10.1.9) with
/// strict-mode error behavior per §13.15.2 (assignment in strict code).
///
/// Behaves identically to [`__esc_rt_set_prop`] except that property errors
/// (frozen, sealed, or non-extensible objects) throw a `TypeError` instead of
/// being silently ignored.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryset
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_set_prop_strict(obj: u64, key: u64, val: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);

    // Null/undefined receiver: throw TypeError per §7.3.3 Set step 4
    if v.is_null() || v.is_undefined() {
        let name = key_to_string(key);
        let desc = if v.is_null() { "null" } else { "undefined" };
        let msg = format!("Cannot set properties of {desc} (setting '{name}')");
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    // Symbol key: use PropertyKey::Symbol for shape-based storage
    let key_val = JsValue::from_raw_bits(key);
    if let Some(sym_id) = key_val.as_symbol() {
        return set_prop_by_symbol_key(obj, sym_id, val);
    }

    if !v.is_object() {
        return val;
    }

    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return val;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    // §10.1.9 OrdinarySet ( O, P, V, Receiver ) — with strict-mode error reporting
    if let Some(u) = uni {
        match u.kind {
            InternalKind::Ordinary => {
                let name = key_to_string(key);
                // 1. Let ownDesc be ? O.[[GetOwnProperty]](P).
                // Check for shape-based accessor property (setter)
                let accessor_setter = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.get_accessor_setter(&name, &shapes, &interner)
                    })
                });
                // 3. If IsAccessorDescriptor(ownDesc) is true, then
                //   a. Let setter be ownDesc.[[Set]].
                //   b. If setter is undefined, return false.
                //     (strict mode: TypeError instead of silent false)
                //   c. Perform ? Call(setter, Receiver, « V »).
                //   d. Return true.
                if let Some(setter) = accessor_setter {
                    if !setter.is_undefined() {
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter was found by shape-based accessor lookup.
                            __esc_rt_call_indirect(setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    // Accessor with no setter in strict mode: TypeError (step 3b)
                    let msg = format!("Cannot set property {name} which has only a getter");
                    let err = __esc_rt_create_error(
                        exceptions::error_tag::TYPE_ERROR,
                        make_rt_string(msg),
                    );
                    __esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
                // Also check legacy __set_<name> convention
                let setter_key = format!("__set_{name}");
                let setter = lookup_property_chain(obj, &setter_key);
                if setter != JsValue::undefined().raw_bits() {
                    let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                    let call_args = [val];
                    unsafe {
                        // SAFETY: setter was found by property lookup.
                        __esc_rt_call_indirect(setter, 1, call_args.as_ptr());
                    }
                    super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                    return val;
                }
                // §10.1.9 step 2: If ownDesc is undefined, walk the prototype
                // chain looking for an inherited accessor setter.
                let has_own = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.has_own_property(&name, &shapes, &interner)
                    })
                });
                if !has_own && let Some(proto_setter) = lookup_proto_chain_setter(obj, &name) {
                    if !proto_setter.is_undefined() {
                        // Invoke the inherited setter with the original receiver as this
                        let prev_this = super::CURRENT_THIS.with(|cell| cell.replace(obj));
                        let call_args = [val];
                        unsafe {
                            // SAFETY: setter was found by prototype chain walk.
                            __esc_rt_call_indirect(proto_setter.raw_bits(), 1, call_args.as_ptr());
                        }
                        super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                        return val;
                    }
                    // Accessor on prototype but no setter: TypeError in strict mode
                    let msg = format!("Cannot set property {name} which has only a getter");
                    let err = __esc_rt_create_error(
                        exceptions::error_tag::TYPE_ERROR,
                        make_rt_string(msg),
                    );
                    __esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
                // 4-5. Data property set — strict mode throws on frozen/sealed/non-extensible
                let result = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let mut shapes = shapes.borrow_mut();
                        let interner = interner.borrow();
                        u.set_property_strict(
                            &name,
                            JsValue::from_raw_bits(val),
                            &mut shapes,
                            &interner,
                        )
                    })
                });
                if let Err(e) = result {
                    let msg = strict_set_error_message(&name, e);
                    let err = __esc_rt_create_error(
                        exceptions::error_tag::TYPE_ERROR,
                        make_rt_string(msg),
                    );
                    __esc_rt_throw(err);
                    return JsValue::undefined().raw_bits();
                }
            }
            InternalKind::Array => {
                let name = key_to_string(key);
                if name == "length" {
                    let val_js = JsValue::from_raw_bits(val);
                    let num = if let Some(i) = val_js.as_int() {
                        i as f64
                    } else {
                        val_js.as_number().unwrap_or(f64::NAN)
                    };
                    let as_u32 = num as u32;
                    if num.is_nan() || num < 0.0 || (as_u32 as f64) != num {
                        let msg = make_rt_string("Invalid array length".to_string());
                        let err = __esc_rt_create_error(exceptions::error_tag::RANGE_ERROR, msg);
                        __esc_rt_throw(err);
                        return JsValue::undefined().raw_bits();
                    }
                    // Truncate elements and update length atomically
                    u.array_set_length(as_u32);
                    return val;
                }
                let kv = JsValue::from_raw_bits(key);
                if let Some(idx) = as_array_index(kv) {
                    // Use has_own_property (shape-only) to check if a shape entry exists.
                    // Do NOT use get_property_descriptor here — it checks dense elements first,
                    // which would incorrectly match arr[0]=val as a "shape property".
                    let in_shape = SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let shapes = shapes.borrow();
                            let interner = interner.borrow();
                            u.has_own_property(&name, &shapes, &interner)
                        })
                    });
                    if in_shape {
                        // Shape property (stored via defineProperty) — check writable/accessor
                        let shape_desc = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let shapes = shapes.borrow();
                                let interner = interner.borrow();
                                u.get_property_descriptor(&name, &shapes, &interner)
                            })
                        });
                        match shape_desc {
                            Some(crate::property::OwnPropertyDescriptor::Data {
                                writable: false,
                                ..
                            }) => {
                                // Non-writable in strict mode: TypeError
                                let msg = format!(
                                    "Cannot assign to read only property '{}' of object",
                                    name
                                );
                                let err = __esc_rt_create_error(
                                    exceptions::error_tag::TYPE_ERROR,
                                    make_rt_string(msg),
                                );
                                __esc_rt_throw(err);
                                return JsValue::undefined().raw_bits();
                            }
                            Some(crate::property::OwnPropertyDescriptor::Data { .. }) => {
                                // Writable shape property — update shape slot
                                SHAPES.with(|shapes| {
                                    INTERNER.with(|interner| {
                                        let mut shapes = shapes.borrow_mut();
                                        let interner = interner.borrow();
                                        u.set_slot_by_name(
                                            &name,
                                            JsValue::from_raw_bits(val),
                                            &mut shapes,
                                            &interner,
                                        );
                                    });
                                });
                                return val;
                            }
                            Some(crate::property::OwnPropertyDescriptor::Accessor {
                                setter,
                                ..
                            }) => {
                                if !setter.is_undefined() {
                                    let prev_this =
                                        super::CURRENT_THIS.with(|cell| cell.replace(obj));
                                    let call_args = [val];
                                    unsafe {
                                        // SAFETY: setter found by shape accessor lookup.
                                        __esc_rt_call_indirect(
                                            setter.raw_bits(),
                                            1,
                                            call_args.as_ptr(),
                                        );
                                    }
                                    super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                                } else {
                                    // Accessor with no setter in strict mode: TypeError
                                    let msg = format!(
                                        "Cannot set property {} which has only a getter",
                                        name
                                    );
                                    let err = __esc_rt_create_error(
                                        exceptions::error_tag::TYPE_ERROR,
                                        make_rt_string(msg),
                                    );
                                    __esc_rt_throw(err);
                                    return JsValue::undefined().raw_bits();
                                }
                                return val;
                            }
                            None => {
                                // has_own_property said yes but get_property_descriptor returned
                                // None — shouldn't happen, but fall through to dense element write
                            }
                        }
                    } else {
                        // No shape property — use dense element storage (normal array path)
                        u.set_element(idx, JsValue::from_raw_bits(val));
                        if let Some(InternalData::Array { length, .. }) = u.internal_data_mut()
                            && idx >= *length
                        {
                            *length = idx + 1;
                        }
                    }
                    return val;
                }
                // Handle numeric string keys like "0", "1", "2" (Gap B fix)
                if let Ok(idx) = name.parse::<u32>() {
                    // Use has_own_property (shape-only) — same rationale as above
                    let in_shape = SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let shapes = shapes.borrow();
                            let interner = interner.borrow();
                            u.has_own_property(&name, &shapes, &interner)
                        })
                    });
                    if in_shape {
                        let shape_desc = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let shapes = shapes.borrow();
                                let interner = interner.borrow();
                                u.get_property_descriptor(&name, &shapes, &interner)
                            })
                        });
                        match shape_desc {
                            Some(crate::property::OwnPropertyDescriptor::Data {
                                writable: false,
                                ..
                            }) => {
                                let msg = format!(
                                    "Cannot assign to read only property '{}' of object",
                                    name
                                );
                                let err = __esc_rt_create_error(
                                    exceptions::error_tag::TYPE_ERROR,
                                    make_rt_string(msg),
                                );
                                __esc_rt_throw(err);
                                return JsValue::undefined().raw_bits();
                            }
                            Some(crate::property::OwnPropertyDescriptor::Data { .. }) => {
                                SHAPES.with(|shapes| {
                                    INTERNER.with(|interner| {
                                        let mut shapes = shapes.borrow_mut();
                                        let interner = interner.borrow();
                                        u.set_slot_by_name(
                                            &name,
                                            JsValue::from_raw_bits(val),
                                            &mut shapes,
                                            &interner,
                                        );
                                    });
                                });
                                return val;
                            }
                            Some(crate::property::OwnPropertyDescriptor::Accessor {
                                setter,
                                ..
                            }) => {
                                if !setter.is_undefined() {
                                    let prev_this =
                                        super::CURRENT_THIS.with(|cell| cell.replace(obj));
                                    let call_args = [val];
                                    unsafe {
                                        // SAFETY: setter found by shape accessor lookup.
                                        __esc_rt_call_indirect(
                                            setter.raw_bits(),
                                            1,
                                            call_args.as_ptr(),
                                        );
                                    }
                                    super::CURRENT_THIS.with(|cell| cell.set(prev_this));
                                } else {
                                    let msg = format!(
                                        "Cannot set property {} which has only a getter",
                                        name
                                    );
                                    let err = __esc_rt_create_error(
                                        exceptions::error_tag::TYPE_ERROR,
                                        make_rt_string(msg),
                                    );
                                    __esc_rt_throw(err);
                                    return JsValue::undefined().raw_bits();
                                }
                                return val;
                            }
                            None => {
                                // Fall through to dense element write
                            }
                        }
                    } else {
                        u.set_element(idx, JsValue::from_raw_bits(val));
                        if let Some(InternalData::Array { length, .. }) = u.internal_data_mut()
                            && idx >= *length
                        {
                            *length = idx + 1;
                        }
                    }
                    return val;
                }
                // Non-index named properties on arrays (strict mode variant)
                SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let mut shapes = shapes.borrow_mut();
                        let interner = interner.borrow();
                        u.set_slot_by_name(
                            &name,
                            JsValue::from_raw_bits(val),
                            &mut shapes,
                            &interner,
                        );
                    });
                });
            }
            InternalKind::Proxy => {
                let key_name = key_to_string(key);
                match crate::proxy::proxy_set(obj, key, val, &key_name) {
                    Ok(accepted) => {
                        if !accepted {
                            // In strict mode, trap returning false throws
                            let msg = make_rt_string(format!(
                                "Cannot set property '{key_name}' on proxy: trap returned false"
                            ));
                            let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                            __esc_rt_throw(err);
                        }
                        return val;
                    }
                    Err(e) => {
                        let msg = make_rt_string(e.to_string());
                        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                        __esc_rt_throw(err);
                        return JsValue::undefined().raw_bits();
                    }
                }
            }
            InternalKind::ErrorObj => {
                let name = key_to_string(key);
                // "name", "message", "stack" are stored in InternalData::Error (read-only).
                // All other properties stored in shape slot table.
                if name != "name" && name != "message" && name != "stack" {
                    SHAPES.with(|shapes| {
                        INTERNER.with(|interner| {
                            let mut shapes = shapes.borrow_mut();
                            let interner = interner.borrow();
                            u.set_slot_by_name(
                                &name,
                                JsValue::from_raw_bits(val),
                                &mut shapes,
                                &interner,
                            );
                        });
                    });
                }
            }
            InternalKind::Function | InternalKind::Closure => {
                let name = key_to_string(key);
                // Per ES spec, Function.name and Function.length are non-writable
                // but configurable. In strict mode, assignment to non-writable
                // property throws TypeError.
                if name == "name" || name == "length" {
                    let already_set = OBJECT_PROPS.with(|props| {
                        let props = props.borrow();
                        props.get(&obj).is_some_and(|m| m.contains_key(&name))
                    });
                    if already_set {
                        let msg = make_rt_string(format!(
                            "Cannot assign to read only property '{name}' of function"
                        ));
                        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                        __esc_rt_throw(err);
                        return val;
                    }
                }
                OBJECT_PROPS.with(|props| {
                    let mut props = props.borrow_mut();
                    props.entry(obj).or_default().insert(name, val);
                });
            }
            InternalKind::BooleanObj | InternalKind::NumberObj | InternalKind::StringObj => {
                // Wrapper objects support user-set own properties stored in OBJECT_PROPS.
                // In strict mode, these are always writable (no TypeError).
                let name = key_to_string(key);
                OBJECT_PROPS.with(|props| {
                    props.borrow_mut().entry(obj).or_default().insert(name, val);
                });
            }
            _ => {
                // All remaining exotic kinds (DateObj, RegExpObj, MapObj, SetObj,
                // WeakMapObj, WeakSetObj, WeakRefObj, NativeFunc, SymbolObj, Generator,
                // Promise, etc.) are ordinary objects for [[Set]] per ES spec.
                // User-set properties are stored in the OBJECT_PROPS side-table.
                let name = key_to_string(key);
                OBJECT_PROPS.with(|props| {
                    props.borrow_mut().entry(obj).or_default().insert(name, val);
                });
            }
        }
    }

    val
}

/// `OrdinaryDelete ( O, P )`
///
/// Deletes property P from object O, returning a NaN-boxed boolean indicating
/// success. Implements the `[[Delete]]` internal method (§10.1.10).
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinarydelete
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_delete_prop(obj: u64, key: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    // Non-object: delete returns true per §13.5.1.2 (delete on non-Reference)
    if !v.is_object() {
        return JsValue::bool(true).raw_bits();
    }

    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::bool(false).raw_bits();
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    if let Some(u) = uni {
        if u.kind == InternalKind::Ordinary {
            // §10.1.10 OrdinaryDelete ( O, P )
            // 1. Let desc be ? O.[[GetOwnProperty]](P).
            // 2. If desc is undefined, return true.
            // 3. If desc.[[Configurable]] is true, then
            //   a. Remove the own property with name P from O.
            //   b. Return true.
            // 4. Return false.
            // TODO: Step 4 — non-configurable properties should return false, not be deleted
            let name = key_to_string(key);
            let deleted = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.delete_slot_by_name(&name, &shapes, &interner)
                })
            });
            if deleted {
                // Track the deletion so has_prop/get_prop correctly report
                // the property as absent. The shape system can't truly remove
                // a property (only tombstones the slot to undefined), so we
                // need this supplementary tracking.
                DELETED_PROPS.with(|dp| {
                    dp.borrow_mut().entry(obj).or_default().insert(name.clone());
                });
                // Also remove from OBJECT_PROPS if present
                OBJECT_PROPS.with(|props| {
                    let mut props = props.borrow_mut();
                    if let Some(m) = props.get_mut(&obj) {
                        m.remove(&name);
                    }
                });
            }
            return JsValue::bool(deleted).raw_bits();
        }
        if u.kind == InternalKind::Array {
            let kv = JsValue::from_raw_bits(key);
            if let Some(idx) = as_array_index(kv) {
                let deleted = u.delete_element(idx);
                return JsValue::bool(deleted).raw_bits();
            }
            return JsValue::bool(false).raw_bits();
        }
        // Proxy: §10.5.10 [[Delete]]
        if u.kind == InternalKind::Proxy {
            let key_name = key_to_string(key);
            match crate::proxy::proxy_delete_property(obj, key, &key_name) {
                Ok(result) => return JsValue::bool(result).raw_bits(),
                Err(e) => {
                    let msg = make_rt_string(e.to_string());
                    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    __esc_rt_throw(err);
                    return JsValue::bool(false).raw_bits();
                }
            }
        }
    }

    JsValue::bool(false).raw_bits()
}

/// `OrdinaryHasProperty ( O, P )`
///
/// Checks whether object O has property P (own or inherited). This is the
/// runtime implementation of the `[[HasProperty]]` internal method (§10.1.7),
/// used by the `in` operator.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryhasproperty
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_has_prop(obj: u64, key: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        return JsValue::bool(false).raw_bits();
    }

    let tag = read_obj_tag(obj);

    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::bool(false).raw_bits();
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::bool(false).raw_bits();
    };

    // §10.1.7 OrdinaryHasProperty ( O, P )
    // 1. Let hasOwn be ? O.[[GetOwnProperty]](P).
    // 2. If hasOwn is not undefined, return true.
    // 3. Let parent be ? O.[[GetPrototypeOf]]().
    // 4. If parent is not null, then
    //   a. Return ? parent.[[HasProperty]](P).
    // 5. Return false.
    //
    // NOTE: We use has_property_in_chain (not lookup_property_chain) for the prototype
    // chain walk because lookup_property_chain returns `undefined` for accessor properties
    // with no getter — which is indistinguishable from "property not found". The spec's
    // [[HasProperty]] must return `true` even for accessor properties with no getter.
    match u.kind {
        InternalKind::Ordinary => {
            let name = key_to_string(key);
            // Check if this property was deleted (tombstone workaround)
            let is_deleted =
                DELETED_PROPS.with(|dp| dp.borrow().get(&obj).is_some_and(|s| s.contains(&name)));
            if is_deleted {
                // Property was deleted — skip own check, walk prototype chain only.
                // Use lookup_property_chain here which skips the tombstoned own property
                // (the shape slot still contains the old value but DELETED_PROPS marks it gone).
                // We use the raw value check: if lookup_property_chain finds it in a prototype,
                // the value will be non-undefined (a data property) or we accept the false negative
                // for accessor-with-no-getter in prototypes (edge case for deleted properties).
                let result = lookup_property_chain(obj, &name);
                return JsValue::bool(result != JsValue::undefined().raw_bits()).raw_bits();
            }
            // Step 1: Check own properties via shape-based lookup
            let has_own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(&name, &shapes, &interner)
                })
            });
            // Step 2: If hasOwn is not undefined, return true
            if has_own {
                return JsValue::bool(true).raw_bits();
            }
            // Steps 3-4: Check prototype chain using existence check (not value check).
            // has_property_in_chain correctly handles accessor-with-no-getter.
            JsValue::bool(has_property_in_chain(obj, &name)).raw_bits()
        }
        InternalKind::Array => {
            let name = key_to_string(key);
            if name == "length" {
                return JsValue::bool(true).raw_bits();
            }
            let kv = JsValue::from_raw_bits(key);
            if let Some(idx) = as_array_index(kv) {
                // Check dense elements first
                if u.get_element(idx).is_some() {
                    return JsValue::bool(true).raw_bits();
                }
                // Also check shape — Object.defineProperty stores in shape
                let in_shape = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        u.has_own_property(&name, &shapes, &interner)
                    })
                });
                if in_shape {
                    return JsValue::bool(true).raw_bits();
                }
                // Check prototype chain
                return JsValue::bool(has_property_in_chain(obj, &name)).raw_bits();
            }
            // Check own shape properties (non-index named properties on arrays)
            let in_shape = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(&name, &shapes, &interner)
                })
            });
            if in_shape {
                return JsValue::bool(true).raw_bits();
            }
            // Check prototype chain
            JsValue::bool(has_property_in_chain(obj, &name)).raw_bits()
        }
        InternalKind::Function | InternalKind::Closure => {
            let name = key_to_string(key);
            // Check user-set properties first
            let has_user_prop = OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj).is_some_and(|m| m.contains_key(&name))
            });
            if has_user_prop {
                return JsValue::bool(true).raw_bits();
            }
            // Well-known function properties (including restricted .caller/.arguments)
            if matches!(
                name.as_str(),
                "name" | "length" | "prototype" | "caller" | "arguments"
            ) {
                return JsValue::bool(true).raw_bits();
            }
            // Check prototype chain
            JsValue::bool(has_property_in_chain(obj, &name)).raw_bits()
        }
        InternalKind::BooleanObj | InternalKind::NumberObj | InternalKind::StringObj => {
            let name = key_to_string(key);
            // Check own shape-based properties
            let has_own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(&name, &shapes, &interner)
                })
            });
            if has_own {
                return JsValue::bool(true).raw_bits();
            }
            // Check OBJECT_PROPS side-table (user-set properties like boolObj.foo = "bar")
            let in_side_table = OBJECT_PROPS.with(|props| {
                props
                    .borrow()
                    .get(&obj)
                    .is_some_and(|m| m.contains_key(&name))
            });
            if in_side_table {
                return JsValue::bool(true).raw_bits();
            }
            // Check prototype chain (Boolean.prototype, Number.prototype, String.prototype)
            JsValue::bool(has_property_in_chain(obj, &name)).raw_bits()
        }
        InternalKind::Proxy => {
            let key_name = key_to_string(key);
            match crate::proxy::proxy_has(obj, key, &key_name) {
                Ok(result) => JsValue::bool(result).raw_bits(),
                Err(e) => {
                    let msg = make_rt_string(e.to_string());
                    let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
                    __esc_rt_throw(err);
                    JsValue::bool(false).raw_bits()
                }
            }
        }
        _ => {
            // For all other kinds: check shape-based properties and OBJECT_PROPS, then prototype.
            let name = key_to_string(key);
            let has_own = SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let shapes = shapes.borrow();
                    let interner = interner.borrow();
                    u.has_own_property(&name, &shapes, &interner)
                })
            });
            if has_own {
                return JsValue::bool(true).raw_bits();
            }
            // Check OBJECT_PROPS side-table (NativeFunc, etc.)
            let in_side_table = OBJECT_PROPS.with(|props| {
                props
                    .borrow()
                    .get(&obj)
                    .is_some_and(|m| m.contains_key(&name))
            });
            if in_side_table {
                return JsValue::bool(true).raw_bits();
            }
            JsValue::bool(has_property_in_chain(obj, &name)).raw_bits()
        }
    }
}

/// `OrdinaryGet ( O, P, Receiver )` — element access variant
///
/// Gets an element from an object/array by numeric index. Delegates to
/// `__esc_rt_get_prop` which implements §10.1.8 OrdinaryGet.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryget
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_get_elem(obj: u64, idx: u64) -> u64 {
    __esc_rt_get_prop(obj, idx)
}

/// `OrdinarySet ( O, P, V, Receiver )` — element access variant
///
/// Sets an element on an object/array by numeric index. Delegates to
/// `__esc_rt_set_prop` which implements §10.1.9 OrdinarySet.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinaryset
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_set_elem(obj: u64, idx: u64, val: u64) -> u64 {
    __esc_rt_set_prop(obj, idx, val)
}

/// `CopyDataProperties ( target, source, excludedItems )`
///
/// Creates a new object containing all own enumerable properties from `source`
/// except those whose names are in `excluded_keys`. Implements §7.3.74
/// CopyDataProperties, used for object destructuring rest patterns
/// (`let { a, b, ...rest } = obj`).
///
/// [spec]: https://tc39.es/ecma262/#sec-copydataproperties
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_object_rest(source: u64, excluded_keys: u64) -> u64 {
    // §7.3.74 CopyDataProperties ( target, source, excludedItems )
    // 1. If source is undefined or null, return target.
    // TODO: Step 1 — should return early on undefined/null source
    // 2. Let from be ! ToObject(source).
    // 3. Let keys be ? from.[[OwnPropertyKeys]]().
    // 4. For each element nextKey of keys, do
    //   a. Let excluded be false.
    //   b. For each element e of excludedItems, do
    //     i. If SameValue(e, nextKey) is true, set excluded to true.
    //   c. If excluded is false, then
    //     i. Let desc be ? from.[[GetOwnProperty]](nextKey).
    //     ii. If desc is not undefined and desc.[[Enumerable]] is true, then
    //       1. Let propValue be ? Get(from, nextKey).
    //       2. Perform ! CreateDataPropertyOrThrow(target, nextKey, propValue).
    // 5. Return target.

    // Build the excluded key set from the array (step 4b)
    let mut excluded: Vec<String> = Vec::new();
    let excl_tag = read_obj_tag(excluded_keys);
    if excl_tag == Some(ObjTag::Unified as u8) {
        let excl_uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(excluded_keys)
        };
        if let Some(eu) = excl_uni {
            for elem in eu.array_elements_resolved() {
                excluded.push(display::display_value(elem));
            }
        }
    }

    // Read all keys from source and copy non-excluded ones.
    let tag = read_obj_tag(source);

    if tag != Some(ObjTag::Unified as u8) {
        return __esc_rt_create_object();
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(source)
    };
    let Some(u) = uni else {
        return __esc_rt_create_object();
    };
    if u.kind != InternalKind::Ordinary {
        return __esc_rt_create_object();
    }
    let collected: Vec<(String, u64)> = SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            let mut pairs = Vec::new();
            for key in u.enumerable_keys(&shapes, &interner) {
                if key == "__proto__" || excluded.contains(&key) {
                    continue;
                }
                if let Some(val) = u.get_slot_by_name(&key, &shapes, &interner) {
                    pairs.push((key, val.raw_bits()));
                }
            }
            pairs
        })
    });
    let result = __esc_rt_create_object();
    for (name, val_bits) in collected {
        let key_bits = make_rt_string(name);
        __esc_rt_set_prop(result, key_bits, val_bits);
    }
    result
}

/// `OrdinaryDefineOwnProperty ( O, P, Desc )` — accessor property variant
///
/// Defines an accessor property on an object. Convenience function for
/// object literal `get`/`set` syntax. Defines a property with a getter
/// and/or setter via the shape-based accessor model, with attributes
/// `{enumerable: true, configurable: true}`.
///
/// Implements a subset of §10.1.6 OrdinaryDefineOwnProperty for accessor
/// descriptors.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinarydefineownproperty
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_define_accessor(obj: u64, key: u64, getter: u64, setter: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        return obj;
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return obj;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else { return obj };

    let prop_name = key_to_string(key);
    let getter_val = JsValue::from_raw_bits(getter);
    let setter_val = JsValue::from_raw_bits(setter);

    SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let mut shapes = shapes.borrow_mut();
            let interner = interner.borrow();
            let opts = crate::property::DefinePropertyOptions {
                value: None,
                writable: None,
                enumerable: Some(true),
                configurable: Some(true),
                getter: if getter_val.is_undefined() {
                    None
                } else {
                    Some(getter_val)
                },
                setter: if setter_val.is_undefined() {
                    None
                } else {
                    Some(setter_val)
                },
            };
            let _ = u.define_own_property(&prop_name, &opts, &mut shapes, &interner);
        })
    });
    obj
}

// =========================================================================
// Super property access
// =========================================================================

/// `MakeSuperPropertyReference ( actualThis, propertyKey )` — get variant
///
/// Gets a property via `super.prop`. Per §13.3.7.1 (Runtime Semantics:
/// Evaluation of SuperProperty), the home object's `[[Prototype]]` is used
/// as the base for property lookup, while `this` remains the receiver.
///
/// `this_val` is the receiver (`this`), `key` is the property name.
/// The property is resolved starting from `this.__proto__.__proto__`
/// (i.e., the parent class's prototype).
///
/// [spec]: https://tc39.es/ecma262/#sec-super-keyword-runtime-semantics-evaluation
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_get_super(this_val: u64, key: u64) -> u64 {
    // 1. Let env be GetThisEnvironment().
    // 2. Let baseValue be ? env.GetSuperBase().
    //   (GetSuperBase returns homeObject.[[GetPrototypeOf]]())
    // Get the prototype chain: this.__proto__ → DerivedClass.prototype
    let proto_key = make_rt_string("__proto__".to_string());
    let this_proto = __esc_rt_get_prop(this_val, proto_key);

    // Then get the parent prototype: DerivedClass.prototype.__proto__ → BaseClass.prototype
    let parent_proto_key = make_rt_string("__proto__".to_string());
    let parent_proto = __esc_rt_get_prop(this_proto, parent_proto_key);

    let parent_val = JsValue::from_raw_bits(parent_proto);
    if parent_val.is_null() || parent_val.is_undefined() {
        return JsValue::undefined().raw_bits();
    }

    // 3. Return ? GetValue(MakeSuperPropertyReference(baseValue, propertyKey, this))
    // TODO: Should use this_val as receiver for correct [[Get]] semantics
    __esc_rt_get_prop(parent_proto, key)
}

/// `MakeSuperPropertyReference ( actualThis, propertyKey )` — set variant
///
/// Sets a property via `super.prop = val`. Per §13.3.7.1 (Runtime Semantics:
/// Evaluation of SuperProperty), the property is set on the receiver (`this`),
/// but setters are resolved from the parent prototype chain.
///
/// For simplicity, this currently just sets the property directly on `this`,
/// which matches the most common super property set pattern.
///
/// [spec]: https://tc39.es/ecma262/#sec-super-keyword-runtime-semantics-evaluation
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_set_super(this_val: u64, key: u64, val: u64) {
    // Per spec, super.prop = val sets the property on the receiver (this),
    // not on the parent prototype. The parent prototype is only consulted
    // for setter lookup.
    // TODO: Should resolve setter from homeObject.[[GetPrototypeOf]]() first
    __esc_rt_set_prop(this_val, key, val);
}

/// Format a strict-mode property set error message.
fn strict_set_error_message(name: &str, e: crate::property::PropertyError) -> String {
    match e {
        crate::property::PropertyError::Frozen | crate::property::PropertyError::NotWritable => {
            format!("Cannot assign to read only property '{name}' of object '#<Object>'")
        }
        crate::property::PropertyError::Sealed | crate::property::PropertyError::NotExtensible => {
            format!("Cannot add property {name}, object is not extensible")
        }
        crate::property::PropertyError::NotConfigurable => {
            format!("Cannot redefine property: {name}")
        }
        crate::property::PropertyError::MixedDescriptor => {
            "Invalid property descriptor. Cannot both specify accessors and a value or writable attribute".to_string()
        }
        crate::property::PropertyError::NoSetter => {
            format!("Cannot set property {name} which has only a getter")
        }
        crate::property::PropertyError::InvalidArrayLength => {
            "Invalid array length".to_string()
        }
    }
}

// =========================================================================
// CreateDataPropertyOrThrow (ES2023 7.3.7)
// =========================================================================

/// `CreateDataPropertyOrThrow ( O, P, V )`
///
/// Creates a data property on the given object with attributes
/// `{[[Writable]]: true, [[Enumerable]]: true, [[Configurable]]: true}`.
/// If the operation fails, throws a `TypeError`.
///
/// This differs from `[[Set]]` in that it always creates a *new* data
/// property rather than invoking an inherited setter. Used by array
/// spread, `Object.fromEntries`, and similar spec operations.
///
/// [spec]: https://tc39.es/ecma262/#sec-createdatapropertyorthrow
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_create_data_property(obj: u64, key: u64, val: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    // 1. Let success be ? CreateDataProperty(O, P, V).
    if !v.is_object() {
        let msg = make_rt_string("Cannot create property on a non-object".to_string());
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, msg);
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }

    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return val;
    }

    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else { return val };

    let prop_name = key_to_string(key);

    // §7.3.6 CreateDataProperty ( O, P, V )
    // 1. Let newDesc be the PropertyDescriptor {
    //      [[Value]]: V, [[Writable]]: true,
    //      [[Enumerable]]: true, [[Configurable]]: true }.
    // 2. Return ? O.[[DefineOwnProperty]](P, newDesc).
    let result = SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let mut shapes = shapes.borrow_mut();
            let interner = interner.borrow();
            let opts = crate::property::DefinePropertyOptions {
                value: Some(JsValue::from_raw_bits(val)),
                writable: Some(true),
                enumerable: Some(true),
                configurable: Some(true),
                ..Default::default()
            };
            u.define_own_property(&prop_name, &opts, &mut shapes, &interner)
        })
    });

    // 2. If success is false, throw a TypeError exception.
    if let Err(e) = result {
        let msg = format!("Cannot define property '{prop_name}': {e}");
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
    }

    // 3. Return unused.
    val
}

// =========================================================================
// Private field operations (v0.4 — class #field support)
// =========================================================================

/// Extract a u32 private name ID from a NaN-boxed value.
///
/// Handles both NaN-boxed integers (from `ConstI32` opcodes) and
/// NaN-boxed floats (from `ConstF64`).
fn extract_private_id(bits: u64) -> u32 {
    let v = JsValue::from_raw_bits(bits);
    if let Some(i) = v.as_int() {
        i as u32
    } else if let Some(f) = v.as_number() {
        f as u32
    } else {
        0
    }
}

/// `PrivateFieldAdd ( O, P, value )`
///
/// Installs a private field on an object during class construction.
/// Adds `PropertyKey::Private(id)` to the object's shape and stores the value
/// in the new slot. Bypasses extensibility checks (private fields can be
/// installed on frozen/sealed objects per the ECMAScript spec, §7.3.31).
///
/// `private_id` is a NaN-boxed i32 representing the compile-time private name ID.
///
/// [spec]: https://tc39.es/ecma262/#sec-privatefieldadd
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_install_private_field(obj: u64, private_id: u64, value: u64) -> u64 {
    // §7.3.31 PrivateFieldAdd ( O, P, value )
    // 1. If O is not an object, throw a TypeError exception.
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        return JsValue::undefined().raw_bits();
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::undefined().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::undefined().raw_bits();
    };

    let pid = extract_private_id(private_id);
    let key = shapes::PropertyKey::Private(pid);

    // 2. If O has a private element whose [[Key]] is P, throw a TypeError.
    // TODO: Step 2 — should throw TypeError if private field already exists (duplicate install)
    // 3. Append PrivateElement { [[Key]]: P, [[Kind]]: field, [[Value]]: value } to O.[[PrivateElements]].
    SHAPES.with(|shapes| {
        let mut shapes = shapes.borrow_mut();
        // Private fields bypass extensibility — always add the property.
        // Use add_property_key which handles transitions.
        let new_shape = shapes.add_property_key(u.shape_id, key);
        u.shape_id = new_shape;
    });

    // Store the value in the new slot
    u.slots.push(JsValue::from_raw_bits(value));

    JsValue::undefined().raw_bits()
}

/// `PrivateGet ( O, P )`
///
/// Gets a private field value by compile-time private name ID.
/// Looks up `PropertyKey::Private(id)` in the object's shape. If not found,
/// throws a `TypeError` (brand check failure per §7.3.29).
///
/// `private_id` is a NaN-boxed i32 representing the compile-time private name ID.
///
/// [spec]: https://tc39.es/ecma262/#sec-privateget
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_private_field_get(obj: u64, private_id: u64) -> u64 {
    // §7.3.29 PrivateGet ( O, P )
    // 1. If O is not an object, throw a TypeError exception.
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        let msg = "Cannot read private member from a non-object".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        let msg =
            "Cannot read private member from an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        let msg =
            "Cannot read private member from an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    };

    let pid = extract_private_id(private_id);
    let key = shapes::PropertyKey::Private(pid);

    // 2. Let entry be PrivateElementFind(O, P).
    let found = SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        u.get_slot_by_key(&key, &shapes)
    });

    // 3. If entry is not empty, then
    //   a. If entry.[[Kind]] is field, return entry.[[Value]].
    //   b. If entry.[[Kind]] is method, return entry.[[Value]].
    //   c. (accessor) — not yet implemented for private accessors
    if let Some(val) = found {
        val.raw_bits()
    } else {
        // 4. Throw a TypeError exception.
        let msg =
            "Cannot read private member from an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        JsValue::undefined().raw_bits()
    }
}

/// `PrivateSet ( O, P, value )`
///
/// Sets a private field value by compile-time private name ID.
/// Looks up `PropertyKey::Private(id)` in the object's shape. If not found,
/// throws a `TypeError` (brand check failure per §7.3.30). Otherwise updates
/// the slot value.
///
/// `private_id` is a NaN-boxed i32 representing the compile-time private name ID.
///
/// [spec]: https://tc39.es/ecma262/#sec-privateset
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_private_field_set(obj: u64, private_id: u64, value: u64) -> u64 {
    // §7.3.30 PrivateSet ( O, P, value )
    // 1. If O is not an object, throw a TypeError exception.
    let v = JsValue::from_raw_bits(obj);
    if !v.is_object() {
        let msg = "Cannot write private member to a non-object".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        let msg =
            "Cannot write private member to an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        let msg =
            "Cannot write private member to an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    };

    let pid = extract_private_id(private_id);
    let key = shapes::PropertyKey::Private(pid);

    // 2. Let entry be PrivateElementFind(O, P).
    let found = SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        shapes.lookup_key(u.shape_id, &key).map(|d| d.offset)
    });

    // 3. If entry is not empty, then
    //   a. If entry.[[Kind]] is field, set entry.[[Value]] to value.
    //   b. If entry.[[Kind]] is method, throw a TypeError (methods are not writable).
    //   c. (accessor) — not yet implemented for private accessors
    if let Some(offset) = found {
        let idx = offset as usize;
        if idx < u.slots.len() {
            u.slots[idx] = JsValue::from_raw_bits(value);
        }
        JsValue::undefined().raw_bits()
    } else {
        // 4. Throw a TypeError exception.
        let msg =
            "Cannot write private member to an object whose class did not declare it".to_string();
        let err = __esc_rt_create_error(exceptions::error_tag::TYPE_ERROR, make_rt_string(msg));
        __esc_rt_throw(err);
        JsValue::undefined().raw_bits()
    }
}

/// `PrivateElementFind ( O, P )` — ergonomic `#field in obj` check
///
/// Checks if an object has a private field (`#x in obj`).
/// Returns a NaN-boxed boolean. Uses PrivateElementFind (§7.3.28) to determine
/// if the object has the private element. Does not throw — returns `false` if
/// the object does not have the private field.
///
/// `private_id` is a NaN-boxed i32 representing the compile-time private name ID.
///
/// [spec]: https://tc39.es/ecma262/#sec-privateelementfind
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_private_field_has(obj: u64, private_id: u64) -> u64 {
    let v = JsValue::from_raw_bits(obj);
    // If not an object, #field in non-object is false
    if !v.is_object() {
        return JsValue::bool(false).raw_bits();
    }
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::bool(false).raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::bool(false).raw_bits();
    };

    let pid = extract_private_id(private_id);
    let key = shapes::PropertyKey::Private(pid);

    // §7.3.28 PrivateElementFind ( O, P )
    // 1. If O.[[PrivateElements]] contains a PrivateElement pe such that
    //    pe.[[Key]] is P, return pe.
    // 2. Return empty.
    let found = SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        shapes.lookup_key(u.shape_id, &key).is_some()
    });

    JsValue::bool(found).raw_bits()
}

// =========================================================================
// Built-in constructor property dispatch (Gap A fix)
// =========================================================================

/// Check if a string names a known built-in constructor or namespace.
///
/// Returns `true` for all ECMAScript built-in constructors and well-known
/// namespaces that should support property access (e.g., `Array.isArray`,
/// `Object.keys`, `String.fromCharCode`).
fn is_builtin_constructor(name: &str) -> bool {
    matches!(
        name,
        "Array"
            | "Object"
            | "String"
            | "Number"
            | "Boolean"
            | "Function"
            | "Error"
            | "TypeError"
            | "RangeError"
            | "ReferenceError"
            | "SyntaxError"
            | "URIError"
            | "EvalError"
            | "Promise"
            | "RegExp"
            | "Date"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "WeakRef"
            | "JSON"
            | "Math"
            | "Reflect"
            | "Proxy"
            | "Symbol"
    )
}

/// Return the known static method names for a built-in constructor.
///
/// Returns `Some(list)` if the constructor has static methods we can wrap as
/// `NativeFunc`, or `None` if no methods are known.
///
/// For builtins migrated to the [`BuiltInBuilder`](crate::builtin_builder)
/// (currently Array and Object), the method list is read from the global
/// registry. Non-migrated builtins still use hardcoded arrays.
pub(crate) fn builtin_static_methods(builtin: &str) -> Option<&'static [&'static str]> {
    // Check the builder registry first for migrated builtins.
    if let Some(reg) = crate::builtin_builder::get_registration(builtin) {
        let names = reg.static_method_names();
        if names.is_empty() {
            return None;
        }
        return Some(names);
    }
    match builtin {
        "String" => Some(&["fromCharCode", "fromCodePoint", "raw"]),
        "Number" => Some(&[
            "isNaN",
            "isFinite",
            "isInteger",
            "isSafeInteger",
            "parseInt",
            "parseFloat",
        ]),
        "JSON" => Some(&["parse", "stringify"]),
        "Reflect" => Some(&[
            "get",
            "set",
            "has",
            "deleteProperty",
            "defineProperty",
            "getOwnPropertyDescriptor",
            "ownKeys",
            "getPrototypeOf",
            "setPrototypeOf",
            "isExtensible",
            "preventExtensions",
            "apply",
            "construct",
        ]),
        "Math" => Some(&[
            "abs", "floor", "ceil", "round", "sqrt", "pow", "max", "min", "random", "log", "log2",
            "log10", "exp", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sign", "trunc",
            "cbrt", "hypot", "fround", "clz32", "imul", "sinh", "cosh", "tanh", "asinh", "acosh",
            "atanh", "expm1", "log1p",
        ]),
        "Date" => Some(&["now", "parse", "UTC"]),
        "Promise" => Some(&["resolve", "reject", "all", "allSettled", "any", "race"]),
        "Symbol" => Some(&["for", "keyFor"]),
        _ => None,
    }
}

/// Heap-allocated context for a built-in static method NativeFunc trampoline.
///
/// Stores the constructor name and method name so the trampoline can forward
/// to `dispatch_global_namespace_method`.
struct BuiltinMethodContext {
    /// The built-in constructor name (e.g., `"Array"`).
    builtin: String,
    /// The method name (e.g., `"isArray"`).
    method: String,
}

/// Trampoline for built-in static method NativeFunc wrappers.
///
/// Reads `CURRENT_ARGC` / `CURRENT_ARGV` from thread-locals (set by
/// `__esc_rt_call_indirect` before calling the NativeFunc) and forwards
/// to `dispatch_global_namespace_method`.
fn builtin_static_method_trampoline(context: u64) -> u64 {
    let ctx = unsafe {
        // SAFETY: context is a pointer from Box::into_raw in get_or_create_builtin_method.
        &*(context as *const BuiltinMethodContext)
    };
    let argc = CURRENT_ARGC.with(|cell| cell.get());
    let argv = CURRENT_ARGV.with(|cell| cell.get());

    dispatch_global_namespace_method(&ctx.builtin, &ctx.method, argc, argv)
        .unwrap_or_else(|| JsValue::undefined().raw_bits())
}

/// Get or create a cached NativeFunc wrapper for a built-in static method.
///
/// Returns the NaN-boxed bits of the NativeFunc, creating and caching it on
/// first access. Identity semantics are preserved via the cache.
pub(crate) fn get_or_create_builtin_method(builtin: &str, method: &str) -> u64 {
    let cache_key = (builtin.to_string(), method.to_string());
    BUILTIN_METHOD_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&bits) = cache.get(&cache_key) {
            return bits;
        }

        // Create a heap-allocated context for the trampoline
        let ctx = Box::new(BuiltinMethodContext {
            builtin: builtin.to_string(),
            method: method.to_string(),
        });
        let ctx_ptr = Box::into_raw(ctx) as u64;

        // Create the NativeFunc wrapper
        let func_bits = TaggedObj::boxed(
            ObjTag::Unified,
            UnifiedObject::native_func(builtin_static_method_trampoline, ctx_ptr),
        );

        // Store name, length, and non-constructor marker in OBJECT_PROPS
        OBJECT_PROPS.with(|props| {
            let mut props = props.borrow_mut();
            let map = props.entry(func_bits).or_default();
            map.insert("name".to_string(), make_rt_string(method.to_string()));
            // Most built-in methods have length based on spec, default to 1
            let length = builtin_method_length(builtin, method);
            map.insert(
                "length".to_string(),
                JsValue::number(length as f64).raw_bits(),
            );
            // §10.3.2: Built-in methods do not have [[Construct]].
            // Mark so __esc_rt_call_new throws TypeError.
            map.insert("__non_ctor__".to_string(), JsValue::bool(true).raw_bits());
        });

        cache.insert(cache_key, func_bits);
        func_bits
    })
}

/// Return the `.length` (formal parameter count) for a built-in static method.
///
/// Per ES2023 spec, each built-in function has a defined `.length` property.
/// Returns the expected value for known methods, defaulting to 1.
///
/// For builtins migrated to the [`BuiltInBuilder`](crate::builtin_builder)
/// (currently Array and Object), the length is read from the global registry.
/// Non-migrated builtins still use hardcoded match arms.
fn builtin_method_length(builtin: &str, method: &str) -> u32 {
    // Check the builder registry first for migrated builtins.
    if let Some(reg) = crate::builtin_builder::get_registration(builtin) {
        if let Some(len) = reg.static_method_length(method) {
            return len;
        }
        // Also check instance methods (used by get_or_create_builtin_instance_method).
        if let Some(len) = reg.instance_method_length(method) {
            return len;
        }
    }
    match (builtin, method) {
        // String static methods
        ("String", "fromCharCode" | "fromCodePoint") => 1,
        ("String", "raw") => 1,
        // String instance methods with non-zero length
        ("String", "localeCompare") => 1,
        // Number static methods
        ("Number", "isNaN" | "isFinite" | "isInteger" | "isSafeInteger") => 1,
        ("Number", "parseInt") => 2,
        ("Number", "parseFloat") => 1,
        // JSON
        ("JSON", "parse") => 2,
        ("JSON", "stringify") => 3,
        // Reflect
        ("Reflect", "get") => 2,
        ("Reflect", "set") => 3,
        ("Reflect", "has" | "deleteProperty" | "defineProperty") => 2,
        ("Reflect", "getOwnPropertyDescriptor") => 2,
        ("Reflect", "ownKeys") => 1,
        ("Reflect", "getPrototypeOf" | "isExtensible" | "preventExtensions") => 1,
        ("Reflect", "setPrototypeOf") => 2,
        ("Reflect", "apply") => 3,
        ("Reflect", "construct") => 2,
        // Math
        ("Math", "abs" | "floor" | "ceil" | "round" | "sqrt" | "sign" | "trunc" | "cbrt") => 1,
        ("Math", "pow" | "atan2" | "imul") => 2,
        ("Math", "fround" | "clz32") => 1,
        ("Math", "max" | "min" | "hypot") => 2,
        ("Math", "random") => 0,
        ("Math", "log" | "log2" | "log10" | "exp") => 1,
        ("Math", "sin" | "cos" | "tan" | "asin" | "acos" | "atan") => 1,
        ("Math", "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "expm1" | "log1p") => 1,
        // Date
        ("Date", "now") => 0,
        ("Date", "parse") => 1,
        ("Date", "UTC") => 7,
        // Promise
        ("Promise", "resolve" | "reject") => 1,
        ("Promise", "all" | "allSettled" | "any" | "race") => 1,
        // Symbol
        ("Symbol", "for" | "keyFor") => 1,
        // Function
        ("Function", "call") => 1,
        ("Function", "apply") => 2,
        ("Function", "bind") => 1,
        ("Function", "toString") => 0,
        // Boolean
        ("Boolean", "toString" | "valueOf") => 0,
        // Number
        ("Number", "toString") => 1,
        ("Number", "valueOf") => 0,
        ("Number", "toFixed" | "toExponential" | "toPrecision") => 1,
        // String instance methods (per spec lengths)
        ("String", "charAt" | "charCodeAt" | "codePointAt" | "at") => 1,
        ("String", "indexOf" | "lastIndexOf") => 1,
        ("String", "includes" | "startsWith" | "endsWith") => 1,
        ("String", "slice" | "substring") => 2,
        ("String", "split") => 2,
        ("String", "replace" | "replaceAll") => 2,
        ("String", "match" | "matchAll" | "search") => 1,
        ("String", "repeat" | "padStart" | "padEnd") => 1,
        ("String", "trim" | "trimStart" | "trimEnd") => 0,
        ("String", "toUpperCase" | "toLowerCase") => 0,
        ("String", "normalize" | "concat") => 1,
        ("String", "toString" | "valueOf") => 0,
        // Date instance methods
        ("Date", "toString" | "toDateString" | "toTimeString" | "toISOString") => 0,
        ("Date", "toUTCString" | "toJSON" | "toLocaleDateString" | "toLocaleTimeString") => 0,
        ("Date", "toLocaleString" | "valueOf" | "getTime" | "getTimezoneOffset") => 0,
        ("Date", "getFullYear" | "getMonth" | "getDate" | "getDay") => 0,
        ("Date", "getHours" | "getMinutes" | "getSeconds" | "getMilliseconds") => 0,
        ("Date", "getUTCFullYear" | "getUTCMonth" | "getUTCDate" | "getUTCDay") => 0,
        ("Date", "getUTCHours" | "getUTCMinutes" | "getUTCSeconds" | "getUTCMilliseconds") => 0,
        ("Date", "setTime" | "setMilliseconds" | "setUTCMilliseconds") => 1,
        ("Date", "setFullYear" | "setMonth" | "setDate") => 1,
        ("Date", "setHours" | "setMinutes" | "setSeconds") => 1,
        ("Date", "setUTCFullYear" | "setUTCMonth" | "setUTCDate") => 1,
        ("Date", "setUTCHours" | "setUTCMinutes" | "setUTCSeconds") => 1,
        // Array instance methods
        ("Array", "toString" | "toLocaleString") => 0,
        ("Array", "pop" | "shift" | "reverse" | "values" | "keys" | "entries") => 0,
        ("Array", "push" | "join" | "indexOf" | "lastIndexOf" | "includes") => 1,
        ("Array", "forEach" | "map" | "filter" | "find" | "findIndex" | "some" | "every") => 1,
        ("Array", "reduce" | "reduceRight") => 1,
        ("Array", "slice" | "splice" | "fill" | "copyWithin") => 2,
        ("Array", "sort" | "flat" | "flatMap" | "at") => 1,
        ("Array", "concat" | "unshift") => 1,
        _ => 1,
    }
}

/// Get or create a cached prototype object for a built-in constructor.
///
/// Returns the NaN-boxed bits of the prototype object. The prototype is an
/// ordinary object populated with NativeFunc wrappers for all instance methods.
/// A `__builtin_proto__` marker property is also set for backward-compatible
/// detection.
///
/// The `constructor` property is set to the real constructor object (from the
/// global object registry), so that `Array.prototype.constructor === Array`
/// holds true.
pub(crate) fn get_or_create_builtin_prototype(builtin: &str) -> u64 {
    BUILTIN_PROTO_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&bits) = cache.get(builtin) {
            return bits;
        }

        // Create the prototype object. Per spec:
        // - Boolean.prototype is a Boolean object with [[BooleanData]] = false (§20.3.3)
        // - Number.prototype is a Number object with [[NumberData]] = +0 (§21.1.3)
        // - String.prototype is a String object with [[StringData]] = "" (§22.1.3)
        // - Function.prototype is a built-in function that returns undefined (§20.2.3)
        // All others are ordinary objects.
        let proto = match builtin {
            "Boolean" => {
                let obj = crate::internal_data::UnifiedObject::boolean_wrapper(
                    shapes::ShapeId(0),
                    nanbox::JsValue::bool(false).raw_bits(),
                );
                crate::tagged_obj::TaggedObj::boxed(crate::tagged_obj::ObjTag::Unified, obj)
            }
            "Number" => {
                let obj = crate::internal_data::UnifiedObject::number_wrapper(
                    shapes::ShapeId(0),
                    nanbox::JsValue::number(0.0).raw_bits(),
                );
                crate::tagged_obj::TaggedObj::boxed(crate::tagged_obj::ObjTag::Unified, obj)
            }
            "String" => {
                let obj = crate::internal_data::UnifiedObject::string_wrapper(
                    shapes::ShapeId(0),
                    make_rt_string(String::new()),
                );
                crate::tagged_obj::TaggedObj::boxed(crate::tagged_obj::ObjTag::Unified, obj)
            }
            "Function" => {
                // §20.2.3: Function.prototype is a built-in function object.
                // When invoked, it accepts any arguments and returns undefined.
                // typeof Function.prototype === "function"
                fn function_prototype_trampoline(_arg: u64) -> u64 {
                    JsValue::undefined().raw_bits()
                }
                let obj = crate::internal_data::UnifiedObject::native_func(
                    function_prototype_trampoline,
                    0,
                );
                let bits =
                    crate::tagged_obj::TaggedObj::boxed(crate::tagged_obj::ObjTag::Unified, obj);
                // §20.2.3: Function.prototype.length = 0
                // §20.2.3: Function.prototype.name = ""
                // Mark as non-constructor per §20.2.3
                OBJECT_PROPS.with(|props| {
                    let mut props = props.borrow_mut();
                    let map = props.entry(bits).or_default();
                    map.insert("length".to_string(), JsValue::number(0.0).raw_bits());
                    map.insert("name".to_string(), make_rt_string(String::new()));
                    map.insert("__non_ctor__".to_string(), JsValue::bool(true).raw_bits());
                });
                bits
            }
            _ => __esc_rt_create_object(),
        };

        // Helper: set a property on the prototype with specific descriptor flags.
        // Per ES2023, built-in prototype methods must be:
        //   { writable: true, enumerable: false, configurable: true }
        // The marker and constructor properties are also non-enumerable.
        let set_proto_prop = |proto_bits: u64,
                              name: &str,
                              value: u64,
                              writable: bool,
                              enumerable: bool,
                              configurable: bool| {
            let tag = read_obj_tag(proto_bits);
            if tag != Some(ObjTag::Unified as u8) {
                return;
            }
            let uni = unsafe {
                // SAFETY: tag check confirms this is a unified object.
                deref_tagged_mut::<crate::internal_data::UnifiedObject>(proto_bits)
            };
            let Some(u) = uni else { return };
            SHAPES.with(|shapes| {
                INTERNER.with(|interner| {
                    let mut shapes = shapes.borrow_mut();
                    let interner = interner.borrow();
                    u.set_slot_by_name_with_flags(
                        name,
                        JsValue::from_raw_bits(value),
                        writable,
                        enumerable,
                        configurable,
                        &mut shapes,
                        &interner,
                    );
                });
            });
        };

        // Set a marker property so get_prop can detect this is a builtin prototype.
        // Non-enumerable so it doesn't appear in Object.keys.
        let marker_val = make_rt_string(builtin.to_string());
        set_proto_prop(proto, "__builtin_proto__", marker_val, true, false, true);

        // Set the constructor property to the real constructor object.
        // Per ES spec, constructor is { writable: true, enumerable: false, configurable: true }.
        let ctor_bits = super::get_global_object(builtin);
        set_proto_prop(proto, "constructor", ctor_bits, true, false, true);

        // Error prototypes: set .name and .message properties.
        // Per ES2024 §20.5.3.3: Error.prototype.name = "Error"
        // Per ES2024 §20.5.3.2: Error.prototype.message = ""
        // Same pattern for TypeError, RangeError, etc.
        if matches!(
            builtin,
            "Error"
                | "TypeError"
                | "RangeError"
                | "ReferenceError"
                | "SyntaxError"
                | "URIError"
                | "EvalError"
        ) {
            let name_val = make_rt_string(builtin.to_string());
            set_proto_prop(proto, "name", name_val, true, false, true);
            let msg_val = make_rt_string(String::new());
            set_proto_prop(proto, "message", msg_val, true, false, true);
        }

        // Eagerly populate NativeFunc wrappers for all instance methods.
        // Per ES2023, all built-in prototype methods have:
        //   { writable: true, enumerable: false, configurable: true }
        let methods = builtin_instance_method_list(builtin);
        for method in methods {
            let func_bits = get_or_create_builtin_instance_method(builtin, method);
            set_proto_prop(proto, method, func_bits, true, false, true);
        }

        // Cache before returning so recursive access finds it
        cache.insert(builtin.to_string(), proto);
        proto
    })
}

/// Get or lazily create the `Object.prototype` singleton.
///
/// Used by `Object.getPrototypeOf()` and similar explicit prototype queries.
/// Triggers lazy creation of `Object.prototype` if it doesn't exist yet.
pub(crate) fn get_object_prototype() -> u64 {
    get_or_create_builtin_prototype("Object")
}

/// The `.length` (formal parameter count) for a built-in constructor function.
///
/// Per ES2023 spec, each constructor has a `.length` equal to the number of
/// formal parameters it expects.
///
/// For builtins migrated to the [`BuiltInBuilder`](crate::builtin_builder)
/// (currently Array and Object), the length is read from the global registry.
/// Non-migrated builtins still use hardcoded match arms.
fn builtin_constructor_length(name: &str) -> i32 {
    // Check the builder registry first for migrated builtins.
    if let Some(reg) = crate::builtin_builder::get_registration(name) {
        return reg.constructor_length as i32;
    }
    match name {
        "Function" | "Error" | "TypeError" | "RangeError" | "ReferenceError" | "SyntaxError"
        | "URIError" | "EvalError" | "Promise" => 1,
        "Boolean" | "Number" | "String" | "RegExp" | "Proxy" => 1,
        "Date" => 7,
        "Map" | "Set" | "WeakMap" | "WeakSet" | "WeakRef" => 0,
        "JSON" | "Math" | "Reflect" => 0,
        _ => 0,
    }
}

/// Dispatch a property access on a built-in constructor or namespace.
///
/// Handles `.prototype`, static methods, `.name`, `.length`, and
/// `Symbol.iterator`-related well-known properties. Returns `Some(bits)` if
/// the property is handled, `None` if the caller should fall through to
/// the default behavior.
fn dispatch_builtin_property(builtin: &str, prop: &str) -> Option<u64> {
    // 1. prototype access — return a cached prototype object
    if prop == "prototype" {
        // JSON, Math, Reflect are not constructors — they have no .prototype
        if matches!(builtin, "JSON" | "Math" | "Reflect") {
            return Some(JsValue::undefined().raw_bits());
        }
        let proto_bits = get_or_create_builtin_prototype(builtin);

        // Also store the prototype in OBJECT_PROPS on the constructor's real
        // global object. This ensures that property lookups via the object
        // reference (e.g., `let x = Array; x.prototype`) find it through
        // the standard lookup_property_chain → OBJECT_PROPS path.
        let ctor_bits = super::get_global_object(builtin);
        if ctor_bits != JsValue::undefined().raw_bits() {
            OBJECT_PROPS.with(|props| {
                let mut props = props.borrow_mut();
                let map = props.entry(ctor_bits).or_default();
                map.entry("prototype".to_string()).or_insert(proto_bits);
            });
        }

        return Some(proto_bits);
    }

    // 2. Constructor name property
    if prop == "name" {
        return Some(make_rt_string(builtin.to_string()));
    }

    // 3. Constructor length property
    if prop == "length" {
        return Some(JsValue::int(builtin_constructor_length(builtin)).raw_bits());
    }

    // 4. Static method access — return a cached NativeFunc wrapper
    if let Some(methods) = builtin_static_methods(builtin)
        && methods.contains(&prop)
    {
        return Some(get_or_create_builtin_method(builtin, prop));
    }

    // 5. Builtin constant properties (Number.NaN, Number.MAX_VALUE, Math.PI, etc.)
    if let Some(val) = builtin_constant(builtin, prop) {
        return Some(val);
    }

    None
}

/// Resolve a constant property on a built-in constructor/namespace.
///
/// Returns `Some(value)` for known constants like `Number.NaN`, `Math.PI`, etc.
pub(crate) fn builtin_constant(builtin: &str, prop: &str) -> Option<u64> {
    match builtin {
        "Number" => match prop {
            "NaN" => Some(JsValue::number(f64::NAN).raw_bits()),
            "MAX_VALUE" => Some(JsValue::number(f64::MAX).raw_bits()),
            "MIN_VALUE" => Some(JsValue::number(5e-324).raw_bits()),
            "POSITIVE_INFINITY" => Some(JsValue::number(f64::INFINITY).raw_bits()),
            "NEGATIVE_INFINITY" => Some(JsValue::number(f64::NEG_INFINITY).raw_bits()),
            "EPSILON" => Some(JsValue::number(f64::EPSILON).raw_bits()),
            "MAX_SAFE_INTEGER" => Some(JsValue::number(9_007_199_254_740_991.0).raw_bits()),
            "MIN_SAFE_INTEGER" => Some(JsValue::number(-9_007_199_254_740_991.0).raw_bits()),
            _ => None,
        },
        "Symbol" => match prop {
            "iterator" => Some(JsValue::symbol(crate::symbol::SYMBOL_ITERATOR).raw_bits()),
            "toPrimitive" => Some(JsValue::symbol(crate::symbol::SYMBOL_TO_PRIMITIVE).raw_bits()),
            "hasInstance" => Some(JsValue::symbol(crate::symbol::SYMBOL_HAS_INSTANCE).raw_bits()),
            "toStringTag" => Some(JsValue::symbol(crate::symbol::SYMBOL_TO_STRING_TAG).raw_bits()),
            "asyncIterator" => {
                Some(JsValue::symbol(crate::symbol::SYMBOL_ASYNC_ITERATOR).raw_bits())
            }
            "species" => Some(JsValue::symbol(crate::symbol::SYMBOL_SPECIES).raw_bits()),
            "unscopables" => Some(JsValue::symbol(crate::symbol::SYMBOL_UNSCOPABLES).raw_bits()),
            _ => None,
        },
        "Math" => match prop {
            "E" => Some(JsValue::number(std::f64::consts::E).raw_bits()),
            "LN2" => Some(JsValue::number(std::f64::consts::LN_2).raw_bits()),
            "LN10" => Some(JsValue::number(std::f64::consts::LN_10).raw_bits()),
            "LOG2E" => Some(JsValue::number(std::f64::consts::LOG2_E).raw_bits()),
            "LOG10E" => Some(JsValue::number(std::f64::consts::LOG10_E).raw_bits()),
            "PI" => Some(JsValue::number(std::f64::consts::PI).raw_bits()),
            "SQRT2" => Some(JsValue::number(std::f64::consts::SQRT_2).raw_bits()),
            "SQRT1_2" => Some(JsValue::number(1.0 / std::f64::consts::SQRT_2).raw_bits()),
            _ => None,
        },
        _ => None,
    }
}

/// Detect if an object is a built-in prototype created by
/// `get_or_create_builtin_prototype` and return the constructor name.
///
/// Checks for the `__builtin_proto__` marker property using direct property
/// chain lookup (NOT `__esc_rt_get_prop`, to avoid infinite recursion).
/// Returns `Some(name)` if found, `None` otherwise.
fn detect_builtin_prototype(obj_bits: u64) -> Option<String> {
    let marker_val = lookup_property_chain(obj_bits, "__builtin_proto__");
    let v = JsValue::from_raw_bits(marker_val);
    if v.is_undefined() {
        return None;
    }
    if v.as_string().is_some() {
        let data = crate::string_ops::get_string_data(v);
        if is_builtin_constructor(&data) {
            return Some(data);
        }
    }
    None
}

/// The known instance method names for a built-in prototype.
///
/// Returns `true` if `method` is a recognized instance method of the given
/// builtin constructor. Used to create NativeFunc wrappers on prototype objects.
///
/// For builtins migrated to the [`BuiltInBuilder`](crate::builtin_builder)
/// (currently Array and Object), the lookup is done via the global registry.
/// Non-migrated builtins still use hardcoded match arms.
fn is_builtin_instance_method(builtin: &str, method: &str) -> bool {
    // Check the builder registry first for migrated builtins.
    if let Some(reg) = crate::builtin_builder::get_registration(builtin) {
        return reg.instance_method_names().contains(&method);
    }
    match builtin {
        "String" => matches!(
            method,
            "charAt"
                | "charCodeAt"
                | "codePointAt"
                | "indexOf"
                | "lastIndexOf"
                | "includes"
                | "startsWith"
                | "endsWith"
                | "slice"
                | "substring"
                | "trim"
                | "trimStart"
                | "trimEnd"
                | "toUpperCase"
                | "toLowerCase"
                | "split"
                | "replace"
                | "replaceAll"
                | "match"
                | "matchAll"
                | "search"
                | "repeat"
                | "padStart"
                | "padEnd"
                | "at"
                | "normalize"
                | "localeCompare"
                | "concat"
                | "toString"
                | "valueOf"
                | "raw"
        ),
        "Function" => matches!(method, "call" | "apply" | "bind" | "toString"),
        "Number" => matches!(
            method,
            "toFixed" | "toExponential" | "toPrecision" | "toString" | "valueOf"
        ),
        "Boolean" => matches!(method, "toString" | "valueOf"),
        "Date" => matches!(
            method,
            "getTime"
                | "getFullYear"
                | "getMonth"
                | "getDate"
                | "getDay"
                | "getHours"
                | "getMinutes"
                | "getSeconds"
                | "getMilliseconds"
                | "getTimezoneOffset"
                | "getUTCFullYear"
                | "getUTCMonth"
                | "getUTCDate"
                | "getUTCDay"
                | "getUTCHours"
                | "getUTCMinutes"
                | "getUTCSeconds"
                | "getUTCMilliseconds"
                | "setTime"
                | "setFullYear"
                | "setMonth"
                | "setDate"
                | "setHours"
                | "setMinutes"
                | "setSeconds"
                | "setMilliseconds"
                | "setUTCFullYear"
                | "setUTCMonth"
                | "setUTCDate"
                | "setUTCHours"
                | "setUTCMinutes"
                | "setUTCSeconds"
                | "setUTCMilliseconds"
                | "toString"
                | "toDateString"
                | "toTimeString"
                | "toISOString"
                | "toUTCString"
                | "toJSON"
                | "toLocaleDateString"
                | "toLocaleTimeString"
                | "toLocaleString"
                | "valueOf"
        ),
        _ => false,
    }
}

/// Returns the list of all instance method names for a built-in prototype.
///
/// Used during prototype creation to eagerly populate NativeFunc wrappers
/// for all instance methods on the prototype object.
///
/// For builtins migrated to the [`BuiltInBuilder`](crate::builtin_builder)
/// (currently Array and Object), the method list is read from the global
/// registry. Non-migrated builtins still use hardcoded arrays.
fn builtin_instance_method_list(builtin: &str) -> &'static [&'static str] {
    // Check the builder registry first for migrated builtins.
    if let Some(reg) = crate::builtin_builder::get_registration(builtin) {
        return reg.instance_method_names();
    }
    match builtin {
        "String" => &[
            "charAt",
            "charCodeAt",
            "codePointAt",
            "concat",
            "endsWith",
            "includes",
            "indexOf",
            "lastIndexOf",
            "match",
            "matchAll",
            "normalize",
            "localeCompare",
            "padEnd",
            "padStart",
            "repeat",
            "replace",
            "replaceAll",
            "search",
            "slice",
            "split",
            "startsWith",
            "substring",
            "toLowerCase",
            "toUpperCase",
            "trim",
            "trimEnd",
            "trimStart",
            "toString",
            "valueOf",
            "at",
            "raw",
        ],
        "Number" => &[
            "toFixed",
            "toExponential",
            "toPrecision",
            "toString",
            "valueOf",
        ],
        "Boolean" => &["toString", "valueOf"],
        "Function" => &["call", "apply", "bind", "toString"],
        "Map" => &[
            "get", "set", "has", "delete", "clear", "forEach", "entries", "keys", "values",
            "toString",
        ],
        "Set" => &[
            "add",
            "has",
            "delete",
            "clear",
            "forEach",
            "entries",
            "keys",
            "values",
            "union",
            "intersection",
            "difference",
            "symmetricDifference",
            "isSubsetOf",
            "isSupersetOf",
            "isDisjointFrom",
            "toString",
        ],
        "WeakMap" => &["get", "set", "has", "delete"],
        "WeakSet" => &["add", "has", "delete"],
        "WeakRef" => &["deref"],
        "RegExp" => &["test", "exec", "toString"],
        "Date" => &[
            "getTime",
            "getFullYear",
            "getMonth",
            "getDate",
            "getDay",
            "getHours",
            "getMinutes",
            "getSeconds",
            "getMilliseconds",
            "getTimezoneOffset",
            "getUTCFullYear",
            "getUTCMonth",
            "getUTCDate",
            "getUTCDay",
            "getUTCHours",
            "getUTCMinutes",
            "getUTCSeconds",
            "getUTCMilliseconds",
            "setTime",
            "setFullYear",
            "setMonth",
            "setDate",
            "setHours",
            "setMinutes",
            "setSeconds",
            "setMilliseconds",
            "setUTCFullYear",
            "setUTCMonth",
            "setUTCDate",
            "setUTCHours",
            "setUTCMinutes",
            "setUTCSeconds",
            "setUTCMilliseconds",
            "toString",
            "toDateString",
            "toTimeString",
            "toISOString",
            "toUTCString",
            "toJSON",
            "toLocaleDateString",
            "toLocaleTimeString",
            "toLocaleString",
            "valueOf",
        ],
        _ => &[],
    }
}

/// Heap-allocated context for a built-in instance method NativeFunc trampoline.
///
/// Stores the constructor name and method name so the trampoline can dispatch
/// instance method calls via the standard method routing infrastructure.
struct BuiltinInstanceMethodContext {
    /// The built-in constructor name (e.g., `"Array"`).
    builtin: String,
    /// The method name (e.g., `"forEach"`).
    method: String,
}

/// Trampoline for built-in instance method NativeFunc wrappers.
///
/// When called (e.g., via `Array.prototype.forEach.call(obj, fn)`),
/// reads `CURRENT_THIS`, `CURRENT_ARGC`, `CURRENT_ARGV` from thread-locals
/// and dispatches to the appropriate instance method handler.
fn builtin_instance_method_trampoline(context: u64) -> u64 {
    let ctx = unsafe {
        // SAFETY: context is a pointer from Box::into_raw in the instance method cache.
        &*(context as *const BuiltinInstanceMethodContext)
    };
    let argc = CURRENT_ARGC.with(|cell| cell.get());
    let argv = CURRENT_ARGV.with(|cell| cell.get());
    let this = super::CURRENT_THIS.with(|cell| cell.get());

    match ctx.builtin.as_str() {
        "Array" => super::dispatch_array_method(this, &ctx.method, argc, argv),
        "String" => super::dispatch_string_method(this, &ctx.method, argc, argv),
        "Object" => super::dispatch_object_proto_method(this, &ctx.method, argc, argv)
            .unwrap_or_else(|| JsValue::undefined().raw_bits()),
        "Number" => {
            let val = JsValue::from_raw_bits(this);
            super::dispatch_number_instance_method(val, &ctx.method, argc, argv)
                .unwrap_or_else(|| JsValue::undefined().raw_bits())
        }
        "Boolean" => {
            let val = JsValue::from_raw_bits(this);
            super::dispatch_boolean_method(val, &ctx.method)
                .unwrap_or_else(|| JsValue::undefined().raw_bits())
        }
        "Function" => {
            // Function.prototype methods (call/apply/bind/toString) invoked
            // via a NativeFunc wrapper, e.g. `Function.prototype.call.call(fn)`.
            // `this` is the function that the method is called on.
            match ctx.method.as_str() {
                "call" => unsafe {
                    // SAFETY: argc/argv are valid per CURRENT_ARGC/CURRENT_ARGV.
                    super::dispatch_core::dispatch_function_call(this, argc, argv)
                },
                "apply" => unsafe {
                    // SAFETY: argc/argv are valid per CURRENT_ARGC/CURRENT_ARGV.
                    super::dispatch_core::dispatch_function_apply(this, argc, argv)
                },
                "bind" => super::dispatch_core::dispatch_function_bind(this, argc, argv),
                "toString" => super::dispatch_function_to_string(this),
                _ => JsValue::undefined().raw_bits(),
            }
        }
        _ => JsValue::undefined().raw_bits(),
    }
}

/// Get or create a cached NativeFunc wrapper for a built-in instance method.
///
/// Returns the NaN-boxed bits of the NativeFunc. The wrapper will dispatch
/// instance method calls using `CURRENT_THIS` as the receiver.
fn get_or_create_builtin_instance_method(builtin: &str, method: &str) -> u64 {
    // Reuse the same cache but with a "proto:" prefix to distinguish from statics
    let cache_key = (format!("proto:{builtin}"), method.to_string());
    BUILTIN_METHOD_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&bits) = cache.get(&cache_key) {
            return bits;
        }

        let ctx = Box::new(BuiltinInstanceMethodContext {
            builtin: builtin.to_string(),
            method: method.to_string(),
        });
        let ctx_ptr = Box::into_raw(ctx) as u64;

        let func_bits = TaggedObj::boxed(
            ObjTag::Unified,
            UnifiedObject::native_func(builtin_instance_method_trampoline, ctx_ptr),
        );

        // Store name, length, and non-constructor marker.
        let length = builtin_method_length(builtin, method);
        OBJECT_PROPS.with(|props| {
            let mut props = props.borrow_mut();
            let map = props.entry(func_bits).or_default();
            map.insert("name".to_string(), make_rt_string(method.to_string()));
            map.insert(
                "length".to_string(),
                JsValue::number(length as f64).raw_bits(),
            );
            // §10.3.2: Built-in methods do not have [[Construct]].
            map.insert("__non_ctor__".to_string(), JsValue::bool(true).raw_bits());
        });

        cache.insert(cache_key, func_bits);
        func_bits
    })
}

// =========================================================================
// process namespace property dispatch
// =========================================================================

/// Dispatch a property access on the `process` global namespace.
///
/// Returns the NaN-boxed bits for the property value. Handles static
/// properties (`argv`, `platform`, `arch`, `pid`, `version`, `env`) and
/// returns `undefined` for methods (those are dispatched via `__esc_rt_call_method`).
fn dispatch_process_property(name: &str) -> u64 {
    match name {
        "argv" => build_process_argv(),
        "env" => build_process_env(),
        "platform" => make_rt_string(process_platform().to_string()),
        "arch" => make_rt_string(process_arch().to_string()),
        "pid" => JsValue::int(std::process::id() as i32).raw_bits(),
        "version" => make_rt_string(format!("v{}", env!("CARGO_PKG_VERSION"))),
        // Methods (exit, cwd, hrtime) return undefined when accessed as properties;
        // they are dispatched through __esc_rt_call_method instead.
        "exit" | "cwd" | "hrtime" => JsValue::undefined().raw_bits(),
        _ => JsValue::undefined().raw_bits(),
    }
}

/// Build the `process.argv` array from the host ABI.
fn build_process_argv() -> u64 {
    let count = host::abi::__esc_host_args_count();
    let mut elements = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut buf = vec![0u8; 4096];
        // SAFETY: buf is a valid mutable slice of known length.
        let len = unsafe { host::abi::__esc_host_args_get(i, buf.as_mut_ptr(), buf.len() as u32) };
        if len >= 0 {
            let actual_len = (len as usize).min(buf.len());
            let s = String::from_utf8_lossy(&buf[..actual_len]).into_owned();
            elements.push(JsValue::from_raw_bits(make_rt_string(s)));
        }
    }
    create_array_from_elements(elements)
}

/// Build the `process.env` object as a snapshot of environment variables.
fn build_process_env() -> u64 {
    let obj = super::__esc_rt_create_object();
    for (key, val) in std::env::vars() {
        let key_bits = make_rt_string(key);
        let val_bits = make_rt_string(val);
        super::__esc_rt_set_prop(obj, key_bits, val_bits);
    }
    obj
}

/// Return the platform string using Node.js conventions.
fn process_platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else {
        "unknown"
    }
}

/// Return the architecture string using Node.js conventions.
fn process_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}
