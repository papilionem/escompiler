//! Reflect namespace dispatch.
//!
//! Implements all 13 `Reflect.*` static methods. Unlike `Object.*` methods,
//! Reflect methods return booleans for success/failure instead of throwing on
//! error (e.g. `Reflect.defineProperty` returns `false` where
//! `Object.defineProperty` throws `TypeError`).

use nanbox::JsValue;

use crate::internal_data::{InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, deref_tagged, deref_tagged_mut, read_obj_tag};

use super::{
    __esc_rt_delete_prop, __esc_rt_get_prop, __esc_rt_has_prop, __esc_rt_set_prop, INTERNER,
    SHAPES, create_array_from_elements, create_empty_array, key_to_string, make_rt_string,
    read_argv,
};

/// Throw a TypeError with the given message and return `undefined` bits.
///
/// Used by Reflect methods when the target is not an Object (spec step 1).
fn throw_type_error(msg: &str) -> u64 {
    let msg_bits = make_rt_string(format!("TypeError: {msg}"));
    let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg_bits);
    super::__esc_rt_throw(err);
    JsValue::undefined().raw_bits()
}

/// Check if a NaN-boxed value is callable (has `[[Call]]` internal method).
///
/// Returns `true` for closures, functions, and native functions.
fn is_callable_value(bits: u64) -> bool {
    let tag = read_obj_tag(bits);
    if tag != Some(ObjTag::Unified as u8) {
        return false;
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    uni.is_some_and(|u| u.is_callable())
}

/// Check if a value is a constructor (callable AND has [[Construct]]).
///
/// Built-in methods marked with `__non_ctor__` in OBJECT_PROPS are callable
/// but not constructible per §10.3.2.
fn is_constructor_value(bits: u64) -> bool {
    if !is_callable_value(bits) {
        return false;
    }
    // Check if it's a NativeFunc marked as non-constructible
    let is_non_ctor = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props
            .get(&bits)
            .and_then(|m| m.get("__non_ctor__"))
            .is_some()
    });
    !is_non_ctor
}

/// Dispatch a `Reflect.*` static method.
///
/// Returns `Some(result_bits)` if `method` is a known Reflect method,
/// `None` otherwise.
pub(crate) fn dispatch_reflect_method(method: &str, argc: u32, argv: *const u64) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        "get" => Some(reflect_get(&args)),
        "set" => Some(reflect_set(&args)),
        "has" => Some(reflect_has(&args)),
        "deleteProperty" => Some(reflect_delete_property(&args)),
        "defineProperty" => Some(reflect_define_property(&args)),
        "getOwnPropertyDescriptor" => Some(reflect_get_own_property_descriptor(&args)),
        "ownKeys" => Some(reflect_own_keys(&args)),
        "getPrototypeOf" => Some(reflect_get_prototype_of(&args)),
        "setPrototypeOf" => Some(reflect_set_prototype_of(&args)),
        "isExtensible" => Some(reflect_is_extensible(&args)),
        "preventExtensions" => Some(reflect_prevent_extensions(&args)),
        "apply" => Some(reflect_apply(&args)),
        "construct" => Some(reflect_construct(&args)),
        _ => None,
    }
}

/// `Reflect.get ( target, propertyKey [ , receiver ] )`
///
/// Returns the value of the property, or `undefined` if it does not exist.
/// The optional `receiver` is the `this` value for getter accessors (currently
/// ignored — falls back to plain `__esc_rt_get_prop`).
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.get
fn reflect_get(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(target);
    if !target_val.is_object() {
        return throw_type_error("Reflect.get called on non-object");
    }
    // 2. Let key be ? ToPropertyKey(propertyKey).
    let key = args
        .get(1)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    // 3. If receiver is not present, then
    //   a. Set receiver to target.
    // TODO: Step 3 — receiver (args[2]) is ignored for now; plain [[Get]] is used.
    // 4. Return ? target.[[Get]](key, receiver).
    __esc_rt_get_prop(target, key)
}

/// `Reflect.set ( target, propertyKey, V [ , receiver ] )`
///
/// Returns `true` if the set succeeded, `false` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.set
fn reflect_set(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    // 2. Let key be ? ToPropertyKey(propertyKey).
    let key = args
        .get(1)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    // 3. (V is the value argument.)
    let value = args
        .get(2)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

    let target_val = JsValue::from_raw_bits(target);
    if !target_val.is_object() {
        return throw_type_error("Reflect.set called on non-object");
    }

    // 4. If receiver is not present, then
    //   a. Set receiver to target.
    // TODO: Step 4 — receiver (args[3]) is ignored; always uses target as receiver.

    // Check if target is frozen — set would fail
    let tag = read_obj_tag(target);
    if tag == Some(ObjTag::Unified as u8) {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(target) };
        if let Some(u) = uni
            && u.is_frozen()
        {
            return JsValue::bool(false).raw_bits();
        }
    }

    // 5. Return ? target.[[Set]](key, V, receiver).
    __esc_rt_set_prop(target, key, value);
    JsValue::bool(true).raw_bits()
}

