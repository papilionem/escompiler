//! Unit tests for the generator/async transform.

use ir::builder::{TypedBasicBlock, TypedFunction, TypedIrBuilder, TypedModule};
use ir::types::TypedInstruction;
use ir::{BlockId, IrType, Op, ValueId};

use crate::analysis::{self, SuspensionPoint};
use crate::codegen;
use crate::split;
use crate::yield_delegate;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a minimal TypedFunction with the given blocks.
fn build_function(
    name: &str,
    blocks: Vec<TypedBasicBlock>,
    is_generator: bool,
    is_async: bool,
    next_value: u32,
    next_block: u32,
) -> TypedFunction {
    TypedFunction {
        name: name.to_string(),
        params: vec![],
        return_type: IrType::JSValue,
        blocks,
        next_value,
        next_block,
        is_generator,
        is_async,
    }
}

/// Build a TypedFunction with parameters.
fn build_function_with_params(
    name: &str,
    params: Vec<(&str, IrType)>,
    blocks: Vec<TypedBasicBlock>,
    is_generator: bool,
    is_async: bool,
    next_value: u32,
    next_block: u32,
) -> TypedFunction {
    TypedFunction {
        name: name.to_string(),
        params: params
            .into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect(),
        return_type: IrType::JSValue,
        blocks,
        next_value,
        next_block,
        is_generator,
        is_async,
    }
}

/// Build a TypedModule wrapping a single function.
fn build_module(func: TypedFunction) -> TypedModule {
    TypedModule {
        functions: vec![func],
        struct_types: vec![],
        entry: Some(0),
    }
}

/// Create a simple instruction with the given op, id, operands, and block targets.
fn make_inst(
    id: u32,
    op: Op,
    ty: IrType,
    operands: Vec<u32>,
    block_targets: Vec<u32>,
) -> TypedInstruction {
    TypedInstruction {
        id: ValueId(id),
        op,
        ty,
        operands: operands.into_iter().map(ValueId).collect(),
        block_targets: block_targets.into_iter().map(BlockId).collect(),
        span: common::SourceSpan::DUMMY,
    }
}

/// Create a basic block with the given id, instructions, and predecessors.
fn make_block(
    id: u32,
    instructions: Vec<TypedInstruction>,
    predecessors: Vec<u32>,
) -> TypedBasicBlock {
    TypedBasicBlock {
        id: BlockId(id),
        instructions,
        sealed: true,
        predecessors: predecessors.into_iter().map(BlockId).collect(),
    }
}

// ===========================================================================
// Suspension point discovery tests
// ===========================================================================

#[test]
fn test_discover_no_yields() {
    // A function with no yields should produce no suspension points.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 2, 1);

    let points = analysis::discover_suspension_points(&func);
    assert!(
        points.is_empty(),
        "non-generator body should have no suspension points"
    );
}

#[test]
fn test_discover_single_yield() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::BoxI32, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Yield, IrType::JSValue, vec![1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 4, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].index, 0);
    assert_eq!(points[0].block_id, BlockId(0));
    assert_eq!(points[0].instruction_index, 2);
    assert_eq!(points[0].yield_value, Some(ValueId(1)));
}

#[test]
fn test_discover_multiple_yields() {
    // Two yields in the same block
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstI32(2), IrType::I32, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 5, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].index, 0);
    assert_eq!(points[0].instruction_index, 1);
    assert_eq!(points[1].index, 1);
    assert_eq!(points[1].instruction_index, 3);
}

#[test]
fn test_discover_yields_across_blocks() {
    // Yields in different blocks
    let block0 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![],
    );
    let block1 = make_block(
        1,
        vec![
            make_inst(3, Op::ConstI32(2), IrType::I32, vec![], vec![]),
            make_inst(4, Op::Yield, IrType::JSValue, vec![3], vec![]),
            make_inst(5, Op::Ret, IrType::Void, vec![4], vec![]),
        ],
        vec![0],
    );
    let func = build_function("test", vec![block0, block1], true, false, 6, 2);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].block_id, BlockId(0));
    assert_eq!(points[1].block_id, BlockId(1));
}

#[test]
fn test_discover_three_yield_types() {
    // Yield, Await, and YieldDelegate
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Await, IrType::JSValue, vec![1], vec![]),
            make_inst(3, Op::YieldDelegate, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, true, 5, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 3);
    assert!(matches!(points[0].op, Op::Yield));
    assert!(matches!(points[1].op, Op::Await));
    assert!(matches!(points[2].op, Op::YieldDelegate));
}

// ===========================================================================
// Liveness analysis tests
// ===========================================================================

#[test]
fn test_liveness_value_defined_before_used_after() {
    // v0 = const 42      (defined before yield)
    // v1 = yield(v0)
    // v2 = add_js(v0, v1) (v0 used after yield -> live across)
    // ret v2
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 4, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);

    let liveness = analysis::analyze_liveness(&func, &points);
    let live = &liveness.live_across[&0];

    // v0 is defined before yield and used after -> live across
    assert!(
        live.contains(&ValueId(0)),
        "v0 should be live across yield (defined before, used after)"
    );
    // v1 is the yield result, defined AT the yield -> NOT live across
    assert!(
        !live.contains(&ValueId(1)),
        "v1 (yield result) should not be live across"
    );
}

#[test]
fn test_liveness_value_defined_after_not_live() {
    // v0 = yield(undefined)
    // v1 = const 42         (defined AFTER yield)
    // v2 = add_js(v0, v1)
    // ret v2
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(3, Op::AddJS, IrType::JSValue, vec![1, 2], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 5, 1);

    let points = analysis::discover_suspension_points(&func);
    let liveness = analysis::analyze_liveness(&func, &points);
    let live = &liveness.live_across[&0];

    // v2 (const 42) is defined after yield -> NOT live across
    assert!(
        !live.contains(&ValueId(2)),
        "v2 should not be live across (defined after yield)"
    );
    // v0 (undefined) is used as yield operand but not used after yield
    // It IS used as the operand of the yield instruction itself, but the yield
    // instruction is AT the suspension point, not after it.
    // v0 is not used after the yield -> should not be live across.
    assert!(
        !live.contains(&ValueId(0)),
        "v0 should not be live across (not used after yield)"
    );
}

