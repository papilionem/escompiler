//! Type specialization pass — replaces generic JS opcodes with typed variants.
//!
//! When the type inference engine proves that both operands of a binary
//! operation are a specific concrete type (e.g. `F64`), this pass rewrites
//! the generic JS opcode (e.g. `AddJS`) to the specialized native opcode
//! (e.g. `AddF64`), eliminating runtime type checks and coercion.
//!
//! The pass operates in-place on a [`TypedModule`], guided by the
//! [`TypeAnnotations`] side-table produced by [`crate::inference::infer_function`].

use ir::builder::{TypedFunction, TypedModule};
use ir::types::{IrType, Op};

use crate::inference::{TypeAnnotations, infer_function};
use crate::lattice::InferredType;
use crate::trust::TrustCategory;

/// Statistics from a specialization pass run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SpecializationStats {
    /// Number of instructions that were specialized.
    pub specialized_count: usize,
    /// Number of instructions that were inspected but not specialized.
    pub skipped_count: usize,
}

/// Run the specialization pass on every function in a module.
///
/// Infers types for each function, then rewrites generic JS opcodes to
/// type-specific variants where both operands have a proven concrete type.
///
/// Returns statistics about the number of instructions specialized.
pub fn specialize_module(module: &mut TypedModule) -> SpecializationStats {
    let mut total_stats = SpecializationStats::default();

    for func in &mut module.functions {
        let ann = infer_function(func);
        let stats = specialize_function(func, &ann);
        total_stats.specialized_count += stats.specialized_count;
        total_stats.skipped_count += stats.skipped_count;
    }

    total_stats
}

/// Run the specialization pass on a single function using pre-computed annotations.
///
/// Walks every instruction and rewrites generic JS opcodes to specialized
/// variants when operand types are proven concrete.
pub fn specialize_function(func: &mut TypedFunction, ann: &TypeAnnotations) -> SpecializationStats {
    let mut stats = SpecializationStats::default();

    for block in &mut func.blocks {
        for inst in &mut block.instructions {
            if let Some((new_op, new_ty)) = try_specialize_op(&inst.op, &inst.operands, ann) {
                inst.op = new_op;
                inst.ty = new_ty;
                stats.specialized_count += 1;
            } else if is_specializable(&inst.op) {
                stats.skipped_count += 1;
            }
        }
    }

    stats
}

/// Returns `true` if the opcode is a candidate for specialization.
fn is_specializable(op: &Op) -> bool {
    matches!(
        op,
        Op::AddJS
            | Op::SubJS
            | Op::MulJS
            | Op::DivJS
            | Op::ModJS
            | Op::NegJS
            | Op::LtJS
            | Op::LeJS
            | Op::GtJS
            | Op::GeJS
            | Op::EqAbstract
            | Op::NeAbstract
    )
}

/// Attempt to specialize a single instruction.
///
/// Returns `Some((new_op, new_type))` if the instruction can be rewritten,
/// or `None` if it must remain generic.
fn try_specialize_op(
    op: &Op,
    operands: &[ir::ValueId],
    ann: &TypeAnnotations,
) -> Option<(Op, IrType)> {
    match op {
        // Binary arithmetic: AddJS, SubJS, MulJS, DivJS, ModJS
        Op::AddJS | Op::SubJS | Op::MulJS | Op::DivJS | Op::ModJS => {
            specialize_binary_arithmetic(op, operands, ann)
        }

        // Unary arithmetic: NegJS
        Op::NegJS => specialize_unary_arithmetic(operands, ann),

        // Relational comparisons: LtJS, LeJS, GtJS, GeJS
        Op::LtJS | Op::LeJS | Op::GtJS | Op::GeJS => {
            specialize_relational_comparison(op, operands, ann)
        }

        // Abstract equality: EqAbstract, NeAbstract
        Op::EqAbstract | Op::NeAbstract => specialize_equality(op, operands, ann),

        _ => None,
    }
}

/// Get the proven concrete type and trust for an operand, if available.
fn get_proven_type(
    operands: &[ir::ValueId],
    index: usize,
    ann: &TypeAnnotations,
) -> Option<(IrType, TrustCategory)> {
    let val = operands.get(index)?;
    let ty = ann.get_type(*val);
    let trust = ann.get_trust(*val);

    // Only specialize when we have a concrete type with sufficient trust.
    // Provable = compiler-proven (constants, typed ops).
    // Annotated = TypeScript annotations.
    // We require at least "trusted" level (Provable or Annotated).
    if trust.is_trusted()
        && let InferredType::Concrete(ir_ty) = ty
    {
        return Some((ir_ty.clone(), trust));
    }

    None
}

