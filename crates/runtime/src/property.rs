//! Property access: get/set/has/delete on JsObject via shape lookup.

use interner::Interner;
use nanbox::JsValue;
use shapes::ShapeTable;
use thiserror::Error;

use crate::object::{JsObject, PropertyStorage};

/// Errors that can occur during property mutation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PropertyError {
    /// The object is not extensible and the property does not exist.
    #[error("object is not extensible")]
    NotExtensible,
    /// The object is frozen; no modifications are allowed.
    #[error("object is frozen")]
    Frozen,
    /// The object is sealed; cannot add or delete properties.
    #[error("object is sealed")]
    Sealed,
    /// The property is not writable.
    #[error("property is not writable")]
    NotWritable,
    /// The property is not configurable and cannot be redefined.
    #[error("property is not configurable")]
    NotConfigurable,
    /// Invalid property descriptor: accessor and data fields cannot be mixed.
    #[error(
        "Invalid property descriptor. Cannot both specify accessors and a value or writable attribute"
    )]
    MixedDescriptor,
    /// The accessor has no setter (strict mode assignment).
    #[error("Cannot set property which has only a getter")]
    NoSetter,
    /// Invalid array length (RangeError).
    #[error("Invalid array length")]
    InvalidArrayLength,
}

/// Gets a property value from an object's own properties by name.
///
/// Looks up the property in the shape table, then reads from storage.
/// Returns `None` if the property does not exist on this object.
pub fn get_property(
    obj: &JsObject,
    name: &str,
    shapes: &ShapeTable,
    interner: &Interner,
) -> Option<JsValue> {
    let atom = interner.intern(name);
    let desc = shapes.lookup(obj.shape_id, atom)?;
    match &obj.storage {
        PropertyStorage::Inline(slots) => slots.get(desc.offset as usize).copied(),
        PropertyStorage::Dictionary(entries) => {
            entries.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
        }
    }
}

/// Sets a property on an object.
///
/// If the property already exists, updates its value in place.
/// If the property does not exist, transitions the shape and adds a new slot.
///
/// Returns an error if the object is frozen, or if the property is new and the
/// object is sealed or non-extensible.
pub fn set_property(
    obj: &mut JsObject,
    name: &str,
    value: JsValue,
    shapes: &mut ShapeTable,
    interner: &Interner,
) -> Result<(), PropertyError> {
    if obj.is_frozen() {
        return Err(PropertyError::Frozen);
    }

    let atom = interner.intern(name);

    // Check if property already exists
    if let Some(desc) = shapes.lookup(obj.shape_id, atom) {
        // Enforce writable check
        if !desc.writable {
            return Err(PropertyError::NotWritable);
        }
        let offset = desc.offset as usize;
        match &mut obj.storage {
            PropertyStorage::Inline(slots) => {
                if offset < slots.len() {
                    slots[offset] = value;
                }
            }
            PropertyStorage::Dictionary(entries) => {
                if let Some(entry) = entries.iter_mut().find(|(k, _)| k == name) {
                    entry.1 = value;
                }
            }
        }
        return Ok(());
    }

    // New property — check extensibility
    if !obj.is_extensible() {
        return Err(PropertyError::NotExtensible);
    }
    if obj.is_sealed() {
        return Err(PropertyError::Sealed);
    }

    // Transition shape and add slot
    let new_shape = shapes.add_property(obj.shape_id, atom);
    obj.shape_id = new_shape;

    match &mut obj.storage {
        PropertyStorage::Inline(slots) => {
            slots.push(value);
        }
        PropertyStorage::Dictionary(entries) => {
            entries.push((name.to_string(), value));
        }
    }

    Ok(())
}

/// Returns `true` if the object has its own property with the given name.
pub fn has_own_property(
    obj: &JsObject,
    name: &str,
    shapes: &ShapeTable,
    interner: &Interner,
) -> bool {
    let atom = interner.intern(name);
    shapes.lookup(obj.shape_id, atom).is_some()
}

/// Returns the string property keys of an object.
///
/// Only returns `PropertyKey::String` keys; symbol and private keys are excluded.
pub fn property_keys(obj: &JsObject, shapes: &ShapeTable, interner: &Interner) -> Vec<String> {
    match &obj.storage {
        PropertyStorage::Inline(_slots) => {
            if let Some(shape) = shapes.get(obj.shape_id) {
                let mut keys: Vec<(String, u32)> = shape
                    .properties
                    .iter()
                    .filter_map(|(key, desc)| {
                        key.as_string()
                            .map(|atom| (interner.resolve(*atom).to_string(), desc.offset))
                    })
                    .collect();
                // Sort by insertion order (offset)
                keys.sort_by_key(|(_name, offset)| *offset);
                keys.into_iter().map(|(name, _)| name).collect()
            } else {
                Vec::new()
            }
        }
        PropertyStorage::Dictionary(entries) => entries.iter().map(|(k, _)| k.clone()).collect(),
    }
}

