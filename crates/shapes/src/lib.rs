//! Shape system, transitions, property descriptors (hidden classes).
//!
//! Property keys support three variants: interned string atoms (most common),
//! symbol keys (ES6+ `Symbol`), and private field/method keys (class `#field`).
//! The [`PropertyKey`] enum unifies these so that shapes can track all three
//! kinds of properties in a single transition tree.

use std::fmt;

use interner::Atom;

/// A property key in the shape system.
///
/// Supports string keys (most common), symbol keys (ES6+), and private
/// field keys (class private fields). Symbol and Private variants use
/// globally unique `u32` identifiers assigned at compile time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    /// Regular string property key (interned atom).
    String(Atom),
    /// Symbol property key (unique ID, e.g., `Symbol.iterator`).
    Symbol(u32),
    /// Private field/method key (unique ID per `#field` per class).
    Private(u32),
}

impl PropertyKey {
    /// Returns `true` if this is a regular string key.
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    /// Returns `true` if this is a symbol key.
    pub fn is_symbol(&self) -> bool {
        matches!(self, Self::Symbol(_))
    }

    /// Returns `true` if this is a private field/method key.
    pub fn is_private(&self) -> bool {
        matches!(self, Self::Private(_))
    }

    /// Returns the string atom if this is a `String` key, `None` otherwise.
    pub fn as_string(&self) -> Option<&Atom> {
        match self {
            Self::String(atom) => Some(atom),
            _ => None,
        }
    }

    /// Returns `true` if this key should be visible in property enumeration
    /// (`Object.keys`, `for...in`). Symbols and Private keys are NOT
    /// enumerable by default in these APIs.
    pub fn is_enumerable_by_default(&self) -> bool {
        matches!(self, Self::String(_))
    }
}

impl From<Atom> for PropertyKey {
    fn from(atom: Atom) -> Self {
        Self::String(atom)
    }
}

impl fmt::Display for PropertyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(atom) => write!(f, "Atom({:?})", atom),
            Self::Symbol(id) => write!(f, "Symbol({id})"),
            Self::Private(id) => write!(f, "#private({id})"),
        }
    }
}

/// Unique identifier for a shape in the shape table.
///
/// Each shape represents a particular property layout (set of keys with
/// offsets and descriptor flags). Objects sharing the same shape can be
/// accessed uniformly without per-object property maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShapeId(pub u32);

/// Whether a property holds a data value or an accessor (getter/setter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    /// A normal data property that stores a value directly.
    Data,
    /// An accessor property backed by getter and/or setter functions.
    Accessor,
}

/// Metadata for a single property on a shape: its storage offset and
/// ECMAScript descriptor flags (`writable`, `enumerable`, `configurable`).
#[derive(Debug, Clone)]
pub struct PropertyDescriptor {
    /// Slot index in the object's inline property storage.
    ///
    /// For data properties, this is the index of the single value slot.
    /// For accessor properties, this is the index of the getter slot;
    /// the setter slot is at `offset + 1`.
    pub offset: u32,
    /// Whether the property value can be changed via assignment.
    ///
    /// Only meaningful for data properties; accessor properties ignore this
    /// field (accessors are "writable" via their setter, not this flag).
    pub writable: bool,
    /// Whether the property appears in `for-in` loops and `Object.keys`.
    pub enumerable: bool,
    /// Whether the property can be deleted or its descriptor modified.
    pub configurable: bool,
    /// Whether this is a data property or an accessor property.
    pub kind: PropertyKind,
}

impl PropertyDescriptor {
    /// Returns `true` if this is a data property.
    pub fn is_data(&self) -> bool {
        matches!(self.kind, PropertyKind::Data)
    }

    /// Returns `true` if this is an accessor property.
    pub fn is_accessor(&self) -> bool {
        matches!(self.kind, PropertyKind::Accessor)
    }

    /// Returns `true` if this property is enumerable.
    pub fn is_enumerable(&self) -> bool {
        self.enumerable
    }

    /// Returns `true` if this property is configurable.
    pub fn is_configurable(&self) -> bool {
        self.configurable
    }

    /// Create a default data descriptor: writable, enumerable, configurable (all true).
    pub fn default_data(offset: u32) -> Self {
        Self {
            offset,
            writable: true,
            enumerable: true,
            configurable: true,
            kind: PropertyKind::Data,
        }
    }

    /// Create a default accessor descriptor: enumerable, configurable (both true).
    ///
    /// The offset points to the getter slot; the setter slot is at `offset + 1`.
    pub fn default_accessor(offset: u32) -> Self {
        Self {
            offset,
            writable: false, // not meaningful for accessors
            enumerable: true,
            configurable: true,
            kind: PropertyKind::Accessor,
        }
    }

    /// Returns the number of slots this property occupies.
    ///
    /// Data properties use 1 slot. Accessor properties use 2 slots
    /// (getter at `offset`, setter at `offset + 1`).
    pub fn slot_count(&self) -> u32 {
        match self.kind {
            PropertyKind::Data => 1,
            PropertyKind::Accessor => 2,
        }
    }
}

/// The mode of a shape: either a transition-tree shape or a dictionary shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// Normal transition-tree mode: property additions create new child shapes.
    Transition,
    /// Dictionary mode: the object has left the transition tree (e.g. after
    /// property deletion) and manages properties independently.
    Dictionary,
}

/// An edge in the transition tree linking a parent shape to a child shape
/// via the addition of a specific property key.
#[derive(Debug, Clone)]
pub struct Transition {
    /// The property key that triggers this transition.
    pub key: PropertyKey,
    /// The child shape reached by adding this key to the parent.
    pub target: ShapeId,
}

/// A hidden class describing the property layout of one or more objects.
///
/// Each shape stores the full property list (keys + descriptors) and
/// outgoing transitions to child shapes. Shapes form a tree rooted at the
/// empty shape (`ShapeId(0)`).
#[derive(Debug, Clone)]
pub struct Shape {
    /// This shape's unique identifier.
    pub id: ShapeId,
    /// The parent shape from which this shape was derived, or `None` for the root.
    pub parent: Option<ShapeId>,
    /// All properties present on objects with this shape, in insertion order.
    pub properties: Vec<(PropertyKey, PropertyDescriptor)>,
    /// Outgoing transitions to child shapes (one per added key).
    pub transitions: Vec<Transition>,
    /// Whether this shape is in transition-tree mode or dictionary mode.
    pub kind: ShapeKind,
    /// The prototype shape reference for objects with this shape.
    ///
    /// `None` means the prototype has not been set on this shape (the object
    /// either uses the default prototype or has `null` as its prototype).
    /// This field is preparation for prototype-on-shape (step 0.3.2).
    pub prototype: Option<ShapeId>,
    /// Epoch counter for IC invalidation when prototypes change.
    ///
    /// Preparation for v0.4 prototype-chain IC optimization. Currently
    /// initialized to 0 and not modified by shape operations.
    pub epoch: u32,
}

impl Shape {
    /// Returns the number of properties in this shape.
    pub fn property_count(&self) -> usize {
        self.properties.len()
    }
}

/// Central registry of all shapes, indexed by [`ShapeId`].
///
/// The table is initialized with a single empty root shape (`ShapeId(0)`).
/// New shapes are created by adding properties via [`add_property`](Self::add_property)
/// or [`add_property_with_flags`](Self::add_property_with_flags), which return
/// the resulting child shape's ID.
pub struct ShapeTable {
    shapes: Vec<Shape>,
    next_id: u32,
}

impl ShapeTable {
    /// The ID of the root empty shape (no properties).
    pub const EMPTY_SHAPE: ShapeId = ShapeId(0);

    /// Create a new shape table containing only the root empty shape.
    pub fn new() -> Self {
        let root = Shape {
            id: ShapeId(0),
            parent: None,
            properties: Vec::new(),
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: None,
            epoch: 0,
        };
        Self {
            shapes: vec![root],
            next_id: 1,
        }
    }

    /// Returns the root (empty) shape ID.
    pub fn root(&self) -> ShapeId {
        ShapeId(0)
    }

    /// Returns a list of well-known shape names and their descriptions.
    pub fn well_known_shapes() -> Vec<(&'static str, &'static str)> {
        vec![
            ("empty", "Root empty shape (ShapeId 0)"),
            ("array", "Array shape with 'length'"),
            (
                "function",
                "Function shape with 'name', 'length', 'prototype'",
            ),
            ("error", "Error shape with 'message', 'name', 'stack'"),
        ]
    }

