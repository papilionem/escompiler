//! Object static method dispatch.
//!
//! Contains `dispatch_object_static_method` for methods like `Object.create`,
//! `Object.keys`, `Object.freeze`, `Object.defineProperty`, etc.

use nanbox::JsValue;

use crate::internal_data::{InternalKind, UnifiedObject};
use crate::tagged_obj::{ObjTag, deref_tagged, deref_tagged_mut, read_obj_tag};

use super::{
    __esc_rt_create_object, __esc_rt_get_prop, __esc_rt_object_create, __esc_rt_object_keys,
    __esc_rt_set_prop, INTERNER, PROTO_OBJECTS, SHAPES, create_array_from_elements,
    create_empty_array, key_to_string, make_rt_string, read_argv,
};

/// Get a property descriptor for a function/closure object.
///
/// Handles the well-known function properties (`name`, `length`, `prototype`)
/// and user-set properties stored in `OBJECT_PROPS`. Returns `None` if the
/// property does not exist on the function.
fn get_function_property_descriptor(
    obj_bits: u64,
    u: &UnifiedObject,
    prop_name: &str,
) -> Option<crate::property::OwnPropertyDescriptor> {
    use crate::internal_data::InternalData;

    // Check OBJECT_PROPS first (user-set or desugar-set properties like name/length)
    let user_val = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&obj_bits).and_then(|m| m.get(prop_name).copied())
    });

    match prop_name {
        "name" => {
            let val_bits = if let Some(v) = user_val {
                v
            } else if let Some(InternalData::Function { name, .. }) = u.internal_data() {
                let n = JsValue::from_raw_bits(*name);
                if n.is_undefined() || *name == 0 {
                    make_rt_string(String::new())
                } else {
                    *name
                }
            } else if u.kind == crate::internal_data::InternalKind::NativeFunc {
                // NativeFunc without a name in OBJECT_PROPS: default to empty string
                make_rt_string(String::new())
            } else {
                return None;
            };
            // Function.name: writable=false, enumerable=false, configurable=true
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: false,
                enumerable: false,
                configurable: true,
            })
        }
        "length" => {
            let val_bits = if let Some(v) = user_val {
                v
            } else if let Some(InternalData::Function { param_count, .. }) = u.internal_data() {
                JsValue::number(*param_count as f64).raw_bits()
            } else if u.kind == crate::internal_data::InternalKind::NativeFunc {
                // NativeFunc without a length in OBJECT_PROPS: default to 0
                JsValue::number(0.0).raw_bits()
            } else {
                return None;
            };
            // Function.length: writable=false, enumerable=false, configurable=true
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: false,
                enumerable: false,
                configurable: true,
            })
        }
        "prototype" => {
            let val_bits = user_val?;
            // Function.prototype: writable=true, enumerable=false, configurable=false
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: true,
                enumerable: false,
                configurable: false,
            })
        }
        _ => {
            // Other user-set properties: writable=true, enumerable=true, configurable=true
            user_val.map(|v| crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(v),
                writable: true,
                enumerable: true,
                configurable: true,
            })
        }
    }
}

/// Get a property descriptor for a NativeFunc object (built-in constructor or
/// method wrapper).
///
/// NativeFunc objects store `.name`, `.length`, and `.prototype` in the
/// `OBJECT_PROPS` side-table. Per the ES spec:
/// - `.name`: `{ writable: false, enumerable: false, configurable: true }`
/// - `.length`: `{ writable: false, enumerable: false, configurable: true }`
/// - `.prototype` (constructors only): `{ writable: false, enumerable: false, configurable: false }`
///
/// Returns `None` if the property is not a known NativeFunc property.
fn get_native_func_property_descriptor(
    obj_bits: u64,
    _u: &UnifiedObject,
    prop_name: &str,
) -> Option<crate::property::OwnPropertyDescriptor> {
    let user_val = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&obj_bits).and_then(|m| m.get(prop_name).copied())
    });

    match prop_name {
        "name" => {
            let val_bits = user_val.unwrap_or_else(|| make_rt_string(String::new()));
            // Per ES spec 20.2.3.2: Function.name is
            // { writable: false, enumerable: false, configurable: true }
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: false,
                enumerable: false,
                configurable: true,
            })
        }
        "length" => {
            let val_bits = user_val.unwrap_or_else(|| JsValue::number(0.0).raw_bits());
            // Per ES spec 20.2.3.1: Function.length is
            // { writable: false, enumerable: false, configurable: true }
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: false,
                enumerable: false,
                configurable: true,
            })
        }
        "prototype" => {
            let val_bits = user_val?;
            // Per ES spec: Constructor.prototype is
            // { writable: false, enumerable: false, configurable: false }
            Some(crate::property::OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val_bits),
                writable: false,
                enumerable: false,
                configurable: false,
            })
        }
        _ => {
            // First check if the property is in OBJECT_PROPS (explicitly stored).
            if let Some(v) = user_val {
                // Other OBJECT_PROPS entries (static methods on constructors like Object.keys,
                // Array.isArray, etc.).
                // Per ES spec §17.3 (Properties of Built-in Function Objects):
                // Built-in methods are { writable: true, enumerable: false, configurable: true }.
                return Some(crate::property::OwnPropertyDescriptor::Data {
                    value: JsValue::from_raw_bits(v),
                    writable: true,
                    enumerable: false,
                    configurable: true,
                });
            }

            // Not in OBJECT_PROPS. Check if this NativeFunc is a registered builtin
            // constructor (Object, Array, etc.) and prop_name is one of its static methods.
            // These static methods are dispatched virtually (not stored in OBJECT_PROPS),
            // but Object.getOwnPropertyDescriptor should still be able to report them.
            // Per ES spec: built-in static methods have { writable:true, enumerable:false, configurable:true }.
            let ctor_name = super::OBJECT_PROPS.with(|props| {
                let props = props.borrow();
                props.get(&obj_bits).and_then(|m| {
                    m.get("name").map(|&bits| {
                        crate::string_ops::get_string_data(JsValue::from_raw_bits(bits))
                    })
                })
            });

            if let Some(ref ctor) = ctor_name {
                // Check the builtin_builder registry for registered static methods.
                if let Some(reg) = crate::builtin_builder::get_registration(ctor)
                    && reg.static_method_names().contains(&prop_name)
                {
                    // Create a NativeFunc wrapper for this static method.
                    let method_val = super::property::get_or_create_builtin_method(ctor, prop_name);
                    return Some(crate::property::OwnPropertyDescriptor::Data {
                        value: JsValue::from_raw_bits(method_val),
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    });
                }

                // For constructors not in the registry, check builtin_static_methods.
                // This covers String, Number, Math methods, etc.
                if let Some(methods) = super::property::builtin_static_methods(ctor)
                    && methods.contains(&prop_name)
                {
                    let method_val = super::property::get_or_create_builtin_method(ctor, prop_name);
                    return Some(crate::property::OwnPropertyDescriptor::Data {
                        value: JsValue::from_raw_bits(method_val),
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    });
                }
            }

            None
        }
    }
}

/// Collect all own property names for a function/closure/native-func object.
///
/// Includes `name`, `length`, and (if present) `prototype`, as well as any
/// other user-set properties stored in `OBJECT_PROPS`. The result preserves
/// insertion order for OBJECT_PROPS keys, with `length`, `name`, and
/// `prototype` prepended in spec-consistent order.
fn collect_function_own_keys(
    obj_bits: u64,
    u: &UnifiedObject,
    shapes: &shapes::ShapeTable,
    interner: &interner::Interner,
) -> Vec<String> {
    use std::collections::HashSet;

    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    // 1. Always include "length" and "name" for function objects
    keys.push("length".to_string());
    seen.insert("length".to_string());
    keys.push("name".to_string());
    seen.insert("name".to_string());

    // 2. Include "prototype" if present in OBJECT_PROPS
    let has_prototype = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props
            .get(&obj_bits)
            .is_some_and(|m| m.contains_key("prototype"))
    });
    if has_prototype {
        keys.push("prototype".to_string());
        seen.insert("prototype".to_string());
    }

    // 3. Include any other user-set properties from OBJECT_PROPS
    super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        if let Some(m) = props.get(&obj_bits) {
            for k in m.keys() {
                if seen.insert(k.clone()) {
                    keys.push(k.clone());
                }
            }
        }
    });

    // 4. Include shape-based properties
    let shape_keys = u.own_keys(shapes, interner);
    for k in shape_keys {
        if seen.insert(k.clone()) {
            keys.push(k);
        }
    }

    keys
}