#[test]
fn test_liveness_across_blocks() {
    // block0: v0 = const 42; yield(v0); br block1
    // block1: v3 = add_js(v0, v0); ret v3
    // v0 is used in block1 which is after the yield -> live across
    let block0 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![],
    );
    let block1 = make_block(
        1,
        vec![
            make_inst(3, Op::AddJS, IrType::JSValue, vec![0, 0], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![0],
    );
    let func = build_function("test", vec![block0, block1], true, false, 5, 2);

    let points = analysis::discover_suspension_points(&func);
    let liveness = analysis::analyze_liveness(&func, &points);
    let live = &liveness.live_across[&0];

    assert!(
        live.contains(&ValueId(0)),
        "v0 should be live across yield (used in successor block)"
    );
}

#[test]
fn test_liveness_loop_with_yield() {
    // Simulates: for (let i = 0; i < n; i++) { yield i; }
    // block0 (entry):  v0=const 0 (i); v1=param n; br block1
    // block1 (check):  v2=lt(v0,v1); brif v2, block2, block3
    // block2 (body):   yield(v0); v3=const 1; v4=add(v0,v3); br block1
    //   (v0 used in block2 -> phi needed, but for simplicity we use v0 directly)
    // block3 (done):   ret undefined
    //
    // v0 and v1 are live across the yield (used in loop check after yield)
    let block0 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(0), IrType::I32, vec![], vec![]),
            make_inst(1, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(2, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![],
    );
    let block1 = make_block(
        1,
        vec![
            make_inst(3, Op::LtJS, IrType::Bool, vec![0, 1], vec![]),
            make_inst(4, Op::BrIf, IrType::Void, vec![3], vec![2, 3]),
        ],
        vec![0, 2], // predecessors: entry and loop back
    );
    let block2 = make_block(
        2,
        vec![
            make_inst(5, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(6, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(7, Op::AddJS, IrType::JSValue, vec![0, 6], vec![]),
            make_inst(8, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![1],
    );
    let block3 = make_block(
        3,
        vec![
            make_inst(9, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(10, Op::Ret, IrType::Void, vec![9], vec![]),
        ],
        vec![1],
    );
    let func = build_function(
        "range",
        vec![block0, block1, block2, block3],
        true,
        false,
        11,
        4,
    );

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].block_id, BlockId(2));

    let liveness = analysis::analyze_liveness(&func, &points);
    let live = &liveness.live_across[&0];

    // v0 (i) is used after yield in the same block (add_js operand)
    assert!(
        live.contains(&ValueId(0)),
        "v0 (i) should be live across yield"
    );
    // v1 (n) is used in block1 which is a successor of the loop back
    // and live_out of block2 should include v1 since block1 uses it
    assert!(
        live.contains(&ValueId(1)),
        "v1 (n) should be live across yield"
    );
}

// ===========================================================================
// Slot assignment tests
// ===========================================================================

#[test]
fn test_slot_assignment_unique_slots() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::ConstI32(2), IrType::I32, vec![], vec![]),
            make_inst(2, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(3, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 5, 1);

    let points = analysis::discover_suspension_points(&func);
    let mut liveness = analysis::analyze_liveness(&func, &points);
    analysis::assign_slots(&mut liveness);

    // Both v0 and v1 should be live across
    assert!(liveness.slot_assignment.contains_key(&ValueId(0)));
    assert!(liveness.slot_assignment.contains_key(&ValueId(1)));

    // Each should have a unique slot
    let slot0 = liveness.slot_assignment[&ValueId(0)];
    let slot1 = liveness.slot_assignment[&ValueId(1)];
    assert_ne!(slot0, slot1, "different values should get different slots");
    assert_eq!(liveness.num_slots, 2);
}

#[test]
fn test_slot_assignment_no_live_vars() {
    // yield 'a'; yield 'b'; yield 'c' -- no values cross yield boundaries
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstString(0), IrType::JSString, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstString(1), IrType::JSString, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::ConstString(2), IrType::JSString, vec![], vec![]),
            make_inst(5, Op::Yield, IrType::JSValue, vec![4], vec![]),
            make_inst(6, Op::Ret, IrType::Void, vec![5], vec![]),
        ],
        vec![],
    );
    let func = build_function("abc", vec![block], true, false, 7, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 3);

    let mut liveness = analysis::analyze_liveness(&func, &points);
    analysis::assign_slots(&mut liveness);

    assert_eq!(liveness.num_slots, 0, "no values cross yield boundaries");
    assert!(liveness.slot_assignment.is_empty());
}

// ===========================================================================
// Block splitting tests
// ===========================================================================

#[test]
fn test_split_yield_in_middle_of_block() {
    // Block has: const, yield, add, ret
    // After split: block0 has const, yield; resume_block has add, ret
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("test", vec![block], true, false, 4, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);

    let result = split::split_and_identify(&mut func, &points).expect("split should succeed");

    // Should have 2 blocks after split
    assert_eq!(
        result.modified_blocks.len(),
        2,
        "block should be split into 2"
    );

    // Original block should end with the yield
    let original = &result.modified_blocks[0];
    assert_eq!(original.instructions.len(), 2); // const + yield
    assert!(matches!(
        original.instructions.last().map(|i| &i.op),
        Some(Op::Yield)
    ));

    // Resume block should have the remaining instructions
    let resume = &result.modified_blocks[1];
    assert_eq!(resume.instructions.len(), 2); // add + ret
    assert!(matches!(resume.instructions[0].op, Op::AddJS));
    assert!(matches!(resume.instructions[1].op, Op::Ret));
}

#[test]
fn test_split_yield_before_terminator() {
    // Block has: const, yield, br
    // After split: block0 has const, yield; resume_block has br
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![],
    );
    let block1 = make_block(
        1,
        vec![
            make_inst(3, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![0],
    );
    let mut func = build_function("test", vec![block, block1], true, false, 5, 2);

    let points = analysis::discover_suspension_points(&func);
    let result = split::split_and_identify(&mut func, &points).expect("split should succeed");

    // Should have 3 blocks: original (const, yield), resume (br), block1 (ret)
    assert_eq!(result.modified_blocks.len(), 3);
}

// ===========================================================================
// Segment identification tests
// ===========================================================================

#[test]
fn test_segments_single_yield() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("test", vec![block], true, false, 3, 1);

    let points = analysis::discover_suspension_points(&func);
    let result = split::split_and_identify(&mut func, &points).expect("split should succeed");

    // Should have 2 segments: before yield, after yield
    assert_eq!(result.segments.len(), 2);
    assert_eq!(result.segments[0].index, 0);
    assert_eq!(result.segments[0].entry_block, BlockId(0));
    assert_eq!(result.segments[0].suspension_point, Some(0));

    assert_eq!(result.segments[1].index, 1);
    // Resume block is newly created
    assert_eq!(result.segments[1].suspension_point, None); // last segment
}

#[test]
fn test_segments_no_yields() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("test", vec![block], true, false, 2, 1);

    let points = analysis::discover_suspension_points(&func);
    assert!(points.is_empty());

    // No suspension points -> segments are just the whole function
    let segments = split::identify_segments(&func, &points);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].index, 0);
    assert_eq!(segments[0].suspension_point, None);
}

#[test]
fn test_segments_three_yields() {
    // yield 'a'; yield 'b'; yield 'c'
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstString(0), IrType::JSString, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstString(1), IrType::JSString, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::ConstString(2), IrType::JSString, vec![], vec![]),
            make_inst(5, Op::Yield, IrType::JSValue, vec![4], vec![]),
            make_inst(6, Op::Ret, IrType::Void, vec![5], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("abc", vec![block], true, false, 7, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 3);

    let result = split::split_and_identify(&mut func, &points).expect("split should succeed");

    // Should have 4 segments: before yield 0, after yield 0, after yield 1, after yield 2
    assert_eq!(result.segments.len(), 4);
    assert_eq!(result.segments[0].index, 0);
    assert_eq!(result.segments[0].suspension_point, Some(0));
    assert_eq!(result.segments[1].index, 1);
    assert_eq!(result.segments[1].suspension_point, Some(1));
    assert_eq!(result.segments[2].index, 2);
    assert_eq!(result.segments[2].suspension_point, Some(2));
    assert_eq!(result.segments[3].index, 3);
    assert_eq!(result.segments[3].suspension_point, None); // last
}

// ===========================================================================
// transform_module integration tests
// ===========================================================================

#[test]
fn test_transform_module_skips_non_generator() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("regular", vec![block], false, false, 2, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert!(
        results.is_empty(),
        "non-generator function should be skipped"
    );
}

#[test]
fn test_transform_module_processes_generator() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let func = build_function("gen", vec![block], true, false, 4, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 1, "generator function should be processed");

    let (idx, result) = &results[0];
    assert_eq!(*idx, 0);
    assert_eq!(result.liveness.suspension_points.len(), 1);
    assert!(!result.liveness.live_across[&0].is_empty());
    assert!(result.split.segments.len() >= 2);
}

