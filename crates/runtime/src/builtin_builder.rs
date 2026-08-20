//! Fluent API for registering builtin constructor methods.
//!
//! Replaces the hardcoded arrays in `rt_api/property.rs` with a declarative
//! builder that co-locates method name, function pointer, and spec `.length`.
//!
//! # Usage
//!
//! ```ignore
//! let reg = BuiltInBuilder::new("Array")
//!     .constructor_length(1)
//!     .static_method("isArray", 1)
//!     .static_method("from", 1)
//!     .method("push", 1)
//!     .method("pop", 0)
//!     .method("forEach", 1)
//!     .build();
//! ```
//!
//! The global registry is initialized via [`init_registry`] and can be queried
//! with [`get_registration`].

use std::sync::OnceLock;

/// A registered builtin constructor with its methods and metadata.
///
/// Created by [`BuiltInBuilder::build`]. Stores the constructor name,
/// its spec `.length`, and lists of instance and static methods.
pub struct BuiltInRegistration {
    /// Constructor name (e.g., `"Array"`).
    pub name: &'static str,
    /// Constructor `.length` property value.
    pub constructor_length: u32,
    /// Instance methods (on `.prototype`).
    pub instance_methods: Vec<BuiltInMethodInfo>,
    /// Static methods (on the constructor itself).
    pub static_methods: Vec<BuiltInMethodInfo>,
    // Cached slices for efficient lookup by the existing dispatch code.
    instance_method_names: Vec<&'static str>,
    static_method_names: Vec<&'static str>,
}

impl BuiltInRegistration {
    /// Returns the instance method names as a slice.
    ///
    /// This is a cached projection of [`instance_methods`](Self::instance_methods)
    /// for use by the dispatch layer, which expects `&[&str]`.
    pub fn instance_method_names(&self) -> &[&'static str] {
        &self.instance_method_names
    }

    /// Returns the static method names as a slice.
    ///
    /// This is a cached projection of [`static_methods`](Self::static_methods)
    /// for use by the dispatch layer, which expects `&[&str]`.
    pub fn static_method_names(&self) -> &[&'static str] {
        &self.static_method_names
    }

    /// Look up the spec `.length` for an instance method by name.
    ///
    /// Returns `None` if the method is not registered.
    pub fn instance_method_length(&self, name: &str) -> Option<u32> {
        self.instance_methods
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.length)
    }

    /// Look up the spec `.length` for a static method by name.
    ///
    /// Returns `None` if the method is not registered.
    pub fn static_method_length(&self, name: &str) -> Option<u32> {
        self.static_methods
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.length)
    }
}

/// Metadata for a single builtin method.
///
/// Stores the method name and its spec `.length` (formal parameter count).
pub struct BuiltInMethodInfo {
    /// Method name (e.g., `"push"`).
    pub name: &'static str,
    /// Spec `.length` value (number of formal parameters).
    pub length: u32,
}

/// Fluent builder for registering builtin constructors.
///
/// Collects method metadata and produces a [`BuiltInRegistration`] via
/// [`build`](Self::build).
pub struct BuiltInBuilder {
    /// Constructor name.
    name: &'static str,
    /// Constructor `.length`.
    constructor_length: u32,
    /// Instance methods accumulated so far.
    instance_methods: Vec<BuiltInMethodInfo>,
    /// Static methods accumulated so far.
    static_methods: Vec<BuiltInMethodInfo>,
}

impl BuiltInBuilder {
    /// Create a new builder for the given constructor name.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            constructor_length: 0,
            instance_methods: Vec::new(),
            static_methods: Vec::new(),
        }
    }

    /// Set the constructor's spec `.length` property.
    pub fn constructor_length(mut self, len: u32) -> Self {
        self.constructor_length = len;
        self
    }

    /// Register an instance method (on `.prototype`) with its spec `.length`.
    pub fn method(mut self, name: &'static str, length: u32) -> Self {
        self.instance_methods
            .push(BuiltInMethodInfo { name, length });
        self
    }

    /// Register a static method (on the constructor itself) with its spec `.length`.
    pub fn static_method(mut self, name: &'static str, length: u32) -> Self {
        self.static_methods.push(BuiltInMethodInfo { name, length });
        self
    }

    /// Consume the builder and produce a [`BuiltInRegistration`].
    pub fn build(self) -> BuiltInRegistration {
        let instance_method_names: Vec<&'static str> =
            self.instance_methods.iter().map(|m| m.name).collect();
        let static_method_names: Vec<&'static str> =
            self.static_methods.iter().map(|m| m.name).collect();
        BuiltInRegistration {
            name: self.name,
            constructor_length: self.constructor_length,
            instance_methods: self.instance_methods,
            static_methods: self.static_methods,
            instance_method_names,
            static_method_names,
        }
    }
}