    /// Creates a shape for arrays with a "length" property.
    ///
    /// Per the ECMAScript spec, `Array.length` is writable but not enumerable
    /// and not configurable.
    pub fn create_array_shape(&mut self, interner: &interner::Interner) -> ShapeId {
        let length = interner.intern("length");
        // Array.length: writable=true, enumerable=false, configurable=false
        self.add_property_with_flags(Self::EMPTY_SHAPE, length, true, false, false)
    }

    /// Creates a shape for functions with "name", "length", and "prototype" properties.
    ///
    /// Per the ECMAScript spec:
    /// - `Function.name`: configurable=true, writable=false, enumerable=false
    /// - `Function.length`: configurable=true, writable=false, enumerable=false
    /// - `Function.prototype`: writable=true, enumerable=false, configurable=false
    pub fn create_function_shape(&mut self, interner: &interner::Interner) -> ShapeId {
        let name = interner.intern("name");
        let length = interner.intern("length");
        let prototype = interner.intern("prototype");

        // Function.name: writable=false, enumerable=false, configurable=true
        let s1 = self.add_property_with_flags(Self::EMPTY_SHAPE, name, false, false, true);
        // Function.length: writable=false, enumerable=false, configurable=true
        let s2 = self.add_property_with_flags(s1, length, false, false, true);
        // Function.prototype: writable=true, enumerable=false, configurable=false
        self.add_property_with_flags(s2, prototype, true, false, false)
    }

    /// Creates a shape for errors with "message", "name", and "stack" properties.
    ///
    /// Per the ECMAScript spec, all three error properties are writable and
    /// configurable but not enumerable.
    pub fn create_error_shape(&mut self, interner: &interner::Interner) -> ShapeId {
        let message = interner.intern("message");
        let name = interner.intern("name");
        let stack = interner.intern("stack");

        // Error.message: writable=true, enumerable=false, configurable=true
        let s1 = self.add_property_with_flags(Self::EMPTY_SHAPE, message, true, false, true);
        // Error.name: writable=true, enumerable=false, configurable=true
        let s2 = self.add_property_with_flags(s1, name, true, false, true);
        // Error.stack: writable=true, enumerable=false, configurable=true
        self.add_property_with_flags(s2, stack, true, false, true)
    }

    /// Compute the next available slot offset for a shape, accounting for
    /// accessor properties that occupy 2 slots.
    fn next_slot_offset(&self, shape: ShapeId) -> u32 {
        let s = &self.shapes[shape.0 as usize];
        s.properties
            .iter()
            .map(|(_, desc)| desc.offset + desc.slot_count())
            .max()
            .unwrap_or(0)
    }

    /// Add a property to a shape, returning the new shape ID via transition.
    pub fn add_property(&mut self, shape: ShapeId, key: Atom) -> ShapeId {
        self.add_property_key(shape, PropertyKey::String(key))
    }

    /// Add a property to a shape using a [`PropertyKey`], returning the new shape ID.
    ///
    /// This is the general form that supports string, symbol, and private keys.
    /// [`add_property`](Self::add_property) is a convenience wrapper for string keys.
    pub fn add_property_key(&mut self, shape: ShapeId, key: PropertyKey) -> ShapeId {
        // Check for existing transition
        if let Some(existing) = self.shapes[shape.0 as usize]
            .transitions
            .iter()
            .find(|t| t.key == key)
        {
            return existing.target;
        }

        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let offset = self.next_slot_offset(shape);

        let mut properties = self.shapes[shape.0 as usize].properties.clone();
        properties.push((
            key.clone(),
            PropertyDescriptor {
                offset,
                writable: true,
                enumerable: true,
                configurable: true,
                kind: PropertyKind::Data,
            },
        ));

        let parent_proto = self.shapes[shape.0 as usize].prototype;

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };

        self.shapes.push(new_shape);
        self.shapes[shape.0 as usize].transitions.push(Transition {
            key,
            target: new_id,
        });