#[test]
fn test_transform_module_processes_async() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("asyncfn", vec![block], false, true, 3, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 1, "async function should be processed");
}

#[test]
fn test_transform_module_generator_no_yields() {
    // A generator with no yields (valid JS — immediately returns done).
    // Still transformed into ramp+resume to support the generator protocol.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("empty_gen", vec![block], true, false, 2, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(
        results.len(),
        1,
        "no-yield generator should still be transformed"
    );
    assert_eq!(module.functions.len(), 2, "should have ramp + resume");
}

#[test]
fn test_transform_module_async_no_await() {
    // An async function with no await must still be transformed into ramp+resume
    // so it returns a Promise (the ramp wraps the generator in async_wrap).
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("asyncfn", vec![block], false, true, 2, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(
        results.len(),
        1,
        "async function without await should still be transformed"
    );
    assert_eq!(module.functions.len(), 2, "should have ramp + resume");

    // The ramp must wrap the generator in async_wrap (sentinel u32::MAX - 7).
    let ramp = &module.functions[0];
    let has_async_wrap = ramp.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstString(c) if c == u32::MAX - 7))
    });
    assert!(
        has_async_wrap,
        "async ramp must call async_wrap to return a Promise"
    );
}

#[test]
fn test_ramp_saves_env_when_capturing() {
    // A function capturing a closure environment reads it via LoadParam at index
    // `params.len()`. The ramp must save that env to the state object so the
    // resume function can reload captured variables.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let has_env_load = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::LoadParam(0)));
    assert!(has_env_load, "ramp must load the closure environment");

    let has_env_key = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::ConstString(3)));
    assert!(
        has_env_key,
        "ramp must store the env under key 3 + params.len()"
    );
}

#[test]
fn test_resume_remaps_load_param_to_state() {
    // The resume function receives (state, sent_value, resume_mode), so the
    // original function's LoadParam must be rewritten to GetProp from state.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function_with_params(
        "gen",
        vec![("a", IrType::JSValue)],
        vec![block],
        true,
        false,
        3,
        1,
    );
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // The resume function should load the parameter from state via GetProp with
    // key 3 (param_0), not via LoadParam.
    let load_param_count = resume
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.op, Op::LoadParam(_)))
        .count();
    // Only the 3 entry LoadParam (state, sent_value, resume_mode) remain; the
    // original LoadParam(0) is remapped.
    assert_eq!(load_param_count, 3, "resume must remap body LoadParam");
}

// ===========================================================================
// Builder integration tests (is_generator / is_async flags)
// ===========================================================================

#[test]
fn test_builder_generator_flag() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("gen", vec![], IrType::JSValue);
    b.set_generator(true);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.seal_block(entry);
    let undef = b.const_undefined();
    b.ret(Some(undef));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    assert!(module.functions[0].is_generator);
    assert!(!module.functions[0].is_async);
}

#[test]
fn test_builder_async_flag() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("asyncfn", vec![], IrType::JSValue);
    b.set_async(true);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.seal_block(entry);
    let undef = b.const_undefined();
    b.ret(Some(undef));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    assert!(!module.functions[0].is_generator);
    assert!(module.functions[0].is_async);
}

#[test]
fn test_builder_async_generator_flags() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("asyncgen", vec![], IrType::JSValue);
    b.set_generator(true);
    b.set_async(true);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.seal_block(entry);
    let undef = b.const_undefined();
    b.ret(Some(undef));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    assert!(module.functions[0].is_generator);
    assert!(module.functions[0].is_async);
}

#[test]
fn test_function_mut_sets_flags_after_build() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.seal_block(entry);
    let undef = b.const_undefined();
    b.ret(Some(undef));
    b.end_function();

    // Set flags after function is complete using function_mut
    b.function_mut(0).is_generator = true;

    b.set_entry(0);
    let module = b.finish();

    assert!(module.functions[0].is_generator);
}

// ===========================================================================
// Error path tests
// ===========================================================================

#[test]
fn test_split_invalid_block_error() {
    // Create a suspension point that references a non-existent block
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("test", vec![block], true, false, 2, 1);

    let fake_sp = vec![SuspensionPoint {
        index: 0,
        block_id: BlockId(99), // non-existent
        instruction_index: 0,
        op: Op::Yield,
        yield_value: None,
    }];

    let result = split::split_and_identify(&mut func, &fake_sp);
    assert!(result.is_err(), "should fail on invalid block ID");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("non-existent block"),
        "error should mention non-existent block: {err_msg}"
    );
}

#[test]
fn test_split_invalid_instruction_index_error() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("test", vec![block], true, false, 2, 1);

    let fake_sp = vec![SuspensionPoint {
        index: 0,
        block_id: BlockId(0),
        instruction_index: 99, // out of range
        op: Op::Yield,
        yield_value: None,
    }];

    let result = split::split_and_identify(&mut func, &fake_sp);
    assert!(result.is_err(), "should fail on invalid instruction index");
}

// ===========================================================================
// Codegen tests — ramp function
// ===========================================================================

/// Helper: run full analysis + split on a function, returning liveness and split.
fn analyze_and_split(
    func: &mut TypedFunction,
) -> (crate::analysis::LivenessResult, crate::split::SplitResult) {
    let points = analysis::discover_suspension_points(func);
    let mut liveness = analysis::analyze_liveness(func, &points);
    analysis::assign_slots(&mut liveness);
    let split_result =
        split::split_and_identify(func, &liveness.suspension_points).expect("split should succeed");
    (liveness, split_result)
}

#[test]
fn test_ramp_creates_state_object() {
    // Simple generator: function* gen() { yield 42; }
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Ramp should have exactly 1 block
    assert_eq!(func.blocks.len(), 1, "ramp should have exactly 1 block");

    // Check for CreateObject instruction
    let has_create_object = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::CreateObject));
    assert!(has_create_object, "ramp must create a state object");
}

#[test]
fn test_ramp_sets_state_index() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Should have SetProp for state_index (ConstString(0) is "state_index")
    let has_set_prop = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::SetProp));
    assert!(has_set_prop, "ramp must set properties on state object");

    // Should have ConstI32(-1) for state_index = not started
    let has_not_started = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::ConstI32(-1)));
    assert!(
        has_not_started,
        "ramp must set state_index to -1 (not started)"
    );
}

#[test]
fn test_ramp_calls_create_generator_runtime() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Should call CallRuntime
    let has_call_runtime = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::CallRuntime));
    assert!(
        has_call_runtime,
        "ramp must call runtime to create generator"
    );

    // Should end with Ret
    let last_op = &func.blocks[0].instructions.last().map(|i| &i.op);
    assert!(matches!(last_op, Some(Op::Ret)), "ramp must end with Ret");
}

#[test]
fn test_ramp_saves_params() {
    // function* gen(a, b) { yield a; }
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::LoadParam(1), IrType::JSValue, vec![], vec![]),
            make_inst(2, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function_with_params(
        "gen",
        vec![("a", IrType::JSValue), ("b", IrType::JSValue)],
        vec![block],
        true,
        false,
        4,
        1,
    );
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Should have LoadParam instructions for saving params
    let load_param_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::LoadParam(_)))
        .count();
    assert_eq!(
        load_param_count, 2,
        "ramp must load both parameters to save them"
    );

    // Should have SetProp for each param (plus 2 for state_index and resume_mode)
    let set_prop_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::SetProp))
        .count();
    // 2 (state_index, resume_mode) + 2 (param_0, param_1) = 4
    assert_eq!(
        set_prop_count, 4,
        "ramp must SetProp for state_index, resume_mode, and each param"
    );
}

