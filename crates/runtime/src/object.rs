//! JsObject: header + property storage + shape.

use nanbox::JsValue;
use shapes::ShapeId;

/// Flags controlling object mutability and extensibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectFlags(u32);

impl ObjectFlags {
    pub const NONE: u32 = 0;
    pub const FROZEN: u32 = 1;
    pub const SEALED: u32 = 2;
    pub const NON_EXTENSIBLE: u32 = 4;
}

/// Metadata header for every JsObject.
#[derive(Debug)]
pub struct ObjectHeader {
    /// Bitfield of `ObjectFlags` constants.
    pub flags: u32,
    /// Allocation class: 1=static zone, 2=dynamic zone, 3=heap.
    pub alloc_class: u8,
}

/// How properties are stored on an object.
#[derive(Debug)]
pub enum PropertyStorage {
    /// Shape-indexed inline storage (common case).
    Inline(Vec<JsValue>),
    /// Fallback for highly dynamic objects after many delete/add cycles.
    Dictionary(Vec<(String, JsValue)>),
}

/// A JavaScript object with header, shape, property storage, and prototype link.
#[derive(Debug)]
pub struct JsObject {
    pub header: ObjectHeader,
    pub shape_id: ShapeId,
    pub storage: PropertyStorage,
    pub prototype: Option<Box<JsObject>>,
}

impl JsObject {
    /// Creates a new object with empty inline storage.
    pub fn new(shape_id: ShapeId, alloc_class: u8) -> Self {
        Self {
            header: ObjectHeader {
                flags: ObjectFlags::NONE,
                alloc_class,
            },
            shape_id,
            storage: PropertyStorage::Inline(Vec::new()),
            prototype: None,
        }
    }

    /// Returns `true` if the object is frozen (no modifications allowed).
    pub fn is_frozen(&self) -> bool {
        self.header.flags & ObjectFlags::FROZEN != 0
    }

    /// Returns `true` if the object is sealed (existing properties are
    /// configurable=false, no new properties can be added).
    pub fn is_sealed(&self) -> bool {
        self.header.flags & ObjectFlags::SEALED != 0
    }

    /// Returns `true` if the object is extensible (new properties can be added).
    pub fn is_extensible(&self) -> bool {
        self.header.flags & ObjectFlags::NON_EXTENSIBLE == 0
    }

    /// Freezes the object, preventing all modifications.
    pub fn freeze(&mut self) {
        self.header.flags |= ObjectFlags::FROZEN;
    }

    /// Seals the object, preventing adding/removing properties.
    pub fn seal(&mut self) {
        self.header.flags |= ObjectFlags::SEALED;
    }

    /// Prevents extensions on the object.
    pub fn prevent_extensions(&mut self) {
        self.header.flags |= ObjectFlags::NON_EXTENSIBLE;
    }
}
