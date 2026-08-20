//! Tests for memory strategy assignment.

use std::collections::HashMap;

use ir::IrType;
use ir::builder::TypedIrBuilder;

use escape::{EscapeResult, EscapeState, ZoneAssignment};

use crate::policy::{AllocPolicy, HeapOnlyAllocPolicy, NormalAllocPolicy};
use crate::{AllocationClass, MemoryLayer, assign_memory};

/// Helper: build a single-function TypedFunction.
fn build_func(f: impl FnOnce(&mut TypedIrBuilder)) -> ir::builder::TypedFunction {
    let mut b = TypedIrBuilder::new();
    f(&mut b);
    let module = b.finish();
    module.functions.into_iter().next().unwrap()
}

fn empty_zones() -> ZoneAssignment {
    ZoneAssignment {
        assignments: HashMap::new(),
    }
}

// =========================================================================
// Memory layer assignment tests
// =========================================================================

#[test]
fn test_memory_layer_local() {
    let func = build_func(|b| {
        b.begin_function("local", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let obj = b.create_object();
        b.load_field(obj, 0);
        b.ret(None);
        b.end_function();
    });

    let obj_id = func.blocks[0].instructions[0].id;
    let mut states = HashMap::new();
    states.insert(obj_id.0, EscapeState::Local);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].layer, MemoryLayer::L2ScopeOwn);
    assert_eq!(decisions[0].class, AllocationClass::Class1Static);
}

#[test]
fn test_memory_layer_zone() {
    let func = build_func(|b| {
        b.begin_function("zone", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let obj = b.create_object();
        b.load_field(obj, 0);
        b.ret(None);
        b.end_function();
    });

    let obj_id = func.blocks[0].instructions[0].id;
    let mut states = HashMap::new();
    states.insert(obj_id.0, EscapeState::ZoneCandidate);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].layer, MemoryLayer::L3Region);
    assert_eq!(decisions[0].class, AllocationClass::Class1Static);
}

#[test]
fn test_memory_layer_zone_dynamic() {
    // Currently our implementation treats all zone candidates as static shape,
    // but verify the policy returns correct values for dynamic shapes.
    let policy = NormalAllocPolicy;
    let (layer, class) = policy.classify_zone_candidate(false);
    assert_eq!(layer, MemoryLayer::L3Region);
    assert_eq!(class, AllocationClass::Class2Dynamic);
}

#[test]
fn test_memory_layer_escaped() {
    let func = build_func(|b| {
        b.begin_function("escaped", vec![], IrType::JSObject);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let obj = b.create_object();
        b.ret(Some(obj));
        b.end_function();
    });

    let obj_id = func.blocks[0].instructions[0].id;
    let mut states = HashMap::new();
    states.insert(obj_id.0, EscapeState::Escapes);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].layer, MemoryLayer::L4ZoneRc);
    assert_eq!(decisions[0].class, AllocationClass::Class3Heap);
}

#[test]
fn test_heap_only_mode() {
    let func = build_func(|b| {
        b.begin_function("heap_only", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        // Three allocations with different escape states.
        let a = b.create_object();
        let _b = b.create_array(vec![]);
        let c = b.alloc_zone(IrType::ZonePtr);
        b.load_field(a, 0);
        b.load_field(c, 0);
        b.ret(None);
        b.end_function();
    });

    // Give them different escape states.
    let allocs: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| escape::classifier::EscapeClassifier::is_allocation(&i.op))
        .map(|i| i.id)
        .collect();
    assert_eq!(allocs.len(), 3);

    let mut states = HashMap::new();
    states.insert(allocs[0].0, EscapeState::Local);
    states.insert(allocs[1].0, EscapeState::ZoneCandidate);
    states.insert(allocs[2].0, EscapeState::Escapes);
    let escapes = EscapeResult { states };

    // heap_only = true → all should be L4ZoneRc / Class3Heap.
    let decisions = assign_memory(&func, &escapes, &empty_zones(), true);
    assert_eq!(decisions.len(), 3);
    for d in &decisions {
        assert_eq!(
            d.layer,
            MemoryLayer::L4ZoneRc,
            "heap_only: all should be L4"
        );
        assert_eq!(
            d.class,
            AllocationClass::Class3Heap,
            "heap_only: all should be Class3"
        );
    }
}

// =========================================================================
// Policy tests
// =========================================================================

