//! Control flow translation: maps IR blocks and terminators to Cranelift
//! blocks, branches, and block parameters (for phi nodes).
//!
//! Phi nodes are handled using Cranelift's `Variable` API, which automatically
//! creates block parameters and threads values through intermediate blocks.

use std::collections::HashMap;

use ::ir::{BlockId, Op, ValueId};
use cranelift_codegen::ir::Block;
use cranelift_frontend::{FunctionBuilder, Variable};

use crate::error::CodegenError;
use crate::types::ir_type_to_cranelift;

/// Information about a single phi node for Cranelift lowering.
///
/// Each phi is backed by a Cranelift `Variable`. The incoming edges record
/// which IR value should be `def_var`'d from which predecessor block.
#[derive(Debug, Clone)]
pub struct PhiInfo {
    /// The IR ValueId of the phi instruction (the result value).
    pub value_id: ValueId,
    /// The Cranelift Variable backing this phi.
    pub variable: Variable,
}

/// Record that a particular IR ValueId feeds into a phi.
///
/// When the instruction producing this ValueId is lowered, the codegen
/// must call `def_var` for the associated phi Variable.
#[derive(Debug, Clone)]
pub struct PhiOperandBinding {
    /// The Cranelift Variable to def.
    pub variable: Variable,
}

/// Maps IR blocks to Cranelift blocks and tracks phi info.
pub struct BlockMap {
    /// IR BlockId -> Cranelift Block
    pub blocks: HashMap<u32, Block>,
    /// IR BlockId -> list of phi infos for that block
    pub phis: HashMap<u32, Vec<PhiInfo>>,
    /// IR ValueId (raw u32) -> list of phi Variables this value feeds into.
    /// When an instruction producing this ValueId is lowered, all associated
    /// Variables must be `def_var`'d with the produced Cranelift value.
    pub phi_operand_bindings: HashMap<u32, Vec<PhiOperandBinding>>,
}

impl BlockMap {
    /// Create Cranelift blocks for all IR blocks in a function, and set up
    /// phi Variables via Cranelift's SSA builder.
    pub fn build(
        func: &::ir::builder::TypedFunction,
        builder: &mut FunctionBuilder<'_>,
        isa: &dyn cranelift_codegen::isa::TargetIsa,
    ) -> Result<Self, CodegenError> {
        let mut blocks = HashMap::new();
        let mut phis: HashMap<u32, Vec<PhiInfo>> = HashMap::new();
        let mut phi_operand_bindings: HashMap<u32, Vec<PhiOperandBinding>> = HashMap::new();

        // First pass: create all blocks
        for bb in &func.blocks {
            let cl_block = builder.create_block();
            blocks.insert(bb.id.0, cl_block);
        }

        // Second pass: declare Variables for phi nodes and record operand bindings
        for bb in &func.blocks {
            for inst in &bb.instructions {
                if matches!(inst.op, Op::Phi)
                    && let Some(cl_ty) = ir_type_to_cranelift(&inst.ty, isa)?
                {
                    let var = builder.declare_var(cl_ty);

                    if inst.operands.is_empty() {
                        // Empty phi (unresolved closure capture). Mark as
                        // needing a default undefined value, but don't add
                        // operand bindings.
                    } else {
                        // Record that each phi operand ValueId feeds into
                        // this Variable.
                        for &operand_id in &inst.operands {
                            phi_operand_bindings
                                .entry(operand_id.0)
                                .or_default()
                                .push(PhiOperandBinding { variable: var });
                        }
                    }

                    phis.entry(bb.id.0).or_default().push(PhiInfo {
                        value_id: inst.id,
                        variable: var,
                    });
                }
            }
        }

        Ok(Self {
            blocks,
            phis,
            phi_operand_bindings,
        })
    }

    /// Get the Cranelift block for an IR block.
    pub fn get(&self, block_id: BlockId) -> Result<Block, CodegenError> {
        self.blocks
            .get(&block_id.0)
            .copied()
            .ok_or(CodegenError::UndefinedValue(block_id.0))
    }

    /// Get phi information for a given IR block, if any.
    pub fn get_phis(&self, block_id: BlockId) -> &[PhiInfo] {
        self.phis
            .get(&block_id.0)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the phi operand bindings for a given IR ValueId, if any.
    pub fn get_phi_bindings(&self, value_id: ValueId) -> &[PhiOperandBinding] {
        self.phi_operand_bindings
            .get(&value_id.0)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