/// `Reflect.has ( target, propertyKey )`
///
/// Returns `true` if the target has the property (own or inherited).
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.has
fn reflect_has(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(target);
    if !target_val.is_object() {
        return throw_type_error("Reflect.has called on non-object");
    }
    // 2. Let key be ? ToPropertyKey(propertyKey).
    let key = args
        .get(1)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    // 3. Return ? target.[[HasProperty]](key).
    __esc_rt_has_prop(target, key)
}

/// `Reflect.deleteProperty ( target, propertyKey )`
///
/// Returns `true` if the property was successfully deleted, `false` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.deleteproperty
fn reflect_delete_property(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(target);
    if !target_val.is_object() {
        return throw_type_error("Reflect.deleteProperty called on non-object");
    }
    // 2. Let key be ? ToPropertyKey(propertyKey).
    let key = args
        .get(1)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    // 3. Return ? target.[[Delete]](key).
    __esc_rt_delete_prop(target, key)
}

/// `Reflect.defineProperty ( target, propertyKey, attributes )`
///
/// Returns `true` if the property was defined successfully, `false` otherwise.
/// Unlike `Object.defineProperty`, this does NOT throw on failure.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.defineproperty
fn reflect_define_property(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    if args.is_empty() || !args[0].is_object() {
        return throw_type_error("Reflect.defineProperty called on non-object");
    }
    if args.len() < 3 {
        return JsValue::bool(false).raw_bits();
    }
    let obj = args[0].raw_bits();
    let prop = args[1].raw_bits();
    let descriptor = args[2].raw_bits();

    let obj_tag = read_obj_tag(obj);
    if obj_tag != Some(ObjTag::Unified as u8) {
        return JsValue::bool(false).raw_bits();
    }

    // 2. Let key be ? ToPropertyKey(propertyKey).
    // (Implicit — prop is already a key.)

    // 3. Let desc be ? ToPropertyDescriptor(attributes).
    // Extract descriptor fields
    let writable_key = make_rt_string("writable".to_string());
    let enumerable_key = make_rt_string("enumerable".to_string());
    let configurable_key = make_rt_string("configurable".to_string());
    let value_key = make_rt_string("value".to_string());
    let get_key = make_rt_string("get".to_string());
    let set_key = make_rt_string("set".to_string());

    let writable_bits = __esc_rt_get_prop(descriptor, writable_key);
    let enumerable_bits = __esc_rt_get_prop(descriptor, enumerable_key);
    let configurable_bits = __esc_rt_get_prop(descriptor, configurable_key);
    let val_bits = __esc_rt_get_prop(descriptor, value_key);
    let getter_bits = __esc_rt_get_prop(descriptor, get_key);
    let setter_bits = __esc_rt_get_prop(descriptor, set_key);

    let writable = {
        let v = JsValue::from_raw_bits(writable_bits);
        if v.is_undefined() { None } else { v.as_bool() }
    };
    let enumerable = {
        let v = JsValue::from_raw_bits(enumerable_bits);
        if v.is_undefined() { None } else { v.as_bool() }
    };
    let configurable = {
        let v = JsValue::from_raw_bits(configurable_bits);
        if v.is_undefined() { None } else { v.as_bool() }
    };
    let value = {
        let v = JsValue::from_raw_bits(val_bits);
        if v.is_undefined() { None } else { Some(v) }
    };
    let getter = {
        let v = JsValue::from_raw_bits(getter_bits);
        if v.is_undefined() { None } else { Some(v) }
    };
    let setter = {
        let v = JsValue::from_raw_bits(setter_bits);
        if v.is_undefined() { None } else { Some(v) }
    };

    let prop_name = key_to_string(prop);
    let uni = unsafe {
        // SAFETY: tag check above confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::bool(false).raw_bits();
    };

    // 4. Return ? target.[[DefineOwnProperty]](key, desc).
    let result = SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let mut shapes = shapes.borrow_mut();
            let interner = interner.borrow();
            let opts = crate::property::DefinePropertyOptions {
                value,
                writable,
                enumerable,
                configurable,
                getter,
                setter,
            };
            u.define_own_property(&prop_name, &opts, &mut shapes, &interner)
        })
    });

    // Unlike Object.defineProperty which throws, Reflect returns false on failure
    JsValue::bool(result.is_ok()).raw_bits()
}