/// Returns only the enumerable string property keys of an object.
///
/// Unlike [`property_keys`], this filters out properties whose `enumerable`
/// flag is `false` in their descriptor. Also excludes symbol and private keys.
/// Used by `Object.keys`.
pub fn enumerable_property_keys(
    obj: &JsObject,
    shapes: &ShapeTable,
    interner: &Interner,
) -> Vec<String> {
    match &obj.storage {
        PropertyStorage::Inline(_slots) => {
            if let Some(shape) = shapes.get(obj.shape_id) {
                let mut keys: Vec<(String, u32)> = shape
                    .properties
                    .iter()
                    .filter(|(key, desc)| desc.enumerable && key.is_string())
                    .filter_map(|(key, desc)| {
                        key.as_string()
                            .map(|atom| (interner.resolve(*atom).to_string(), desc.offset))
                    })
                    .collect();
                keys.sort_by_key(|(_name, offset)| *offset);
                keys.into_iter().map(|(name, _)| name).collect()
            } else {
                Vec::new()
            }
        }
        PropertyStorage::Dictionary(entries) => entries.iter().map(|(k, _)| k.clone()).collect(),
    }
}

/// Options for [`define_property`], bundling the optional descriptor fields.
///
/// If `getter` or `setter` is `Some`, the descriptor is treated as an accessor.
/// If `value` or `writable` is `Some`, the descriptor is treated as data.
/// Setting both accessor and data fields is an error (TypeError per spec).
#[derive(Debug, Clone, Default)]
pub struct DefinePropertyOptions {
    /// The property value. `None` means keep existing (or `undefined` for new).
    pub value: Option<JsValue>,
    /// Writable flag. `None` means keep existing (or `false` for new per spec).
    pub writable: Option<bool>,
    /// Enumerable flag. `None` means keep existing (or `false` for new per spec).
    pub enumerable: Option<bool>,
    /// Configurable flag. `None` means keep existing (or `false` for new per spec).
    pub configurable: Option<bool>,
    /// Getter function for an accessor descriptor.
    pub getter: Option<JsValue>,
    /// Setter function for an accessor descriptor.
    pub setter: Option<JsValue>,
}

impl DefinePropertyOptions {
    /// Returns `true` if this descriptor specifies accessor fields (get/set).
    pub fn is_accessor_descriptor(&self) -> bool {
        self.getter.is_some() || self.setter.is_some()
    }