/// Get a property descriptor for a virtual builtin property (namespace objects
/// and builtin constructor properties like Math.PI, Number.MAX_VALUE, etc.).
///
/// These properties don't live in the shape system — they are resolved via
/// `dispatch_builtin_property` / `builtin_constant` in the property access path.
/// This function mirrors that resolution for `Object.getOwnPropertyDescriptor`.
///
/// Returns `Some(descriptor)` if the property is a known builtin virtual property,
/// `None` otherwise.
fn get_builtin_virtual_property_descriptor(
    obj_bits: u64,
    prop_name: &str,
) -> Option<crate::property::OwnPropertyDescriptor> {
    use crate::property::OwnPropertyDescriptor;

    // Check if this object is a namespace object (has __namespace__ marker)
    let ns_name = super::OBJECT_PROPS.with(|props| {
        let props = props.borrow();
        props.get(&obj_bits).and_then(|m| {
            m.get("__namespace__")
                .map(|&bits| crate::string_ops::get_string_data(JsValue::from_raw_bits(bits)))
        })
    });

    if let Some(ref ns) = ns_name {
        // Namespace constant properties (Math.PI, Math.E, etc.)
        // Per spec: { writable: false, enumerable: false, configurable: false }
        if let Some(val) = super::property::builtin_constant(ns, prop_name) {
            return Some(OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(val),
                writable: false,
                enumerable: false,
                configurable: false,
            });
        }

        // Namespace method properties (Math.floor, JSON.parse, etc.)
        // Per spec: { writable: true, enumerable: false, configurable: true }
        if let Some(methods) = super::property::builtin_static_methods(ns)
            && methods.contains(&prop_name)
        {
            let method_val = super::property::get_or_create_builtin_method(ns, prop_name);
            return Some(OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(method_val),
                writable: true,
                enumerable: false,
                configurable: true,
            });
        }

        // @@toStringTag for namespace objects
        // Per spec: { writable: false, enumerable: false, configurable: true }
        if prop_name == "@@toStringTag" || prop_name == "Symbol(Symbol.toStringTag)" {
            return Some(OwnPropertyDescriptor::Data {
                value: JsValue::from_raw_bits(super::make_rt_string(ns.clone())),
                writable: false,
                enumerable: false,
                configurable: true,
            });
        }
    }

    None
}

/// Perform `ToObject(argument)` inline for Object.* static methods.
///
/// If the argument is `null` or `undefined`, throws a TypeError and returns
/// `None` (caller should return early with a default value). Otherwise returns
/// `Some(bits)` — for objects the raw bits unchanged, for primitives whatever
/// `__esc_rt_to_object` returns (currently identity; once Wave 0 wrappers land,
/// this will wrap primitives).
///
/// [spec]: https://tc39.es/ecma262/#sec-toobject
fn to_object_or_throw(val: u64, method_name: &str) -> Option<u64> {
    let v = JsValue::from_raw_bits(val);
    if v.is_null() || v.is_undefined() {
        let desc = if v.is_null() { "null" } else { "undefined" };
        let msg = super::make_rt_string(format!(
            "Cannot convert {desc} to object (called from Object.{method_name})"
        ));
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return None;
    }
    // For objects, return as-is. For primitives, call ToObject which will
    // wrap them once the wrapper InternalKinds are fully implemented.
    Some(super::conversion::__esc_rt_to_object(val))
}