// =========================================================================
// Global registry
// =========================================================================

/// Global registry of builtin registrations, initialized once on first access.
static BUILTIN_REGISTRY: OnceLock<Vec<BuiltInRegistration>> = OnceLock::new();

/// Initialize the builtin registry with all registered builtins.
///
/// Returns a reference to the statically cached slice. Safe to call from
/// multiple threads; the initialization runs exactly once.
pub fn init_registry() -> &'static [BuiltInRegistration] {
    BUILTIN_REGISTRY.get_or_init(|| vec![build_array_registration(), build_object_registration()])
}

/// Look up a builtin registration by constructor name.
///
/// Returns `None` if the constructor has not been migrated to the builder yet.
pub fn get_registration(name: &str) -> Option<&'static BuiltInRegistration> {
    init_registry().iter().find(|r| r.name == name)
}

// =========================================================================
// Array registration
// =========================================================================

/// Build the [`BuiltInRegistration`] for `Array`.
///
/// Instance methods and static methods are taken from the existing hardcoded
/// lists in `rt_api/property.rs`, with spec `.length` values from
/// `builtin_method_length`.
fn build_array_registration() -> BuiltInRegistration {
    BuiltInBuilder::new("Array")
        .constructor_length(1)
        // Static methods
        .static_method("isArray", 1)
        .static_method("from", 1)
        .static_method("of", 0)
        // Instance methods (from builtin_instance_method_list for "Array")
        .method("concat", 1)
        .method("copyWithin", 2)
        .method("entries", 0)
        .method("every", 1)
        .method("fill", 1)
        .method("filter", 1)
        .method("find", 1)
        .method("findIndex", 1)
        .method("findLast", 1)
        .method("findLastIndex", 1)
        .method("flat", 0)
        .method("flatMap", 1)
        .method("forEach", 1)
        .method("includes", 1)
        .method("indexOf", 1)
        .method("join", 1)
        .method("keys", 0)
        .method("lastIndexOf", 1)
        .method("map", 1)
        .method("pop", 0)
        .method("push", 1)
        .method("reduce", 1)
        .method("reduceRight", 1)
        .method("reverse", 0)
        .method("shift", 0)
        .method("slice", 2)
        .method("some", 1)
        .method("sort", 1)
        .method("splice", 2)
        .method("toReversed", 0)
        .method("toSorted", 1)
        .method("toSpliced", 2)
        .method("unshift", 1)
        .method("values", 0)
        .method("at", 1)
        .method("toString", 0)
        .build()
}

// =========================================================================
// Object registration
// =========================================================================

/// Build the [`BuiltInRegistration`] for `Object`.
///
/// Static methods are taken from `builtin_static_methods` for "Object" with
/// spec `.length` values from `builtin_method_length`. Instance methods are
/// from `builtin_instance_method_list` for "Object".
fn build_object_registration() -> BuiltInRegistration {
    BuiltInBuilder::new("Object")
        .constructor_length(1)
        // Static methods (from builtin_static_methods for "Object")
        .static_method("keys", 1)
        .static_method("values", 1)
        .static_method("entries", 1)
        .static_method("assign", 2)
        .static_method("create", 2)
        .static_method("defineProperty", 3)
        .static_method("defineProperties", 2)
        .static_method("freeze", 1)
        .static_method("seal", 1)
        .static_method("isFrozen", 1)
        .static_method("isSealed", 1)
        .static_method("isExtensible", 1)
        .static_method("preventExtensions", 1)
        .static_method("getOwnPropertyDescriptor", 2)
        .static_method("getOwnPropertyDescriptors", 1)
        .static_method("getOwnPropertyNames", 1)
        .static_method("getOwnPropertySymbols", 1)
        .static_method("getPrototypeOf", 1)
        .static_method("setPrototypeOf", 1)
        .static_method("hasOwn", 2)
        .static_method("fromEntries", 1)
        .static_method("is", 2)
        // Instance methods (from builtin_instance_method_list for "Object")
        .method("toString", 0)
        .method("valueOf", 0)
        .method("hasOwnProperty", 1)
        .method("propertyIsEnumerable", 1)
        .method("isPrototypeOf", 1)
        .method("toLocaleString", 0)
        .build()
}
