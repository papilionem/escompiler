//! Object built-in methods.
//!
//! Provides `Object.keys()`, `Object.values()`, `Object.entries()`,
//! `Object.create()`, `Object.assign()`, `Object.defineProperty()`,
//! `Object.freeze()`, `Object.seal()`, and related methods.
//!
//! Freeze/seal state is stored on the object's header flags via
//! `runtime::internal_data::UnifiedObject`, not in a separate side-table.

use nanbox::JsValue;
use runtime::internal_data::UnifiedObject;
use runtime::tagged_obj::{ObjTag, deref_tagged, deref_tagged_mut, read_obj_tag};

/// `Object.keys(obj)` — enumerate own string property names.
///
/// Requires runtime shape walking for full implementation. Returns an empty
/// array placeholder.
pub fn object_keys(_args: &[JsValue]) -> JsValue {
    // Full implementation requires iterating the object's shape/property storage
    // Return empty array representation (int 0 = zero keys)
    JsValue::int(0)
}

/// `Object.values(obj)` — enumerate own property values.
///
/// Requires runtime shape walking for full implementation. Returns an empty
/// array placeholder.
pub fn object_values(_args: &[JsValue]) -> JsValue {
    JsValue::int(0)
}

/// `Object.entries(obj)` — return `[key, value]` pairs.
///
/// Requires runtime shape walking for full implementation. Returns an empty
/// array placeholder.
pub fn object_entries(_args: &[JsValue]) -> JsValue {
    JsValue::int(0)
}

/// `Object.create(proto)` — create a new object with the specified prototype.
///
/// Returns an object value. Full prototype chain wiring requires the runtime.
pub fn object_create(args: &[JsValue]) -> JsValue {
    let _proto = args.first().copied().unwrap_or_else(JsValue::null);
    // Create a minimal object — prototype chain wiring is a runtime concern
    JsValue::object(std::ptr::null())
}

/// `Object.assign(target, ...sources)` — copy own enumerable properties.
///
/// Structural placeholder — returns the target object. Full property copying
/// requires runtime property enumeration.
pub fn object_assign(args: &[JsValue]) -> JsValue {
    args.first().copied().unwrap_or_else(JsValue::undefined)
}

/// `Object.defineProperty(obj, key, descriptor)` — define a property.
///
/// Structural placeholder — returns the object. Full descriptor support
/// requires the runtime property model.
pub fn object_define_property(args: &[JsValue]) -> JsValue {
    args.first().copied().unwrap_or_else(JsValue::undefined)
}

/// `Object.getOwnPropertyDescriptor(obj, key)` — read a property descriptor.
///
/// Returns `undefined` — full descriptor support requires runtime.
pub fn object_get_own_property_descriptor(_args: &[JsValue]) -> JsValue {
    JsValue::undefined()
}

/// `Object.freeze(obj)` — make an object immutable.
///
/// Sets the frozen flag on the object's header. Returns the object.
pub fn object_freeze(args: &[JsValue]) -> JsValue {
    let obj = args.first().copied().unwrap_or_else(JsValue::undefined);
    if !obj.is_object() {
        return obj;
    }
    let tag = read_obj_tag(obj.raw_bits());
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(obj.raw_bits())
        };
        if let Some(u) = uni {
            u.freeze();
        }
    }
    obj
}

/// `Object.seal(obj)` — prevent new properties from being added.
///
/// Sets the sealed flag on the object's header. Returns the object.
pub fn object_seal(args: &[JsValue]) -> JsValue {
    let obj = args.first().copied().unwrap_or_else(JsValue::undefined);
    if !obj.is_object() {
        return obj;
    }
    let tag = read_obj_tag(obj.raw_bits());
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged_mut::<UnifiedObject>(obj.raw_bits())
        };
        if let Some(u) = uni {
            u.seal();
        }
    }
    obj
}

/// `Object.isFrozen(obj)` — check if an object is frozen.
pub fn object_is_frozen(args: &[JsValue]) -> JsValue {
    let obj = args.first().copied().unwrap_or_else(JsValue::undefined);
    if !obj.is_object() {
        return JsValue::bool(true);
    }
    let tag = read_obj_tag(obj.raw_bits());
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj.raw_bits())
        };
        JsValue::bool(uni.is_some_and(|u| u.is_frozen()))
    } else {
        // Non-objects are vacuously frozen per spec
        JsValue::bool(true)
    }
}

/// `Object.isSealed(obj)` — check if an object is sealed.
pub fn object_is_sealed(args: &[JsValue]) -> JsValue {
    let obj = args.first().copied().unwrap_or_else(JsValue::undefined);
    if !obj.is_object() {
        return JsValue::bool(true);
    }
    let tag = read_obj_tag(obj.raw_bits());
    if tag == Some(ObjTag::Unified as u8) {
        let uni = unsafe {
            // SAFETY: tag check confirms this is a unified object.
            deref_tagged::<UnifiedObject>(obj.raw_bits())
        };
        JsValue::bool(uni.is_some_and(|u| u.is_sealed() || u.is_frozen()))
    } else {
        JsValue::bool(true)
    }
}

/// `Object.getPrototypeOf(obj)` — get the prototype of an object.
///
/// Returns `null` — full prototype chain requires runtime support.
pub fn object_get_prototype_of(_args: &[JsValue]) -> JsValue {
    JsValue::null()
}

/// `Object.setPrototypeOf(obj, proto)` — set the prototype of an object.
///
/// Structural placeholder — returns the object. Full prototype chain
/// wiring requires the runtime.
pub fn object_set_prototype_of(args: &[JsValue]) -> JsValue {
    args.first().copied().unwrap_or_else(JsValue::undefined)
}

/// `Object.hasOwn(obj, key)` — ES2022 replacement for `hasOwnProperty`.
///
/// Structural placeholder — returns `false`. Full implementation requires
/// runtime property lookup.
pub fn object_has_own(_args: &[JsValue]) -> JsValue {
    JsValue::bool(false)
}