    /// Returns `true` if this descriptor specifies data fields (value/writable).
    pub fn is_data_descriptor(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// Returns `true` if this descriptor mixes accessor and data fields,
    /// which is invalid per the ECMAScript spec.
    pub fn is_invalid_mixed(&self) -> bool {
        self.is_accessor_descriptor() && self.is_data_descriptor()
    }

    /// Returns `true` if no fields are specified in the descriptor.
    ///
    /// Per ES spec §10.1.6.3 step 2: if `Desc` has no fields, `ValidateAndApply…`
    /// always succeeds without changing anything. Callers can short-circuit on this.
    pub fn is_empty(&self) -> bool {
        self.value.is_none()
            && self.writable.is_none()
            && self.enumerable.is_none()
            && self.configurable.is_none()
            && self.getter.is_none()
            && self.setter.is_none()
    }
}

/// Defines a property on an object with specific descriptor flags.
///
/// If the property already exists and is configurable, updates its value and flags.
/// If the property does not exist, adds it with the specified descriptor.
/// Returns an error if the property exists but is not configurable and cannot
/// be redefined, or if the object is not extensible and the property is new.
pub fn define_property(
    obj: &mut JsObject,
    name: &str,
    opts: &DefinePropertyOptions,
    shapes: &mut ShapeTable,
    interner: &Interner,
) -> Result<(), PropertyError> {
    // Mixed accessor+data descriptor is invalid per spec
    if opts.is_invalid_mixed() {
        return Err(PropertyError::MixedDescriptor);
    }

    let atom = interner.intern(name);

    // Check if property already exists
    if let Some(existing) = shapes.lookup(obj.shape_id, atom) {
        let is_accessor_desc = opts.is_accessor_descriptor();
        let existing_is_accessor = existing.is_accessor();

        // If not configurable, restrict what can be changed
        if !existing.configurable {
            let new_configurable = opts.configurable.unwrap_or(existing.configurable);
            let new_enumerable = opts.enumerable.unwrap_or(existing.enumerable);

            // Cannot make non-configurable property configurable
            if new_configurable {
                return Err(PropertyError::NotConfigurable);
            }
            // Cannot change enumerable on non-configurable property
            if new_enumerable != existing.enumerable {
                return Err(PropertyError::NotConfigurable);
            }
            // Cannot change kind on non-configurable property
            if is_accessor_desc && !existing_is_accessor {
                return Err(PropertyError::NotConfigurable);
            }
            if !is_accessor_desc && existing_is_accessor && opts.is_data_descriptor() {
                return Err(PropertyError::NotConfigurable);
            }
            // For non-configurable data: cannot make non-writable -> writable
            if !existing_is_accessor {
                let new_writable = opts.writable.unwrap_or(existing.writable);
                if !existing.writable && new_writable {
                    return Err(PropertyError::NotConfigurable);
                }
                // If non-writable and non-configurable, check value via SameValue
                if !existing.writable {
                    if let Some(new_val) = opts.value {
                        let old_val = match &obj.storage {
                            PropertyStorage::Inline(slots) => slots
                                .get(existing.offset as usize)
                                .copied()
                                .unwrap_or(JsValue::undefined()),
                            PropertyStorage::Dictionary(entries) => entries
                                .iter()
                                .find(|(k, _)| k == name)
                                .map(|(_, v)| *v)
                                .unwrap_or(JsValue::undefined()),
                        };
                        if !crate::value_ops::same_value(new_val, old_val) {
                            return Err(PropertyError::NotWritable);
                        }
                    }
                    return Ok(());
                }
            }
            // For non-configurable accessor: cannot change getter or setter
            if existing_is_accessor {
                let offset = existing.offset as usize;
                if let Some(new_getter) = opts.getter {
                    let old_getter = match &obj.storage {
                        PropertyStorage::Inline(slots) => {
                            slots.get(offset).copied().unwrap_or(JsValue::undefined())
                        }
                        PropertyStorage::Dictionary(_) => JsValue::undefined(),
                    };
                    if !crate::value_ops::same_value(new_getter, old_getter) {
                        return Err(PropertyError::NotConfigurable);
                    }
                }
                if let Some(new_setter) = opts.setter {
                    let old_setter = match &obj.storage {
                        PropertyStorage::Inline(slots) => slots
                            .get(offset + 1)
                            .copied()
                            .unwrap_or(JsValue::undefined()),
                        PropertyStorage::Dictionary(_) => JsValue::undefined(),
                    };
                    if !crate::value_ops::same_value(new_setter, old_setter) {
                        return Err(PropertyError::NotConfigurable);
                    }
                }
                return Ok(());
            }
        }

        let offset = existing.offset as usize;
        let w = opts.writable.unwrap_or(existing.writable);
        let e = opts.enumerable.unwrap_or(existing.enumerable);
        let c = opts.configurable.unwrap_or(existing.configurable);

        // Update the value in storage if provided
        if let Some(val) = opts.value {
            match &mut obj.storage {
                PropertyStorage::Inline(slots) => {
                    if offset < slots.len() {
                        slots[offset] = val;
                    }
                }
                PropertyStorage::Dictionary(entries) => {
                    if let Some(entry) = entries.iter_mut().find(|(k, _)| k == name) {
                        entry.1 = val;
                    }
                }
            }
        }

        // Transition shape to reflect new flags
        if let Some(new_shape) =
            shapes.update_property_flags(obj.shape_id, atom, Some(w), Some(e), Some(c))
        {
            obj.shape_id = new_shape;
        }

        return Ok(());
    }

    // New property — check extensibility
    if !obj.is_extensible() {
        return Err(PropertyError::NotExtensible);
    }
    if obj.is_sealed() {
        return Err(PropertyError::Sealed);
    }

    // Add with custom flags (defaults per spec: writable=false, enumerable=false,
    // configurable=false for defineProperty, but callers pass explicit values)
    let w = opts.writable.unwrap_or(false);
    let e = opts.enumerable.unwrap_or(false);
    let c = opts.configurable.unwrap_or(false);
    let val = opts.value.unwrap_or(JsValue::undefined());

    let new_shape = shapes.add_property_with_flags(obj.shape_id, atom, w, e, c);
    obj.shape_id = new_shape;

    match &mut obj.storage {
        PropertyStorage::Inline(slots) => {
            slots.push(val);
        }
        PropertyStorage::Dictionary(entries) => {
            entries.push((name.to_string(), val));
        }
    }

    Ok(())
}

/// Descriptor information returned by [`get_own_property_descriptor`].
///
/// May represent either a data descriptor (with value+writable) or an
/// accessor descriptor (with getter+setter).
#[derive(Debug, Clone)]
pub enum OwnPropertyDescriptor {
    /// A data property descriptor.
    Data {
        /// The property value.
        value: JsValue,
        /// Whether the property value can be changed.
        writable: bool,
        /// Whether the property shows up in for-in / Object.keys.
        enumerable: bool,
        /// Whether the property can be deleted or its descriptor changed.
        configurable: bool,
    },
    /// An accessor property descriptor.
    Accessor {
        /// The getter function, or `undefined` if no getter.
        getter: JsValue,
        /// The setter function, or `undefined` if no setter.
        setter: JsValue,
        /// Whether the property shows up in for-in / Object.keys.
        enumerable: bool,
        /// Whether the property can be deleted or its descriptor changed.
        configurable: bool,
    },
}

impl OwnPropertyDescriptor {
    /// Returns `true` if this is a data descriptor.
    pub fn is_data(&self) -> bool {
        matches!(self, Self::Data { .. })
    }