/// `Reflect.getOwnPropertyDescriptor ( target, propertyKey )`
///
/// Returns a property descriptor object, or `undefined` if the property does not
/// exist on the target.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.getownpropertydescriptor
fn reflect_get_own_property_descriptor(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    if args.is_empty() || !args[0].is_object() {
        return throw_type_error("Reflect.getOwnPropertyDescriptor called on non-object");
    }
    if args.len() < 2 {
        return JsValue::undefined().raw_bits();
    }
    // 2. Let key be ? ToPropertyKey(propertyKey).
    // (Implicit — key is already extracted by dispatch.)
    // 3. Let desc be ? target.[[GetOwnProperty]](key).
    // 4. Return FromPropertyDescriptor(desc).
    // Delegate to Object.getOwnPropertyDescriptor — same semantics for steps 3-4
    super::dispatch_object_static_method(
        "getOwnPropertyDescriptor",
        args.len() as u32,
        raw_ptr(args),
    )
    .unwrap_or(JsValue::undefined().raw_bits())
}

/// `Reflect.ownKeys ( target )`
///
/// Returns an array of the target's own property keys (strings and symbols).
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.ownkeys
fn reflect_own_keys(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let obj = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(obj);
    if !target_val.is_object() {
        return throw_type_error("Reflect.ownKeys called on non-object");
    }

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

    // 2. Let keys be ? target.[[OwnPropertyKeys]]().
    // 3. Return CreateArrayFromList(keys).
    SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            let keys = u.own_keys(&shapes, &interner);
            let values: Vec<JsValue> = keys
                .into_iter()
                .map(|k| JsValue::from_raw_bits(make_rt_string(k)))
                .collect();
            create_array_from_elements(values)
        })
    })
}

/// `Reflect.getPrototypeOf ( target )`
///
/// Returns the prototype of the target, or `null` if it has none.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.getprototypeof
fn reflect_get_prototype_of(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let obj = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(obj);
    if !target_val.is_object() {
        return throw_type_error("Reflect.getPrototypeOf called on non-object");
    }
    // 2. Return ? target.[[GetPrototypeOf]]().
    // Delegate to Object.getPrototypeOf — same semantics
    super::dispatch_object_static_method("getPrototypeOf", 1, &obj as *const u64)
        .unwrap_or(JsValue::null().raw_bits())
}

/// `Reflect.setPrototypeOf ( target, proto )`
///
/// Returns `true` if the prototype was set successfully, `false` otherwise.
/// Unlike `Object.setPrototypeOf` which throws on failure, Reflect returns `false`.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.setprototypeof
fn reflect_set_prototype_of(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    if args.is_empty() || !args[0].is_object() {
        return throw_type_error("Reflect.setPrototypeOf called on non-object");
    }
    if args.len() < 2 {
        return JsValue::bool(false).raw_bits();
    }
    let obj = args[0].raw_bits();
    let proto = args[1].raw_bits();

    // 2. If proto is not an Object and proto is not null, throw a TypeError exception.
    let proto_val = JsValue::from_raw_bits(proto);
    if !proto_val.is_object() && !proto_val.is_null() {
        return throw_type_error("Reflect.setPrototypeOf requires proto to be Object or null");
    }

    // Check if non-extensible — can't change prototype
    let tag = read_obj_tag(obj);
    if tag == Some(ObjTag::Unified as u8) {
        // SAFETY: tag check confirms this is a unified object.
        let uni = unsafe { deref_tagged::<UnifiedObject>(obj) };
        if let Some(u) = uni
            && !u.is_extensible()
        {
            // Non-extensible: can only succeed if proto is the current prototype
            let current_proto =
                super::dispatch_object_static_method("getPrototypeOf", 1, &obj as *const u64)
                    .unwrap_or(JsValue::null().raw_bits());
            if current_proto != proto {
                return JsValue::bool(false).raw_bits();
            }
            return JsValue::bool(true).raw_bits();
        }
    }

    // 3. Return ? target.[[SetPrototypeOf]](proto).
    // Delegate to Object.setPrototypeOf
    let raw_args = [obj, proto];
    super::dispatch_object_static_method("setPrototypeOf", 2, raw_args.as_ptr());
    JsValue::bool(true).raw_bits()
}

/// `Reflect.isExtensible ( target )`
///
/// Returns `true` if the target is extensible, `false` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.isextensible
fn reflect_is_extensible(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let obj = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let target_val = JsValue::from_raw_bits(obj);
    if !target_val.is_object() {
        return throw_type_error("Reflect.isExtensible called on non-object");
    }
    // 2. Return ? target.[[IsExtensible]]().
    // Delegate to Object.isExtensible — same semantics
    super::dispatch_object_static_method("isExtensible", 1, &obj as *const u64)
        .unwrap_or(JsValue::bool(false).raw_bits())
}

