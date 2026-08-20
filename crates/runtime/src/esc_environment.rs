//! Dynamic environment for functions containing `with` or `eval`.
//!
//! Only "poisoned" functions (those that use `with` statements or call `eval`)
//! need this dynamic environment. Non-poisoned functions continue using the
//! fast-path [`Environment`](crate::environment::Environment) with numeric
//! `(depth, slot)` access.
//!
//! [`EscEnvironment`] provides:
//! - Pre-allocated slots for statically known variables (fast indexed access)
//! - A name-to-slot map for dynamic name lookup
//! - A dynamic bindings map for eval-introduced variables
//! - A parent chain via `Arc` for scope walking
//! - Optional `with` object integration for `with` statement semantics

use std::collections::HashMap;
use std::sync::Arc;

use interner::Atom;
use nanbox::JsValue;

use crate::rt_api::{
    __esc_rt_get_prop, __esc_rt_has_prop, __esc_rt_set_prop, get_prop_by_symbol_key,
};

/// Dynamic environment for functions containing `with` or `eval`.
///
/// Only used in "poisoned" functions. Non-poisoned functions continue
/// using the existing fast-path `Environment` with numeric `(depth, slot)` access.
///
/// The environment forms a chain via the `outer` pointer, mirroring the
/// lexical scope chain. Dynamic lookups walk this chain checking the
/// `with_object`, `slot_map`, and `bindings` at each level.
pub struct EscEnvironment {
    /// Pre-allocated value slots for statically known variables.
    /// Indexed by the same slot numbers as `EnvLoad`/`EnvStore`.
    pub slots: Vec<u64>,

    /// Name-to-slot-index mapping for statically known variables.
    /// Used by dynamic lookup to find variables by name.
    pub slot_map: HashMap<Atom, u32>,

    /// Dynamic bindings introduced by `eval` at runtime.
    /// These do NOT have pre-allocated slots.
    pub bindings: HashMap<Atom, u64>,

    /// Parent environment (`Arc`-managed for safe sharing).
    pub outer: Option<Arc<EscEnvironment>>,

    /// The `with` object, if this is a with-environment.
    /// When set, name lookups first check this object's properties.
    pub with_object: Option<u64>,

    /// Whether `eval` has extended this environment with new bindings.
    pub eval_extended: bool,

    /// Whether this is a `VariableEnvironment` (for sloppy eval var leaking).
    pub is_var_env: bool,
}

impl EscEnvironment {
    /// Creates a new environment with `slot_count` pre-allocated slots,
    /// all initialized to `undefined`.
    pub fn new(slot_count: usize) -> Self {
        Self {
            slots: vec![JsValue::undefined().raw_bits(); slot_count],
            slot_map: HashMap::new(),
            bindings: HashMap::new(),
            outer: None,
            with_object: None,
            eval_extended: false,
            is_var_env: false,
        }
    }

    /// Creates a new environment with `slot_count` pre-allocated slots
    /// and a parent environment.
    pub fn with_outer(slot_count: usize, outer: Arc<EscEnvironment>) -> Self {
        Self {
            slots: vec![JsValue::undefined().raw_bits(); slot_count],
            slot_map: HashMap::new(),
            bindings: HashMap::new(),
            outer: Some(outer),
            with_object: None,
            eval_extended: false,
            is_var_env: false,
        }
    }

    /// Creates a with-environment that delegates lookups to the given object
    /// before falling through to the outer environment.
    ///
    /// `obj` is the NaN-boxed `with` target object. The environment has no
    /// slots of its own — all variable access goes through the object first.
    pub fn with_object(obj: u64, outer: Arc<EscEnvironment>) -> Self {
        Self {
            slots: Vec::new(),
            slot_map: HashMap::new(),
            bindings: HashMap::new(),
            outer: Some(outer),
            with_object: Some(obj),
            eval_extended: false,
            is_var_env: false,
        }
    }

    /// Fast-path slot read by numeric index.
    ///
    /// Returns `undefined` if the index is out of range.
    pub fn get_slot(&self, index: u32) -> u64 {
        self.slots
            .get(index as usize)
            .copied()
            .unwrap_or(JsValue::undefined().raw_bits())
    }

    /// Fast-path slot write by numeric index.
    ///
    /// Does nothing if the index is out of range.
    pub fn set_slot(&mut self, index: u32, value: u64) {
        if (index as usize) < self.slots.len() {
            self.slots[index as usize] = value;
        }
    }

    /// Dynamic name lookup: searches the scope chain for a binding with the given name.
    ///
    /// Search order at each environment level:
    /// 1. If `with_object` is set, check `Symbol.unscopables`, then check properties.
    /// 2. Check `slot_map` for a named slot.
    /// 3. Check `bindings` for eval-introduced bindings.
    /// 4. Recurse to `outer` if not found.
    ///
    /// Returns `None` if the name is not found in any environment in the chain.
    pub fn lookup(&self, name: &Atom) -> Option<u64> {
        // 1. Check with_object (respecting Symbol.unscopables)
        if let Some(obj) = self.with_object
            && has_property_on_object(obj, name)
            && !is_unscopable(obj, name)
        {
            return Some(get_property_from_object(obj, name));
        }

        // 2. Check slot_map for named slot
        if let Some(&slot_idx) = self.slot_map.get(name) {
            return Some(self.get_slot(slot_idx));
        }

        // 3. Check eval-introduced bindings
        if let Some(&val) = self.bindings.get(name) {
            return Some(val);
        }

        // 4. Recurse to outer
        if let Some(ref outer) = self.outer {
            return outer.lookup(name);
        }

        None
    }