#[test]
fn test_ramp_resume_func_index() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    // Pass resume_func_idx = 5
    codegen::rewrite_as_ramp(&mut func, &liveness, 5).expect("rewrite should succeed");

    // Should have ConstI32(5) for the resume function index
    let has_resume_idx = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::ConstI32(5)));
    assert!(has_resume_idx, "ramp must encode the resume function index");
}

// ===========================================================================
// Codegen tests — resume function
// ===========================================================================

#[test]
fn test_resume_function_name() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("my_gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    assert_eq!(
        resume.name, "my_gen_resume",
        "resume function name should be original_resume"
    );
}

#[test]
fn test_resume_function_params() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Resume function takes 3 params: state, sent_value, resume_mode
    assert_eq!(resume.params.len(), 3);
    assert_eq!(resume.params[0].0, "state");
    assert_eq!(resume.params[1].0, "sent_value");
    assert_eq!(resume.params[2].0, "resume_mode");
}

#[test]
fn test_resume_has_reentry_check() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have ConstI32(-3) for STATE_EXECUTING check
    let has_executing_check = resume.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstI32(-3)))
    });
    assert!(
        has_executing_check,
        "resume must check for re-entrancy (state_index == -3)"
    );

    // Should have a Throw instruction for the re-entrancy error
    let has_throw = resume
        .blocks
        .iter()
        .any(|b| b.instructions.iter().any(|i| matches!(i.op, Op::Throw)));
    assert!(has_throw, "resume must throw on re-entrancy");
}

#[test]
fn test_resume_has_done_check() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have ConstI32(-2) for STATE_DONE check
    let has_done_check = resume.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstI32(-2)))
    });
    assert!(
        has_done_check,
        "resume must check for completion (state_index == -2)"
    );
}

#[test]
fn test_resume_marks_executing() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // The dispatch block should set state_index to -3 (executing)
    // There should be a SetProp writing BoxI32(-3) to state
    let has_set_executing = resume.blocks.iter().any(|b| {
        let has_neg3 = b
            .instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstI32(-3)));
        let has_set_prop = b.instructions.iter().any(|i| matches!(i.op, Op::SetProp));
        has_neg3 && has_set_prop
    });
    assert!(
        has_set_executing,
        "resume must mark state as executing (-3) before dispatch"
    );
}

#[test]
fn test_resume_has_switch_dispatch() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have BrIf instructions for dispatch chain
    let brif_count: usize = resume
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.op, Op::BrIf))
        .count();
    assert!(
        brif_count >= 2,
        "resume must have BrIf dispatch chain (got {brif_count})"
    );
}

#[test]
fn test_resume_has_completion_block() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have at least one block that sets state_index to -2 (done)
    let has_completion = resume.blocks.iter().any(|b| {
        let has_done = b
            .instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstI32(-2)));
        let has_ret = b.instructions.iter().any(|i| matches!(i.op, Op::Ret));
        has_done && has_ret
    });
    assert!(
        has_completion,
        "resume must have a completion block that sets state_index to -2 and returns"
    );
}

#[test]
fn test_resume_not_generator_flag() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Resume function is NOT a generator or async — it's a plain function
    assert!(
        !resume.is_generator,
        "resume function must not be marked as generator"
    );
    assert!(
        !resume.is_async,
        "resume function must not be marked as async"
    );
}

// ===========================================================================
// Codegen tests — live variable save/restore
// ===========================================================================

#[test]
fn test_resume_saves_live_vars_at_yield() {
    // v0 = const 42 (live across yield)
    // v1 = yield(v0)
    // v2 = add_js(v0, v1) -> v0 must be saved
    // ret v2
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 4, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    // Verify v0 is live across
    assert!(liveness.live_across[&0].contains(&ValueId(0)));
    assert!(liveness.slot_assignment.contains_key(&ValueId(0)));

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // The segment containing the yield should have SetProp to save live vars
    // Look for ConstString(100+) which is the slot key for live variable saves
    let has_slot_save = resume.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| match &i.op {
            Op::ConstString(idx) => *idx >= 100 && *idx < 200,
            _ => false,
        })
    });
    assert!(
        has_slot_save,
        "resume must save live variables at yield points using slot keys"
    );
}

#[test]
fn test_resume_loads_live_vars_at_resume() {
    // Same as above — after resuming, v0 must be loaded from state
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::AddJS, IrType::JSValue, vec![0, 1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 4, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // The segment after the yield should have GetProp to load saved live vars
    let has_slot_load = resume.blocks.iter().any(|b| {
        let has_slot_key = b.instructions.iter().any(|i| match &i.op {
            Op::ConstString(idx) => *idx >= 100 && *idx < 200,
            _ => false,
        });
        let has_get_prop = b.instructions.iter().any(|i| matches!(i.op, Op::GetProp));
        has_slot_key && has_get_prop
    });
    assert!(
        has_slot_load,
        "resume must load live variables after yield using GetProp"
    );
}

// ===========================================================================
// Codegen tests — yield return
// ===========================================================================

#[test]
fn test_resume_yields_with_done_false() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // At yield points, should have ConstBool(false) for done=false
    let has_done_false = resume.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstBool(false)))
    });
    assert!(
        has_done_false,
        "resume must create iterator result with done=false at yield"
    );

    // Should call runtime to create iter result
    let has_call_runtime = resume.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::CallRuntime))
    });
    assert!(
        has_call_runtime,
        "resume must call runtime to create iterator result"
    );
}

// ===========================================================================
// Codegen tests — multiple yields
// ===========================================================================

#[test]
fn test_resume_multiple_yields_state_transitions() {
    // yield 'a'; yield 'b'; yield 'c'
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstString(0), IrType::JSString, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstString(1), IrType::JSString, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::ConstString(2), IrType::JSString, vec![], vec![]),
            make_inst(5, Op::Yield, IrType::JSValue, vec![4], vec![]),
            make_inst(6, Op::Ret, IrType::Void, vec![5], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("abc", vec![block], true, false, 7, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have state_index values 0, 1, 2 set in the segment blocks
    let state_index_values: Vec<i32> = resume
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter_map(|i| match &i.op {
            Op::ConstI32(v) if *v >= 0 && *v <= 2 => Some(*v),
            _ => None,
        })
        .collect();

    // Should include at least 0, 1, 2 as state indices for the 3 yield points
    assert!(
        state_index_values.contains(&0),
        "should set state_index to 0 for first yield"
    );
    assert!(
        state_index_values.contains(&1),
        "should set state_index to 1 for second yield"
    );
    assert!(
        state_index_values.contains(&2),
        "should set state_index to 2 for third yield"
    );
}

#[test]
fn test_resume_dispatch_for_multiple_yields() {
    // With 3 yields, dispatch should check state_index == -1, 0, 1, 2
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstString(0), IrType::JSString, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstString(1), IrType::JSString, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::ConstString(2), IrType::JSString, vec![], vec![]),
            make_inst(5, Op::Yield, IrType::JSValue, vec![4], vec![]),
            make_inst(6, Op::Ret, IrType::Void, vec![5], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("abc", vec![block], true, false, 7, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Should have ConstI32(-1) for initial entry dispatch
    let has_neg1_dispatch = resume.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::ConstI32(-1)))
    });
    assert!(
        has_neg1_dispatch,
        "dispatch must check state_index == -1 for initial entry"
    );
}

// ===========================================================================
// Codegen tests — transform_module integration
// ===========================================================================

