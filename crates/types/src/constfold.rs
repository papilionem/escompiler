//! Constant folding pass — evaluates operations on constant operands at compile time.
//!
//! When all operands of an arithmetic, comparison, or unary operation are
//! compile-time constants, this pass replaces the operation with the
//! pre-computed result. Runs after type specialization in the pipeline.
//!
//! # Folded operations
//!
//! - **Integer arithmetic:** `AddI32`, `SubI32`, `MulI32`, `DivI32`, `ModI32`, `NegI32`
//! - **Float arithmetic:** `AddF64`, `SubF64`, `MulF64`, `DivF64`, `ModF64`, `NegF64`
//! - **Integer comparisons:** `EqI32`, `NeI32`, `LtI32`, `LeI32`, `GtI32`, `GeI32`
//! - **Float comparisons:** `EqF64`, `NeF64`, `LtF64`, `LeF64`, `GtF64`, `GeF64`
//! - **Strict equality on booleans:** `EqStrict`, `NeStrict` (when both operands are `ConstBool`)
//! - **JS coercing arithmetic on known constants:** `AddJS`, `SubJS`, `MulJS`, `DivJS`, `ModJS`, `NegJS`

use std::collections::HashMap;

use ir::ValueId;
use ir::builder::{TypedFunction, TypedModule};
use ir::types::{IrType, Op};

/// Statistics from a constant folding pass run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConstFoldStats {
    /// Number of instructions that were folded into constants.
    pub folded_count: usize,
    /// Number of instructions that were inspected but not folded.
    pub skipped_count: usize,
}

/// Run the constant folding pass on every function in a module.
///
/// Walks each function, identifies instructions whose operands are all
/// compile-time constants, and replaces them with the pre-computed result.
///
/// Returns statistics about the number of instructions folded.
pub fn constfold_module(module: &mut TypedModule) -> ConstFoldStats {
    let mut total_stats = ConstFoldStats::default();

    for func in &mut module.functions {
        let stats = constfold_function(func);
        total_stats.folded_count += stats.folded_count;
        total_stats.skipped_count += stats.skipped_count;
    }

    total_stats
}