    /// Dynamic name store: searches the scope chain and stores the value.
    ///
    /// Search order at each environment level:
    /// 1. If `with_object` is set, check `Symbol.unscopables`, then check properties.
    /// 2. Check `slot_map` for a named slot: write to the slot.
    /// 3. Check `bindings`: update the binding.
    /// 4. Recurse to `outer` if not found.
    ///
    /// Returns `false` if the name is not found anywhere in the chain
    /// (caller should handle implicit global creation in sloppy mode).
    pub fn store(&mut self, name: &Atom, value: u64) -> bool {
        // 1. Check with_object (respecting Symbol.unscopables)
        if let Some(obj) = self.with_object
            && has_property_on_object(obj, name)
            && !is_unscopable(obj, name)
        {
            set_property_on_object(obj, name, value);
            return true;
        }

        // 2. Check slot_map for named slot
        if let Some(&slot_idx) = self.slot_map.get(name) {
            self.set_slot(slot_idx, value);
            return true;
        }

        // 3. Check eval-introduced bindings
        if self.bindings.contains_key(name) {
            self.bindings.insert(*name, value);
            return true;
        }

        // 4. Cannot recurse mutably through Arc — return false to let caller handle
        false
    }

    /// Add a new dynamic binding introduced by `eval`.
    ///
    /// Sets the `eval_extended` flag and inserts the binding into the
    /// `bindings` map.
    pub fn add_binding(&mut self, name: Atom, value: u64) {
        self.eval_extended = true;
        self.bindings.insert(name, value);
    }

    /// Walk the parent chain to find the nearest `VariableEnvironment`.
    ///
    /// In sloppy mode, `eval` var declarations leak to the enclosing
    /// `VariableEnvironment`. This method finds that environment.
    ///
    /// Returns `None` if no `VariableEnvironment` is found in the chain.
    pub fn find_var_env(self: &Arc<Self>) -> Option<Arc<EscEnvironment>> {
        if self.is_var_env {
            return Some(Arc::clone(self));
        }
        if let Some(ref outer) = self.outer {
            return outer.find_var_env();
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Helper functions for with-object property access
// ---------------------------------------------------------------------------

/// Check whether the with-object has a property with the given atom name.
///
/// Uses the runtime `__esc_rt_has_prop` ABI function to check object properties.
/// Returns `false` if the atom cannot be resolved to a property name.
fn has_property_on_object(obj: u64, name: &Atom) -> bool {
    let name_bits = atom_to_string_bits(name);
    let result = __esc_rt_has_prop(obj, name_bits);
    JsValue::from_raw_bits(result).as_bool() == Some(true)
}

/// Get a property value from the with-object by atom name.
///
/// Uses the runtime `__esc_rt_get_prop` ABI function.
fn get_property_from_object(obj: u64, name: &Atom) -> u64 {
    let name_bits = atom_to_string_bits(name);
    __esc_rt_get_prop(obj, name_bits)
}

/// Set a property value on the with-object by atom name.
///
/// Uses the runtime `__esc_rt_set_prop` ABI function.
fn set_property_on_object(obj: u64, name: &Atom, value: u64) {
    let name_bits = atom_to_string_bits(name);
    __esc_rt_set_prop(obj, name_bits, value);
}

/// Check whether the name is marked as unscopable on the with-object.
///
/// Per ES2025 spec Section 9.1.1.2.1 (HasBinding), step 5:
/// If the object has a `Symbol.unscopables` property that is an object,
/// and that object has a truthy property for the given name, then the
/// name is "unscopable" and should NOT be found via the with-object.
fn is_unscopable(obj: u64, name: &Atom) -> bool {
    use crate::symbol::SYMBOL_UNSCOPABLES;

    // Get @@unscopables from the with-object
    let unscopables_bits = get_prop_by_symbol_key(obj, SYMBOL_UNSCOPABLES);
    let unscopables = JsValue::from_raw_bits(unscopables_bits);

    // If @@unscopables is not an object, nothing is unscopable
    if !unscopables.is_object() {
        return false;
    }

    // Check if unscopables[name] is truthy
    let name_bits = atom_to_string_bits(name);
    let entry = __esc_rt_get_prop(unscopables_bits, name_bits);
    let entry_val = JsValue::from_raw_bits(entry);

    crate::value_ops::to_boolean(entry_val)
}

/// Convert an `Atom` to a NaN-boxed string for use with property access ABI functions.
///
/// Resolves the atom to its string representation using the thread-local interner,
/// then creates a runtime string from it.
fn atom_to_string_bits(name: &Atom) -> u64 {
    use crate::rt_api::{INTERNER, make_rt_string};

    INTERNER.with(|interner| {
        let interner = interner.borrow();
        let s = interner
            .try_resolve(*name)
            .unwrap_or("<unresolved>")
            .to_string();
        make_rt_string(s)
    })
}
