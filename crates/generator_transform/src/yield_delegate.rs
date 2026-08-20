//! yield* delegation desugaring.
//!
//! `yield* expr` is desugared into a forwarding loop that delegates to the
//! inner iterable. This module provides a pre-pass that rewrites
//! `YieldDelegate` opcodes into explicit iteration loops with normal `Yield`
//! instructions, so the main generator transform only needs to handle `Yield`.
//!
//! ## Desugaring
//!
//! ```text
//! result = yield* inner();
//! ```
//!
//! becomes (roughly):
//!
//! ```text
//! let iter = __esc_rt_iter_init(inner());
//! let next_val = undefined;
//! loop {
//!     let step = __esc_rt_iter_next(iter);
//!     let done = __esc_rt_iter_done(step);
//!     if done {
//!         result = __esc_rt_iter_value(step);
//!         break;
//!     }
//!     let yielded = __esc_rt_iter_value(step);
//!     next_val = yield yielded;
//! }
//! ```
//!
//! The full ES spec version also handles `.throw()` and `.return()` forwarding,
//! but for v0.4 we implement the simplified version above that covers the
//! common case of delegating to another generator or iterable.

use ir::builder::{TypedBasicBlock, TypedFunction};
use ir::types::TypedInstruction;
use ir::{BlockId, IrType, Op, ValueId};

use crate::TransformError;

/// Rewrite all `YieldDelegate` opcodes in a function into explicit delegation loops.
///
/// Scans each block for `YieldDelegate` instructions and replaces them with
/// an iteration loop that yields each inner value and collects the final result.
///
/// This pass must run before suspension point discovery, because it replaces
/// `YieldDelegate` with regular `Yield` instructions that the main transform
/// handles.
///
/// # Errors
///
/// Returns [`TransformError`] if a yield delegate is in an invalid position.
pub fn desugar_yield_delegate(func: &mut TypedFunction) -> Result<bool, TransformError> {
    // Collect positions of YieldDelegate instructions
    let mut delegates: Vec<(BlockId, usize)> = Vec::new();
    for block in &func.blocks {
        for (idx, inst) in block.instructions.iter().enumerate() {
            if matches!(inst.op, Op::YieldDelegate) {
                delegates.push((block.id, idx));
            }
        }
    }

    if delegates.is_empty() {
        return Ok(false);
    }

    // Process each YieldDelegate in reverse order (so indices remain valid)
    for &(block_id, instr_idx) in delegates.iter().rev() {
        rewrite_single_delegate(func, block_id, instr_idx)?;
    }

    Ok(true)
}

