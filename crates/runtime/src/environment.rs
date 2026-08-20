//! Closure environment: a chain of variable slots for captured variables.
//!
//! Each `Environment` holds a fixed-size vector of NaN-boxed slots and an
//! optional pointer to a parent environment, forming a scope chain.

use nanbox::JsValue;

/// A closure environment holding captured variables.
///
/// Environments form a linked list via the `parent` pointer, mirroring
/// the lexical scope chain. Compiled closures capture an `Environment`
/// pointer and use `(depth, slot)` pairs to read/write variables.
pub struct Environment {
    /// Variable slots (NaN-boxed values).
    pub slots: Vec<u64>,
    /// Parent environment, or null if this is the top-level scope.
    pub parent: *mut Environment,
}

impl Environment {
    /// Creates a new environment with the given number of slots,
    /// all initialized to `undefined`.
    pub fn new(slot_count: u32, parent: *mut Environment) -> Self {
        Self {
            slots: vec![JsValue::undefined().raw_bits(); slot_count as usize],
            parent,
        }
    }

    /// Loads a value from a slot at the given depth in the scope chain.
    ///
    /// Walks `depth` parent links, then reads `slot` from that environment.
    /// Returns `undefined` if the depth or slot is out of range.
    pub fn load(&self, depth: u32, slot: u32) -> u64 {
        let env = self.walk(depth);
        let env_ref = unsafe {
            // SAFETY: walk() returns a valid pointer to an Environment
            // allocated via Box::into_raw, or self if depth is 0.
            &*env
        };
        env_ref
            .slots
            .get(slot as usize)
            .copied()
            .unwrap_or(JsValue::undefined().raw_bits())
    }

    /// Stores a value into a slot at the given depth in the scope chain.
    ///
    /// Walks `depth` parent links, then writes `val` to `slot`.
    /// Does nothing if the depth or slot is out of range.
    pub fn store(&mut self, depth: u32, slot: u32, val: u64) {
        let env = self.walk_mut(depth);
        let env_ref = unsafe {
            // SAFETY: walk_mut() returns a valid pointer to an Environment
            // allocated via Box::into_raw, or self if depth is 0.
            &mut *env
        };
        if (slot as usize) < env_ref.slots.len() {
            env_ref.slots[slot as usize] = val;
        }
    }

    /// Walks the parent chain `depth` times, returning a pointer to the
    /// target environment. Returns `self` if depth is 0.
    fn walk(&self, depth: u32) -> *const Environment {
        let mut current: *const Environment = self;
        for _ in 0..depth {
            let cur = unsafe {
                // SAFETY: current points to a valid Environment from Box::into_raw.
                &*current
            };
            if cur.parent.is_null() {
                return current;
            }
            current = cur.parent;
        }
        current
    }

    /// Mutable version of `walk`.
    fn walk_mut(&mut self, depth: u32) -> *mut Environment {
        let mut current: *mut Environment = self;
        for _ in 0..depth {
            let cur = unsafe {
                // SAFETY: current points to a valid Environment from Box::into_raw.
                &*current
            };
            if cur.parent.is_null() {
                return current;
            }
            current = cur.parent;
        }
        current
    }
}
