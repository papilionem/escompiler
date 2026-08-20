//! IR verifier — validates structural invariants of SSA IR functions.
//!
//! Provides verification for both the legacy `Function` type and the new
//! `TypedFunction`/`TypedModule` types.

use std::collections::HashSet;

use thiserror::Error;

use crate::builder::{TypedFunction, TypedModule};
use crate::{BlockId, Function, Op, ValueId};

// ---------------------------------------------------------------------------
// VerifyError
// ---------------------------------------------------------------------------

/// An error discovered during IR verification.
#[derive(Debug, Clone, Error)]
#[error("{kind:?}: {message}")]
pub struct VerifyError {
    /// The category of verification failure.
    pub kind: VerifyErrorKind,
    /// Human-readable description of the error.
    pub message: String,
}

/// Classification of IR verification failures.
#[derive(Debug, Clone, PartialEq)]
pub enum VerifyErrorKind {
    /// Structural problem (empty blocks, missing functions).
    StructuralError,
    /// A referenced value was never defined.
    UndefinedValue,
    /// A value is used before it is defined in the SSA dominance order.
    UseBeforeDef,
    /// An instruction's operand or result has an unexpected type.
    TypeMismatch,
    /// A phi node has an incorrect number of operands or predecessor mismatch.
    InvalidPhi,
    /// A block has no path from the entry block.
    UnreachableBlock,
    /// A block does not end with a terminator instruction.
    InvalidTerminator,
}

// ---------------------------------------------------------------------------
// Legacy verifier (unchanged API)
// ---------------------------------------------------------------------------

/// Verify structural integrity of an IR function (legacy type system).
///
/// Returns `Ok(())` if the function is well-formed, or a list of error messages.
pub fn verify_function(_func: &Function) -> Result<(), Vec<String>> {
    // TODO: implement verification passes for legacy types
    Ok(())
}

// ---------------------------------------------------------------------------
// Typed verifier — TypedFunction
// ---------------------------------------------------------------------------