        new_id
    }

    /// Add a property with explicit descriptor flags, returning the new shape ID.
    ///
    /// Unlike [`add_property`](Self::add_property), this does NOT reuse transitions
    /// because custom flags make shapes unique. A new shape is always created.
    pub fn add_property_with_flags(
        &mut self,
        shape: ShapeId,
        key: Atom,
        writable: bool,
        enumerable: bool,
        configurable: bool,
    ) -> ShapeId {
        self.add_property_key_with_flags(
            shape,
            PropertyKey::String(key),
            writable,
            enumerable,
            configurable,
        )
    }

    /// Add a property with a [`PropertyKey`] and explicit descriptor flags.
    ///
    /// This is the general form that supports string, symbol, and private keys.
    /// [`add_property_with_flags`](Self::add_property_with_flags) is a convenience
    /// wrapper for string keys.
    pub fn add_property_key_with_flags(
        &mut self,
        shape: ShapeId,
        key: PropertyKey,
        writable: bool,
        enumerable: bool,
        configurable: bool,
    ) -> ShapeId {
        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut properties = self.shapes[shape.0 as usize].properties.clone();
        let parent_proto = self.shapes[shape.0 as usize].prototype;

        // If the property already exists, update its flags in place
        if let Some(pos) = properties.iter().position(|(k, _)| *k == key) {
            properties[pos].1.writable = writable;
            properties[pos].1.enumerable = enumerable;
            properties[pos].1.configurable = configurable;

            let new_shape = Shape {
                id: new_id,
                parent: Some(shape),
                properties,
                transitions: Vec::new(),
                kind: ShapeKind::Transition,
                prototype: parent_proto,
                epoch: 0,
            };
            self.shapes.push(new_shape);
            return new_id;
        }

        let offset = self.next_slot_offset(shape);

        // New property with custom flags
        properties.push((
            key,
            PropertyDescriptor {
                offset,
                writable,
                enumerable,
                configurable,
                kind: PropertyKind::Data,
            },
        ));

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };

        self.shapes.push(new_shape);
        new_id
    }

    /// Add an accessor property to a shape, returning the new shape ID.
    ///
    /// Accessor properties occupy 2 consecutive slots: the getter at `offset`
    /// and the setter at `offset + 1`. If the property already exists, its
    /// kind is changed to accessor and its offset may be updated to accommodate
    /// the extra slot.
    pub fn add_property_as_accessor(
        &mut self,
        shape: ShapeId,
        key: Atom,
        enumerable: bool,
        configurable: bool,
    ) -> ShapeId {
        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut properties = self.shapes[shape.0 as usize].properties.clone();
        let parent_proto = self.shapes[shape.0 as usize].prototype;

        let prop_key = PropertyKey::String(key);

        // If the property already exists, convert it to accessor
        if let Some(pos) = properties.iter().position(|(k, _)| *k == prop_key) {
            // If already an accessor, just update flags
            if properties[pos].1.kind == PropertyKind::Accessor {
                properties[pos].1.enumerable = enumerable;
                properties[pos].1.configurable = configurable;
            } else {
                // Convert from data to accessor: need a new offset for 2 slots
                let offset = self.next_slot_offset_from_properties_excluding(&properties, pos);
                properties[pos].1 = PropertyDescriptor {
                    offset,
                    writable: false,
                    enumerable,
                    configurable,
                    kind: PropertyKind::Accessor,
                };
            }

            let new_shape = Shape {
                id: new_id,
                parent: Some(shape),
                properties,
                transitions: Vec::new(),
                kind: ShapeKind::Transition,
                prototype: parent_proto,
                epoch: 0,
            };
            self.shapes.push(new_shape);
            return new_id;
        }

        let offset = self.next_slot_offset(shape);

        properties.push((
            prop_key,
            PropertyDescriptor {
                offset,
                writable: false, // not meaningful for accessors
                enumerable,
                configurable,
                kind: PropertyKind::Accessor,
            },
        ));

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };

        self.shapes.push(new_shape);
        new_id
    }

    /// Compute the next slot offset from a properties list, excluding the
    /// property at index `exclude_idx` (used when converting a property's kind).
    fn next_slot_offset_from_properties_excluding(
        &self,
        properties: &[(PropertyKey, PropertyDescriptor)],
        exclude_idx: usize,
    ) -> u32 {
        properties
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != exclude_idx)
            .map(|(_, (_, desc))| desc.offset + desc.slot_count())
            .max()
            .unwrap_or(0)
    }

    /// Update the kind of an existing property on a shape (data <-> accessor).
    ///
    /// Creates a new shape with the modified kind and returns its ID.
    /// When converting to accessor, a new 2-slot offset is allocated.
    /// When converting to data, the existing offset is reused (only 1 slot needed).
    /// Returns `None` if the property does not exist.
    pub fn update_property_kind(
        &mut self,
        shape: ShapeId,
        key: Atom,
        new_kind: PropertyKind,
        enumerable: Option<bool>,
        configurable: Option<bool>,
    ) -> Option<ShapeId> {
        let prop_key = PropertyKey::String(key);
        let s = self.shapes.get(shape.0 as usize)?;
        let properties = &s.properties;
        let pos = properties.iter().position(|(k, _)| *k == prop_key)?;
        let parent_proto = s.prototype;
        let old_kind = properties[pos].1.kind;

        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut new_properties = properties.clone();

        // Update flags
        if let Some(e) = enumerable {
            new_properties[pos].1.enumerable = e;
        }
        if let Some(c) = configurable {
            new_properties[pos].1.configurable = c;
        }

        if old_kind != new_kind {
            // Compute offset before mutating desc (avoids borrow conflict)
            let new_offset = if new_kind == PropertyKind::Accessor {
                self.next_slot_offset_from_properties_excluding(&new_properties, pos)
            } else {
                new_properties[pos].1.offset
            };

            new_properties[pos].1.kind = new_kind;
            new_properties[pos].1.offset = new_offset;
            match new_kind {
                PropertyKind::Accessor => {
                    new_properties[pos].1.writable = false; // not meaningful
                }
                PropertyKind::Data => {
                    new_properties[pos].1.writable = false; // spec default
                }
            }
        }

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties: new_properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };
        self.shapes.push(new_shape);
        Some(new_id)
    }

    /// Update descriptor flags for an existing property on a shape.
    ///
    /// Creates a new shape with the modified flags and returns its ID.
    /// Returns `None` if the property does not exist on the shape.
    pub fn update_property_flags(
        &mut self,
        shape: ShapeId,
        key: Atom,
        writable: Option<bool>,
        enumerable: Option<bool>,
        configurable: Option<bool>,
    ) -> Option<ShapeId> {
        self.update_property_key_flags(
            shape,
            &PropertyKey::String(key),
            writable,
            enumerable,
            configurable,
        )
    }

    /// Update descriptor flags for an existing property identified by [`PropertyKey`].
    ///
    /// Creates a new shape with the modified flags and returns its ID.
    /// Returns `None` if the property does not exist on the shape.
    pub fn update_property_key_flags(
        &mut self,
        shape: ShapeId,
        key: &PropertyKey,
        writable: Option<bool>,
        enumerable: Option<bool>,
        configurable: Option<bool>,
    ) -> Option<ShapeId> {
        let s = self.shapes.get(shape.0 as usize)?;
        let properties = &s.properties;
        if !properties.iter().any(|(k, _)| k == key) {
            return None;
        }
        let parent_proto = s.prototype;

        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut new_properties = properties.clone();
        if let Some(pos) = new_properties.iter().position(|(k, _)| k == key) {
            if let Some(w) = writable {
                new_properties[pos].1.writable = w;
            }
            if let Some(e) = enumerable {
                new_properties[pos].1.enumerable = e;
            }
            if let Some(c) = configurable {
                new_properties[pos].1.configurable = c;
            }
        }

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties: new_properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };
        self.shapes.push(new_shape);
        Some(new_id)
    }

    /// Mark all properties on a shape as non-writable and non-configurable (for `Object.freeze`).
    ///
    /// Creates a new shape with all properties frozen and returns its ID.
    pub fn freeze_all_properties(&mut self, shape: ShapeId) -> Option<ShapeId> {
        let s = self.shapes.get(shape.0 as usize)?;
        let properties = &s.properties;
        if properties.is_empty() {
            return Some(shape);
        }
        let parent_proto = s.prototype;

        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut new_properties = properties.clone();
        for (_, desc) in &mut new_properties {
            // Only data properties have `writable`; accessor properties
            // are made non-configurable but `writable` is not meaningful.
            if desc.kind == PropertyKind::Data {
                desc.writable = false;
            }
            desc.configurable = false;
        }

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties: new_properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };
        self.shapes.push(new_shape);
        Some(new_id)
    }

    /// Mark all properties on a shape as non-configurable (for `Object.seal`).
    ///
    /// Creates a new shape with all properties sealed and returns its ID.
    pub fn seal_all_properties(&mut self, shape: ShapeId) -> Option<ShapeId> {
        let s = self.shapes.get(shape.0 as usize)?;
        let properties = &s.properties;
        if properties.is_empty() {
            return Some(shape);
        }
        let parent_proto = s.prototype;

        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let mut new_properties = properties.clone();
        for (_, desc) in &mut new_properties {
            desc.configurable = false;
        }

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape),
            properties: new_properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: parent_proto,
            epoch: 0,
        };
        self.shapes.push(new_shape);
        Some(new_id)
    }

    /// Set the prototype shape on an object's shape, creating a new child shape.
    ///
    /// The returned shape has the same properties as the input shape but records
    /// `proto_shape_id` as its prototype. This links the object into a prototype
    /// chain: the runtime uses `get_prototype` to find the prototype shape and
    /// then looks up the actual prototype object via the `PROTO_OBJECTS` registry.
    pub fn set_prototype(&mut self, shape_id: ShapeId, proto_shape_id: ShapeId) -> ShapeId {
        let new_id = ShapeId(self.next_id);
        self.next_id += 1;

        let properties = self
            .shapes
            .get(shape_id.0 as usize)
            .map(|s| s.properties.clone())
            .unwrap_or_default();

        let new_shape = Shape {
            id: new_id,
            parent: Some(shape_id),
            properties,
            transitions: Vec::new(),
            kind: ShapeKind::Transition,
            prototype: Some(proto_shape_id),
            epoch: 0,
        };
        self.shapes.push(new_shape);
        new_id
    }

    /// Get the prototype shape ID for a given shape, if one has been set.
    pub fn get_prototype(&self, shape_id: ShapeId) -> Option<ShapeId> {
        self.shapes.get(shape_id.0 as usize)?.prototype
    }

    /// Look up a property descriptor by interned string key on a shape.
    pub fn lookup(&self, shape: ShapeId, key: Atom) -> Option<&PropertyDescriptor> {
        self.lookup_key(shape, &PropertyKey::String(key))
    }

    /// Look up a property descriptor by [`PropertyKey`] on a shape.
    ///
    /// This is the general form that supports string, symbol, and private keys.
    /// [`lookup`](Self::lookup) is a convenience wrapper for string keys.
    pub fn lookup_key(&self, shape: ShapeId, key: &PropertyKey) -> Option<&PropertyDescriptor> {
        self.shapes
            .get(shape.0 as usize)?
            .properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, desc)| desc)
    }

    /// Returns the number of shapes in the table.
    pub fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// Gets a shape by its ID.
    pub fn get(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.get(id.0 as usize)
    }

    /// Converts a shape to dictionary mode.
    pub fn to_dictionary(&mut self, shape_id: ShapeId) {
        if let Some(shape) = self.shapes.get_mut(shape_id.0 as usize) {
            shape.kind = ShapeKind::Dictionary;
        }
    }
}

