//! Allocation policy trait and implementations.
//!
//! Defines the mapping from escape states to memory layers and allocation
//! classes. Two policies are provided: normal (uses the full L1-L4 hierarchy)
//! and heap-only (everything goes to L4/Class3 for differential testing).

use super::{AllocationClass, MemoryLayer};

/// Trait for mapping escape analysis results to memory decisions.
pub trait AllocPolicy {
    /// Classify a value that does not escape its defining block.
    fn classify_local(&self) -> (MemoryLayer, AllocationClass);

    /// Classify a value that is a zone candidate (does not escape function).
    fn classify_zone_candidate(&self, is_static_shape: bool) -> (MemoryLayer, AllocationClass);

    /// Classify a value that escapes the function.
    fn classify_escaped(&self) -> (MemoryLayer, AllocationClass);
}

/// Normal allocation policy using the full memory hierarchy.
pub struct NormalAllocPolicy;

impl AllocPolicy for NormalAllocPolicy {
    fn classify_local(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L2ScopeOwn, AllocationClass::Class1Static)
    }

    fn classify_zone_candidate(&self, is_static_shape: bool) -> (MemoryLayer, AllocationClass) {
        if is_static_shape {
            (MemoryLayer::L3Region, AllocationClass::Class1Static)
        } else {
            (MemoryLayer::L3Region, AllocationClass::Class2Dynamic)
        }
    }

    fn classify_escaped(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L4ZoneRc, AllocationClass::Class3Heap)
    }
}

/// Heap-only policy: all allocations go to L4/Class3.
///
/// Used for differential testing (`--heap-only` mode) to verify correctness
/// independent of zone allocation.
pub struct HeapOnlyAllocPolicy;

impl AllocPolicy for HeapOnlyAllocPolicy {
    fn classify_local(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L4ZoneRc, AllocationClass::Class3Heap)
    }

    fn classify_zone_candidate(&self, _is_static_shape: bool) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L4ZoneRc, AllocationClass::Class3Heap)
    }

    fn classify_escaped(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L4ZoneRc, AllocationClass::Class3Heap)
    }
}