#[test]
fn test_normal_policy() {
    let policy = NormalAllocPolicy;

    let (l, c) = policy.classify_local();
    assert_eq!(l, MemoryLayer::L2ScopeOwn);
    assert_eq!(c, AllocationClass::Class1Static);

    let (l, c) = policy.classify_zone_candidate(true);
    assert_eq!(l, MemoryLayer::L3Region);
    assert_eq!(c, AllocationClass::Class1Static);

    let (l, c) = policy.classify_zone_candidate(false);
    assert_eq!(l, MemoryLayer::L3Region);
    assert_eq!(c, AllocationClass::Class2Dynamic);

    let (l, c) = policy.classify_escaped();
    assert_eq!(l, MemoryLayer::L4ZoneRc);
    assert_eq!(c, AllocationClass::Class3Heap);
}

#[test]
fn test_heap_only_policy() {
    let policy = HeapOnlyAllocPolicy;

    let (l, c) = policy.classify_local();
    assert_eq!(l, MemoryLayer::L4ZoneRc);
    assert_eq!(c, AllocationClass::Class3Heap);

    let (l, c) = policy.classify_zone_candidate(true);
    assert_eq!(l, MemoryLayer::L4ZoneRc);
    assert_eq!(c, AllocationClass::Class3Heap);

    let (l, c) = policy.classify_zone_candidate(false);
    assert_eq!(l, MemoryLayer::L4ZoneRc);
    assert_eq!(c, AllocationClass::Class3Heap);

    let (l, c) = policy.classify_escaped();
    assert_eq!(l, MemoryLayer::L4ZoneRc);
    assert_eq!(c, AllocationClass::Class3Heap);
}

// =========================================================================
// Empty function tests
// =========================================================================

#[test]
fn test_no_allocations_produces_empty_decisions() {
    let func = build_func(|b| {
        b.begin_function("no_allocs", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        // Only non-allocation ops
        let val = b.const_i32(42);
        b.ret(Some(val));
        b.end_function();
    });

    let escapes = EscapeResult {
        states: HashMap::new(),
    };
    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert!(decisions.is_empty());
}

#[test]
fn test_no_allocations_heap_only_also_empty() {
    let func = build_func(|b| {
        b.begin_function("no_allocs_heap", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        b.ret(None);
        b.end_function();
    });

    let escapes = EscapeResult {
        states: HashMap::new(),
    };
    let decisions = assign_memory(&func, &escapes, &empty_zones(), true);
    assert!(decisions.is_empty());
}

// =========================================================================
// Default escape state (missing from map → Local)
// =========================================================================

#[test]
fn test_missing_escape_state_defaults_to_local() {
    let func = build_func(|b| {
        b.begin_function("default_local", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let obj = b.create_object();
        b.load_field(obj, 0);
        b.ret(None);
        b.end_function();
    });

    // Empty escape states → should default to Local
    let escapes = EscapeResult {
        states: HashMap::new(),
    };
    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].layer, MemoryLayer::L2ScopeOwn);
    assert_eq!(decisions[0].class, AllocationClass::Class1Static);
}

// =========================================================================
// Multiple allocations with mixed escape states
// =========================================================================

#[test]
fn test_mixed_escape_states_normal_policy() {
    let func = build_func(|b| {
        b.begin_function("mixed", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let a = b.create_object(); // will be Local
        let c = b.create_array(vec![]); // will be ZoneCandidate
        let e = b.alloc_zone(IrType::ZonePtr); // will be Escapes
        b.load_field(a, 0);
        b.load_field(c, 0);
        b.load_field(e, 0);
        b.ret(None);
        b.end_function();
    });

    let allocs: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| escape::classifier::EscapeClassifier::is_allocation(&i.op))
        .map(|i| i.id)
        .collect();
    assert_eq!(allocs.len(), 3);

    let mut states = HashMap::new();
    states.insert(allocs[0].0, EscapeState::Local);
    states.insert(allocs[1].0, EscapeState::ZoneCandidate);
    states.insert(allocs[2].0, EscapeState::Escapes);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 3);

    // Local → L2/Class1
    assert_eq!(decisions[0].layer, MemoryLayer::L2ScopeOwn);
    assert_eq!(decisions[0].class, AllocationClass::Class1Static);

    // ZoneCandidate → L3/Class1 (static shape default)
    assert_eq!(decisions[1].layer, MemoryLayer::L3Region);
    assert_eq!(decisions[1].class, AllocationClass::Class1Static);

    // Escapes → L4/Class3
    assert_eq!(decisions[2].layer, MemoryLayer::L4ZoneRc);
    assert_eq!(decisions[2].class, AllocationClass::Class3Heap);
}

// =========================================================================
// Custom policy via trait
// =========================================================================

/// A test-only policy that puts everything in L1Unbox/Class1Static.
struct AllUnboxPolicy;

impl AllocPolicy for AllUnboxPolicy {
    fn classify_local(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L1Unbox, AllocationClass::Class1Static)
    }
    fn classify_zone_candidate(&self, _is_static_shape: bool) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L1Unbox, AllocationClass::Class1Static)
    }
    fn classify_escaped(&self) -> (MemoryLayer, AllocationClass) {
        (MemoryLayer::L1Unbox, AllocationClass::Class1Static)
    }
}