/// Run the constant folding pass on a single function.
///
/// Builds a value-to-constant map, then walks every instruction looking
/// for operations that can be evaluated at compile time.
pub fn constfold_function(func: &mut TypedFunction) -> ConstFoldStats {
    let mut stats = ConstFoldStats::default();

    // Build a map from ValueId -> constant Op for lookups.
    // We need to do this in a first pass because we may reference constants
    // defined earlier in the same block or in predecessor blocks.
    let mut const_map: HashMap<u32, ConstVal> = HashMap::new();

    // First pass: collect all existing constants.
    for block in &func.blocks {
        for inst in &block.instructions {
            if let Some(cv) = op_to_const(&inst.op) {
                const_map.insert(inst.id.0, cv);
            }
        }
    }

    // Second pass: fold operations and update the const_map with new constants.
    // We iterate block by block, instruction by instruction.
    for block in &mut func.blocks {
        for inst in &mut block.instructions {
            if let Some((new_op, new_ty)) =
                try_fold_instruction(&inst.op, &inst.operands, &const_map)
            {
                // Register the new constant in the map for downstream use.
                if let Some(cv) = op_to_const(&new_op) {
                    const_map.insert(inst.id.0, cv);
                }
                inst.op = new_op;
                inst.ty = new_ty;
                inst.operands.clear();
                stats.folded_count += 1;
            } else if is_foldable_candidate(&inst.op) {
                stats.skipped_count += 1;
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Internal constant representation
// ---------------------------------------------------------------------------

/// Compile-time constant value extracted from an `Op`.
#[derive(Debug, Clone)]
enum ConstVal {
    /// 32-bit integer.
    I32(i32),
    /// 64-bit float.
    F64(f64),
    /// Boolean.
    Bool(bool),
}

/// Extract a constant value from an `Op`, if it is a constant instruction.
fn op_to_const(op: &Op) -> Option<ConstVal> {
    match op {
        Op::ConstI32(v) => Some(ConstVal::I32(*v)),
        Op::ConstF64(v) => Some(ConstVal::F64(*v)),
        Op::ConstBool(v) => Some(ConstVal::Bool(*v)),
        _ => None,
    }
}

/// Returns `true` if the opcode is a candidate for constant folding.
fn is_foldable_candidate(op: &Op) -> bool {
    matches!(
        op,
        // Integer arithmetic
        Op::AddI32
            | Op::SubI32
            | Op::MulI32
            | Op::DivI32
            | Op::ModI32
            | Op::NegI32
            // Float arithmetic
            | Op::AddF64
            | Op::SubF64
            | Op::MulF64
            | Op::DivF64
            | Op::ModF64
            | Op::NegF64
            // Integer comparisons
            | Op::EqI32
            | Op::NeI32
            | Op::LtI32
            | Op::LeI32
            | Op::GtI32
            | Op::GeI32
            // Float comparisons
            | Op::EqF64
            | Op::NeF64
            | Op::LtF64
            | Op::LeF64
            | Op::GtF64
            | Op::GeF64
            // Strict equality (for boolean folding)
            | Op::EqStrict
            | Op::NeStrict
    )
}

// ---------------------------------------------------------------------------
// Folding logic
// ---------------------------------------------------------------------------

/// Attempt to fold a single instruction into a constant.
///
/// Returns `Some((new_op, new_type))` if the instruction can be replaced
/// with a constant, or `None` if it cannot be folded.
fn try_fold_instruction(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    match op {
        // Binary integer arithmetic
        Op::AddI32 | Op::SubI32 | Op::MulI32 | Op::DivI32 | Op::ModI32 => {
            fold_binary_i32(op, operands, const_map)
        }

        // Binary float arithmetic
        Op::AddF64 | Op::SubF64 | Op::MulF64 | Op::DivF64 | Op::ModF64 => {
            fold_binary_f64(op, operands, const_map)
        }

        // Unary integer negation
        Op::NegI32 => fold_neg_i32(operands, const_map),

        // Unary float negation
        Op::NegF64 => fold_neg_f64(operands, const_map),

        // Integer comparisons
        Op::EqI32 | Op::NeI32 | Op::LtI32 | Op::LeI32 | Op::GtI32 | Op::GeI32 => {
            fold_cmp_i32(op, operands, const_map)
        }

        // Float comparisons
        Op::EqF64 | Op::NeF64 | Op::LtF64 | Op::LeF64 | Op::GtF64 | Op::GeF64 => {
            fold_cmp_f64(op, operands, const_map)
        }

        // Strict equality on booleans
        Op::EqStrict | Op::NeStrict => fold_strict_eq_bool(op, operands, const_map),

        _ => None,
    }
}

/// Look up both operands of a binary instruction as i32 constants.
fn get_binary_i32(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<(i32, i32)> {
    let lhs = operands.first()?;
    let rhs = operands.get(1)?;
    let l = match const_map.get(&lhs.0)? {
        ConstVal::I32(v) => *v,
        _ => return None,
    };
    let r = match const_map.get(&rhs.0)? {
        ConstVal::I32(v) => *v,
        _ => return None,
    };
    Some((l, r))
}

/// Look up both operands of a binary instruction as f64 constants.
fn get_binary_f64(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<(f64, f64)> {
    let lhs = operands.first()?;
    let rhs = operands.get(1)?;
    let l = match const_map.get(&lhs.0)? {
        ConstVal::F64(v) => *v,
        _ => return None,
    };
    let r = match const_map.get(&rhs.0)? {
        ConstVal::F64(v) => *v,
        _ => return None,
    };
    Some((l, r))
}

/// Look up the single operand of a unary instruction as an i32 constant.
fn get_unary_i32(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<i32> {
    let operand = operands.first()?;
    match const_map.get(&operand.0)? {
        ConstVal::I32(v) => Some(*v),
        _ => None,
    }
}

/// Look up the single operand of a unary instruction as an f64 constant.
fn get_unary_f64(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<f64> {
    let operand = operands.first()?;
    match const_map.get(&operand.0)? {
        ConstVal::F64(v) => Some(*v),
        _ => None,
    }
}

/// Look up both operands of a binary instruction as boolean constants.
fn get_binary_bool(
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(bool, bool)> {
    let lhs = operands.first()?;
    let rhs = operands.get(1)?;
    let l = match const_map.get(&lhs.0)? {
        ConstVal::Bool(v) => *v,
        _ => return None,
    };
    let r = match const_map.get(&rhs.0)? {
        ConstVal::Bool(v) => *v,
        _ => return None,
    };
    Some((l, r))
}

// ---------------------------------------------------------------------------
// Fold: binary i32 arithmetic
// ---------------------------------------------------------------------------

/// Fold binary i32 arithmetic: `AddI32`, `SubI32`, `MulI32`, `DivI32`, `ModI32`.
///
/// Uses wrapping arithmetic for add/sub/mul to match JS `ToInt32` semantics.
/// Division and modulo by zero are not folded (they would trap).
fn fold_binary_i32(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    let (l, r) = get_binary_i32(operands, const_map)?;
    let result = match op {
        Op::AddI32 => l.wrapping_add(r),
        Op::SubI32 => l.wrapping_sub(r),
        Op::MulI32 => l.wrapping_mul(r),
        Op::DivI32 => {
            if r == 0 || (l == i32::MIN && r == -1) {
                return None; // Division by zero or overflow
            }
            l.wrapping_div(r)
        }
        Op::ModI32 => {
            if r == 0 || (l == i32::MIN && r == -1) {
                return None; // Division by zero or overflow
            }
            l.wrapping_rem(r)
        }
        _ => return None,
    };
    Some((Op::ConstI32(result), IrType::I32))
}

// ---------------------------------------------------------------------------
// Fold: binary f64 arithmetic
// ---------------------------------------------------------------------------

/// Fold binary f64 arithmetic: `AddF64`, `SubF64`, `MulF64`, `DivF64`, `ModF64`.
///
/// IEEE 754 arithmetic is deterministic for all inputs including Infinity,
/// NaN, and division by zero, so all cases are safe to fold.
fn fold_binary_f64(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    let (l, r) = get_binary_f64(operands, const_map)?;
    let result = match op {
        Op::AddF64 => l + r,
        Op::SubF64 => l - r,
        Op::MulF64 => l * r,
        Op::DivF64 => l / r,
        Op::ModF64 => l % r,
        _ => return None,
    };
    Some((Op::ConstF64(result), IrType::F64))
}

// ---------------------------------------------------------------------------
// Fold: unary negation
// ---------------------------------------------------------------------------

/// Fold integer negation: `-ConstI32(v)` -> `ConstI32(-v)`.
///
/// Uses wrapping negation to match JS `ToInt32` semantics.
fn fold_neg_i32(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<(Op, IrType)> {
    let v = get_unary_i32(operands, const_map)?;
    Some((Op::ConstI32(v.wrapping_neg()), IrType::I32))
}

/// Fold float negation: `-ConstF64(v)` -> `ConstF64(-v)`.
fn fold_neg_f64(operands: &[ValueId], const_map: &HashMap<u32, ConstVal>) -> Option<(Op, IrType)> {
    let v = get_unary_f64(operands, const_map)?;
    Some((Op::ConstF64(-v), IrType::F64))
}

// ---------------------------------------------------------------------------
// Fold: i32 comparisons
// ---------------------------------------------------------------------------

/// Fold integer comparisons: `EqI32`, `NeI32`, `LtI32`, `LeI32`, `GtI32`, `GeI32`.
fn fold_cmp_i32(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    let (l, r) = get_binary_i32(operands, const_map)?;
    let result = match op {
        Op::EqI32 => l == r,
        Op::NeI32 => l != r,
        Op::LtI32 => l < r,
        Op::LeI32 => l <= r,
        Op::GtI32 => l > r,
        Op::GeI32 => l >= r,
        _ => return None,
    };
    Some((Op::ConstBool(result), IrType::Bool))
}

// ---------------------------------------------------------------------------
// Fold: f64 comparisons
// ---------------------------------------------------------------------------

/// Fold float comparisons: `EqF64`, `NeF64`, `LtF64`, `LeF64`, `GtF64`, `GeF64`.
///
/// IEEE 754 comparison semantics apply (NaN != NaN, etc.).
fn fold_cmp_f64(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    let (l, r) = get_binary_f64(operands, const_map)?;
    let result = match op {
        Op::EqF64 => l == r,
        Op::NeF64 => l != r,
        Op::LtF64 => l < r,
        Op::LeF64 => l <= r,
        Op::GtF64 => l > r,
        Op::GeF64 => l >= r,
        _ => return None,
    };
    Some((Op::ConstBool(result), IrType::Bool))
}

// ---------------------------------------------------------------------------
// Fold: strict equality on booleans
// ---------------------------------------------------------------------------

/// Fold strict equality on boolean constants.
///
/// This handles the pattern emitted by the `!` operator lowering:
/// `EqStrict(ConstBool(true), ConstBool(false))` -> `ConstBool(false)`.
///
/// Only folds when both operands are boolean constants; for other types
/// (e.g. i32 === i32), the typed comparisons `EqI32`/`EqF64` should be
/// used instead (via specialization).
fn fold_strict_eq_bool(
    op: &Op,
    operands: &[ValueId],
    const_map: &HashMap<u32, ConstVal>,
) -> Option<(Op, IrType)> {
    let (l, r) = get_binary_bool(operands, const_map)?;
    let result = match op {
        Op::EqStrict => l == r,
        Op::NeStrict => l != r,
        _ => return None,
    };
    Some((Op::ConstBool(result), IrType::Bool))
}