/// Dispatch an Object static method (`Object.create`, `Object.keys`, etc.).
///
/// Implements the static methods of the `Object` constructor as defined in
/// ES2024 [section 20.1.2](https://tc39.es/ecma262/#sec-properties-of-the-object-constructor).
///
/// Returns `Some(bits)` if the method is a known Object static method, `None` otherwise.
pub(crate) fn dispatch_object_static_method(
    method: &str,
    argc: u32,
    argv: *const u64,
) -> Option<u64> {
    let args = read_argv(argc, argv);
    match method {
        // ---------------------------------------------------------------
        // Object.create ( O, Properties )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.create
        //
        // 1. If O is not an Object and O is not null, throw a TypeError exception.
        // 2. Let obj be OrdinaryObjectCreate(O).
        // 3. If Properties is not undefined, then
        //    a. Return ? ObjectDefineProperties(obj, Properties).
        // 4. Return obj.
        // ---------------------------------------------------------------
        "create" => {
            let proto = args
                .first()
                .map_or(JsValue::null().raw_bits(), |v| v.raw_bits());

            // Step 1: If O is not an Object and O is not null, throw a TypeError exception.
            let proto_val = JsValue::from_raw_bits(proto);
            if !proto_val.is_object() && !proto_val.is_null() {
                let msg = super::make_rt_string(
                    "Object prototype may only be an Object or null".to_string(),
                );
                let err =
                    super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
                return Some(JsValue::undefined().raw_bits());
            }

            // Step 2: Let obj be OrdinaryObjectCreate(O).
            let obj_bits = __esc_rt_object_create(proto);

            // Step 3: If Properties is not undefined, then
            //   a. Return ? ObjectDefineProperties(obj, Properties).
            if let Some(properties_arg) = args.get(1) {
                let properties = properties_arg.raw_bits();
                let prop_val = JsValue::from_raw_bits(properties);
                if !prop_val.is_undefined() {
                    // Delegate to ObjectDefineProperties via our "defineProperties" handler
                    let dp_args = [JsValue::from_raw_bits(obj_bits), *properties_arg];
                    dispatch_object_static_method(
                        "defineProperties",
                        2,
                        dp_args
                            .iter()
                            .map(|v| v.raw_bits())
                            .collect::<Vec<_>>()
                            .as_ptr(),
                    );
                }
            }

            // Step 4: Return obj.
            Some(obj_bits)
        }

        // ---------------------------------------------------------------
        // Object.keys ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.keys
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let nameList be ? EnumerableOwnProperties(obj, key).
        // 3. Return CreateArrayFromList(nameList).
        // ---------------------------------------------------------------
        "keys" => {
            let raw_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // Step 1: Let obj be ? ToObject(O).
            let Some(obj) = to_object_or_throw(raw_arg, "keys") else {
                return Some(create_empty_array());
            };
            // Steps 2-3: get enumerable own property keys and return as array.
            Some(__esc_rt_object_keys(obj))
        }

        // ---------------------------------------------------------------
        // Object.defineProperty ( O, P, Attributes )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.defineproperty
        //
        // 1. If O is not an Object, throw a TypeError exception.
        // 2. Let key be ? ToPropertyKey(P).
        // 3. Let desc be ? ToPropertyDescriptor(Attributes).
        // 4. Perform ? O.[[DefineOwnProperty]](key, desc).
        // 5. Return O.
        // ---------------------------------------------------------------
        "defineProperty" => {
            if args.len() >= 3 {
                let obj = args[0].raw_bits();
                let prop = args[1].raw_bits();
                let descriptor = args[2].raw_bits();

                // Step 1: If O is not an Object, throw a TypeError exception.
                let obj_val = JsValue::from_raw_bits(obj);
                if !obj_val.is_object() {
                    let msg = super::make_rt_string(
                        "Object.defineProperty called on non-object".to_string(),
                    );
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(obj);
                }

                // Step 3: Let desc be ? ToPropertyDescriptor(Attributes).
                // [spec]: https://tc39.es/ecma262/#sec-topropertydescriptor
                // 1. If Obj is not an Object, throw a TypeError exception.
                let desc_val = JsValue::from_raw_bits(descriptor);
                if !desc_val.is_object() {
                    let msg =
                        super::make_rt_string("Property description must be an object".to_string());
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(obj);
                }

                // Proxy intercept: delegate to proxy_define_property trap
                if let Some(tag) = read_obj_tag(obj)
                    && tag == ObjTag::Unified as u8
                    && let Some(u) = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(obj)
                    }
                    && u.kind == InternalKind::Proxy
                {
                    let key_name = key_to_string(prop);
                    match crate::proxy::proxy_define_property(obj, prop, descriptor, &key_name) {
                        Ok(success) => {
                            if !success {
                                let msg = super::make_rt_string(format!(
                                    "Cannot define property '{key_name}' on proxy: trap returned false"
                                ));
                                let err = super::__esc_rt_create_error(
                                    crate::exceptions::error_tag::TYPE_ERROR,
                                    msg,
                                );
                                super::__esc_rt_throw(err);
                            }
                            return Some(obj);
                        }
                        Err(e) => {
                            let msg = super::make_rt_string(e.to_string());
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                            return Some(obj);
                        }
                    }
                }

                // Step 2: ToPropertyKey(P) — we convert to string key.
                // Step 3: ToPropertyDescriptor(Attributes) — extract descriptor fields.
                // Per ES2024 §10.1.14 ToPropertyDescriptor:
                // Presence of a key is tested with [[HasProperty]] (checks prototype chain),
                // so {get: undefined} is an accessor descriptor, not an empty descriptor.
                // We use __esc_rt_has_prop (which traverses the prototype chain) to correctly
                // detect key presence per spec (inherited descriptor keys are valid).

                let writable_key = make_rt_string("writable".to_string());
                let enumerable_key = make_rt_string("enumerable".to_string());
                let configurable_key = make_rt_string("configurable".to_string());
                let value_key = make_rt_string("value".to_string());
                let get_key = make_rt_string("get".to_string());
                let set_key = make_rt_string("set".to_string());

                // §10.1.14 ToPropertyDescriptor steps 3-8: use [[HasProperty]] (prototype chain)
                // to detect key presence. This correctly handles inherited descriptor fields
                // (e.g., when passing an object with inherited "value" or "get" properties).
                let has_writable = JsValue::from_raw_bits(super::property::__esc_rt_has_prop(
                    descriptor,
                    writable_key,
                ))
                .as_bool()
                .unwrap_or(false);
                let has_enumerable = JsValue::from_raw_bits(super::property::__esc_rt_has_prop(
                    descriptor,
                    enumerable_key,
                ))
                .as_bool()
                .unwrap_or(false);
                let has_configurable = JsValue::from_raw_bits(super::property::__esc_rt_has_prop(
                    descriptor,
                    configurable_key,
                ))
                .as_bool()
                .unwrap_or(false);
                let has_value = JsValue::from_raw_bits(super::property::__esc_rt_has_prop(
                    descriptor, value_key,
                ))
                .as_bool()
                .unwrap_or(false);
                let has_get =
                    JsValue::from_raw_bits(super::property::__esc_rt_has_prop(descriptor, get_key))
                        .as_bool()
                        .unwrap_or(false);
                let has_set =
                    JsValue::from_raw_bits(super::property::__esc_rt_has_prop(descriptor, set_key))
                        .as_bool()
                        .unwrap_or(false);

                let writable_bits = __esc_rt_get_prop(descriptor, writable_key);
                let enumerable_bits = __esc_rt_get_prop(descriptor, enumerable_key);
                let configurable_bits = __esc_rt_get_prop(descriptor, configurable_key);
                let val_bits = __esc_rt_get_prop(descriptor, value_key);
                let getter_bits = __esc_rt_get_prop(descriptor, get_key);
                let setter_bits = __esc_rt_get_prop(descriptor, set_key);

                // ToPropertyDescriptor steps 3-8: check for presence and coerce.
                // Use has_xxx flags to distinguish "key absent" from "key=undefined".
                // Coerce enumerable/configurable/writable to boolean via ToBoolean.
                let writable = if has_writable {
                    let v = JsValue::from_raw_bits(writable_bits);
                    Some(crate::value_ops::to_boolean(v))
                } else {
                    None
                };
                let enumerable = if has_enumerable {
                    let v = JsValue::from_raw_bits(enumerable_bits);
                    Some(crate::value_ops::to_boolean(v))
                } else {
                    None
                };
                let configurable = if has_configurable {
                    let v = JsValue::from_raw_bits(configurable_bits);
                    Some(crate::value_ops::to_boolean(v))
                } else {
                    None
                };
                // Step 5: If Obj has a "value" property, set desc.[[Value]] to that value.
                let value = if has_value {
                    Some(JsValue::from_raw_bits(val_bits))
                } else {
                    None
                };
                // ToPropertyDescriptor step 7: If Obj has a "get" property, then
                //   a. Let getter be ? Get(Obj, "get").
                //   b. If IsCallable(getter) is false and getter is not undefined,
                //      throw a TypeError.
                let getter = if has_get {
                    let v = JsValue::from_raw_bits(getter_bits);
                    if v.is_undefined() {
                        // {get: undefined} is a valid accessor with no getter
                        Some(JsValue::undefined())
                    } else if v.is_object() {
                        // Check if callable
                        let is_callable = if let Some(gtag) = read_obj_tag(v.raw_bits())
                            && gtag == ObjTag::Unified as u8
                            && let Some(gu) = unsafe {
                                // SAFETY: tag check confirms this is a unified object.
                                deref_tagged::<UnifiedObject>(v.raw_bits())
                            } {
                            gu.is_callable()
                        } else {
                            false
                        };
                        if !is_callable {
                            let msg =
                                super::make_rt_string("Getter must be a function".to_string());
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                            return Some(obj);
                        }
                        Some(v)
                    } else {
                        // Not undefined and not an object → invalid
                        let msg = super::make_rt_string("Getter must be a function".to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                        return Some(obj);
                    }
                } else {
                    None
                };
                // ToPropertyDescriptor step 8: If Obj has a "set" property, then
                //   a. Let setter be ? Get(Obj, "set").
                //   b. If IsCallable(setter) is false and setter is not undefined,
                //      throw a TypeError.
                let setter = if has_set {
                    let v = JsValue::from_raw_bits(setter_bits);
                    if v.is_undefined() {
                        // {set: undefined} is a valid accessor with no setter
                        Some(JsValue::undefined())
                    } else if v.is_object() {
                        // Check if callable
                        let is_callable = if let Some(stag) = read_obj_tag(v.raw_bits())
                            && stag == ObjTag::Unified as u8
                            && let Some(su) = unsafe {
                                // SAFETY: tag check confirms this is a unified object.
                                deref_tagged::<UnifiedObject>(v.raw_bits())
                            } {
                            su.is_callable()
                        } else {
                            false
                        };
                        if !is_callable {
                            let msg =
                                super::make_rt_string("Setter must be a function".to_string());
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                            return Some(obj);
                        }
                        Some(v)
                    } else {
                        // Not undefined and not an object → invalid
                        let msg = super::make_rt_string("Setter must be a function".to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                        return Some(obj);
                    }
                } else {
                    None
                };

                // ToPropertyDescriptor step 9: If desc has a [[Get]] or [[Set]] field,
                // then if desc also has a [[Value]] or [[Writable]] field, throw TypeError.
                // Note: getter/setter being Some(undefined) still counts as accessor.
                let has_accessor = has_get || has_set;
                let has_data = has_value || has_writable;
                if has_accessor && has_data {
                    let msg = super::make_rt_string(
                        "Invalid property descriptor. Cannot both specify accessors and a value or writable attribute".to_string(),
                    );
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(obj);
                }

                // Step 4: Perform ? O.[[DefineOwnProperty]](key, desc).
                let obj_tag = read_obj_tag(obj);
                if obj_tag == Some(ObjTag::Unified as u8) {
                    let prop_name = key_to_string(prop);

                    // Array exotic: defineProperty("length", {value: <obj>}) must call
                    // ToUint32(value), which internally calls ToPrimitive → valueOf/toString
                    // on the value object.  That invocation re-enters property lookup (SHAPES
                    // borrow) while we are about to take borrow_mut — causing a RefCell panic.
                    // Pre-convert the length value to a primitive *before* taking borrow_mut.
                    let is_array = {
                        let u_tmp = unsafe {
                            // SAFETY: tag check above confirms this is a unified object.
                            deref_tagged::<UnifiedObject>(obj)
                        };
                        u_tmp.is_some_and(|u| u.kind == crate::internal_data::InternalKind::Array)
                    };
                    let value = if is_array
                        && prop_name == "length"
                        && let Some(v) = value
                        && v.is_object()
                    {
                        // Pre-convert to number primitive so SHAPES is not re-entered.
                        let prim = crate::value_ops::to_number(v);
                        Some(JsValue::number(prim))
                    } else {
                        value
                    };

                    let uni = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged_mut::<UnifiedObject>(obj)
                    };
                    if let Some(u) = uni {
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
                        if let Err(e) = result {
                            let msg = format!("{e}");
                            let error_tag = if matches!(
                                e,
                                crate::property::PropertyError::InvalidArrayLength
                            ) {
                                crate::exceptions::error_tag::RANGE_ERROR
                            } else {
                                crate::exceptions::error_tag::TYPE_ERROR
                            };
                            let err = super::__esc_rt_create_error(error_tag, make_rt_string(msg));
                            super::__esc_rt_throw(err);
                        }
                    }
                } else if let Some(val) = value {
                    // Fallback for non-plain objects: just set the value
                    __esc_rt_set_prop(obj, prop, val.raw_bits());
                }
                // Step 5: Return O.
                Some(obj)
            } else {
                None
            }
        }

        // ---------------------------------------------------------------
        // Object.defineProperties ( O, Properties )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.defineproperties
        //
        // 1. If O is not an Object, throw a TypeError exception.
        // 2. Return ? ObjectDefineProperties(O, Properties).
        //
        // ObjectDefineProperties ( O, Properties ) — abstract operation
        // [spec]: https://tc39.es/ecma262/#sec-objectdefineproperties
        //
        // 1. Let props be ? ToObject(Properties).
        // 2. Let keys be ? props.[[OwnPropertyKeys]]().
        // 3. Let descriptors be a new empty List.
        // 4. For each element nextKey of keys, do
        //    a. Let propDesc be ? props.[[GetOwnProperty]](nextKey).
        //    b. If propDesc is not undefined and propDesc.[[Enumerable]] is true, then
        //       i. Let descObj be ? Get(props, nextKey).
        //       ii. Let desc be ? ToPropertyDescriptor(descObj).
        //       iii. Append the Record { [[Key]]: nextKey, [[Descriptor]]: desc } to descriptors.
        // 5. For each element pair of descriptors, do
        //    a. Perform ? DefinePropertyOrThrow(O, pair.[[Key]], pair.[[Descriptor]]).
        // 6. Return O.
        // ---------------------------------------------------------------
        "defineProperties" => {
            if args.len() >= 2 {
                let obj_bits = args[0].raw_bits();
                let props_bits = args[1].raw_bits();

                // Step 1: If O is not an Object, throw a TypeError exception.
                let obj_val = JsValue::from_raw_bits(obj_bits);
                if !obj_val.is_object() {
                    let msg = super::make_rt_string(
                        "Object.defineProperties called on non-object".to_string(),
                    );
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(obj_bits);
                }

                // Step 2 (ObjectDefineProperties):
                // Step 2.1: Let props be ? ToObject(Properties).
                // TODO: Step 2.1 — ToObject is not called; non-object Properties not handled.
                // Step 2.2: Let keys be ? props.[[OwnPropertyKeys]]().
                let keys = __esc_rt_object_keys(props_bits);
                // SAFETY: __esc_rt_object_keys returns a UnifiedObject array via TaggedObj::boxed.
                if let Some(key_arr) = unsafe { deref_tagged::<UnifiedObject>(keys) }
                    && key_arr.kind == InternalKind::Array
                {
                    // Steps 2.3-2.4: Collect ALL descriptors first, THEN define them.
                    // The spec requires two-pass: first read all descriptor objects from
                    // the properties source, then define them on the target. This matters
                    // when defineProperty has side effects (e.g., accessors).
                    let mut collected: Vec<(JsValue, u64)> =
                        Vec::with_capacity(key_arr.array_elements().len());
                    for key_val in key_arr.array_elements() {
                        let descriptor = __esc_rt_get_prop(props_bits, key_val.raw_bits());
                        collected.push((*key_val, descriptor));
                    }

                    // Step 2.5: For each element pair of descriptors, do
                    //   a. Perform ? DefinePropertyOrThrow(O, pair.[[Key]], pair.[[Descriptor]]).
                    for (key_val, descriptor) in collected {
                        let dp_args = [
                            JsValue::from_raw_bits(obj_bits),
                            key_val,
                            JsValue::from_raw_bits(descriptor),
                        ];
                        dispatch_object_static_method(
                            "defineProperty",
                            3,
                            dp_args
                                .iter()
                                .map(|v| v.raw_bits())
                                .collect::<Vec<_>>()
                                .as_ptr(),
                        );
                    }
                }
                // Step 2.6: Return O.
                Some(obj_bits)
            } else {
                None
            }
        }

        // ---------------------------------------------------------------
        // Object.fromEntries ( iterable )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.fromentries
        //
        // 1. Perform ? RequireObjectCoercible(iterable).
        // 2. Let obj be OrdinaryObjectCreate(%Object.prototype%).
        // 3. Assert: obj is an extensible ordinary object with no own properties.
        // 4. Let closure be a new Abstract Closure with parameters (key, value)
        //    that captures obj and performs the following steps when called:
        //    a. Let propertyKey be ? ToPropertyKey(key).
        //    b. Perform ! CreateDataPropertyOrThrow(obj, propertyKey, value).
        //    c. Return undefined.
        // 5. Let adder be CreateBuiltinFunction(closure, 2, "", « »).
        // 6. Return ? AddEntriesFromIterable(obj, iterable, adder).
        // ---------------------------------------------------------------
        "fromEntries" => {
            let iter_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

            // Step 1: Perform ? RequireObjectCoercible(iterable).
            // Throws TypeError if iterable is null or undefined.
            let iter_val = JsValue::from_raw_bits(iter_arg);
            if iter_val.is_null() || iter_val.is_undefined() {
                let msg = super::make_rt_string(format!(
                    "Cannot read properties of {}",
                    if iter_val.is_null() {
                        "null"
                    } else {
                        "undefined"
                    }
                ));
                let err =
                    super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
                return Some(JsValue::undefined().raw_bits());
            }

            // Step 2: Let obj be OrdinaryObjectCreate(%Object.prototype%).
            let result = __esc_rt_create_object();

            // Steps 4-6: AddEntriesFromIterable(obj, iterable, adder).
            // Use the iterator protocol to iterate over entries.
            let iter = super::__esc_rt_iter_init(iter_arg);
            loop {
                let next_result = super::__esc_rt_iter_next(iter);
                let done = super::__esc_rt_iter_done(next_result);
                if crate::value_ops::to_boolean(JsValue::from_raw_bits(done)) {
                    break;
                }
                let entry = super::__esc_rt_iter_value(next_result);
                // Each entry should be a [key, value] pair.
                // Step 4a: Let propertyKey be ? ToPropertyKey(key).
                let key_bits = make_rt_string("0".to_string());
                let val_key = make_rt_string("1".to_string());
                let k = __esc_rt_get_prop(entry, key_bits);
                let v = __esc_rt_get_prop(entry, val_key);
                // Step 4b: Perform ! CreateDataPropertyOrThrow(obj, propertyKey, value).
                __esc_rt_set_prop(result, k, v);
            }
            Some(result)
        }

        // ---------------------------------------------------------------
        // Object.assign ( target, ...sources )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.assign
        //
        // 1. Let to be ? ToObject(target).
        // 2. If only one argument was passed, return to.
        // 3. For each element nextSource of sources, do
        //    a. If nextSource is neither undefined nor null, then
        //       i. Let from be ! ToObject(nextSource).
        //       ii. Let keys be ? from.[[OwnPropertyKeys]]().
        //       iii. For each element nextKey of keys, do
        //            1. Let desc be ? from.[[GetOwnProperty]](nextKey).
        //            2. If desc is not undefined and desc.[[Enumerable]] is true, then
        //               a. Let propValue be ? Get(from, nextKey).
        //               b. Perform ? Set(to, nextKey, propValue, true).
        // 4. Return to.
        // ---------------------------------------------------------------
        "assign" => {
            if args.is_empty() {
                return None;
            }
            let target_val = args[0];
            // Step 1: Let to be ? ToObject(target).
            // TypeError if target is null or undefined.
            if target_val.is_null() || target_val.is_undefined() {
                let desc = if target_val.is_null() {
                    "null"
                } else {
                    "undefined"
                };
                let msg = make_rt_string(format!("Cannot convert {desc} to object"));
                let err =
                    super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                super::__esc_rt_throw(err);
                return Some(JsValue::undefined().raw_bits());
            }
            let target = target_val.raw_bits();
            // Step 3: For each element nextSource of sources, do
            for source in &args[1..] {
                // Step 3a: If nextSource is neither undefined nor null, then
                if source.is_null() || source.is_undefined() {
                    continue;
                }
                let source_bits = source.raw_bits();
                // Step 3a.i: Let from be ! ToObject(nextSource).
                // Step 3a.ii: Let keys be ? from.[[OwnPropertyKeys]]().
                let keys = __esc_rt_object_keys(source_bits);
                // SAFETY: __esc_rt_object_keys returns a UnifiedObject array via TaggedObj::boxed.
                if let Some(arr) = unsafe { deref_tagged::<UnifiedObject>(keys) }
                    && arr.kind == InternalKind::Array
                {
                    // Step 3a.iii: For each element nextKey of keys, do
                    for elem in arr.array_elements() {
                        // TODO: Step 3a.iii.1 — should check [[GetOwnProperty]] for
                        // enumerable before copying (currently copies all keys from
                        // __esc_rt_object_keys which already filters for enumerable).
                        // Step 3a.iii.2a: Let propValue be ? Get(from, nextKey).
                        let val = __esc_rt_get_prop(source_bits, elem.raw_bits());
                        // Step 3a.iii.2b: Perform ? Set(to, nextKey, propValue, true).
                        __esc_rt_set_prop(target, elem.raw_bits(), val);
                    }
                }
            }
            // Step 4: Return to.
            Some(target)
        }

        // ---------------------------------------------------------------
        // Object.freeze ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.freeze
        //
        // 1. If O is not an Object, return O.
        // 2. Let status be ? SetIntegrityLevel(O, frozen).
        // 3. If status is false, throw a TypeError exception.
        // 4. Return O.
        //
        // SetIntegrityLevel ( O, level ) — abstract operation (frozen)
        // [spec]: https://tc39.es/ecma262/#sec-setintegritylevel
        //
        // 1. Let status be ? O.[[PreventExtensions]]().
        // 2. If status is false, return false.
        // 3. Let keys be ? O.[[OwnPropertyKeys]]().
        // 4. If level is sealed, then ...
        // 5. Else (level is frozen), then
        //    a. For each element k of keys, do
        //       i. Let currentDesc be ? O.[[GetOwnProperty]](k).
        //       ii. If currentDesc is not undefined, then
        //           1. If IsAccessorDescriptor(currentDesc), then
        //              a. Let desc be PropertyDescriptor { [[Configurable]]: false }.
        //           2. Else,
        //              a. Let desc be PropertyDescriptor { [[Configurable]]: false,
        //                 [[Writable]]: false }.
        //           3. Perform ? DefinePropertyOrThrow(O, k, desc).
        // 6. Return true.
        // ---------------------------------------------------------------
        "freeze" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let ftag = read_obj_tag(obj_bits);
            // Step 1: If O is not an Object, return O.
            if ftag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged_mut::<UnifiedObject>(obj_bits)
                };
                if let Some(u) = uni {
                    // Step 2 (SetIntegrityLevel):
                    // Step 2.1: O.[[PreventExtensions]]() — marks object non-extensible.
                    // Step 2.5: For each property, set configurable=false and writable=false.
                    u.freeze();
                    let freeze_ok = SHAPES.with(|shapes| {
                        let mut shapes = shapes.borrow_mut();
                        if let Some(new_shape) = shapes.freeze_all_properties(u.shape_id) {
                            u.shape_id = new_shape;
                        }
                        true
                    });
                    // Step 3: If status is false, throw a TypeError exception.
                    if !freeze_ok {
                        let msg = super::make_rt_string(
                            "Object.freeze: cannot freeze object".to_string(),
                        );
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                    }
                }
            }
            // Step 4: Return O. (Step 1 for non-objects: return O unchanged.)
            Some(obj_bits)
        }

        // ---------------------------------------------------------------
        // Object.seal ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.seal
        //
        // 1. If O is not an Object, return O.
        // 2. Let status be ? SetIntegrityLevel(O, sealed).
        // 3. If status is false, throw a TypeError exception.
        // 4. Return O.
        //
        // SetIntegrityLevel ( O, level ) — abstract operation (sealed)
        // [spec]: https://tc39.es/ecma262/#sec-setintegritylevel
        //
        // 1. Let status be ? O.[[PreventExtensions]]().
        // 2. If status is false, return false.
        // 3. Let keys be ? O.[[OwnPropertyKeys]]().
        // 4. If level is sealed, then
        //    a. For each element k of keys, do
        //       i. Perform ? DefinePropertyOrThrow(O, k,
        //          PropertyDescriptor { [[Configurable]]: false }).
        // 5. ...
        // 6. Return true.
        // ---------------------------------------------------------------
        "seal" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let stag = read_obj_tag(obj_bits);
            // Step 1: If O is not an Object, return O.
            if stag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged_mut::<UnifiedObject>(obj_bits)
                };
                if let Some(u) = uni {
                    // Step 2 (SetIntegrityLevel):
                    // Step 2.1: O.[[PreventExtensions]]() — marks object non-extensible.
                    // Step 2.4a: For each property, set configurable=false.
                    u.seal();
                    let seal_ok = SHAPES.with(|shapes| {
                        let mut shapes = shapes.borrow_mut();
                        if let Some(new_shape) = shapes.seal_all_properties(u.shape_id) {
                            u.shape_id = new_shape;
                        }
                        true
                    });
                    // Step 3: If status is false, throw a TypeError exception.
                    if !seal_ok {
                        let msg =
                            super::make_rt_string("Object.seal: cannot seal object".to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                    }
                }
            }
            // Step 4: Return O. (Step 1 for non-objects: return O unchanged.)
            Some(obj_bits)
        }

        // ---------------------------------------------------------------
        // Object.preventExtensions ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.preventextensions
        //
        // 1. If O is not an Object, return O.
        // 2. Let status be ? O.[[PreventExtensions]]().
        // 3. If status is false, throw a TypeError exception.
        // 4. Return O.
        // ---------------------------------------------------------------
        "preventExtensions" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

            // Proxy intercept: delegate to proxy_prevent_extensions trap
            if let Some(petag) = read_obj_tag(obj_bits)
                && petag == ObjTag::Unified as u8
                && let Some(peu) = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                }
                && peu.kind == InternalKind::Proxy
            {
                // Step 2: Let status be ? O.[[PreventExtensions]]() — Proxy path.
                match crate::proxy::proxy_prevent_extensions(obj_bits) {
                    Ok(success) => {
                        // Step 3: If status is false, throw a TypeError exception.
                        if !success {
                            let msg = super::make_rt_string(
                                "Object.preventExtensions: proxy trap returned false".to_string(),
                            );
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                        }
                        return Some(obj_bits);
                    }
                    Err(e) => {
                        let msg = super::make_rt_string(e.to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                        return Some(obj_bits);
                    }
                }
            }

            // Step 1: If O is not an Object, return O (handled by tag check).
            let ptag = read_obj_tag(obj_bits);
            if ptag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged_mut::<UnifiedObject>(obj_bits)
                };
                if let Some(u) = uni {
                    // Step 2: Let status be ? O.[[PreventExtensions]]().
                    u.prevent_extensions();
                    // Step 3: If status is false, throw a TypeError exception.
                    // (Our implementation always succeeds for ordinary objects,
                    // but the check is here for correctness.)
                    if u.is_extensible() {
                        let msg = super::make_rt_string(
                            "Object.preventExtensions: cannot prevent extensions".to_string(),
                        );
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                    }
                }
            }
            // Step 4: Return O.
            Some(obj_bits)
        }

        // ---------------------------------------------------------------
        // Object.isFrozen ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.isfrozen
        //
        // 1. If O is not an Object, return true.
        // 2. Return ? TestIntegrityLevel(O, frozen).
        //
        // TestIntegrityLevel ( O, level ) — abstract operation (frozen)
        // [spec]: https://tc39.es/ecma262/#sec-testintegritylevel
        //
        // 1. Let extensible be ? O.[[IsExtensible]]().
        // 2. If extensible is true, return false.
        // 3. NOTE: If the object is extensible, none of its properties are examined.
        // 4. Let keys be ? O.[[OwnPropertyKeys]]().
        // 5. For each element k of keys, do
        //    a. Let currentDesc be ? O.[[GetOwnProperty]](k).
        //    b. If currentDesc is not undefined, then
        //       i. If currentDesc.[[Configurable]] is true, return false.
        //       ii. If level is frozen and IsDataDescriptor(currentDesc), then
        //           1. If currentDesc.[[Writable]] is true, return false.
        // 6. Return true.
        // ---------------------------------------------------------------
        "isFrozen" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let iftag = read_obj_tag(obj_bits);
            let frozen = if iftag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                };
                // Step 2: Return ? TestIntegrityLevel(O, frozen).
                uni.is_some_and(|u| u.is_frozen())
            } else {
                // Step 1: If O is not an Object, return true.
                true
            };
            Some(JsValue::bool(frozen).raw_bits())
        }

        // ---------------------------------------------------------------
        // Object.isSealed ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.issealed
        //
        // 1. If O is not an Object, return true.
        // 2. Return ? TestIntegrityLevel(O, sealed).
        //
        // TestIntegrityLevel ( O, level ) — abstract operation (sealed)
        // [spec]: https://tc39.es/ecma262/#sec-testintegritylevel
        //
        // 1. Let extensible be ? O.[[IsExtensible]]().
        // 2. If extensible is true, return false.
        // 3-5. For each own property, if configurable is true, return false.
        // 6. Return true.
        // ---------------------------------------------------------------
        "isSealed" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let istag = read_obj_tag(obj_bits);
            let sealed = if istag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                };
                // Step 2: Return ? TestIntegrityLevel(O, sealed).
                // A frozen object is also sealed.
                uni.is_some_and(|u| u.is_sealed() || u.is_frozen())
            } else {
                // Step 1: If O is not an Object, return true.
                true
            };
            Some(JsValue::bool(sealed).raw_bits())
        }

        // ---------------------------------------------------------------
        // Object.isExtensible ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.isextensible
        //
        // 1. If O is not an Object, return false.
        // 2. Return ? O.[[IsExtensible]]().
        // ---------------------------------------------------------------
        "isExtensible" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

            // Proxy intercept: delegate to proxy_is_extensible trap
            if let Some(ietag2) = read_obj_tag(obj_bits)
                && ietag2 == ObjTag::Unified as u8
                && let Some(ieu) = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                }
                && ieu.kind == InternalKind::Proxy
            {
                // Step 2: Return ? O.[[IsExtensible]]() — Proxy path.
                match crate::proxy::proxy_is_extensible(obj_bits) {
                    Ok(result) => return Some(JsValue::bool(result).raw_bits()),
                    Err(e) => {
                        let msg = super::make_rt_string(e.to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                        return Some(JsValue::bool(false).raw_bits());
                    }
                }
            }

            let ietag = read_obj_tag(obj_bits);
            let extensible = if ietag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                };
                // Step 2: Return ? O.[[IsExtensible]]().
                uni.is_some_and(|u| u.is_extensible())
            } else {
                // Step 1: If O is not an Object, return false.
                false
            };
            Some(JsValue::bool(extensible).raw_bits())
        }

        // ---------------------------------------------------------------
        // Object.values ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.values
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let nameList be ? EnumerableOwnProperties(obj, value).
        // 3. Return CreateArrayFromList(nameList).
        // ---------------------------------------------------------------
        "values" => {
            let raw_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // Step 1: Let obj be ? ToObject(O).
            let Some(obj) = to_object_or_throw(raw_arg, "values") else {
                return Some(create_empty_array());
            };
            // Step 2: Let nameList be ? EnumerableOwnProperties(obj, value).
            let keys = __esc_rt_object_keys(obj);
            // SAFETY: __esc_rt_object_keys returns a UnifiedObject array via TaggedObj::boxed.
            if let Some(key_arr) = unsafe { deref_tagged::<UnifiedObject>(keys) }
                && key_arr.kind == InternalKind::Array
            {
                let elems = key_arr.array_elements();
                let mut values = Vec::with_capacity(elems.len());
                for key_val in elems {
                    let val = __esc_rt_get_prop(obj, key_val.raw_bits());
                    values.push(JsValue::from_raw_bits(val));
                }
                // Step 3: Return CreateArrayFromList(nameList).
                Some(create_array_from_elements(values))
            } else {
                Some(create_empty_array())
            }
        }

        // ---------------------------------------------------------------
        // Object.entries ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.entries
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let nameList be ? EnumerableOwnProperties(obj, key+value).
        // 3. Return CreateArrayFromList(nameList).
        // ---------------------------------------------------------------
        "entries" => {
            let raw_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // Step 1: Let obj be ? ToObject(O).
            let Some(obj) = to_object_or_throw(raw_arg, "entries") else {
                return Some(create_empty_array());
            };
            // Step 2: Let nameList be ? EnumerableOwnProperties(obj, key+value).
            let keys = __esc_rt_object_keys(obj);
            // SAFETY: __esc_rt_object_keys returns a UnifiedObject array via TaggedObj::boxed.
            if let Some(key_arr) = unsafe { deref_tagged::<UnifiedObject>(keys) }
                && key_arr.kind == InternalKind::Array
            {
                let elems = key_arr.array_elements();
                let mut entries = Vec::with_capacity(elems.len());
                for key_val in elems {
                    let val = __esc_rt_get_prop(obj, key_val.raw_bits());
                    // Each entry is a [key, value] pair.
                    let pair =
                        create_array_from_elements(vec![*key_val, JsValue::from_raw_bits(val)]);
                    entries.push(JsValue::from_raw_bits(pair));
                }
                // Step 3: Return CreateArrayFromList(nameList).
                Some(create_array_from_elements(entries))
            } else {
                Some(create_empty_array())
            }
        }

        // ---------------------------------------------------------------
        // Object.getPrototypeOf ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.getprototypeof
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Return ? obj.[[GetPrototypeOf]]().
        // ---------------------------------------------------------------
        "getPrototypeOf" => {
            // Step 1: Let obj be ? ToObject(O).
            // TODO: Step 1 — should call ToObject; currently passes raw value.
            let obj = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());

            // Proxy intercept: delegate to proxy_get_prototype_of trap
            if let Some(gptag) = read_obj_tag(obj)
                && gptag == ObjTag::Unified as u8
                && let Some(gpu) = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj)
                }
                && gpu.kind == InternalKind::Proxy
            {
                // Step 2: Return ? obj.[[GetPrototypeOf]]() — Proxy path.
                match crate::proxy::proxy_get_prototype_of(obj) {
                    Ok(result) => return Some(result),
                    Err(e) => {
                        let msg = super::make_rt_string(e.to_string());
                        let err = super::__esc_rt_create_error(
                            crate::exceptions::error_tag::TYPE_ERROR,
                            msg,
                        );
                        super::__esc_rt_throw(err);
                        return Some(JsValue::null().raw_bits());
                    }
                }
            }

            // Step 2: Return ? obj.[[GetPrototypeOf]]().
            Some(get_prototype_of(obj))
        }

        // ---------------------------------------------------------------
        // Object.setPrototypeOf ( O, proto )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.setprototypeof
        //
        // 1. Set O to ? RequireObjectCoercible(O).
        // 2. If proto is not an Object and proto is not null, throw a TypeError exception.
        // 3. If O is not an Object, return O.
        // 4. Let status be ? O.[[SetPrototypeOf]](proto).
        // 5. If status is false, throw a TypeError exception.
        // 6. Return O.
        // ---------------------------------------------------------------
        "setPrototypeOf" => {
            if args.len() >= 2 {
                let obj = args[0].raw_bits();
                let proto = args[1].raw_bits();

                // Step 1: Set O to ? RequireObjectCoercible(O).
                let obj_val = JsValue::from_raw_bits(obj);
                if obj_val.is_null() || obj_val.is_undefined() {
                    let desc = if obj_val.is_null() {
                        "null"
                    } else {
                        "undefined"
                    };
                    let msg = super::make_rt_string(format!("Cannot convert {desc} to object"));
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(JsValue::undefined().raw_bits());
                }

                // Step 2: If proto is not an Object and proto is not null,
                //         throw a TypeError exception.
                let proto_val = JsValue::from_raw_bits(proto);
                if !proto_val.is_object() && !proto_val.is_null() {
                    let msg = super::make_rt_string(
                        "Object prototype may only be an Object or null".to_string(),
                    );
                    let err =
                        super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
                    super::__esc_rt_throw(err);
                    return Some(obj);
                }

                // Step 3: If O is not an Object, return O.
                if !obj_val.is_object() {
                    return Some(obj);
                }

                // Proxy intercept: delegate to proxy_set_prototype_of trap
                if let Some(sptag) = read_obj_tag(obj)
                    && sptag == ObjTag::Unified as u8
                    && let Some(spu) = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(obj)
                    }
                    && spu.kind == InternalKind::Proxy
                {
                    // Step 4: Let status be ? O.[[SetPrototypeOf]](proto) — Proxy path.
                    match crate::proxy::proxy_set_prototype_of(obj, proto) {
                        Ok(success) => {
                            // Step 5: If status is false, throw a TypeError exception.
                            if !success {
                                let msg = super::make_rt_string(
                                    "Object.setPrototypeOf: proxy trap returned false".to_string(),
                                );
                                let err = super::__esc_rt_create_error(
                                    crate::exceptions::error_tag::TYPE_ERROR,
                                    msg,
                                );
                                super::__esc_rt_throw(err);
                            }
                            return Some(obj);
                        }
                        Err(e) => {
                            let msg = super::make_rt_string(e.to_string());
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                            return Some(obj);
                        }
                    }
                }

                // Step 4: Let status be ? O.[[SetPrototypeOf]](proto).
                set_prototype_of(obj, proto);
                // Step 6: Return O.
                Some(obj)
            } else {
                None
            }
        }

        // ---------------------------------------------------------------
        // Object.getOwnPropertyDescriptor ( O, P )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.getownpropertydescriptor
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let key be ? ToPropertyKey(P).
        // 3. Let desc be ? obj.[[GetOwnProperty]](key).
        // 4. Return FromPropertyDescriptor(desc).
        // ---------------------------------------------------------------
        "getOwnPropertyDescriptor" => {
            if args.len() >= 2 {
                let raw_obj = args[0].raw_bits();
                let prop_bits = args[1].raw_bits();

                // Step 1: Let obj be ? ToObject(O).
                let Some(obj_bits) = to_object_or_throw(raw_obj, "getOwnPropertyDescriptor") else {
                    return Some(JsValue::undefined().raw_bits());
                };

                // Proxy intercept: delegate to proxy_get_own_property_descriptor trap
                if let Some(ptag) = read_obj_tag(obj_bits)
                    && ptag == ObjTag::Unified as u8
                    && let Some(pu) = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(obj_bits)
                    }
                    && pu.kind == InternalKind::Proxy
                {
                    let key_name = key_to_string(prop_bits);
                    // Step 3: Let desc be ? obj.[[GetOwnProperty]](key) — Proxy path.
                    match crate::proxy::proxy_get_own_property_descriptor(
                        obj_bits, prop_bits, &key_name,
                    ) {
                        Ok(result) => return Some(result),
                        Err(e) => {
                            let msg = super::make_rt_string(e.to_string());
                            let err = super::__esc_rt_create_error(
                                crate::exceptions::error_tag::TYPE_ERROR,
                                msg,
                            );
                            super::__esc_rt_throw(err);
                            return Some(JsValue::undefined().raw_bits());
                        }
                    }
                }

                let dtag = read_obj_tag(obj_bits);

                // Step 2: Let key be ? ToPropertyKey(P).
                // Step 3: Let desc be ? obj.[[GetOwnProperty]](key).
                let desc_opt = if dtag == Some(ObjTag::Unified as u8) {
                    let uni = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(obj_bits)
                    };
                    let Some(u) = uni else {
                        return Some(JsValue::undefined().raw_bits());
                    };
                    let prop_name = key_to_string(prop_bits);

                    // For Function/Closure/NativeFunc objects, check OBJECT_PROPS
                    // and internal data since their properties aren't always stored
                    // in the shape system
                    if u.kind == crate::internal_data::InternalKind::Function
                        || u.kind == crate::internal_data::InternalKind::Closure
                    {
                        // Check if it's a well-known function property
                        let func_desc = get_function_property_descriptor(obj_bits, u, &prop_name);
                        if func_desc.is_some() {
                            func_desc
                        } else {
                            SHAPES.with(|shapes| {
                                INTERNER.with(|interner| {
                                    let shapes = shapes.borrow();
                                    let interner = interner.borrow();
                                    u.get_property_descriptor(&prop_name, &shapes, &interner)
                                })
                            })
                        }
                    } else if u.kind == crate::internal_data::InternalKind::NativeFunc {
                        // NativeFunc objects (built-in constructors & method wrappers)
                        // store name/length/prototype in OBJECT_PROPS side-table
                        let nf_desc = get_native_func_property_descriptor(obj_bits, u, &prop_name);
                        if nf_desc.is_some() {
                            nf_desc
                        } else {
                            SHAPES.with(|shapes| {
                                INTERNER.with(|interner| {
                                    let shapes = shapes.borrow();
                                    let interner = interner.borrow();
                                    u.get_property_descriptor(&prop_name, &shapes, &interner)
                                })
                            })
                        }
                    } else {
                        // Try shape-based lookup first
                        let shape_desc = SHAPES.with(|shapes| {
                            INTERNER.with(|interner| {
                                let shapes = shapes.borrow();
                                let interner = interner.borrow();
                                u.get_property_descriptor(&prop_name, &shapes, &interner)
                            })
                        });
                        if shape_desc.is_some() {
                            shape_desc
                        } else {
                            // Check if this is a namespace object (Math, JSON, Reflect)
                            // or a builtin constructor and resolve virtual properties.
                            get_builtin_virtual_property_descriptor(obj_bits, &prop_name)
                        }
                    }
                } else {
                    return Some(JsValue::undefined().raw_bits());
                };

                // Step 4: Return FromPropertyDescriptor(desc).
                // If desc is undefined, return undefined.
                let Some(desc) = desc_opt else {
                    return Some(JsValue::undefined().raw_bits());
                };

                // FromPropertyDescriptor: build a descriptor object.
                let result = __esc_rt_create_object();
                match desc {
                    crate::property::OwnPropertyDescriptor::Data {
                        value,
                        writable,
                        enumerable,
                        configurable,
                    } => {
                        let vk = make_rt_string("value".to_string());
                        __esc_rt_set_prop(result, vk, value.raw_bits());
                        let wk = make_rt_string("writable".to_string());
                        __esc_rt_set_prop(result, wk, JsValue::bool(writable).raw_bits());
                        let ek = make_rt_string("enumerable".to_string());
                        __esc_rt_set_prop(result, ek, JsValue::bool(enumerable).raw_bits());
                        let ck = make_rt_string("configurable".to_string());
                        __esc_rt_set_prop(result, ck, JsValue::bool(configurable).raw_bits());
                    }
                    crate::property::OwnPropertyDescriptor::Accessor {
                        getter,
                        setter,
                        enumerable,
                        configurable,
                    } => {
                        let gk = make_rt_string("get".to_string());
                        __esc_rt_set_prop(result, gk, getter.raw_bits());
                        let sk = make_rt_string("set".to_string());
                        __esc_rt_set_prop(result, sk, setter.raw_bits());
                        let ek = make_rt_string("enumerable".to_string());
                        __esc_rt_set_prop(result, ek, JsValue::bool(enumerable).raw_bits());
                        let ck = make_rt_string("configurable".to_string());
                        __esc_rt_set_prop(result, ck, JsValue::bool(configurable).raw_bits());
                    }
                }
                Some(result)
            } else {
                Some(JsValue::undefined().raw_bits())
            }
        }

        // ---------------------------------------------------------------
        // Object.getOwnPropertyNames ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.getownpropertynames
        //
        // 1. Return CreateArrayFromList(? GetOwnPropertyKeys(O, string)).
        //
        // GetOwnPropertyKeys ( O, type ) — abstract operation
        // [spec]: https://tc39.es/ecma262/#sec-getownpropertykeys
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let keys be ? obj.[[OwnPropertyKeys]]().
        // 3. Let nameList be a new empty List.
        // 4. For each element nextKey of keys, do
        //    a. If nextKey is a String (when type is string), then
        //       i. Append nextKey to nameList.
        // 5. Return nameList.
        // ---------------------------------------------------------------
        "getOwnPropertyNames" => {
            // Step 1 (GetOwnPropertyKeys step 1): Let obj be ? ToObject(O).
            let raw_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let Some(obj_bits) = to_object_or_throw(raw_arg, "getOwnPropertyNames") else {
                return Some(create_empty_array());
            };
            let ntag = read_obj_tag(obj_bits);
            if ntag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                };
                let Some(u) = uni else {
                    return Some(create_empty_array());
                };
                SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        // Step 2: Let keys be ? obj.[[OwnPropertyKeys]]().
                        // For function objects, include name/length/prototype which
                        // live in OBJECT_PROPS / InternalData, not in the shape system.
                        let keys = if u.kind == InternalKind::Function
                            || u.kind == InternalKind::Closure
                            || u.kind == InternalKind::NativeFunc
                        {
                            collect_function_own_keys(obj_bits, u, &shapes, &interner)
                        } else {
                            u.own_keys(&shapes, &interner)
                        };
                        // Steps 3-4: Filter for string keys (all keys here are strings).
                        // Step 5 / outer step 1: Return CreateArrayFromList(nameList).
                        let values: Vec<JsValue> = keys
                            .into_iter()
                            .map(|k| JsValue::from_raw_bits(make_rt_string(k)))
                            .collect();
                        Some(create_array_from_elements(values))
                    })
                })
            } else {
                Some(create_empty_array())
            }
        }

        // ---------------------------------------------------------------
        // Object.getOwnPropertyDescriptors ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.getownpropertydescriptors
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let ownKeys be ? obj.[[OwnPropertyKeys]]().
        // 3. Let descriptors be OrdinaryObjectCreate(%Object.prototype%).
        // 4. For each element key of ownKeys, do
        //    a. Let desc be ? obj.[[GetOwnProperty]](key).
        //    b. Let descriptor be FromPropertyDescriptor(desc).
        //    c. If descriptor is not undefined, perform
        //       ! CreateDataPropertyOrThrow(descriptors, key, descriptor).
        // 5. Return descriptors.
        // ---------------------------------------------------------------
        "getOwnPropertyDescriptors" => {
            // Step 1: Let obj be ? ToObject(O).
            let raw_arg = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            let Some(obj_bits) = to_object_or_throw(raw_arg, "getOwnPropertyDescriptors") else {
                return Some(__esc_rt_create_object());
            };
            let gdtag = read_obj_tag(obj_bits);
            if gdtag == Some(ObjTag::Unified as u8) {
                let uni = unsafe {
                    // SAFETY: tag check confirms this is a unified object.
                    deref_tagged::<UnifiedObject>(obj_bits)
                };
                let Some(u) = uni else {
                    return Some(__esc_rt_create_object());
                };
                // Step 3: Let descriptors be OrdinaryObjectCreate(%Object.prototype%).
                let result = __esc_rt_create_object();
                // Step 2: Let ownKeys be ? obj.[[OwnPropertyKeys]]().
                let all_keys = SHAPES.with(|shapes| {
                    INTERNER.with(|interner| {
                        let shapes = shapes.borrow();
                        let interner = interner.borrow();
                        // For function objects, include name/length/prototype which
                        // live in OBJECT_PROPS / InternalData, not in the shape system.
                        if u.kind == InternalKind::Function
                            || u.kind == InternalKind::Closure
                            || u.kind == InternalKind::NativeFunc
                        {
                            collect_function_own_keys(obj_bits, u, &shapes, &interner)
                        } else {
                            u.own_keys(&shapes, &interner)
                        }
                    })
                });
                // Step 4: For each element key of ownKeys, do
                for key_name in all_keys {
                    let prop_key = make_rt_string(key_name.clone());
                    // Step 4a-b: Get descriptor and convert via FromPropertyDescriptor.
                    let desc_args = [
                        JsValue::from_raw_bits(obj_bits),
                        JsValue::from_raw_bits(prop_key),
                    ];
                    if let Some(desc) = dispatch_object_static_method(
                        "getOwnPropertyDescriptor",
                        2,
                        desc_args
                            .iter()
                            .map(|v| v.raw_bits())
                            .collect::<Vec<_>>()
                            .as_ptr(),
                    ) {
                        // Step 4c: CreateDataPropertyOrThrow(descriptors, key, descriptor).
                        let key_bits = make_rt_string(key_name);
                        __esc_rt_set_prop(result, key_bits, desc);
                    }
                }
                // Step 5: Return descriptors.
                Some(result)
            } else {
                Some(__esc_rt_create_object())
            }
        }

        // ---------------------------------------------------------------
        // Object.is ( value1, value2 )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.is
        //
        // 1. Return SameValue(value1, value2).
        // ---------------------------------------------------------------
        "is" => {
            let a = args.first().map_or(JsValue::undefined(), |v| *v);
            let b = args.get(1).map_or(JsValue::undefined(), |v| *v);
            // Step 1: Return SameValue(value1, value2).
            Some(JsValue::bool(crate::value_ops::same_value(a, b)).raw_bits())
        }

        // ---------------------------------------------------------------
        // Object.hasOwn ( O, P )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.hasown
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let key be ? ToPropertyKey(P).
        // 3. Return ? HasOwnProperty(obj, key).
        // ---------------------------------------------------------------
        "hasOwn" => {
            if args.len() >= 2 {
                let obj_bits = args[0].raw_bits();
                let prop_bits = args[1].raw_bits();
                let htag = read_obj_tag(obj_bits);
                let has = if htag == Some(ObjTag::Unified as u8) {
                    let uni = unsafe {
                        // SAFETY: tag check confirms this is a unified object.
                        deref_tagged::<UnifiedObject>(obj_bits)
                    };
                    if let Some(u) = uni {
                        // Step 2: Let key be ? ToPropertyKey(P).
                        let prop_name = key_to_string(prop_bits);
                        // Step 3: Return ? HasOwnProperty(obj, key).
                        // For function objects, check well-known properties
                        // (name, length) that aren't in the shape system.
                        if (u.kind == InternalKind::Function
                            || u.kind == InternalKind::Closure
                            || u.kind == InternalKind::NativeFunc)
                            && matches!(prop_name.as_str(), "name" | "length")
                        {
                            true
                        } else if (u.kind == InternalKind::Function
                            || u.kind == InternalKind::Closure
                            || u.kind == InternalKind::NativeFunc)
                            && prop_name == "prototype"
                        {
                            super::OBJECT_PROPS.with(|props| {
                                let props = props.borrow();
                                props
                                    .get(&obj_bits)
                                    .is_some_and(|m| m.contains_key("prototype"))
                            })
                        } else {
                            SHAPES.with(|shapes| {
                                INTERNER.with(|interner| {
                                    let shapes = shapes.borrow();
                                    let interner = interner.borrow();
                                    u.has_own_property(&prop_name, &shapes, &interner)
                                })
                            })
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                Some(JsValue::bool(has).raw_bits())
            } else {
                Some(JsValue::bool(false).raw_bits())
            }
        }

        // ---------------------------------------------------------------
        // Object.getOwnPropertySymbols ( O )
        //
        // [spec]: https://tc39.es/ecma262/#sec-object.getownpropertysymbols
        //
        // 1. Return CreateArrayFromList(? GetOwnPropertyKeys(O, symbol)).
        //
        // GetOwnPropertyKeys ( O, type ) — abstract operation
        // [spec]: https://tc39.es/ecma262/#sec-getownpropertykeys
        //
        // 1. Let obj be ? ToObject(O).
        // 2. Let keys be ? obj.[[OwnPropertyKeys]]().
        // 3. Let nameList be a new empty List.
        // 4. For each element nextKey of keys, do
        //    a. If nextKey is a Symbol (when type is symbol), then
        //       i. Append nextKey to nameList.
        // 5. Return nameList.
        // ---------------------------------------------------------------
        "getOwnPropertySymbols" => {
            let obj_bits = args
                .first()
                .map_or(JsValue::undefined().raw_bits(), |v| v.raw_bits());
            // Step 1: Return CreateArrayFromList(? GetOwnPropertyKeys(O, symbol)).
            Some(get_own_property_symbols(obj_bits))
        }
        _ => None,
    }
}

/// `Object.getOwnPropertySymbols ( O )` — helper
///
/// Returns an array of all symbol-keyed own properties on an object.
///
/// Implements `GetOwnPropertyKeys(O, symbol)` from
/// [spec section 20.1.2.10.1](https://tc39.es/ecma262/#sec-getownpropertykeys).
///
/// Walks the object's shape properties and filters for `PropertyKey::Symbol`
/// variants. Each symbol ID is converted back to a `JsValue::symbol()`.
fn get_own_property_symbols(obj: u64) -> u64 {
    // Step 1: Let obj be ? ToObject(O).
    // TODO: Step 1 — ToObject not called; non-object input returns empty array instead of throwing.
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
    // Step 2: Let keys be ? obj.[[OwnPropertyKeys]]().
    // Steps 3-4: Filter for Symbol keys only.
    SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        let Some(shape) = shapes.get(u.shape_id) else {
            return create_empty_array();
        };
        let symbols: Vec<JsValue> = shape
            .properties
            .iter()
            .filter_map(|(key, _)| {
                if let shapes::PropertyKey::Symbol(id) = key {
                    Some(JsValue::symbol(*id))
                } else {
                    None
                }
            })
            .collect();
        // Step 5: Return nameList (wrapped in CreateArrayFromList).
        create_array_from_elements(symbols)
    })
}