#[test]
fn test_transform_module_produces_resume_function() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("gen", vec![block], true, false, 3, 1);
    let mut module = build_module(func);

    assert_eq!(module.functions.len(), 1, "should start with 1 function");

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 1);

    // Module should now have 2 functions: ramp (replacing original) + resume
    assert_eq!(
        module.functions.len(),
        2,
        "module should have 2 functions after transform"
    );

    // First function (index 0) is the ramp
    let ramp = &module.functions[0];
    assert_eq!(ramp.name, "gen");
    // Ramp should have CreateObject
    assert!(
        ramp.blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i.op, Op::CreateObject))
    );

    // Second function (index 1) is the resume
    let resume = &module.functions[1];
    assert_eq!(resume.name, "gen_resume");
    assert_eq!(resume.params.len(), 3);
}

#[test]
fn test_transform_module_multiple_generators() {
    // Two generator functions
    let block1 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func1 = build_function("gen1", vec![block1], true, false, 3, 1);

    let block2 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(2), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func2 = build_function("gen2", vec![block2], true, false, 3, 1);

    let mut module = TypedModule {
        functions: vec![func1, func2],
        struct_types: vec![],
        entry: Some(0),
    };

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 2, "both generators should be processed");

    // Module should now have 4 functions: gen1_ramp, gen2_ramp, gen1_resume, gen2_resume
    assert_eq!(
        module.functions.len(),
        4,
        "module should have 4 functions after transforming 2 generators"
    );

    assert_eq!(module.functions[2].name, "gen1_resume");
    assert_eq!(module.functions[3].name, "gen2_resume");
}

#[test]
fn test_transform_module_mixed_functions() {
    // One regular function, one generator
    let regular_block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let regular_func = build_function("regular", vec![regular_block], false, false, 2, 1);

    let gen_block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let gen_func = build_function("gen", vec![gen_block], true, false, 3, 1);

    let mut module = TypedModule {
        functions: vec![regular_func, gen_func],
        struct_types: vec![],
        entry: Some(0),
    };

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 1, "only generator should be processed");

    // Module should have 3 functions: regular (unchanged), gen (ramp), gen_resume
    assert_eq!(module.functions.len(), 3);
    assert_eq!(module.functions[0].name, "regular");
    assert_eq!(module.functions[1].name, "gen");
    assert_eq!(module.functions[2].name, "gen_resume");

    // Regular function should be unchanged — no CreateObject
    assert!(
        !module.functions[0].blocks[0]
            .instructions
            .iter()
            .any(|i| matches!(i.op, Op::CreateObject))
    );
}

#[test]
fn test_transform_module_generator_no_yields_unchanged() {
    // Generator with no yields is still transformed (needs ramp+resume).
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("empty_gen", vec![block], true, false, 2, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(
        results.len(),
        1,
        "no-yield generator should still be transformed"
    );

    // Module should have 2 functions (ramp + resume)
    assert_eq!(module.functions.len(), 2);
}

// ===========================================================================
// Codegen tests — loop + yield
// ===========================================================================

#[test]
fn test_resume_loop_with_yield_saves_loop_vars() {
    // for (let i = 0; i < n; i++) { yield i; }
    let block0 = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(0), IrType::I32, vec![], vec![]),
            make_inst(1, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(2, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![],
    );
    let block1 = make_block(
        1,
        vec![
            make_inst(3, Op::LtJS, IrType::Bool, vec![0, 1], vec![]),
            make_inst(4, Op::BrIf, IrType::Void, vec![3], vec![2, 3]),
        ],
        vec![0, 2],
    );
    let block2 = make_block(
        2,
        vec![
            make_inst(5, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(6, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(7, Op::AddJS, IrType::JSValue, vec![0, 6], vec![]),
            make_inst(8, Op::Br, IrType::Void, vec![], vec![1]),
        ],
        vec![1],
    );
    let block3 = make_block(
        3,
        vec![
            make_inst(9, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(10, Op::Ret, IrType::Void, vec![9], vec![]),
        ],
        vec![1],
    );
    let mut func = build_function_with_params(
        "range",
        vec![("n", IrType::JSValue)],
        vec![block0, block1, block2, block3],
        true,
        false,
        11,
        4,
    );
    let (liveness, split_result) = analyze_and_split(&mut func);

    // v0 (i) and v1 (n) should be live across
    assert!(liveness.live_across[&0].contains(&ValueId(0)));
    assert!(liveness.live_across[&0].contains(&ValueId(1)));
    assert_eq!(liveness.num_slots, 2, "should need 2 slots for i and n");

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Resume function should have blocks
    assert!(
        resume.blocks.len() >= 4,
        "resume for loop+yield should have multiple blocks"
    );

    // Should have SetProp calls for saving live variables (slot keys 100+)
    let slot_key_count: usize = resume
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| match &i.op {
            Op::ConstString(idx) => *idx >= 100 && *idx < 200,
            _ => false,
        })
        .count();
    assert!(
        slot_key_count >= 2,
        "should reference slot keys for saving/loading 2 live variables (got {slot_key_count})"
    );
}

// ===========================================================================
// Codegen tests — resume function block count and terminators
// ===========================================================================

#[test]
fn test_resume_all_blocks_terminated() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // Every block should end with a terminator instruction
    for block in &resume.blocks {
        assert!(
            !block.instructions.is_empty(),
            "block {} should not be empty",
            block.id
        );
        let last = block.instructions.last().expect("block has instructions");
        assert!(
            last.op.is_terminator(),
            "block {} must end with a terminator, got {:?}",
            block.id,
            last.op
        );
    }
}

#[test]
fn test_resume_has_correct_block_count_single_yield() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    // With a single yield, we need:
    // - entry block (load state_index, check re-entrancy)
    // - re-entrancy error block
    // - done check block
    // - done return block
    // - dispatch block
    // - segment 0 block (initial entry through yield)
    // - segment 1 block (after yield through return)
    // - completion block
    // Plus possibly dispatch chain blocks
    assert!(
        resume.blocks.len() >= 7,
        "single-yield resume should have at least 7 blocks, got {}",
        resume.blocks.len()
    );
}

// ===========================================================================
// Codegen tests — async function handling
// ===========================================================================

#[test]
fn test_resume_from_async_function() {
    // async function with an Await
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_fn", vec![block], false, true, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    assert_eq!(resume.name, "async_fn_resume");
    assert!(!resume.is_generator);
    assert!(!resume.is_async);

    // Should still have the full state machine structure
    assert!(resume.blocks.len() >= 7);
}

// ===========================================================================
// Codegen tests — edge cases
// ===========================================================================

#[test]
fn test_ramp_no_params_no_extra_set_props() {
    // Generator with no params — should only have state_index and resume_mode SetProps
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let set_prop_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::SetProp))
        .count();
    // Only state_index and resume_mode
    assert_eq!(
        set_prop_count, 2,
        "no-param generator should have 2 SetProps"
    );

    let load_param_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::LoadParam(_)))
        .count();
    assert_eq!(
        load_param_count, 0,
        "no-param generator should have 0 LoadParams"
    );
}

#[test]
fn test_resume_returns_jsvalue() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    assert_eq!(
        resume.return_type,
        IrType::JSValue,
        "resume function should return JSValue"
    );
}

// ===========================================================================
// yield* delegation tests
// ===========================================================================

#[test]
fn test_yield_delegate_no_delegates_returns_false() {
    // A function with only Yield (no YieldDelegate) should return false
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let result = yield_delegate::desugar_yield_delegate(&mut func);
    assert!(result.is_ok());
    assert!(!result.unwrap(), "no yield delegates should return false");
    // Blocks unchanged
    assert_eq!(func.blocks.len(), 1);
}