impl Default for ShapeTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interner::Interner;

    #[test]
    fn root_shape_exists_and_is_empty() {
        let table = ShapeTable::new();
        let root = table.get(ShapeTable::EMPTY_SHAPE).unwrap();
        assert_eq!(root.id, ShapeId(0));
        assert!(root.properties.is_empty());
        assert!(root.transitions.is_empty());
        assert_eq!(root.kind, ShapeKind::Transition);
        assert_eq!(root.parent, None);
    }

    #[test]
    fn add_property_creates_transition() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let new_shape = table.add_property(ShapeTable::EMPTY_SHAPE, key);

        assert_ne!(new_shape, ShapeTable::EMPTY_SHAPE);
        assert_eq!(table.shape_count(), 2);

        // Root should have a transition
        let root = table.get(ShapeTable::EMPTY_SHAPE).unwrap();
        assert_eq!(root.transitions.len(), 1);
        assert_eq!(root.transitions[0].key, PropertyKey::String(key));
        assert_eq!(root.transitions[0].target, new_shape);
    }

    #[test]
    fn transition_reuse() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let s2 = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert_eq!(s1, s2);
        assert_eq!(table.shape_count(), 2); // root + one new shape
    }

    #[test]
    fn property_lookup_through_chain() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s1, y);

        // Both properties should be found on s2
        let desc_x = table.lookup(s2, x).unwrap();
        assert_eq!(desc_x.offset, 0);
        let desc_y = table.lookup(s2, y).unwrap();
        assert_eq!(desc_y.offset, 1);

        // Only x on s1
        assert!(table.lookup(s1, x).is_some());
        assert!(table.lookup(s1, y).is_none());
    }

    #[test]
    fn well_known_array_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let array_shape = table.create_array_shape(&interner);

        let length = interner.intern("length");
        let desc = table.lookup(array_shape, length).unwrap();
        assert_eq!(desc.offset, 0);
        // Array.length: writable=true, enumerable=false, configurable=false
        assert!(desc.writable);
        assert!(!desc.enumerable);
        assert!(!desc.configurable);

        let shape = table.get(array_shape).unwrap();
        assert_eq!(shape.property_count(), 1);
    }

    #[test]
    fn well_known_function_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);

        let name = interner.intern("name");
        let length = interner.intern("length");
        let prototype = interner.intern("prototype");

        assert!(table.lookup(func_shape, name).is_some());
        assert!(table.lookup(func_shape, length).is_some());
        assert!(table.lookup(func_shape, prototype).is_some());

        let shape = table.get(func_shape).unwrap();
        assert_eq!(shape.property_count(), 3);

        // Check offsets are sequential
        assert_eq!(table.lookup(func_shape, name).unwrap().offset, 0);
        assert_eq!(table.lookup(func_shape, length).unwrap().offset, 1);
        assert_eq!(table.lookup(func_shape, prototype).unwrap().offset, 2);

        // Function.name: writable=false, enumerable=false, configurable=true
        let name_desc = table.lookup(func_shape, name).unwrap();
        assert!(!name_desc.writable);
        assert!(!name_desc.enumerable);
        assert!(name_desc.configurable);

        // Function.length: writable=false, enumerable=false, configurable=true
        let length_desc = table.lookup(func_shape, length).unwrap();
        assert!(!length_desc.writable);
        assert!(!length_desc.enumerable);
        assert!(length_desc.configurable);

        // Function.prototype: writable=true, enumerable=false, configurable=false
        let proto_desc = table.lookup(func_shape, prototype).unwrap();
        assert!(proto_desc.writable);
        assert!(!proto_desc.enumerable);
        assert!(!proto_desc.configurable);
    }

    #[test]
    fn well_known_error_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);

        let message = interner.intern("message");
        let name = interner.intern("name");
        let stack = interner.intern("stack");

        assert!(table.lookup(error_shape, message).is_some());
        assert!(table.lookup(error_shape, name).is_some());
        assert!(table.lookup(error_shape, stack).is_some());

        let shape = table.get(error_shape).unwrap();
        assert_eq!(shape.property_count(), 3);

        // All error properties: writable=true, enumerable=false, configurable=true
        let msg_desc = table.lookup(error_shape, message).unwrap();
        assert!(msg_desc.writable);
        assert!(!msg_desc.enumerable);
        assert!(msg_desc.configurable);

        let name_desc = table.lookup(error_shape, name).unwrap();
        assert!(name_desc.writable);
        assert!(!name_desc.enumerable);
        assert!(name_desc.configurable);

        let stack_desc = table.lookup(error_shape, stack).unwrap();
        assert!(stack_desc.writable);
        assert!(!stack_desc.enumerable);
        assert!(stack_desc.configurable);
    }

    #[test]
    fn dictionary_mode_stub() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);

        assert_eq!(table.get(s).unwrap().kind, ShapeKind::Transition);
        table.to_dictionary(s);
        assert_eq!(table.get(s).unwrap().kind, ShapeKind::Dictionary);
    }

    #[test]
    fn property_kind_default_is_data() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let desc = table.lookup(s, key).unwrap();
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn property_kind_accessor() {
        // Verify the enum variant exists and can be compared
        let accessor = PropertyKind::Accessor;
        let data = PropertyKind::Data;
        assert_ne!(accessor, data);
    }

    #[test]
    fn shape_count_increases() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        assert_eq!(table.shape_count(), 1); // root only

        let x = interner.intern("x");
        table.add_property(ShapeTable::EMPTY_SHAPE, x);
        assert_eq!(table.shape_count(), 2);

        let y = interner.intern("y");
        table.add_property(ShapeTable::EMPTY_SHAPE, y);
        assert_eq!(table.shape_count(), 3);
    }

    #[test]
    fn get_invalid_shape_returns_none() {
        let table = ShapeTable::new();
        assert!(table.get(ShapeId(999)).is_none());
    }

    #[test]
    fn lookup_on_empty_shape_returns_none() {
        let interner = Interner::new();
        let table = ShapeTable::new();
        let key = interner.intern("nonexistent");
        assert!(table.lookup(ShapeTable::EMPTY_SHAPE, key).is_none());
    }

    #[test]
    fn empty_shape_constant_matches_root() {
        let table = ShapeTable::new();
        assert_eq!(ShapeTable::EMPTY_SHAPE, table.root());
    }

    #[test]
    fn well_known_shapes_list() {
        let wk = ShapeTable::well_known_shapes();
        assert_eq!(wk.len(), 4);
        let names: Vec<&str> = wk.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"empty"));
        assert!(names.contains(&"array"));
        assert!(names.contains(&"function"));
        assert!(names.contains(&"error"));
    }

    #[test]
    fn transition_chain_parent_links() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s1, y);

        assert_eq!(table.get(s1).unwrap().parent, Some(ShapeTable::EMPTY_SHAPE));
        assert_eq!(table.get(s2).unwrap().parent, Some(s1));
    }

    #[test]
    fn default_creates_same_as_new() {
        let t1 = ShapeTable::new();
        let t2 = ShapeTable::default();
        assert_eq!(t1.shape_count(), t2.shape_count());
        assert_eq!(t1.root(), t2.root());
    }

    // =========================================================================
    // Deep transition chains
    // =========================================================================

    #[test]
    fn test_deep_transition_chain_10_properties() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let keys: Vec<Atom> = (0..10)
            .map(|i| interner.intern(&format!("prop_{i}")))
            .collect();

        let mut current = ShapeTable::EMPTY_SHAPE;
        for (i, &key) in keys.iter().enumerate() {
            current = table.add_property(current, key);
            let shape = table.get(current).unwrap();
            assert_eq!(shape.property_count(), i + 1);
        }

        // All 10 properties should be reachable on the final shape
        for (i, &key) in keys.iter().enumerate() {
            let desc = table.lookup(current, key).unwrap();
            assert_eq!(desc.offset, i as u32);
        }

        // 1 root + 10 intermediate shapes
        assert_eq!(table.shape_count(), 11);
    }

    #[test]
    fn test_deep_chain_parent_links_form_chain() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let a = interner.intern("a");
        let b = interner.intern("b");
        let c = interner.intern("c");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, a);
        let s2 = table.add_property(s1, b);
        let s3 = table.add_property(s2, c);

        assert_eq!(table.get(s3).unwrap().parent, Some(s2));
        assert_eq!(table.get(s2).unwrap().parent, Some(s1));
        assert_eq!(table.get(s1).unwrap().parent, Some(ShapeTable::EMPTY_SHAPE));
        assert_eq!(table.get(ShapeTable::EMPTY_SHAPE).unwrap().parent, None);
    }

    // =========================================================================
    // Branching transitions (same parent, different keys)
    // =========================================================================

    #[test]
    fn test_branching_from_same_parent() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");
        let z = interner.intern("z");

        // Three different transitions from root
        let sx = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let sy = table.add_property(ShapeTable::EMPTY_SHAPE, y);
        let sz = table.add_property(ShapeTable::EMPTY_SHAPE, z);

        // All should be different shapes
        assert_ne!(sx, sy);
        assert_ne!(sy, sz);
        assert_ne!(sx, sz);

        // Root should have 3 transitions
        let root = table.get(ShapeTable::EMPTY_SHAPE).unwrap();
        assert_eq!(root.transitions.len(), 3);

        // Each shape has only its own property
        assert!(table.lookup(sx, x).is_some());
        assert!(table.lookup(sx, y).is_none());
        assert!(table.lookup(sy, y).is_some());
        assert!(table.lookup(sy, x).is_none());
    }

    #[test]
    fn test_diamond_shape_pattern() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        // Two paths: root → x → y and root → y → x
        let sx = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let sxy = table.add_property(sx, y);

        let sy = table.add_property(ShapeTable::EMPTY_SHAPE, y);
        let syx = table.add_property(sy, x);

        // Both final shapes have 2 properties but are different shapes
        assert_ne!(sxy, syx);
        assert_eq!(table.get(sxy).unwrap().property_count(), 2);
        assert_eq!(table.get(syx).unwrap().property_count(), 2);

        // Property offsets differ due to insertion order
        let x_in_xy = table.lookup(sxy, x).unwrap().offset;
        let x_in_yx = table.lookup(syx, x).unwrap().offset;
        assert_ne!(x_in_xy, x_in_yx);
    }

    // =========================================================================
    // Dictionary mode
    // =========================================================================

    #[test]
    fn test_dictionary_mode_does_not_affect_lookups() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s, y);
        table.to_dictionary(s2);

        // Properties should still be accessible after dictionary conversion
        assert!(table.lookup(s2, x).is_some());
        assert!(table.lookup(s2, y).is_some());
    }

    #[test]
    fn test_dictionary_on_root_shape() {
        let mut table = ShapeTable::new();
        table.to_dictionary(ShapeTable::EMPTY_SHAPE);
        assert_eq!(
            table.get(ShapeTable::EMPTY_SHAPE).unwrap().kind,
            ShapeKind::Dictionary
        );
    }

    #[test]
    fn test_to_dictionary_invalid_id_no_panic() {
        let mut table = ShapeTable::new();
        // Should not panic on an invalid shape ID
        table.to_dictionary(ShapeId(999));
    }

    // =========================================================================
    // Large shape table
    // =========================================================================

    #[test]
    fn test_large_shape_table_100_properties() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();

        let mut current = ShapeTable::EMPTY_SHAPE;
        for i in 0..100 {
            let key = interner.intern(&format!("p{i}"));
            current = table.add_property(current, key);
        }

        assert_eq!(table.get(current).unwrap().property_count(), 100);
        assert_eq!(table.shape_count(), 101); // root + 100

        // Spot-check first and last
        let first_key = interner.intern("p0");
        let last_key = interner.intern("p99");
        assert_eq!(table.lookup(current, first_key).unwrap().offset, 0);
        assert_eq!(table.lookup(current, last_key).unwrap().offset, 99);
    }

    // =========================================================================
    // Transition reuse across branching
    // =========================================================================

    #[test]
    fn test_transition_reuse_after_branch() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        // Create root → x → y
        let sx = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let sxy = table.add_property(sx, y);

        // Adding y again to sx should reuse the same shape
        let sxy2 = table.add_property(sx, y);
        assert_eq!(sxy, sxy2);

        // Only root, sx, sxy — no duplicates
        assert_eq!(table.shape_count(), 3);
    }

    // =========================================================================
    // Well-known shapes share transitions
    // =========================================================================

    #[test]
    fn test_array_shape_differs_from_manual_add_property() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();

        let manual_length = interner.intern("length");
        // Manual add_property creates writable=true, enumerable=true, configurable=true
        let manual = table.add_property(ShapeTable::EMPTY_SHAPE, manual_length);
        // create_array_shape uses add_property_with_flags with non-default flags
        let array = table.create_array_shape(&interner);

        // They should be different shapes because the array shape has
        // enumerable=false, configurable=false on `length`
        assert_ne!(manual, array);

        // Verify the manual shape has default flags
        let manual_desc = table.lookup(manual, manual_length).unwrap();
        assert!(manual_desc.writable);
        assert!(manual_desc.enumerable);
        assert!(manual_desc.configurable);

        // Verify the array shape has spec-correct flags
        let array_desc = table.lookup(array, manual_length).unwrap();
        assert!(array_desc.writable);
        assert!(!array_desc.enumerable);
        assert!(!array_desc.configurable);
    }

    #[test]
    fn test_error_and_function_shapes_are_different() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);
        let func_shape = table.create_function_shape(&interner);

        // They share some property names (e.g., "name") but differ
        assert_ne!(error_shape, func_shape);
    }

    // =========================================================================
    // PropertyDescriptor defaults
    // =========================================================================

    #[test]
    fn test_property_descriptor_defaults() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("test");
        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let desc = table.lookup(s, key).unwrap();

        assert!(desc.writable);
        assert!(desc.enumerable);
        assert!(desc.configurable);
        assert_eq!(desc.kind, PropertyKind::Data);
        assert_eq!(desc.offset, 0);
    }

    // =========================================================================
    // Lookup miss on nonexistent key
    // =========================================================================

    #[test]
    fn test_lookup_nonexistent_key_on_populated_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");
        let missing = interner.intern("missing");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s, y);

        assert!(table.lookup(s2, missing).is_none());
    }

    #[test]
    fn test_lookup_on_invalid_shape_returns_none() {
        let interner = Interner::new();
        let table = ShapeTable::new();
        let key = interner.intern("x");
        assert!(table.lookup(ShapeId(999), key).is_none());
    }

    // =========================================================================
    // ShapeId equality
    // =========================================================================

    #[test]
    fn test_shape_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ShapeId(0));
        set.insert(ShapeId(1));
        set.insert(ShapeId(0));
        assert_eq!(set.len(), 2);
    }

    // =========================================================================
    // Per-property descriptor flags
    // =========================================================================

    #[test]
    fn test_add_property_with_flags_non_writable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, false, true, true);
        let desc = table.lookup(s, key).unwrap();
        assert!(!desc.writable);
        assert!(desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_add_property_with_flags_non_enumerable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, true, false, true);
        let desc = table.lookup(s, key).unwrap();
        assert!(desc.writable);
        assert!(!desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_add_property_with_flags_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, true, true, false);
        let desc = table.lookup(s, key).unwrap();
        assert!(desc.writable);
        assert!(desc.enumerable);
        assert!(!desc.configurable);
    }

    #[test]
    fn test_update_property_flags_writable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert!(table.lookup(s, key).unwrap().writable);

        let s2 = table
            .update_property_flags(s, key, Some(false), None, None)
            .unwrap();
        assert!(!table.lookup(s2, key).unwrap().writable);
        // Other flags unchanged
        assert!(table.lookup(s2, key).unwrap().enumerable);
        assert!(table.lookup(s2, key).unwrap().configurable);
    }

    #[test]
    fn test_update_property_flags_missing_key() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let missing = interner.intern("y");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert!(
            table
                .update_property_flags(s, missing, Some(false), None, None)
                .is_none()
        );
    }

    #[test]
    fn test_freeze_all_properties() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s1, y);

        let frozen = table.freeze_all_properties(s2).unwrap();
        let desc_x = table.lookup(frozen, x).unwrap();
        let desc_y = table.lookup(frozen, y).unwrap();

        assert!(!desc_x.writable);
        assert!(!desc_x.configurable);
        assert!(!desc_y.writable);
        assert!(!desc_y.configurable);
        // Enumerable is unchanged
        assert!(desc_x.enumerable);
        assert!(desc_y.enumerable);
    }

    #[test]
    fn test_seal_all_properties() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, x);
        let s2 = table.add_property(s1, y);

        let sealed = table.seal_all_properties(s2).unwrap();
        let desc_x = table.lookup(sealed, x).unwrap();
        let desc_y = table.lookup(sealed, y).unwrap();

        // Writable is unchanged (still true)
        assert!(desc_x.writable);
        assert!(desc_y.writable);
        // Configurable is set to false
        assert!(!desc_x.configurable);
        assert!(!desc_y.configurable);
    }

    #[test]
    fn test_freeze_empty_shape_returns_same() {
        let mut table = ShapeTable::new();
        let result = table
            .freeze_all_properties(ShapeTable::EMPTY_SHAPE)
            .unwrap();
        assert_eq!(result, ShapeTable::EMPTY_SHAPE);
    }

    #[test]
    fn test_seal_empty_shape_returns_same() {
        let mut table = ShapeTable::new();
        let result = table.seal_all_properties(ShapeTable::EMPTY_SHAPE).unwrap();
        assert_eq!(result, ShapeTable::EMPTY_SHAPE);
    }

    #[test]
    fn test_freeze_invalid_shape_returns_none() {
        let mut table = ShapeTable::new();
        assert!(table.freeze_all_properties(ShapeId(999)).is_none());
    }

    #[test]
    fn test_seal_invalid_shape_returns_none() {
        let mut table = ShapeTable::new();
        assert!(table.seal_all_properties(ShapeId(999)).is_none());
    }

    #[test]
    fn test_update_property_flags_all_three() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        // All flags initially true
        let desc = table.lookup(s, key).unwrap();
        assert!(desc.writable && desc.enumerable && desc.configurable);

        // Update all three to false
        let s2 = table
            .update_property_flags(s, key, Some(false), Some(false), Some(false))
            .unwrap();
        let desc2 = table.lookup(s2, key).unwrap();
        assert!(!desc2.writable);
        assert!(!desc2.enumerable);
        assert!(!desc2.configurable);
    }

    #[test]
    fn test_update_property_flags_invalid_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        // Invalid shape ID
        assert!(
            table
                .update_property_flags(ShapeId(999), key, Some(false), None, None)
                .is_none()
        );
    }

    #[test]
    fn test_add_property_with_flags_all_false() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, false, false, false);
        let desc = table.lookup(s, key).unwrap();
        assert!(!desc.writable);
        assert!(!desc.enumerable);
        assert!(!desc.configurable);
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_freeze_preserves_enumerable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        // Add a non-enumerable property
        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, true, false, true);
        let frozen = table.freeze_all_properties(s).unwrap();

        let desc = table.lookup(frozen, key).unwrap();
        // Freeze should not change enumerable
        assert!(!desc.enumerable);
        assert!(!desc.writable);
        assert!(!desc.configurable);
    }

    #[test]
    fn test_seal_preserves_writable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        // Add a non-writable property
        let s = table.add_property_with_flags(ShapeTable::EMPTY_SHAPE, key, false, true, true);
        let sealed = table.seal_all_properties(s).unwrap();

        let desc = table.lookup(sealed, key).unwrap();
        // Seal should not change writable
        assert!(!desc.writable);
        assert!(!desc.configurable);
    }

    #[test]
    fn test_add_property_with_flags_updates_existing() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        // First add normally
        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert!(table.lookup(s1, key).unwrap().writable);

        // Now add_property_with_flags on same key should update flags
        let s2 = table.add_property_with_flags(s1, key, false, false, false);
        let desc = table.lookup(s2, key).unwrap();
        assert!(!desc.writable);
        assert!(!desc.enumerable);
        assert!(!desc.configurable);
    }

    // =========================================================================
    // ECMAScript property descriptor flag tests for well-known shapes
    // =========================================================================

    #[test]
    fn test_function_name_is_non_writable_non_enumerable_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);
        let name = interner.intern("name");

        let desc = table.lookup(func_shape, name).unwrap();
        assert!(!desc.writable, "Function.name must be non-writable");
        assert!(!desc.enumerable, "Function.name must be non-enumerable");
        assert!(desc.configurable, "Function.name must be configurable");
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_function_length_is_non_writable_non_enumerable_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);
        let length = interner.intern("length");

        let desc = table.lookup(func_shape, length).unwrap();
        assert!(!desc.writable, "Function.length must be non-writable");
        assert!(!desc.enumerable, "Function.length must be non-enumerable");
        assert!(desc.configurable, "Function.length must be configurable");
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_function_prototype_is_writable_non_enumerable_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);
        let prototype = interner.intern("prototype");

        let desc = table.lookup(func_shape, prototype).unwrap();
        assert!(desc.writable, "Function.prototype must be writable");
        assert!(
            !desc.enumerable,
            "Function.prototype must be non-enumerable"
        );
        assert!(
            !desc.configurable,
            "Function.prototype must be non-configurable"
        );
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_array_length_is_writable_non_enumerable_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let array_shape = table.create_array_shape(&interner);
        let length = interner.intern("length");

        let desc = table.lookup(array_shape, length).unwrap();
        assert!(desc.writable, "Array.length must be writable");
        assert!(!desc.enumerable, "Array.length must be non-enumerable");
        assert!(!desc.configurable, "Array.length must be non-configurable");
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_error_message_is_writable_non_enumerable_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);
        let message = interner.intern("message");

        let desc = table.lookup(error_shape, message).unwrap();
        assert!(desc.writable, "Error.message must be writable");
        assert!(!desc.enumerable, "Error.message must be non-enumerable");
        assert!(desc.configurable, "Error.message must be configurable");
    }

    #[test]
    fn test_error_name_is_writable_non_enumerable_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);
        let name = interner.intern("name");

        let desc = table.lookup(error_shape, name).unwrap();
        assert!(desc.writable, "Error.name must be writable");
        assert!(!desc.enumerable, "Error.name must be non-enumerable");
        assert!(desc.configurable, "Error.name must be configurable");
    }

    #[test]
    fn test_error_stack_is_writable_non_enumerable_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);
        let stack = interner.intern("stack");

        let desc = table.lookup(error_shape, stack).unwrap();
        assert!(desc.writable, "Error.stack must be writable");
        assert!(!desc.enumerable, "Error.stack must be non-enumerable");
        assert!(desc.configurable, "Error.stack must be configurable");
    }

    #[test]
    fn test_freeze_function_shape_makes_all_non_writable_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);

        let frozen = table.freeze_all_properties(func_shape).unwrap();

        let name = interner.intern("name");
        let length = interner.intern("length");
        let prototype = interner.intern("prototype");

        // All properties should be non-writable, non-configurable after freeze
        for key in [name, length, prototype] {
            let desc = table.lookup(frozen, key).unwrap();
            assert!(!desc.writable, "frozen property must be non-writable");
            assert!(
                !desc.configurable,
                "frozen property must be non-configurable"
            );
            // enumerable should be preserved (all were false)
            assert!(
                !desc.enumerable,
                "frozen property should preserve non-enumerable"
            );
        }
    }

    #[test]
    fn test_seal_error_shape_makes_all_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let error_shape = table.create_error_shape(&interner);

        let sealed = table.seal_all_properties(error_shape).unwrap();

        let message = interner.intern("message");
        let name = interner.intern("name");
        let stack = interner.intern("stack");

        // All properties should be non-configurable after seal
        // but writable should be preserved (all were true)
        for key in [message, name, stack] {
            let desc = table.lookup(sealed, key).unwrap();
            assert!(
                !desc.configurable,
                "sealed property must be non-configurable"
            );
            assert!(desc.writable, "sealed property should preserve writable");
            assert!(
                !desc.enumerable,
                "sealed property should preserve non-enumerable"
            );
        }
    }

    #[test]
    fn test_well_known_shapes_are_all_data_kind() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();

        let func_shape = table.create_function_shape(&interner);
        let array_shape = table.create_array_shape(&interner);
        let error_shape = table.create_error_shape(&interner);

        // Verify all properties on well-known shapes are Data kind
        for shape_id in [func_shape, array_shape, error_shape] {
            let shape = table.get(shape_id).unwrap();
            for (_, desc) in &shape.properties {
                assert_eq!(
                    desc.kind,
                    PropertyKind::Data,
                    "all well-known shape properties should be Data kind"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // Prototype field tests (preparation for step 0.3.2)
    // -----------------------------------------------------------------

    #[test]
    fn test_shape_prototype_field_default_none() {
        let table = ShapeTable::new();
        let root = table.get(ShapeTable::EMPTY_SHAPE).unwrap();
        assert_eq!(root.prototype, None);
    }

    #[test]
    fn test_shape_add_property_preserves_prototype_none() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let child = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let shape = table.get(child).unwrap();
        assert_eq!(shape.prototype, None);
    }

    #[test]
    fn test_shape_well_known_shapes_prototype_none() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let func_shape = table.create_function_shape(&interner);
        let array_shape = table.create_array_shape(&interner);
        let error_shape = table.create_error_shape(&interner);

        assert_eq!(table.get(func_shape).unwrap().prototype, None);
        assert_eq!(table.get(array_shape).unwrap().prototype, None);
        assert_eq!(table.get(error_shape).unwrap().prototype, None);
    }

    #[test]
    fn test_set_prototype_creates_child_shape() {
        let mut table = ShapeTable::new();
        let proto_shape = ShapeId(0);
        let child_shape = table.set_prototype(ShapeId(0), proto_shape);
        assert_ne!(child_shape, ShapeId(0));
        assert_eq!(table.get_prototype(child_shape), Some(proto_shape));
    }

    #[test]
    fn test_get_prototype_none_on_root() {
        let table = ShapeTable::new();
        assert_eq!(table.get_prototype(ShapeId(0)), None);
    }

    #[test]
    fn test_prototype_propagated_through_add_property() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let proto_shape = ShapeId(0);
        let with_proto = table.set_prototype(ShapeId(0), proto_shape);
        let key = interner.intern("x");
        let with_prop = table.add_property(with_proto, key);
        assert_eq!(
            table.get_prototype(with_prop),
            Some(proto_shape),
            "prototype should propagate through add_property"
        );
    }

    #[test]
    fn test_prototype_propagated_through_add_property_with_flags() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let proto_shape = ShapeId(0);
        let with_proto = table.set_prototype(ShapeId(0), proto_shape);
        let key = interner.intern("y");
        let with_prop = table.add_property_with_flags(with_proto, key, false, true, false);
        assert_eq!(
            table.get_prototype(with_prop),
            Some(proto_shape),
            "prototype should propagate through add_property_with_flags"
        );
    }

    #[test]
    fn test_prototype_propagated_through_freeze() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let proto_shape = ShapeId(0);
        let with_proto = table.set_prototype(ShapeId(0), proto_shape);
        let key = interner.intern("z");
        let with_prop = table.add_property(with_proto, key);
        let frozen = table.freeze_all_properties(with_prop).unwrap();
        assert_eq!(
            table.get_prototype(frozen),
            Some(proto_shape),
            "prototype should propagate through freeze_all_properties"
        );
    }

    // =========================================================================
    // PropertyKey enum tests
    // =========================================================================

    #[test]
    fn test_property_key_string_equality() {
        let interner = Interner::new();
        let a = interner.intern("x");
        let b = interner.intern("x");
        assert_eq!(PropertyKey::String(a), PropertyKey::String(b));
    }

    #[test]
    fn test_property_key_string_vs_symbol_inequality() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert_ne!(PropertyKey::String(atom), PropertyKey::Symbol(0));
    }

    #[test]
    fn test_property_key_symbol_vs_private_inequality() {
        assert_ne!(PropertyKey::Symbol(0), PropertyKey::Private(0));
    }

    #[test]
    fn test_property_key_string_vs_private_inequality() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert_ne!(PropertyKey::String(atom), PropertyKey::Private(0));
    }

    #[test]
    fn test_property_key_symbol_equality_same_id() {
        assert_eq!(PropertyKey::Symbol(42), PropertyKey::Symbol(42));
    }

    #[test]
    fn test_property_key_symbol_inequality_diff_id() {
        assert_ne!(PropertyKey::Symbol(1), PropertyKey::Symbol(2));
    }

    #[test]
    fn test_property_key_private_equality_same_id() {
        assert_eq!(PropertyKey::Private(7), PropertyKey::Private(7));
    }

    #[test]
    fn test_property_key_private_inequality_diff_id() {
        assert_ne!(PropertyKey::Private(1), PropertyKey::Private(2));
    }

    #[test]
    fn test_property_key_hash_as_map_key() {
        use std::collections::HashMap;
        let interner = Interner::new();
        let atom = interner.intern("x");

        let mut map = HashMap::new();
        map.insert(PropertyKey::String(atom), 1);
        map.insert(PropertyKey::Symbol(0), 2);
        map.insert(PropertyKey::Private(0), 3);

        assert_eq!(map.len(), 3);
        assert_eq!(map[&PropertyKey::String(atom)], 1);
        assert_eq!(map[&PropertyKey::Symbol(0)], 2);
        assert_eq!(map[&PropertyKey::Private(0)], 3);
    }

    #[test]
    fn test_property_key_is_string() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert!(PropertyKey::String(atom).is_string());
        assert!(!PropertyKey::Symbol(0).is_string());
        assert!(!PropertyKey::Private(0).is_string());
    }

    #[test]
    fn test_property_key_is_symbol() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert!(!PropertyKey::String(atom).is_symbol());
        assert!(PropertyKey::Symbol(0).is_symbol());
        assert!(!PropertyKey::Private(0).is_symbol());
    }

    #[test]
    fn test_property_key_is_private() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert!(!PropertyKey::String(atom).is_private());
        assert!(!PropertyKey::Symbol(0).is_private());
        assert!(PropertyKey::Private(0).is_private());
    }

    #[test]
    fn test_property_key_as_string() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert_eq!(PropertyKey::String(atom).as_string(), Some(&atom));
        assert_eq!(PropertyKey::Symbol(0).as_string(), None);
        assert_eq!(PropertyKey::Private(0).as_string(), None);
    }

    #[test]
    fn test_property_key_is_enumerable_by_default() {
        let interner = Interner::new();
        let atom = interner.intern("x");
        assert!(PropertyKey::String(atom).is_enumerable_by_default());
        assert!(!PropertyKey::Symbol(0).is_enumerable_by_default());
        assert!(!PropertyKey::Private(0).is_enumerable_by_default());
    }

    #[test]
    fn test_property_key_from_atom() {
        let interner = Interner::new();
        let atom = interner.intern("hello");
        let key: PropertyKey = atom.into();
        assert_eq!(key, PropertyKey::String(atom));
    }

    #[test]
    fn test_property_key_display() {
        let sym = PropertyKey::Symbol(42);
        assert_eq!(format!("{sym}"), "Symbol(42)");

        let priv_key = PropertyKey::Private(7);
        assert_eq!(format!("{priv_key}"), "#private(7)");
    }

    // =========================================================================
    // Shape operations with PropertyKey variants
    // =========================================================================

    #[test]
    fn test_shape_with_string_key_preserved() {
        // Existing behavior: add_property with Atom works identically
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let desc = table.lookup(s, key).unwrap();
        assert_eq!(desc.offset, 0);
        assert!(desc.writable);
        assert!(desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_shape_transition_with_symbol_key() {
        let mut table = ShapeTable::new();
        let s = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(0));

        // lookup_key should find the symbol property
        let desc = table.lookup_key(s, &PropertyKey::Symbol(0)).unwrap();
        assert_eq!(desc.offset, 0);

        // lookup (Atom-based) should NOT find the symbol
        let interner = Interner::new();
        let unrelated = interner.intern("Symbol(0)");
        assert!(table.lookup(s, unrelated).is_none());
    }

    #[test]
    fn test_shape_transition_with_private_key() {
        let mut table = ShapeTable::new();
        let s = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Private(1));

        let desc = table.lookup_key(s, &PropertyKey::Private(1)).unwrap();
        assert_eq!(desc.offset, 0);

        // Different private ID should not match
        assert!(table.lookup_key(s, &PropertyKey::Private(2)).is_none());
    }

    #[test]
    fn test_mixed_key_types_on_same_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let str_key = interner.intern("x");

        // Add string, symbol, and private keys in sequence
        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, str_key);
        let s2 = table.add_property_key(s1, PropertyKey::Symbol(0));
        let s3 = table.add_property_key(s2, PropertyKey::Private(0));

        // All three should be findable
        assert_eq!(table.lookup(s3, str_key).unwrap().offset, 0);
        assert_eq!(
            table
                .lookup_key(s3, &PropertyKey::Symbol(0))
                .unwrap()
                .offset,
            1
        );
        assert_eq!(
            table
                .lookup_key(s3, &PropertyKey::Private(0))
                .unwrap()
                .offset,
            2
        );

        // Shape should have 3 properties
        assert_eq!(table.get(s3).unwrap().property_count(), 3);
    }

    #[test]
    fn test_symbol_key_transition_reuse() {
        let mut table = ShapeTable::new();
        let s1 = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(5));
        let s2 = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(5));
        assert_eq!(s1, s2, "symbol key transition should be reused");
    }

    #[test]
    fn test_private_key_transition_reuse() {
        let mut table = ShapeTable::new();
        let s1 = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Private(3));
        let s2 = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Private(3));
        assert_eq!(s1, s2, "private key transition should be reused");
    }

    #[test]
    fn test_string_and_symbol_same_shape_no_collision() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let atom = interner.intern("x");

        let s_str = table.add_property(ShapeTable::EMPTY_SHAPE, atom);
        let s_sym = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(0));

        // Should be different shapes
        assert_ne!(s_str, s_sym);
    }

    #[test]
    fn test_enumeration_skips_symbol_and_private_keys() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();

        let str_key = interner.intern("visible");
        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, str_key);
        let s2 = table.add_property_key(s1, PropertyKey::Symbol(0));
        let s3 = table.add_property_key(s2, PropertyKey::Private(0));

        // Only string keys should appear when filtering by is_string()
        let shape = table.get(s3).unwrap();
        let string_keys: Vec<&Atom> = shape
            .properties
            .iter()
            .filter_map(|(k, _)| k.as_string())
            .collect();
        assert_eq!(string_keys.len(), 1);
        assert_eq!(interner.resolve(*string_keys[0]), "visible");
    }

    #[test]
    fn test_add_property_key_with_flags_symbol() {
        let mut table = ShapeTable::new();
        let s = table.add_property_key_with_flags(
            ShapeTable::EMPTY_SHAPE,
            PropertyKey::Symbol(99),
            false,
            false,
            true,
        );
        let desc = table.lookup_key(s, &PropertyKey::Symbol(99)).unwrap();
        assert!(!desc.writable);
        assert!(!desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_update_property_key_flags_symbol() {
        let mut table = ShapeTable::new();
        let s = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(10));
        assert!(
            table
                .lookup_key(s, &PropertyKey::Symbol(10))
                .unwrap()
                .writable
        );

        let s2 = table
            .update_property_key_flags(s, &PropertyKey::Symbol(10), Some(false), None, None)
            .unwrap();
        assert!(
            !table
                .lookup_key(s2, &PropertyKey::Symbol(10))
                .unwrap()
                .writable
        );
    }

    #[test]
    fn test_update_property_key_flags_missing_key() {
        let mut table = ShapeTable::new();
        let s = table.add_property_key(ShapeTable::EMPTY_SHAPE, PropertyKey::Symbol(1));
        assert!(
            table
                .update_property_key_flags(s, &PropertyKey::Symbol(99), Some(false), None, None)
                .is_none()
        );
    }

    // =========================================================================
    // Accessor property tests
    // =========================================================================

    #[test]
    fn test_add_property_as_accessor_creates_new_shape() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, true, true);
        assert_ne!(s, ShapeTable::EMPTY_SHAPE);

        let desc = table.lookup(s, key).unwrap();
        assert_eq!(desc.kind, PropertyKind::Accessor);
        assert!(desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_accessor_property_occupies_two_slots() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, true, true);
        let desc = table.lookup(s, key).unwrap();
        assert_eq!(desc.slot_count(), 2);
        assert_eq!(desc.offset, 0); // getter at 0, setter at 1
    }

    #[test]
    fn test_data_property_occupies_one_slot() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        let desc = table.lookup(s, key).unwrap();
        assert_eq!(desc.slot_count(), 1);
    }

    #[test]
    fn test_accessor_then_data_offset_after_accessor() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let acc = interner.intern("acc");
        let data = interner.intern("data");

        let s1 = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, acc, true, true);
        let s2 = table.add_property(s1, data);

        let acc_desc = table.lookup(s2, acc).unwrap();
        let data_desc = table.lookup(s2, data).unwrap();
        // Accessor at offset 0 (slots 0+1), data should be at offset 2
        assert_eq!(acc_desc.offset, 0);
        assert_eq!(data_desc.offset, 2);
    }

    #[test]
    fn test_property_descriptor_is_data() {
        let desc = PropertyDescriptor::default_data(0);
        assert!(desc.is_data());
        assert!(!desc.is_accessor());
    }

    #[test]
    fn test_property_descriptor_is_accessor() {
        let desc = PropertyDescriptor::default_accessor(0);
        assert!(desc.is_accessor());
        assert!(!desc.is_data());
    }

    #[test]
    fn test_property_descriptor_default_data() {
        let desc = PropertyDescriptor::default_data(5);
        assert_eq!(desc.offset, 5);
        assert!(desc.writable);
        assert!(desc.is_enumerable());
        assert!(desc.is_configurable());
        assert_eq!(desc.kind, PropertyKind::Data);
    }

    #[test]
    fn test_property_descriptor_default_accessor() {
        let desc = PropertyDescriptor::default_accessor(3);
        assert_eq!(desc.offset, 3);
        assert!(desc.is_enumerable());
        assert!(desc.is_configurable());
        assert_eq!(desc.kind, PropertyKind::Accessor);
    }

    #[test]
    fn test_update_property_kind_data_to_accessor() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert_eq!(table.lookup(s1, key).unwrap().kind, PropertyKind::Data);

        let s2 = table
            .update_property_kind(s1, key, PropertyKind::Accessor, None, None)
            .unwrap();
        let desc = table.lookup(s2, key).unwrap();
        assert_eq!(desc.kind, PropertyKind::Accessor);
        assert_eq!(desc.slot_count(), 2);
    }

    #[test]
    fn test_update_property_kind_accessor_to_data() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s1 = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, true, true);
        assert_eq!(table.lookup(s1, key).unwrap().kind, PropertyKind::Accessor);

        let s2 = table
            .update_property_kind(s1, key, PropertyKind::Data, None, None)
            .unwrap();
        let desc = table.lookup(s2, key).unwrap();
        assert_eq!(desc.kind, PropertyKind::Data);
        assert_eq!(desc.slot_count(), 1);
    }

    #[test]
    fn test_update_property_kind_missing_key_returns_none() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");
        let missing = interner.intern("y");

        let s = table.add_property(ShapeTable::EMPTY_SHAPE, key);
        assert!(
            table
                .update_property_kind(s, missing, PropertyKind::Accessor, None, None)
                .is_none()
        );
    }

    #[test]
    fn test_freeze_preserves_accessor_kind() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("acc");

        let s = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, true, true);
        let frozen = table.freeze_all_properties(s).unwrap();

        let desc = table.lookup(frozen, key).unwrap();
        assert_eq!(
            desc.kind,
            PropertyKind::Accessor,
            "freeze should preserve accessor kind"
        );
        assert!(!desc.configurable, "freeze should set non-configurable");
    }

    #[test]
    fn test_accessor_non_enumerable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, false, true);
        let desc = table.lookup(s, key).unwrap();
        assert!(!desc.enumerable);
        assert!(desc.configurable);
    }

    #[test]
    fn test_accessor_non_configurable() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let key = interner.intern("x");

        let s = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, key, true, false);
        let desc = table.lookup(s, key).unwrap();
        assert!(desc.enumerable);
        assert!(!desc.configurable);
    }

    #[test]
    fn test_multiple_accessors_correct_offsets() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let a = interner.intern("a");
        let b = interner.intern("b");

        let s1 = table.add_property_as_accessor(ShapeTable::EMPTY_SHAPE, a, true, true);
        let s2 = table.add_property_as_accessor(s1, b, true, true);

        let desc_a = table.lookup(s2, a).unwrap();
        let desc_b = table.lookup(s2, b).unwrap();
        // First accessor at 0 (2 slots), second at 2 (2 slots)
        assert_eq!(desc_a.offset, 0);
        assert_eq!(desc_b.offset, 2);
    }

    #[test]
    fn test_mixed_data_and_accessor_correct_offsets() {
        let interner = Interner::new();
        let mut table = ShapeTable::new();
        let d1 = interner.intern("d1");
        let acc = interner.intern("acc");
        let d2 = interner.intern("d2");

        let s1 = table.add_property(ShapeTable::EMPTY_SHAPE, d1); // offset 0
        let s2 = table.add_property_as_accessor(s1, acc, true, true); // offset 1 (slots 1+2)
        let s3 = table.add_property(s2, d2); // offset 3

        assert_eq!(table.lookup(s3, d1).unwrap().offset, 0);
        assert_eq!(table.lookup(s3, acc).unwrap().offset, 1);
        assert_eq!(table.lookup(s3, d2).unwrap().offset, 3);
    }
}