/// `OrdinaryGetPrototypeOf ( O )` — internal helper
///
/// Returns the prototype of an object, implementing the `[[GetPrototypeOf]]`
/// internal method for ordinary objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinarygetprototypeof
///
/// 1. Return O.[[Prototype]].
///
/// Checks shape-based prototype first (via PROTO_OBJECTS registry), then
/// falls back to the legacy `__proto__` string property.
pub(crate) fn get_prototype_of(obj: u64) -> u64 {
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return JsValue::null().raw_bits();
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<UnifiedObject>(obj)
    };
    let Some(u) = uni else {
        return JsValue::null().raw_bits();
    };

    // Step 1: Return O.[[Prototype]].
    // Check shape-based prototype via PROTO_OBJECTS registry.
    let shape_proto = SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        shapes.get_prototype(u.shape_id).and_then(|proto_shape_id| {
            PROTO_OBJECTS.with(|protos| protos.borrow().get(&proto_shape_id).copied())
        })
    });
    if let Some(proto_bits) = shape_proto {
        return proto_bits;
    }

    // Legacy fallback: check __proto__ string property.
    let proto_key = make_rt_string("__proto__".to_string());
    let legacy = __esc_rt_get_prop(obj, proto_key);
    let legacy_val = JsValue::from_raw_bits(legacy);
    if legacy_val.is_object() || legacy_val.is_null() {
        return legacy;
    }

    // Implicit prototype: ordinary objects inherit from Object.prototype
    // even when no explicit [[Prototype]] link was set (ES2024 §10.1.1).
    if matches!(u.kind, crate::internal_data::InternalKind::Ordinary) {
        let obj_proto = super::property::get_object_prototype();
        // Don't return Object.prototype for itself (its prototype is null per spec).
        if obj_proto != obj {
            return obj_proto;
        }
    }
    JsValue::null().raw_bits()
}

