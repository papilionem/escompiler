//! Memory strategy assignment (L1-L6 layer decisions).
//!
//! Takes escape analysis results and assigns each allocation a memory layer
//! (L1-L6) and allocation class (Class1-Class3), determining how the value
//! will be allocated and managed at runtime.

/// Allocation policy trait and implementations (normal vs. heap-only).
pub mod policy;
#[cfg(test)]
mod tests;

use ir::ValueId;
use ir::builder::TypedFunction;

use escape::classifier::EscapeClassifier;
use escape::{EscapeResult, EscapeState, ZoneAssignment};

use policy::{AllocPolicy, HeapOnlyAllocPolicy, NormalAllocPolicy};

/// The memory management layer assigned to a value (L1 through L6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLayer {
    /// Unboxed scalar (no allocation).
    L1Unbox,
    /// Scope-owned (stack or enclosing scope).
    L2ScopeOwn,
    /// Region-allocated (short-lived groups).
    L3Region,
    /// Zone + RC (zone-allocated with cross-zone tracking).
    L4ZoneRc,
    /// Cycle collector (Bacon-Rajan for cross-world cycles).
    L5CycleCollect,
    /// QuickJS fallback (eval, Proxy, with).
    L6Fallback,
}

/// The allocation class assigned to a value, determining its storage strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationClass {
    /// Statically-shaped zone objects.
    Class1Static,
    /// Dynamically-shaped zone objects.
    Class2Dynamic,
    /// Heap-allocated objects (per-object ARC).
    Class3Heap,
}

/// The memory strategy assigned to a single allocation in the IR.
#[derive(Debug, Clone)]
pub struct MemoryDecision {
    /// The IR value this decision applies to.
    pub value: ValueId,
    /// The memory management layer (L1-L6) assigned to this value.
    pub layer: MemoryLayer,
    /// The allocation class (static-zone, dynamic-zone, or heap) for this value.
    pub class: AllocationClass,
}

/// Assign memory layers and allocation classes based on escape analysis.
///
/// For each allocation in the function:
/// - If `heap_only` → everything gets L4ZoneRc / Class3Heap.
/// - If `Local` → L2ScopeOwn / Class1Static.
/// - If `ZoneCandidate` → L3Region (Class1Static if static shape, Class2Dynamic otherwise).
/// - If `Escapes` → L4ZoneRc / Class3Heap.
pub fn assign_memory(
    func: &TypedFunction,
    escapes: &EscapeResult,
    _zones: &ZoneAssignment,
    heap_only: bool,
) -> Vec<MemoryDecision> {
    let policy: &dyn AllocPolicy = if heap_only {
        &HeapOnlyAllocPolicy
    } else {
        &NormalAllocPolicy
    };

    let mut decisions = Vec::new();

    for block in &func.blocks {
        for inst in &block.instructions {
            if !EscapeClassifier::is_allocation(&inst.op) {
                continue;
            }

            let value_id = inst.id;
            let state = escapes
                .states
                .get(&value_id.0)
                .unwrap_or(&EscapeState::Local);

            let (layer, class) = match state {
                EscapeState::Local => policy.classify_local(),
                EscapeState::ZoneCandidate => {
                    // For now, treat all zone candidates as static shape.
                    // Future: shape analysis will determine is_static_shape.
                    let is_static_shape = true;
                    policy.classify_zone_candidate(is_static_shape)
                }
                EscapeState::Escapes => policy.classify_escaped(),
            };

            decisions.push(MemoryDecision {
                value: value_id,
                layer,
                class,
            });
        }
    }

    decisions
}