/// Specialize binary arithmetic (AddJS, SubJS, MulJS, DivJS, ModJS).
///
/// When both operands are proven F64, rewrites to the native F64 variant.
/// When both operands are proven I32, rewrites to the native I32 variant.
fn specialize_binary_arithmetic(
    op: &Op,
    operands: &[ir::ValueId],
    ann: &TypeAnnotations,
) -> Option<(Op, IrType)> {
    let (lhs_ty, _) = get_proven_type(operands, 0, ann)?;
    let (rhs_ty, _) = get_proven_type(operands, 1, ann)?;

    // Both F64
    if lhs_ty == IrType::F64 && rhs_ty == IrType::F64 {
        let new_op = match op {
            Op::AddJS => Op::AddF64,
            Op::SubJS => Op::SubF64,
            Op::MulJS => Op::MulF64,
            Op::DivJS => Op::DivF64,
            Op::ModJS => Op::ModF64,
            _ => return None,
        };
        return Some((new_op, IrType::F64));
    }

    // There is deliberately NO I32 arm for arithmetic, and there must never be
    // one. It is unsound for every input class, not merely for literals:
    //
    //   AddI32/SubI32/MulI32 wrap at 32 bits; JS requires the exact f64 result
    //                        (1e5 * 1e5 gave 1410065408, and 2e9 + 2e9 gave
    //                        -294967296).
    //   DivI32               truncates; 7 / 2 gave 3.
    //   ModI32/DivI32        trap on a zero divisor — SIGFPE and SIGILL, where
    //                        JS requires NaN and Infinity.
    //
    // Starving this arm of i32 literals is NOT sufficient: inference types
    // bitwise results as I32 (crates/types/src/inference.rs), so `(a|0) + (b|0)`
    // would still reach it and still wrap. The arm has to be absent.
    //
    // The relational and equality I32 arms below are a different matter and stay:
    // comparison cannot overflow, so they are sound for proven-i32 operands.
    None
}

/// Specialize unary arithmetic (NegJS).
///
/// When the operand is proven F64, rewrites to NegF64.
///
/// There is no I32 arm, for the same reason the binary form has none: `NegI32`
/// wraps at `i32::MIN` (negating it yields itself) and cannot produce `-0`,
/// which JS distinguishes from `0` via `1 / -0 === -Infinity`.
fn specialize_unary_arithmetic(
    operands: &[ir::ValueId],
    ann: &TypeAnnotations,
) -> Option<(Op, IrType)> {
    let (operand_ty, _) = get_proven_type(operands, 0, ann)?;

    match operand_ty {
        IrType::F64 => Some((Op::NegF64, IrType::F64)),
        _ => None,
    }
}

/// Specialize relational comparisons (LtJS, LeJS, GtJS, GeJS).
///
/// When both operands are proven F64, rewrites to the F64 comparison variant.
/// When both operands are proven I32, rewrites to the I32 comparison variant.
fn specialize_relational_comparison(
    op: &Op,
    operands: &[ir::ValueId],
    ann: &TypeAnnotations,
) -> Option<(Op, IrType)> {
    let (lhs_ty, _) = get_proven_type(operands, 0, ann)?;
    let (rhs_ty, _) = get_proven_type(operands, 1, ann)?;

    // Both F64
    if lhs_ty == IrType::F64 && rhs_ty == IrType::F64 {
        let new_op = match op {
            Op::LtJS => Op::LtF64,
            Op::LeJS => Op::LeF64,
            Op::GtJS => Op::GtF64,
            Op::GeJS => Op::GeF64,
            _ => return None,
        };
        return Some((new_op, IrType::Bool));
    }

    // Both I32
    if lhs_ty == IrType::I32 && rhs_ty == IrType::I32 {
        let new_op = match op {
            Op::LtJS => Op::LtI32,
            Op::LeJS => Op::LeI32,
            Op::GtJS => Op::GtI32,
            Op::GeJS => Op::GeI32,
            _ => return None,
        };
        return Some((new_op, IrType::Bool));
    }

    None
}

/// Specialize abstract equality (EqAbstract, NeAbstract).
///
/// When both operands have the same proven concrete type, abstract equality
/// is equivalent to strict equality (no coercion needed).
fn specialize_equality(
    op: &Op,
    operands: &[ir::ValueId],
    ann: &TypeAnnotations,
) -> Option<(Op, IrType)> {
    let (lhs_ty, _) = get_proven_type(operands, 0, ann)?;
    let (rhs_ty, _) = get_proven_type(operands, 1, ann)?;

    // `IrType::JSValue` is not a JavaScript type — it means "NaN-boxed, could be
    // anything". `null` and `undefined` are BOTH inferred as Concrete(JSValue)
    // (crates/types/src/inference.rs), so treating equal IrTypes as equal JS types
    // concluded that `null == undefined` may use strict equality, and answered
    // `false`. The spec says `true`, unconditionally.
    //
    // The defect only appeared for compile-time constants: with runtime operands
    // nothing is proven, the generic path runs, and the runtime's IsLooselyEqual
    // is correct. `f(null, undefined)` was already `true` while
    // `null == undefined` was `false`.
    //
    // Bail on JSValue: it carries no information about the JS type, so no
    // specialization can be justified from it.
    if lhs_ty == IrType::JSValue || rhs_ty == IrType::JSValue {
        return None;
    }

    // Same concrete type => abstract equality is the same as strict equality
    if lhs_ty == rhs_ty {
        // For numeric types, use the specialized comparison
        if lhs_ty == IrType::F64 {
            let new_op = match op {
                Op::EqAbstract => Op::EqF64,
                Op::NeAbstract => Op::NeF64,
                _ => return None,
            };
            return Some((new_op, IrType::Bool));
        }
        if lhs_ty == IrType::I32 {
            let new_op = match op {
                Op::EqAbstract => Op::EqI32,
                Op::NeAbstract => Op::NeI32,
                _ => return None,
            };
            return Some((new_op, IrType::Bool));
        }
        // For other same-type cases, use strict equality (no coercion needed)
        let new_op = match op {
            Op::EqAbstract => Op::EqStrict,
            Op::NeAbstract => Op::NeStrict,
            _ => return None,
        };
        return Some((new_op, IrType::Bool));
    }

    None
}