/// Prototype cycle detection helper.
///
/// Used by `OrdinarySetPrototypeOf` to implement the loop detection
/// described in [ES2024 section 10.1.2 step 8](https://tc39.es/ecma262/#sec-ordinarysetprototypeof).
///
/// Walks the prototype chain of `proto` up to 1000 hops. If `obj` appears
/// anywhere in that chain, setting `proto` as the prototype of `obj` would
/// create a cycle, so this returns `true`.
fn would_create_prototype_cycle(obj: u64, proto: u64) -> bool {
    // Step 8 of OrdinarySetPrototypeOf:
    // 8. Repeat,
    //    a. If p is null, then return true (no cycle).
    //    b. If SameValue(p, O) is true, return false (cycle detected).
    //    c. If p.[[GetPrototypeOf]] is not the ordinary one, return true.
    //    d. Set p to p.[[Prototype]].
    let mut current = proto;
    for _ in 0..1000 {
        // Step 8b: If SameValue(p, O) is true, cycle detected.
        if current == obj {
            return true;
        }
        // Step 8d: Set p to p.[[Prototype]].
        let next = get_prototype_of(current);
        let next_val = JsValue::from_raw_bits(next);
        // Step 8a: If p is null, no cycle.
        if next_val.is_null() || next_val.is_undefined() {
            break;
        }
        current = next;
    }
    false
}