#[test]
fn test_yield_delegate_rewrites_to_loop() {
    // A function with YieldDelegate should be rewritten into multiple blocks
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::YieldDelegate, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let result = yield_delegate::desugar_yield_delegate(&mut func);
    assert!(result.is_ok());
    assert!(result.unwrap(), "should detect yield delegate");
    // Should have more blocks now: original + loop_header + yield + done + continue
    assert!(
        func.blocks.len() >= 5,
        "expected at least 5 blocks after yield* desugaring, got {}",
        func.blocks.len()
    );
}

#[test]
fn test_yield_delegate_produces_yield_opcode() {
    // After desugaring, there should be a Yield opcode in the yield block
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::YieldDelegate, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    yield_delegate::desugar_yield_delegate(&mut func).unwrap();

    // Find a block containing a Yield instruction
    let has_yield = func.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::Yield))
    });
    assert!(
        has_yield,
        "desugared yield* should produce at least one Yield instruction"
    );
}

#[test]
fn test_yield_delegate_no_remaining_yield_delegate() {
    // After desugaring, there should be no YieldDelegate opcodes left
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::YieldDelegate, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    yield_delegate::desugar_yield_delegate(&mut func).unwrap();

    let has_delegate = func.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|inst| matches!(inst.op, Op::YieldDelegate))
    });
    assert!(
        !has_delegate,
        "no YieldDelegate opcodes should remain after desugaring"
    );
}

#[test]
fn test_yield_delegate_ret_remapped() {
    // The Ret instruction that referenced the YieldDelegate result should be
    // remapped to reference the delegation's final value
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::YieldDelegate, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![ValueId(1).0], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    yield_delegate::desugar_yield_delegate(&mut func).unwrap();

    // Find the continue block (last block added) which should have the Ret
    let continue_block = func.blocks.last().unwrap();
    let ret_inst = continue_block
        .instructions
        .iter()
        .find(|i| matches!(i.op, Op::Ret));
    if let Some(ret) = ret_inst {
        // The operand should NOT be ValueId(1) anymore (the old delegate result)
        assert!(
            ret.operands.is_empty() || ret.operands[0] != ValueId(1),
            "Ret operand should be remapped from the delegate result"
        );
    }
}

// ===========================================================================
// transform_module tests
// ===========================================================================

#[test]
fn test_transform_module_skips_non_generators() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Ret, IrType::Void, vec![0], vec![]),
        ],
        vec![],
    );
    let func = build_function("normal", vec![block], false, false, 2, 1);
    let mut module = TypedModule {
        functions: vec![func],
        struct_types: vec![],
        entry: None,
    };
    let results = crate::transform_module(&mut module);
    assert!(results.is_ok());
    assert!(
        results.unwrap().is_empty(),
        "non-generator functions should not be transformed"
    );
    assert_eq!(
        module.functions.len(),
        1,
        "no new functions should be added"
    );
}

#[test]
fn test_transform_module_adds_resume_function() {
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("gen", vec![block], true, false, 3, 1);
    let mut module = TypedModule {
        functions: vec![func],
        struct_types: vec![],
        entry: None,
    };
    let results = crate::transform_module(&mut module);
    assert!(results.is_ok());
    // Should have 2 functions: the rewritten ramp and the new resume
    assert_eq!(
        module.functions.len(),
        2,
        "transform should add a resume function"
    );
    assert!(
        module.functions[1].name.contains("resume"),
        "second function should be the resume function"
    );
}

// ===========================================================================
// Async ramp codegen tests
// ===========================================================================

#[test]
fn test_async_ramp_includes_async_wrap_call() {
    // An async function's ramp should call both create_generator AND async_wrap
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_fn", vec![block], false, true, 3, 1);
    let (liveness, _split_result) = analyze_and_split(&mut func);

    // Rewrite as ramp with resume_func_idx = 1
    let result = codegen::rewrite_as_ramp(&mut func, &liveness, 1);
    assert!(
        result.is_ok(),
        "rewrite_as_ramp should succeed for async functions"
    );

    // The ramp should have two CallRuntime instructions:
    // 1. create_generator
    // 2. async_wrap (wraps the generator into a Promise)
    let call_runtime_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .count();
    assert_eq!(
        call_runtime_count, 2,
        "async ramp should have 2 CallRuntime (create_generator + async_wrap)"
    );

    // The last CallRuntime should have the async_wrap sentinel
    let const_strings: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        const_strings.contains(&(u32::MAX - 7)),
        "async ramp should reference the async_wrap sentinel (u32::MAX - 7)"
    );
}

#[test]
fn test_generator_ramp_no_async_wrap_call() {
    // A plain generator's ramp should NOT have async_wrap
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen_fn", vec![block], true, false, 3, 1);
    let (liveness, _split_result) = analyze_and_split(&mut func);

    let result = codegen::rewrite_as_ramp(&mut func, &liveness, 1);
    assert!(result.is_ok());

    // Only 1 CallRuntime (create_generator only, no async_wrap)
    let call_runtime_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .count();
    assert_eq!(
        call_runtime_count, 1,
        "generator ramp should have only 1 CallRuntime (create_generator)"
    );

    // Should NOT have the async_wrap sentinel
    let const_strings: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        !const_strings.contains(&(u32::MAX - 7)),
        "generator ramp should NOT reference async_wrap sentinel"
    );
}

#[test]
fn test_async_ramp_returns_async_wrap_result() {
    // For async functions, the Ret instruction should return the result of async_wrap,
    // not the result of create_generator directly
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_fn", vec![block], false, true, 3, 1);
    let (liveness, _split_result) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).unwrap();

    // Find the Ret instruction
    let ret_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| matches!(i.op, Op::Ret))
        .expect("ramp should have Ret instruction");

    // The Ret's operand should be the result of the second CallRuntime (async_wrap)
    let second_call_runtime = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .nth(1)
        .expect("should have second CallRuntime for async_wrap");

    assert_eq!(
        ret_inst.operands[0], second_call_runtime.id,
        "Ret should return async_wrap result, not create_generator result"
    );
}

// ===========================================================================
// Block target remapping tests (CFG panic fix)
// ===========================================================================

#[test]
fn test_resume_blocks_have_remapped_block_targets() {
    // A generator with a BrIf (control flow) inside the segment should have
    // its block targets remapped to the resume function's block namespace.
    // This tests the fix for the "add_predecessor: block not found" panic.
    let blocks = vec![
        make_block(
            0,
            vec![
                make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
                make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
                // After yield: instructions in resume block
                make_inst(2, Op::ConstBool(true), IrType::Bool, vec![], vec![]),
                make_inst(3, Op::BrIf, IrType::Void, vec![2], vec![1, 2]),
            ],
            vec![],
        ),
        make_block(
            1,
            vec![make_inst(4, Op::ConstI32(99), IrType::I32, vec![], vec![])],
            vec![0],
        ),
        make_block(
            2,
            vec![make_inst(5, Op::Ret, IrType::Void, vec![0], vec![])],
            vec![0],
        ),
    ];

    let mut func = build_function("gen_branch", blocks, true, false, 6, 3);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("resume generation should succeed");

    // All blocks in the resume function should only reference blocks that exist
    let all_block_ids: std::collections::HashSet<BlockId> =
        resume.blocks.iter().map(|b| b.id).collect();

    for block in &resume.blocks {
        for inst in &block.instructions {
            for target in &inst.block_targets {
                assert!(
                    all_block_ids.contains(target),
                    "block {} has instruction with target {} that doesn't exist in resume function",
                    block.id.0,
                    target.0
                );
            }
        }
    }
}