/// Rewrite a single `YieldDelegate` instruction into an iteration loop.
///
/// Splits the containing block at the `YieldDelegate` position and inserts:
/// 1. An init block (calls `iter_init`)
/// 2. A loop header (calls `iter_next`, checks done)
/// 3. A yield block (yields inner value, loops back)
/// 4. A done block (extracts final value, continues to remainder)
fn rewrite_single_delegate(
    func: &mut TypedFunction,
    block_id: BlockId,
    instr_idx: usize,
) -> Result<(), TransformError> {
    // Find the block
    let block_pos =
        func.blocks
            .iter()
            .position(|b| b.id == block_id)
            .ok_or(TransformError::InvalidBlock {
                index: 0,
                block_id: block_id.0,
            })?;

    let orig_block = &func.blocks[block_pos];
    let delegate_inst = &orig_block.instructions[instr_idx];
    let delegate_id = delegate_inst.id;
    let iterable_operand = delegate_inst.operands.first().copied();
    let span = delegate_inst.span;

    // Split instructions: before the delegate, and after the delegate
    let before_insts: Vec<TypedInstruction> = orig_block.instructions[..instr_idx].to_vec();
    let after_insts: Vec<TypedInstruction> = orig_block.instructions[instr_idx + 1..].to_vec();
    let orig_predecessors = orig_block.predecessors.clone();

    // Allocate new blocks and values
    let loop_header_id = BlockId(func.next_block);
    func.next_block += 1;
    let yield_block_id = BlockId(func.next_block);
    func.next_block += 1;
    let done_block_id = BlockId(func.next_block);
    func.next_block += 1;
    let continue_block_id = BlockId(func.next_block);
    func.next_block += 1;

    // --- Rewrite the original block: before + iter_init + branch to loop_header ---
    let mut init_insts = before_insts;

    // iter = CallRuntime("iter_init", iterable)
    let iterable_val = iterable_operand.unwrap_or_else(|| {
        let id = ValueId(func.next_value);
        func.next_value += 1;
        init_insts.push(TypedInstruction {
            id,
            op: Op::ConstUndefined,
            ty: IrType::JSValue,
            operands: vec![],
            block_targets: vec![],
            span,
        });
        id
    });

    let iter_init_name = alloc_value(func);
    init_insts.push(TypedInstruction {
        id: iter_init_name,
        op: Op::ConstString(u32::MAX - 3), // sentinel for "iter_init"
        ty: IrType::JSString,
        operands: vec![],
        block_targets: vec![],
        span,
    });

    let iter_val = alloc_value(func);
    init_insts.push(TypedInstruction {
        id: iter_val,
        op: Op::CallRuntime,
        ty: IrType::JSValue,
        operands: vec![iter_init_name, iterable_val],
        block_targets: vec![],
        span,
    });

    // Branch to loop header
    let br_to_loop = alloc_value(func);
    init_insts.push(TypedInstruction {
        id: br_to_loop,
        op: Op::Br,
        ty: IrType::Void,
        operands: vec![],
        block_targets: vec![loop_header_id],
        span,
    });

    func.blocks[block_pos] = TypedBasicBlock {
        id: block_id,
        instructions: init_insts,
        sealed: true,
        predecessors: orig_predecessors,
    };

    // --- Loop header block: call iter_next, check done ---
    let mut header_insts = Vec::new();

    // step = CallRuntime("iter_next", iter)
    let iter_next_name = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: iter_next_name,
        op: Op::ConstString(u32::MAX - 4), // sentinel for "iter_next"
        ty: IrType::JSString,
        operands: vec![],
        block_targets: vec![],
        span,
    });

    let step = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: step,
        op: Op::CallRuntime,
        ty: IrType::JSValue,
        operands: vec![iter_next_name, iter_val],
        block_targets: vec![],
        span,
    });

    // done = CallRuntime("iter_done", step)
    let iter_done_name = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: iter_done_name,
        op: Op::ConstString(u32::MAX - 5), // sentinel for "iter_done"
        ty: IrType::JSString,
        operands: vec![],
        block_targets: vec![],
        span,
    });

    let done_raw = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: done_raw,
        op: Op::CallRuntime,
        ty: IrType::JSValue,
        operands: vec![iter_done_name, step],
        block_targets: vec![],
        span,
    });

    let is_done = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: is_done,
        op: Op::ToBoolean,
        ty: IrType::Bool,
        operands: vec![done_raw],
        block_targets: vec![],
        span,
    });

    // BrIf(is_done, done_block, yield_block)
    let br_check = alloc_value(func);
    header_insts.push(TypedInstruction {
        id: br_check,
        op: Op::BrIf,
        ty: IrType::Void,
        operands: vec![is_done],
        block_targets: vec![done_block_id, yield_block_id],
        span,
    });

    func.blocks.push(TypedBasicBlock {
        id: loop_header_id,
        instructions: header_insts,
        sealed: true,
        predecessors: vec![block_id, yield_block_id],
    });

    // --- Yield block: extract value from step, yield it, loop back ---
    let mut yield_insts = Vec::new();

    // yielded = CallRuntime("iter_value", step)
    let iter_value_name = alloc_value(func);
    yield_insts.push(TypedInstruction {
        id: iter_value_name,
        op: Op::ConstString(u32::MAX - 6), // sentinel for "iter_value"
        ty: IrType::JSString,
        operands: vec![],
        block_targets: vec![],
        span,
    });

    let yielded = alloc_value(func);
    yield_insts.push(TypedInstruction {
        id: yielded,
        op: Op::CallRuntime,
        ty: IrType::JSValue,
        operands: vec![iter_value_name, step],
        block_targets: vec![],
        span,
    });

    // next_val = yield yielded
    let yield_result = alloc_value(func);
    yield_insts.push(TypedInstruction {
        id: yield_result,
        op: Op::Yield,
        ty: IrType::JSValue,
        operands: vec![yielded],
        block_targets: vec![],
        span,
    });

    // Branch back to loop header
    let br_back = alloc_value(func);
    yield_insts.push(TypedInstruction {
        id: br_back,
        op: Op::Br,
        ty: IrType::Void,
        operands: vec![],
        block_targets: vec![loop_header_id],
        span,
    });

    func.blocks.push(TypedBasicBlock {
        id: yield_block_id,
        instructions: yield_insts,
        sealed: true,
        predecessors: vec![loop_header_id],
    });

    // --- Done block: extract final value, branch to continue ---
    let mut done_insts = Vec::new();

    // result_value = CallRuntime("iter_value", step)
    let iter_value_name2 = alloc_value(func);
    done_insts.push(TypedInstruction {
        id: iter_value_name2,
        op: Op::ConstString(u32::MAX - 6), // sentinel for "iter_value"
        ty: IrType::JSString,
        operands: vec![],
        block_targets: vec![],
        span,
    });

    let final_value = alloc_value(func);
    done_insts.push(TypedInstruction {
        id: final_value,
        op: Op::CallRuntime,
        ty: IrType::JSValue,
        operands: vec![iter_value_name2, step],
        block_targets: vec![],
        span,
    });

    // Branch to continue block
    let br_continue = alloc_value(func);
    done_insts.push(TypedInstruction {
        id: br_continue,
        op: Op::Br,
        ty: IrType::Void,
        operands: vec![],
        block_targets: vec![continue_block_id],
        span,
    });

    func.blocks.push(TypedBasicBlock {
        id: done_block_id,
        instructions: done_insts,
        sealed: true,
        predecessors: vec![loop_header_id],
    });

    // --- Continue block: remap delegate result to final_value, add remaining instructions ---
    let continue_insts: Vec<TypedInstruction> = after_insts
        .into_iter()
        .map(|mut inst| {
            // Remap references to the old delegate result to the final_value
            for op in &mut inst.operands {
                if *op == delegate_id {
                    *op = final_value;
                }
            }
            inst
        })
        .collect();

    // If there are no instructions after the delegate (no terminator), add a fallthrough
    if continue_insts.is_empty() || continue_insts.last().is_some_and(|i| !i.op.is_terminator()) {
        // This block has no terminator — it will be handled by the caller
        // (the generator transform will add one if needed).
    }

    func.blocks.push(TypedBasicBlock {
        id: continue_block_id,
        instructions: continue_insts,
        sealed: true,
        predecessors: vec![done_block_id],
    });

    Ok(())
}

/// Allocate a fresh `ValueId` from the function.
fn alloc_value(func: &mut TypedFunction) -> ValueId {
    let id = ValueId(func.next_value);
    func.next_value += 1;
    id
}