/// `OrdinarySetPrototypeOf ( O, V )` — internal helper
///
/// Sets the prototype of an object, implementing the `[[SetPrototypeOf]]`
/// internal method for ordinary objects.
///
/// [spec]: https://tc39.es/ecma262/#sec-ordinarysetprototypeof
///
/// 1. Let current be O.[[Prototype]].
/// 2. If SameValue(V, current) is true, return true.
/// 3. Let extensible be O.[[Extensible]].
/// 4. If extensible is false, return false.
/// 5. Let p be V.
/// 6. Let done be false.
/// 7. Repeat, while done is false,
///    a. If p is null, then set done to true.
///    b. Else if SameValue(p, O) is true, return false.
///    c-d. Walk prototype chain.
/// 8. Set O.[[Prototype]] to V.
/// 9. Return true.
///
/// Sets both the legacy `__proto__` property and registers the shape-based
/// prototype in the PROTO_OBJECTS registry. Also bumps the global prototype
/// epoch to invalidate inline caches.
///
/// Throws a `TypeError` if setting the prototype would create a cycle.
fn set_prototype_of(obj: u64, proto: u64) {
    let proto_val = JsValue::from_raw_bits(proto);

    // TODO: Step 1-2 — should check if V is SameValue as current prototype and
    // return early if so.
    // TODO: Step 3-4 — should check [[Extensible]] and return false if not extensible.

    // Steps 5-7: Cycle detection — walk proto's chain; if obj appears, it's a cycle.
    if !proto_val.is_null() && !proto_val.is_undefined() && would_create_prototype_cycle(obj, proto)
    {
        let msg = make_rt_string(
            "Cyclic __proto__ value: setting this prototype would create a cycle".to_string(),
        );
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::TYPE_ERROR, msg);
        super::__esc_rt_throw(err);
        return;
    }

    crate::ic::bump_prototype_epoch();

    // Step 8: Set O.[[Prototype]] to V.
    // 8a. Set legacy __proto__ first (may cause shape transitions).
    if !proto_val.is_null() && !proto_val.is_undefined() {
        let key_bits = make_rt_string("__proto__".to_string());
        __esc_rt_set_prop(obj, key_bits, proto);
    }

    // 8b. Register shape-based prototype on the final shape.
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else { return };

    if proto_val.is_null() || proto_val.is_undefined() {
        // Clear prototype: create a shape with no prototype
        // (keep current shape since prototype is already None by default)
        return;
    }

    SHAPES.with(|shapes| {
        let mut shapes = shapes.borrow_mut();
        let proto_shape_id = shapes::ShapeId(shapes.shape_count() as u32);
        let new_shape_id = shapes.set_prototype(u.shape_id, proto_shape_id);
        u.shape_id = new_shape_id;
        if let Some(sid) = shapes.get_prototype(new_shape_id) {
            PROTO_OBJECTS.with(|protos| {
                protos.borrow_mut().insert(sid, proto);
            });
        }
    });
    // Step 9: Return true (implicit — function returns void).
}

/// `CopyDataProperties ( target, source, excludedItems )`
///
/// Copies enumerable own properties from `source` to `target`. Used by the
/// compiler to implement object spread syntax (`{...source}`).
///
/// [spec]: <https://tc39.es/ecma262/#sec-copydataproperties>
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_spread_into_object(target: u64, source: u64) -> u64 {
    let sv = JsValue::from_raw_bits(source);
    // If source is null or undefined, return target unchanged.
    if sv.is_null() || sv.is_undefined() {
        return target;
    }
    // Get enumerable own string-keyed properties
    let keys = __esc_rt_object_keys(source);
    // SAFETY: __esc_rt_object_keys returns a UnifiedObject array.
    if let Some(arr) = unsafe { deref_tagged::<UnifiedObject>(keys) }
        && arr.kind == InternalKind::Array
    {
        for elem in arr.array_elements() {
            let val = __esc_rt_get_prop(source, elem.raw_bits());
            __esc_rt_set_prop(target, elem.raw_bits(), val);
        }
    }
    target
}