    /// Returns `true` if this is an accessor descriptor.
    pub fn is_accessor(&self) -> bool {
        matches!(self, Self::Accessor { .. })
    }

    /// Returns the enumerable flag.
    pub fn is_enumerable(&self) -> bool {
        match self {
            Self::Data { enumerable, .. } | Self::Accessor { enumerable, .. } => *enumerable,
        }
    }

    /// Returns the configurable flag.
    pub fn is_configurable(&self) -> bool {
        match self {
            Self::Data { configurable, .. } | Self::Accessor { configurable, .. } => *configurable,
        }
    }
}

/// Returns the property descriptor for an own property, or `None` if the
/// property does not exist on the object.
pub fn get_own_property_descriptor(
    obj: &JsObject,
    name: &str,
    shapes: &ShapeTable,
    interner: &Interner,
) -> Option<OwnPropertyDescriptor> {
    let atom = interner.intern(name);
    let desc = shapes.lookup(obj.shape_id, atom)?;

    if desc.is_accessor() {
        // Accessor property: slots[offset] = getter, slots[offset+1] = setter
        let getter = match &obj.storage {
            PropertyStorage::Inline(slots) => slots
                .get(desc.offset as usize)
                .copied()
                .unwrap_or(JsValue::undefined()),
            PropertyStorage::Dictionary(_) => JsValue::undefined(),
        };
        let setter = match &obj.storage {
            PropertyStorage::Inline(slots) => slots
                .get(desc.offset as usize + 1)
                .copied()
                .unwrap_or(JsValue::undefined()),
            PropertyStorage::Dictionary(_) => JsValue::undefined(),
        };
        Some(OwnPropertyDescriptor::Accessor {
            getter,
            setter,
            enumerable: desc.enumerable,
            configurable: desc.configurable,
        })
    } else {
        let value = match &obj.storage {
            PropertyStorage::Inline(slots) => slots
                .get(desc.offset as usize)
                .copied()
                .unwrap_or(JsValue::undefined()),
            PropertyStorage::Dictionary(entries) => entries
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| *v)
                .unwrap_or(JsValue::undefined()),
        };
        Some(OwnPropertyDescriptor::Data {
            value,
            writable: desc.writable,
            enumerable: desc.enumerable,
            configurable: desc.configurable,
        })
    }
}

/// Deletes a property from an object (sloppy-mode semantics).
///
/// Returns `true` if the property existed and was deleted, `false` if it did
/// not exist or if the property is non-configurable (per ES2024 section 10.1.10
/// `[[Delete]]`). For frozen/sealed objects, deletion always returns `false`.
///
/// In strict mode, the caller should throw a TypeError when this returns `false`
/// for a non-configurable property. This function does not throw directly.
pub fn delete_property(
    obj: &mut JsObject,
    name: &str,
    shapes: &ShapeTable,
    interner: &Interner,
) -> bool {
    if obj.is_frozen() || obj.is_sealed() {
        return false;
    }

    let atom = interner.intern(name);

    // Check configurable flag — non-configurable properties cannot be deleted
    if let Some(desc) = shapes.lookup(obj.shape_id, atom) {
        if !desc.configurable {
            return false;
        }
    } else {
        return false;
    }

    // For inline storage, we convert to dictionary mode on delete since
    // shape transitions don't support removal.
    match &mut obj.storage {
        PropertyStorage::Inline(slots) => {
            // Rebuild as dictionary, minus the deleted property
            let Some(shape) = shapes.get(obj.shape_id) else {
                // Shape must exist since lookup above found a descriptor for this shape_id.
                // This indicates a bug in shape table management.
                unreachable!(
                    "BUG: shape_id {:?} not found in shape table after successful lookup",
                    obj.shape_id
                );
            };
            let mut entries = Vec::new();
            let string_key = shapes::PropertyKey::String(atom);
            for (prop_key, desc) in &shape.properties {
                if *prop_key != string_key
                    && let Some(&val) = slots.get(desc.offset as usize)
                    && let Some(prop_atom) = prop_key.as_string()
                {
                    entries.push((interner.resolve(*prop_atom).to_string(), val));
                }
            }
            obj.storage = PropertyStorage::Dictionary(entries);
        }
        PropertyStorage::Dictionary(entries) => {
            entries.retain(|(k, _)| k != name);
        }
    }

    true
}