#[test]
fn test_resume_all_blocks_have_terminators() {
    // Every block in the resume function must end with a terminator instruction.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::ConstI32(2), IrType::I32, vec![], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::ConstI32(3), IrType::I32, vec![], vec![]),
            make_inst(5, Op::Yield, IrType::JSValue, vec![4], vec![]),
            make_inst(6, Op::Ret, IrType::Void, vec![5], vec![]),
        ],
        vec![],
    );

    let mut func = build_function("gen_multi_yield", vec![block], true, false, 7, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("resume generation should succeed");

    for block in &resume.blocks {
        let last = block.instructions.last();
        assert!(
            last.is_some_and(|i| i.op.is_terminator()),
            "block bb{} in resume function does not end with a terminator",
            block.id.0
        );
    }
}

#[test]
fn test_ramp_boxes_resume_func_idx() {
    // The ramp function should NaN-box the resume_func_idx before passing
    // it to __esc_rt_create_generator via CallRuntime.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("gen", vec![block], true, false, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 5).expect("rewrite should succeed");

    // Find the BoxI32 instruction that boxes the resume function index
    let box_i32_insts: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::BoxI32))
        .collect();

    assert!(
        !box_i32_insts.is_empty(),
        "ramp function should have BoxI32 to NaN-box resume_func_idx"
    );

    // Find the CallRuntime for create_generator
    let call_runtime = func.blocks[0]
        .instructions
        .iter()
        .find(|i| matches!(i.op, Op::CallRuntime))
        .expect("ramp should have CallRuntime");

    // The resume_func_idx argument (operand index 2) should reference a BoxI32 result
    let resume_arg = call_runtime.operands[2];
    let is_boxed = box_i32_insts.iter().any(|bi| bi.id == resume_arg);
    assert!(
        is_boxed,
        "resume_func_idx argument to CallRuntime should be a BoxI32 result"
    );
}

// ===========================================================================
// Async generator codegen tests (is_async && is_generator)
// ===========================================================================

#[test]
fn test_async_generator_flags_both_set() {
    // async function* should have both is_async and is_generator set
    let func = build_function("async_gen", vec![], true, true, 0, 0);
    assert!(func.is_async, "async generator must have is_async = true");
    assert!(
        func.is_generator,
        "async generator must have is_generator = true"
    );
}

#[test]
fn test_async_generator_ramp_creates_async_generator() {
    // The ramp for an async generator should call create_generator then
    // create_async_generator (2 CallRuntime calls), using the
    // u32::MAX - 8 sentinel for create_async_generator.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Should have exactly 2 CallRuntime instructions:
    // 1. create_generator (u32::MAX sentinel)
    // 2. create_async_generator (u32::MAX - 8 sentinel)
    let call_runtime_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .count();
    assert_eq!(
        call_runtime_count, 2,
        "async generator ramp should have 2 CallRuntime (create_generator + create_async_generator)"
    );

    // Should have the create_async_generator sentinel (u32::MAX - 8)
    let const_strings: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        const_strings.contains(&(u32::MAX - 8)),
        "async generator ramp should reference create_async_generator sentinel (u32::MAX - 8)"
    );
}

#[test]
fn test_async_generator_ramp_no_async_wrap() {
    // Async generators should NOT use async_wrap (u32::MAX - 7) — that's for
    // plain async functions. They use create_async_generator (u32::MAX - 8).
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let const_strings: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        !const_strings.contains(&(u32::MAX - 7)),
        "async generator ramp should NOT reference async_wrap sentinel (u32::MAX - 7)"
    );
}

#[test]
fn test_async_generator_ramp_returns_async_generator_result() {
    // The Ret in an async generator ramp should return the result of
    // create_async_generator (second CallRuntime), not create_generator.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let ret_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| matches!(i.op, Op::Ret))
        .expect("ramp should have Ret instruction");

    let second_call_runtime = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .nth(1)
        .expect("should have second CallRuntime for create_async_generator");

    assert_eq!(
        ret_inst.operands[0], second_call_runtime.id,
        "Ret should return create_async_generator result, not create_generator result"
    );
}

#[test]
fn test_async_generator_ramp_creates_state_object() {
    // Async generator ramp should still create a state object, just like
    // sync generators and async functions.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let has_create_object = func.blocks[0]
        .instructions
        .iter()
        .any(|i| matches!(i.op, Op::CreateObject));
    assert!(
        has_create_object,
        "async generator ramp must create a state object"
    );
}

#[test]
fn test_async_generator_resume_unchanged() {
    // The resume function for an async generator should be identical to a
    // sync generator's resume — same state machine, same params, not marked
    // as async or generator.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    assert_eq!(resume.name, "async_gen_resume");
    assert!(
        !resume.is_generator,
        "resume must not be marked as generator"
    );
    assert!(!resume.is_async, "resume must not be marked as async");
    assert_eq!(resume.params.len(), 3);
    assert_eq!(resume.params[0].0, "state");
    assert_eq!(resume.params[1].0, "sent_value");
    assert_eq!(resume.params[2].0, "resume_mode");
}

#[test]
fn test_async_generator_yield_produces_yield_opcode() {
    // yield inside an async generator should produce the Yield opcode
    // (same as in a sync generator).
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("async_gen", vec![block], true, true, 3, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);
    assert!(
        matches!(points[0].op, Op::Yield),
        "yield in async generator should produce Yield opcode"
    );
}

#[test]
fn test_async_generator_await_produces_await_opcode() {
    // await inside an async generator should produce the Await opcode
    // (same as in a plain async function).
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("async_gen", vec![block], true, true, 3, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(points.len(), 1);
    assert!(
        matches!(points[0].op, Op::Await),
        "await in async generator should produce Await opcode"
    );
}

#[test]
fn test_async_generator_mixed_yield_and_await() {
    // An async generator can have both Yield and Await suspension points.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Await, IrType::JSValue, vec![1], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("async_gen", vec![block], true, true, 5, 1);

    let points = analysis::discover_suspension_points(&func);
    assert_eq!(
        points.len(),
        3,
        "async generator with 2 yields and 1 await should have 3 suspension points"
    );
    assert!(matches!(points[0].op, Op::Yield));
    assert!(matches!(points[1].op, Op::Await));
    assert!(matches!(points[2].op, Op::Yield));
}

#[test]
fn test_transform_module_handles_async_generator() {
    // transform_module should process async generators (is_async && is_generator)
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let func = build_function("async_gen", vec![block], true, true, 3, 1);
    let mut module = build_module(func);

    assert_eq!(module.functions.len(), 1);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(
        results.len(),
        1,
        "async generator should be processed by transform_module"
    );

    // Module should have 2 functions: ramp + resume
    assert_eq!(module.functions.len(), 2);
    assert_eq!(module.functions[0].name, "async_gen");
    assert_eq!(module.functions[1].name, "async_gen_resume");

    // Ramp should have the create_async_generator sentinel (u32::MAX - 8)
    let const_strings: Vec<_> = module.functions[0].blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) => Some(idx),
            _ => None,
        })
        .collect();
    assert!(
        const_strings.contains(&(u32::MAX - 8)),
        "async generator ramp should use create_async_generator sentinel"
    );
}