#[test]
fn test_custom_policy_trait_object() {
    let policy: &dyn AllocPolicy = &AllUnboxPolicy;

    let (l, c) = policy.classify_local();
    assert_eq!(l, MemoryLayer::L1Unbox);
    assert_eq!(c, AllocationClass::Class1Static);

    let (l, c) = policy.classify_zone_candidate(true);
    assert_eq!(l, MemoryLayer::L1Unbox);
    assert_eq!(c, AllocationClass::Class1Static);

    let (l, c) = policy.classify_escaped();
    assert_eq!(l, MemoryLayer::L1Unbox);
    assert_eq!(c, AllocationClass::Class1Static);
}

// =========================================================================
// MemoryDecision fields
// =========================================================================

#[test]
fn test_decision_value_matches_allocation() {
    let func = build_func(|b| {
        b.begin_function("value_match", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let obj = b.create_object();
        b.load_field(obj, 0);
        b.ret(None);
        b.end_function();
    });

    let obj_id = func.blocks[0].instructions[0].id;
    let mut states = HashMap::new();
    states.insert(obj_id.0, EscapeState::Local);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions[0].value, obj_id);
}

// =========================================================================
// Enum variants exist and are distinct
// =========================================================================

#[test]
fn test_memory_layer_variants_are_distinct() {
    let layers = [
        MemoryLayer::L1Unbox,
        MemoryLayer::L2ScopeOwn,
        MemoryLayer::L3Region,
        MemoryLayer::L4ZoneRc,
        MemoryLayer::L5CycleCollect,
        MemoryLayer::L6Fallback,
    ];
    for (i, a) in layers.iter().enumerate() {
        for (j, b) in layers.iter().enumerate() {
            if i == j {
                assert_eq!(a, b);
            } else {
                assert_ne!(a, b);
            }
        }
    }
}

#[test]
fn test_allocation_class_variants_are_distinct() {
    let classes = [
        AllocationClass::Class1Static,
        AllocationClass::Class2Dynamic,
        AllocationClass::Class3Heap,
    ];
    for (i, a) in classes.iter().enumerate() {
        for (j, b) in classes.iter().enumerate() {
            if i == j {
                assert_eq!(a, b);
            } else {
                assert_ne!(a, b);
            }
        }
    }
}

// =========================================================================
// Debug formatting
// =========================================================================

#[test]
fn test_memory_layer_debug() {
    let dbg = format!("{:?}", MemoryLayer::L3Region);
    assert_eq!(dbg, "L3Region");
}

#[test]
fn test_allocation_class_debug() {
    let dbg = format!("{:?}", AllocationClass::Class2Dynamic);
    assert_eq!(dbg, "Class2Dynamic");
}

#[test]
fn test_memory_decision_debug() {
    let decision = crate::MemoryDecision {
        value: ir::ValueId(42),
        layer: MemoryLayer::L4ZoneRc,
        class: AllocationClass::Class3Heap,
    };
    let dbg = format!("{decision:?}");
    assert!(dbg.contains("L4ZoneRc"));
    assert!(dbg.contains("Class3Heap"));
}

// =========================================================================
// HeapOnly mode: all escape states produce the same result
// =========================================================================

#[test]
fn test_heap_only_ignores_is_static_shape() {
    let policy = HeapOnlyAllocPolicy;
    let (l1, c1) = policy.classify_zone_candidate(true);
    let (l2, c2) = policy.classify_zone_candidate(false);
    assert_eq!(l1, l2);
    assert_eq!(c1, c2);
}

// =========================================================================
// Closure allocation is detected
// =========================================================================

#[test]
fn test_closure_allocation_detected() {
    let func = build_func(|b| {
        b.begin_function("closure_test", vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        let func_val = b.const_i32(0);
        let env_val = b.const_i32(0);
        let flags = b.const_i32(0);
        let closure = b.create_closure(func_val, env_val, flags);
        b.load_field(closure, 0);
        b.ret(None);
        b.end_function();
    });

    let allocs: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| escape::classifier::EscapeClassifier::is_allocation(&i.op))
        .map(|i| i.id)
        .collect();
    assert_eq!(allocs.len(), 1);

    let mut states = HashMap::new();
    states.insert(allocs[0].0, EscapeState::Escapes);
    let escapes = EscapeResult { states };

    let decisions = assign_memory(&func, &escapes, &empty_zones(), false);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].layer, MemoryLayer::L4ZoneRc);
    assert_eq!(decisions[0].class, AllocationClass::Class3Heap);
}
