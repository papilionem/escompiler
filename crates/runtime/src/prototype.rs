//! Prototype chain walking for property lookup.

use interner::Interner;
use nanbox::JsValue;
use shapes::ShapeTable;

use crate::object::JsObject;
use crate::property;

/// Walks the prototype chain looking for a property.
///
/// Checks own properties first, then each prototype in turn.
/// Returns `None` if the property is not found anywhere in the chain.
pub fn prototype_lookup(
    obj: &JsObject,
    name: &str,
    shapes: &ShapeTable,
    interner: &Interner,
) -> Option<JsValue> {
    // Check own properties first
    if let Some(val) = property::get_property(obj, name, shapes, interner) {
        return Some(val);
    }

    // Walk prototype chain
    let mut current = obj.prototype.as_deref();
    while let Some(proto) = current {
        if let Some(val) = property::get_property(proto, name, shapes, interner) {
            return Some(val);
        }
        current = proto.prototype.as_deref();
    }
    None
}

/// Returns `true` if the property exists anywhere in the prototype chain.
pub fn has_property(obj: &JsObject, name: &str, shapes: &ShapeTable, interner: &Interner) -> bool {
    prototype_lookup(obj, name, shapes, interner).is_some()
}