#[test]
fn test_async_generator_ramp_saves_params() {
    // async function* gen(a, b) { yield a; }
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::LoadParam(0), IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::LoadParam(1), IrType::JSValue, vec![], vec![]),
            make_inst(2, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function_with_params(
        "async_gen",
        vec![("a", IrType::JSValue), ("b", IrType::JSValue)],
        vec![block],
        true,
        true,
        4,
        1,
    );
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    // Should have LoadParam for saving params
    let load_param_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::LoadParam(_)))
        .count();
    assert_eq!(
        load_param_count, 2,
        "async generator ramp must load both parameters"
    );

    // SetProp: 2 (state_index, resume_mode) + 2 (params) = 4
    let set_prop_count = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::SetProp))
        .count();
    assert_eq!(
        set_prop_count, 4,
        "async generator ramp must SetProp for state_index, resume_mode, and each param"
    );
}

#[test]
fn test_async_generator_ramp_has_single_block() {
    // The ramp function should always have exactly 1 block.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    assert_eq!(
        func.blocks.len(),
        1,
        "async generator ramp should have exactly 1 block"
    );
}

#[test]
fn test_async_generator_ramp_ends_with_ret() {
    // The ramp should always end with a Ret instruction.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let last_op = func.blocks[0].instructions.last().map(|i| &i.op);
    assert!(
        matches!(last_op, Some(Op::Ret)),
        "async generator ramp must end with Ret"
    );
}

#[test]
fn test_async_generator_ramp_create_generator_first() {
    // The ramp should call create_generator FIRST, then pass the result to
    // create_async_generator.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 3, 1);
    let (liveness, _split) = analyze_and_split(&mut func);

    codegen::rewrite_as_ramp(&mut func, &liveness, 1).expect("rewrite should succeed");

    let call_runtimes: Vec<_> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .collect();
    assert_eq!(call_runtimes.len(), 2);

    // The second CallRuntime (create_async_generator) should reference the
    // result of the first CallRuntime (create_generator) as its operand.
    let first_rt_id = call_runtimes[0].id;
    let second_operands = &call_runtimes[1].operands;
    assert!(
        second_operands.contains(&first_rt_id),
        "create_async_generator should take create_generator result as operand"
    );
}

#[test]
fn test_async_generator_resume_all_blocks_terminated() {
    // The resume function for an async generator should have all blocks
    // properly terminated, same as for sync generators.
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Await, IrType::JSValue, vec![1], vec![]),
            make_inst(3, Op::Ret, IrType::Void, vec![2], vec![]),
        ],
        vec![],
    );
    let mut func = build_function("async_gen", vec![block], true, true, 4, 1);
    let (liveness, split_result) = analyze_and_split(&mut func);

    let resume = codegen::generate_resume_function(&func, &liveness, &split_result)
        .expect("codegen should succeed");

    for block in &resume.blocks {
        assert!(
            !block.instructions.is_empty(),
            "block {} should not be empty",
            block.id
        );
        let last = block.instructions.last().expect("block has instructions");
        assert!(
            last.op.is_terminator(),
            "block {} in async generator resume must end with a terminator, got {:?}",
            block.id,
            last.op
        );
    }
}

#[test]
fn test_async_generator_vs_plain_generator_ramp_difference() {
    // Compare: a plain generator ramp has 1 CallRuntime, an async generator has 2.
    // The async generator uses u32::MAX - 8, not u32::MAX - 7.
    let block_gen = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func_gen = build_function("gen", vec![block_gen], true, false, 3, 1);
    let (liveness_gen, _) = analyze_and_split(&mut func_gen);
    codegen::rewrite_as_ramp(&mut func_gen, &liveness_gen, 1).expect("gen rewrite");

    let block_ag = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func_ag = build_function("async_gen", vec![block_ag], true, true, 3, 1);
    let (liveness_ag, _) = analyze_and_split(&mut func_ag);
    codegen::rewrite_as_ramp(&mut func_ag, &liveness_ag, 1).expect("ag rewrite");

    let gen_call_count = func_gen.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .count();
    let ag_call_count = func_ag.blocks[0]
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::CallRuntime))
        .count();

    assert_eq!(gen_call_count, 1, "plain generator: 1 CallRuntime");
    assert_eq!(ag_call_count, 2, "async generator: 2 CallRuntime");
}

#[test]
fn test_async_generator_vs_async_function_ramp_difference() {
    // Compare: an async function ramp uses async_wrap (u32::MAX - 7),
    // an async generator ramp uses create_async_generator (u32::MAX - 8).
    let block_af = make_block(
        0,
        vec![
            make_inst(0, Op::ConstUndefined, IrType::JSValue, vec![], vec![]),
            make_inst(1, Op::Await, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func_af = build_function("async_fn", vec![block_af], false, true, 3, 1);
    let (liveness_af, _) = analyze_and_split(&mut func_af);
    codegen::rewrite_as_ramp(&mut func_af, &liveness_af, 1).expect("af rewrite");

    let block_ag = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(42), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Ret, IrType::Void, vec![1], vec![]),
        ],
        vec![],
    );
    let mut func_ag = build_function("async_gen", vec![block_ag], true, true, 3, 1);
    let (liveness_ag, _) = analyze_and_split(&mut func_ag);
    codegen::rewrite_as_ramp(&mut func_ag, &liveness_ag, 1).expect("ag rewrite");

    let af_sentinels: Vec<_> = func_af.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) if idx >= u32::MAX - 20 => Some(idx),
            _ => None,
        })
        .collect();
    let ag_sentinels: Vec<_> = func_ag.blocks[0]
        .instructions
        .iter()
        .filter_map(|i| match i.op {
            Op::ConstString(idx) if idx >= u32::MAX - 20 => Some(idx),
            _ => None,
        })
        .collect();

    assert!(
        af_sentinels.contains(&(u32::MAX - 7)),
        "async function should use async_wrap sentinel"
    );
    assert!(
        !af_sentinels.contains(&(u32::MAX - 8)),
        "async function should NOT use create_async_generator sentinel"
    );
    assert!(
        ag_sentinels.contains(&(u32::MAX - 8)),
        "async generator should use create_async_generator sentinel"
    );
    assert!(
        !ag_sentinels.contains(&(u32::MAX - 7)),
        "async generator should NOT use async_wrap sentinel"
    );
}

#[test]
fn test_transform_module_async_generator_with_await_and_yield() {
    // Full integration: async function* with both yield and await
    let block = make_block(
        0,
        vec![
            make_inst(0, Op::ConstI32(1), IrType::I32, vec![], vec![]),
            make_inst(1, Op::Yield, IrType::JSValue, vec![0], vec![]),
            make_inst(2, Op::Await, IrType::JSValue, vec![1], vec![]),
            make_inst(3, Op::Yield, IrType::JSValue, vec![2], vec![]),
            make_inst(4, Op::Ret, IrType::Void, vec![3], vec![]),
        ],
        vec![],
    );
    let func = build_function("async_gen", vec![block], true, true, 5, 1);
    let mut module = build_module(func);

    let results = crate::transform_module(&mut module).expect("transform should succeed");
    assert_eq!(results.len(), 1);

    // Module should have 2 functions: ramp + resume
    assert_eq!(module.functions.len(), 2);

    // Resume should have enough blocks for 3 suspension points + scaffolding
    let resume = &module.functions[1];
    assert!(
        resume.blocks.len() >= 7,
        "async generator resume with 3 suspension points should have at least 7 blocks, got {}",
        resume.blocks.len()
    );

    // All blocks should be properly terminated
    for block in &resume.blocks {
        let last = block.instructions.last();
        assert!(
            last.is_some_and(|i| i.op.is_terminator()),
            "block bb{} in resume should end with a terminator",
            block.id.0
        );
    }
}