/// Verify structural integrity of a typed IR function.
///
/// Runs 7 verification passes and collects all errors found.
pub fn verify_typed_function(func: &TypedFunction) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();

    // Pass 1: Structural — function has at least one block, blocks are non-empty
    verify_structure(func, &mut errors);

    // Pass 2: Value definitions — collect all defined ValueIds
    let defined = collect_definitions(func);

    // Pass 3: Use-before-def — all operands reference defined values
    verify_uses(func, &defined, &mut errors);

    // Pass 4: Terminator consistency — every block ends with a terminator
    verify_terminators(func, &mut errors);

    // Pass 5: Block targets — branch targets reference valid blocks
    verify_block_targets(func, &mut errors);

    // Pass 6: Phi validation — phi nodes at the start of blocks, correct predecessor count
    verify_phis(func, &mut errors);

    // Pass 7: Basic type checks — return type matches ret instruction
    verify_basic_types(func, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verify a typed module.
pub fn verify_typed_module(module: &TypedModule) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    for func in &module.functions {
        if let Err(func_errors) = verify_typed_function(func) {
            errors.extend(func_errors);
        }
    }
    if let Some(entry) = module.entry
        && entry >= module.functions.len()
    {
        errors.push(VerifyError {
            kind: VerifyErrorKind::StructuralError,
            message: format!(
                "entry index {} out of bounds (module has {} functions)",
                entry,
                module.functions.len()
            ),
        });
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Pass 1: Structural verification
// ---------------------------------------------------------------------------

fn verify_structure(func: &TypedFunction, errors: &mut Vec<VerifyError>) {
    if func.blocks.is_empty() {
        errors.push(VerifyError {
            kind: VerifyErrorKind::StructuralError,
            message: format!("function '{}' has no blocks", func.name),
        });
        return;
    }
    for block in &func.blocks {
        if block.instructions.is_empty() {
            errors.push(VerifyError {
                kind: VerifyErrorKind::StructuralError,
                message: format!("block {} in function '{}' is empty", block.id, func.name),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 2: Collect all defined ValueIds
// ---------------------------------------------------------------------------

fn collect_definitions(func: &TypedFunction) -> HashSet<ValueId> {
    let mut defined = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            defined.insert(inst.id);
        }
    }
    defined
}

// ---------------------------------------------------------------------------
// Pass 3: Use-before-def
// ---------------------------------------------------------------------------

fn verify_uses(func: &TypedFunction, defined: &HashSet<ValueId>, errors: &mut Vec<VerifyError>) {
    for block in &func.blocks {
        for inst in &block.instructions {
            for operand in &inst.operands {
                if !defined.contains(operand) {
                    errors.push(VerifyError {
                        kind: VerifyErrorKind::UndefinedValue,
                        message: format!(
                            "instruction %{} in {} uses undefined value %{}",
                            inst.id, block.id, operand
                        ),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 4: Terminator consistency
// ---------------------------------------------------------------------------

fn verify_terminators(func: &TypedFunction, errors: &mut Vec<VerifyError>) {
    for block in &func.blocks {
        if block.instructions.is_empty() {
            // Already caught by structural check
            continue;
        }
        // Safe: we just checked that instructions is non-empty above.
        let Some(last_inst) = block.instructions.last() else {
            continue;
        };
        let last = &last_inst.op;
        if !last.is_terminator() {
            errors.push(VerifyError {
                kind: VerifyErrorKind::InvalidTerminator,
                message: format!(
                    "block {} in function '{}' does not end with a terminator",
                    block.id, func.name
                ),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 5: Block targets — branch targets reference valid blocks
// ---------------------------------------------------------------------------

fn verify_block_targets(func: &TypedFunction, errors: &mut Vec<VerifyError>) {
    let valid_blocks: HashSet<BlockId> = func.blocks.iter().map(|b| b.id).collect();
    for block in &func.blocks {
        for inst in &block.instructions {
            for target in &inst.block_targets {
                if !valid_blocks.contains(target) {
                    errors.push(VerifyError {
                        kind: VerifyErrorKind::StructuralError,
                        message: format!(
                            "instruction %{} in {} references invalid block target {}",
                            inst.id, block.id, target
                        ),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 6: Phi validation
// ---------------------------------------------------------------------------

fn verify_phis(func: &TypedFunction, errors: &mut Vec<VerifyError>) {
    for block in &func.blocks {
        let mut seen_non_phi = false;
        for inst in &block.instructions {
            if inst.op == Op::Phi {
                if seen_non_phi {
                    errors.push(VerifyError {
                        kind: VerifyErrorKind::InvalidPhi,
                        message: format!(
                            "phi %{} in {} appears after non-phi instruction",
                            inst.id, block.id
                        ),
                    });
                }
                // Check operand count matches predecessor count
                if !block.predecessors.is_empty() && inst.operands.len() != block.predecessors.len()
                {
                    errors.push(VerifyError {
                        kind: VerifyErrorKind::InvalidPhi,
                        message: format!(
                            "phi %{} in {} has {} operands but block has {} predecessors",
                            inst.id,
                            block.id,
                            inst.operands.len(),
                            block.predecessors.len()
                        ),
                    });
                }
            } else {
                seen_non_phi = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 7: Basic type checks
// ---------------------------------------------------------------------------

fn verify_basic_types(func: &TypedFunction, errors: &mut Vec<VerifyError>) {
    // Check that functions with non-void return type have at least one Ret
    let has_ret = func
        .blocks
        .iter()
        .any(|b| b.instructions.iter().any(|i| i.op == Op::Ret));

    if !has_ret && !func.blocks.is_empty() {
        // Only warn if there are blocks but none have a return
        // (functions with Throw/Unreachable as terminators are also valid)
        let has_non_return_exit = func.blocks.iter().any(|b| {
            b.instructions
                .last()
                .is_some_and(|i| matches!(i.op, Op::Throw | Op::Rethrow | Op::Unreachable))
        });
        if !has_non_return_exit {
            errors.push(VerifyError {
                kind: VerifyErrorKind::TypeMismatch,
                message: format!("function '{}' has no return or exit terminator", func.name),
            });
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================