/// `Reflect.preventExtensions ( target )`
///
/// Returns `true` if the target was made non-extensible, `false` otherwise.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.preventextensions
fn reflect_prevent_extensions(args: &[JsValue]) -> u64 {
    // 1. If target is not an Object, throw a TypeError exception.
    let obj = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

    let target_val = JsValue::from_raw_bits(obj);
    if !target_val.is_object() {
        return throw_type_error("Reflect.preventExtensions called on non-object");
    }

    // 2. Return ? target.[[PreventExtensions]]().
    let tag = read_obj_tag(obj);
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(obj)
        };
        if let Some(u) = uni {
            u.prevent_extensions();
        }
    }
    JsValue::bool(true).raw_bits()
}

/// `Reflect.apply ( target, thisArgument, argumentsList )`
///
/// Calls the target function with the given `this` value and arguments array.
/// If target is not callable, returns `undefined`.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.apply
fn reflect_apply(args: &[JsValue]) -> u64 {
    // 1. If IsCallable(target) is false, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    if !is_callable_value(target) {
        return throw_type_error("Reflect.apply called on non-callable target");
    }
    // 2. Let args be ? CreateListFromArrayLike(argumentsList).
    let this_arg = args
        .get(1)
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    let args_list = args.get(2).map_or(JsValue::undefined(), |v| *v);

    // Extract arguments from the args array
    let call_args = extract_array_elements(args_list);
    let raw_args: Vec<u64> = call_args.iter().map(|v| v.raw_bits()).collect();

    // 3. Perform PrepareForTailCall().
    // TODO: Step 3 — tail call optimization not implemented

    // 4. Return ? Call(target, thisArgument, args).
    // Set this and call
    super::CURRENT_THIS.with(|cell| cell.set(this_arg));
    // SAFETY: raw_args is valid for the duration of the call.
    unsafe {
        super::__esc_rt_call_indirect(
            target,
            raw_args.len() as i32,
            if raw_args.is_empty() {
                std::ptr::null()
            } else {
                raw_args.as_ptr()
            },
        )
    }
}

/// `Reflect.construct ( target, argumentsList [ , newTarget ] )`
///
/// Creates a new instance of target with the given arguments. If `newTarget`
/// is provided, it is used as the `new.target` value.
///
/// [spec]: https://tc39.es/ecma262/#sec-reflect.construct
fn reflect_construct(args: &[JsValue]) -> u64 {
    // 1. If IsConstructor(target) is false, throw a TypeError exception.
    let target = args
        .first()
        .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
    if !is_constructor_value(target) {
        return throw_type_error("Reflect.construct called on non-constructor target");
    }
    // 2. If newTarget is not present, set newTarget to target.
    let args_list = args.get(1).map_or(JsValue::undefined(), |v| *v);
    let new_target = args.get(2).map_or(target, |v| v.raw_bits());

    // 3. Else if IsConstructor(newTarget) is false, throw a TypeError exception.
    if args.len() >= 3 && !is_constructor_value(new_target) {
        return throw_type_error("Reflect.construct: newTarget is not a constructor");
    }

    // 4. Let args be ? CreateListFromArrayLike(argumentsList).
    // Extract arguments from the args array
    let call_args = extract_array_elements(args_list);
    let raw_args: Vec<u64> = call_args.iter().map(|v| v.raw_bits()).collect();

    // 5. Return ? Construct(target, args, newTarget).
    // Set new.target
    super::CURRENT_NEW_TARGET.with(|cell| cell.set(new_target));

    // SAFETY: raw_args is valid for the duration of the call.
    unsafe {
        super::__esc_rt_call_new(
            target,
            raw_args.len() as u32,
            if raw_args.is_empty() {
                std::ptr::null()
            } else {
                raw_args.as_ptr()
            },
        )
    }
}

/// Extract array elements from a NaN-boxed value that is expected to be an array.
///
/// This is an internal helper with no spec equivalent — it implements the
/// `CreateListFromArrayLike` abstract operation partially for Reflect.apply
/// and Reflect.construct.
///
/// Returns an empty `Vec` if the value is not an array.
fn extract_array_elements(val: JsValue) -> Vec<JsValue> {
    if val.is_undefined() || val.is_null() {
        return Vec::new();
    }
    let bits = val.raw_bits();
    let tag = read_obj_tag(bits);
    if tag != Some(ObjTag::Unified as u8) {
        return Vec::new();
    }
    // SAFETY: tag check confirms this is a unified object.
    let uni = unsafe { deref_tagged::<UnifiedObject>(bits) };
    let Some(u) = uni else {
        return Vec::new();
    };
    if u.kind != InternalKind::Array {
        return Vec::new();
    }
    u.array_elements().to_vec()
}

/// Convert a slice of `JsValue` to a raw `*const u64` for dispatch functions
/// that expect `(argc, argv)`.
fn raw_ptr(args: &[JsValue]) -> *const u64 {
    // JsValue is repr(transparent) over u64, so this cast is safe.
    args.as_ptr().cast::<u64>()
}
