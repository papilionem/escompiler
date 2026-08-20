//! Function lowering: translates typed IR instructions into LLVM IR.
//!
//! The [`FunctionLowerer`] walks each basic block in a [`TypedFunction`],
//! translating each [`TypedInstruction`] into LLVM IR via an inkwell
//! [`Builder`]. This is the core code generation loop.

use std::collections::HashMap;

use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::{AsDIScope, DIScope};
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue};
use ir::builder::TypedFunction;
use ir::{Op, ValueId};

use crate::debug_info::DebugInfoEmitter;
use crate::error::LlvmCodegenError;
use crate::nanbox_emit;
use crate::runtime_calls::RuntimeCalls;

/// A pending phi node: (ir_value_id, llvm_phi, incoming_edges).
type PendingPhi<'ctx> = (
    u32,
    inkwell::values::PhiValue<'ctx>,
    Vec<(ValueId, ir::BlockId)>,
);

/// Lowers a single [`TypedFunction`] into LLVM IR.
pub struct FunctionLowerer<'a, 'ctx> {
    /// Map from IR ValueId to LLVM IntValue (i64, NaN-boxed).
    values: HashMap<u32, IntValue<'ctx>>,
    /// LLVM context.
    ctx: &'ctx Context,
    /// LLVM IR builder.
    builder: &'a Builder<'ctx>,
    /// LLVM module (for declaring external functions, globals).
    module: &'a Module<'ctx>,
    /// Runtime call declarations.
    runtime: &'a mut RuntimeCalls<'ctx>,
    /// Map from IR BlockId to LLVM BasicBlock.
    block_map: HashMap<u32, BasicBlock<'ctx>>,
    /// Declared LLVM functions for intra-module calls, indexed by IR function index.
    _func_values: &'a [FunctionValue<'ctx>],
    /// String table for ConstString resolution.
    string_table: &'a [String],
    /// Phi nodes to resolve after all blocks are emitted.
    /// Each entry: (ir_value_id, llvm_phi_node, vec<(ir_operand, ir_block_id)>).
    pending_phis: Vec<PendingPhi<'ctx>>,
    /// Map from IR ValueId to string table index (for ConstString → CallRuntime resolution).
    const_string_indices: HashMap<u32, u32>,
    /// Optional debug info emitter for DWARF source location tagging.
    debug_emitter: Option<&'a DebugInfoEmitter<'ctx>>,
    /// Optional debug info scope for the current function (set by codegen).
    debug_scope: Option<DIScope<'ctx>>,
}

impl<'a, 'ctx> FunctionLowerer<'a, 'ctx> {
    /// Create a new function lowerer.
    pub fn new(
        ctx: &'ctx Context,
        builder: &'a Builder<'ctx>,
        module: &'a Module<'ctx>,
        runtime: &'a mut RuntimeCalls<'ctx>,
        func_values: &'a [FunctionValue<'ctx>],
        string_table: &'a [String],
    ) -> Self {
        Self {
            values: HashMap::new(),
            ctx,
            builder,
            module,
            runtime,
            block_map: HashMap::new(),
            _func_values: func_values,
            string_table,
            pending_phis: Vec::new(),
            const_string_indices: HashMap::new(),
            debug_emitter: None,
            debug_scope: None,
        }
    }

    /// Create a new function lowerer with debug info support.
    ///
    /// When `debug_emitter` is provided, each IR instruction with a non-dummy
    /// [`SourceSpan`] will have its DWARF source location set on the LLVM
    /// builder before emission.
    pub fn new_with_debug(
        ctx: &'ctx Context,
        builder: &'a Builder<'ctx>,
        module: &'a Module<'ctx>,
        runtime: &'a mut RuntimeCalls<'ctx>,
        func_values: &'a [FunctionValue<'ctx>],
        string_table: &'a [String],
        debug_emitter: &'a DebugInfoEmitter<'ctx>,
    ) -> Self {
        Self {
            values: HashMap::new(),
            ctx,
            builder,
            module,
            runtime,
            block_map: HashMap::new(),
            _func_values: func_values,
            string_table,
            pending_phis: Vec::new(),
            const_string_indices: HashMap::new(),
            debug_emitter: Some(debug_emitter),
            debug_scope: None,
        }
    }

    /// Lower a complete IR function into the given LLVM function value.
    pub fn lower(
        &mut self,
        func: &TypedFunction,
        llvm_func: FunctionValue<'ctx>,
    ) -> Result<(), LlvmCodegenError> {
        // Phase 0: Create debug info subprogram if debug info is enabled
        if let Some(emitter) = self.debug_emitter {
            // Determine the function's first source line from its first instruction span
            let first_line = func
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find(|inst| inst.span.file_id.0 != u32::MAX)
                .map(|inst| inst.span.start)
                .unwrap_or(0);

            let line_no = if first_line > 0 {
                // Use the line table to convert byte offset to line number
                let lt = &emitter.line_tables[0];
                lt.offset_to_line_col(first_line).0
            } else {
                0
            };

            let is_local = !func.name.starts_with("__cs_main");
            let subprogram = emitter
                .create_function_scope(&func.name, None, line_no, is_local, false, llvm_func);
            self.debug_scope = Some(subprogram.as_debug_info_scope());
        }

        // Phase 1: Create all basic blocks
        for block in &func.blocks {
            let bb = self
                .ctx
                .append_basic_block(llvm_func, &format!("bb{}", block.id.0));
            self.block_map.insert(block.id.0, bb);
        }

        // Phase 2: Map function parameters (they become the first block's values)
        if let Some(first_block) = func.blocks.first() {
            let bb = self.block_map[&first_block.id.0];
            self.builder.position_at_end(bb);

            // Parameters are available as LLVM function params
            for (i, _param) in func.params.iter().enumerate() {
                // Parameters in the IR are referenced by LoadParam(i) ops, handled below
                let _ = (i, llvm_func.get_nth_param(i as u32));
            }
        }

        // Phase 3: Lower each block's instructions
        for block in &func.blocks {
            let bb = self.block_map[&block.id.0];
            self.builder.position_at_end(bb);

            for inst in &block.instructions {
                // Set debug location before each instruction if debug info is enabled
                if let (Some(emitter), Some(scope)) = (self.debug_emitter, self.debug_scope) {
                    emitter.set_location(self.builder, self.ctx, &inst.span, scope);
                }
                self.lower_instruction(inst, func, llvm_func)?;
            }
        }

        // Phase 4: Resolve phi nodes
        for (_, phi, incoming) in &self.pending_phis {
            let mut phi_incoming: Vec<(BasicValueEnum<'ctx>, BasicBlock<'ctx>)> = Vec::new();
            for (val_id, block_id) in incoming {
                let val = self.get_value(val_id.0)?;
                let bb = self.block_map.get(&block_id.0).ok_or_else(|| {
                    LlvmCodegenError::Module(format!("missing block bb{}", block_id.0))
                })?;
                phi_incoming.push((val.into(), *bb));
            }
            let refs: Vec<(&dyn inkwell::values::BasicValue<'ctx>, BasicBlock<'ctx>)> =
                phi_incoming
                    .iter()
                    .map(|(v, b)| (v as &dyn inkwell::values::BasicValue<'ctx>, *b))
                    .collect();
            phi.add_incoming(&refs);
        }

        Ok(())
    }

    /// Get a previously-lowered value by IR ValueId.
    fn get_value(&self, id: u32) -> Result<IntValue<'ctx>, LlvmCodegenError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(LlvmCodegenError::UndefinedValue(id))
    }

    /// Store a lowered value.
    fn set_value(&mut self, id: u32, val: IntValue<'ctx>) {
        self.values.insert(id, val);
    }

    /// Lower a single IR instruction.
    fn lower_instruction(
        &mut self,
        inst: &ir::TypedInstruction,
        _func: &TypedFunction,
        llvm_func: FunctionValue<'ctx>,
    ) -> Result<(), LlvmCodegenError> {
        let vid = inst.id.0;

        match &inst.op {
            // === Constants ===
            Op::ConstI32(n) => {
                let raw = self.ctx.i32_type().const_int(*n as u64, true);
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, raw);
                self.set_value(vid, boxed);
            }
            Op::ConstI64(n) => {
                let val = self.ctx.i64_type().const_int(*n as u64, true);
                self.set_value(vid, val);
            }
            Op::ConstF64(n) => {
                let raw = self.ctx.f64_type().const_float(*n);
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, raw);
                self.set_value(vid, boxed);
            }
            Op::ConstBool(b) => {
                let raw = self.ctx.bool_type().const_int(*b as u64, false);
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, raw);
                self.set_value(vid, boxed);
            }
            Op::ConstNull => {
                let val = nanbox_emit::emit_box_null(self.ctx);
                self.set_value(vid, val);
            }
            Op::ConstUndefined => {
                let val = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, val);
            }
            Op::ConstString(idx) => {
                // Emit a call to __esc_rt_string_intern with the string data
                let s = self
                    .string_table
                    .get(*idx as usize)
                    .cloned()
                    .unwrap_or_default();
                let global_str = self
                    .builder
                    .build_global_string_ptr(&s, &format!("str_{idx}"))
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let intern_fn = self
                    .runtime
                    .get_string_intern("__esc_rt_string_intern", self.module);
                let len_val = self.ctx.i32_type().const_int(s.len() as u64, false);
                let result = self
                    .builder
                    .build_call(
                        intern_fn,
                        &[global_str.as_pointer_value().into(), len_val.into()],
                        "const_str",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("string intern returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
                self.const_string_indices.insert(vid, *idx);
            }

            // === LoadGlobal — resolve a built-in global by name ===
            Op::LoadGlobal(idx) => {
                // Intern the name string, then call __esc_rt_get_global(name_bits) -> i64.
                let s = self
                    .string_table
                    .get(*idx as usize)
                    .cloned()
                    .unwrap_or_default();
                let global_str = self
                    .builder
                    .build_global_string_ptr(&s, &format!("global_name_{idx}"))
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let intern_fn = self
                    .runtime
                    .get_string_intern("__esc_rt_string_intern", self.module);
                let len_val = self.ctx.i32_type().const_int(s.len() as u64, false);
                let name_bits = self
                    .builder
                    .build_call(
                        intern_fn,
                        &[global_str.as_pointer_value().into(), len_val.into()],
                        "global_name_str",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("string intern returned void".into()))?
                    .into_int_value();
                let get_global_fn = self
                    .runtime
                    .get_unary_js_op("__esc_rt_get_global", self.module);
                let result = self
                    .builder
                    .build_call(get_global_fn, &[name_bits.into()], "load_global_result")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("get_global returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
                self.const_string_indices.insert(vid, *idx);
            }

            // === JS Arithmetic (call runtime) ===
            Op::AddJS => {
                let result = self.emit_binary_rt_call("__esc_rt_add_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SubJS => {
                let result = self.emit_binary_rt_call("__esc_rt_sub_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::MulJS => {
                let result = self.emit_binary_rt_call("__esc_rt_mul_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::DivJS => {
                let result = self.emit_binary_rt_call("__esc_rt_div_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ModJS => {
                let result = self.emit_binary_rt_call("__esc_rt_mod_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::NegJS => {
                let result = self.emit_unary_rt_call("__esc_rt_neg_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ExpJS => {
                let result = self.emit_binary_rt_call("__esc_rt_exp_js", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Typed Arithmetic (native LLVM ops) ===
            Op::AddI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_int_add(a, b, "add_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::SubI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_int_sub(a, b, "sub_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::MulI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_int_mul(a, b, "mul_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::DivI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_int_signed_div(a, b, "div_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::ModI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_int_signed_rem(a, b, "mod_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::NegI32 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                let result = self
                    .builder
                    .build_int_neg(unboxed, "neg_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::AddF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_float_add(a, b, "add_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::SubF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_float_sub(a, b, "sub_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::MulF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_float_mul(a, b, "mul_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::DivF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_float_div(a, b, "div_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::ModF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let result = self
                    .builder
                    .build_float_rem(a, b, "mod_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }
            Op::NegF64 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_f64(self.builder, self.ctx, a);
                let result = self
                    .builder
                    .build_float_neg(unboxed, "neg_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, result);
                self.set_value(vid, boxed);
            }

            // === Bitwise ops (call runtime for JS semantics) ===
            Op::BitwiseAnd => {
                let result = self.emit_binary_rt_call("__esc_rt_bitwise_and", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::BitwiseOr => {
                let result = self.emit_binary_rt_call("__esc_rt_bitwise_or", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::BitwiseXor => {
                let result = self.emit_binary_rt_call("__esc_rt_bitwise_xor", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::BitwiseNot => {
                let result = self.emit_unary_rt_call("__esc_rt_bitwise_not", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ShiftLeft => {
                let result = self.emit_binary_rt_call("__esc_rt_shl", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ShiftRight => {
                let result = self.emit_binary_rt_call("__esc_rt_shr", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ShiftRightUnsigned => {
                let result = self.emit_binary_rt_call("__esc_rt_ushr", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Comparison ===
            Op::EqStrict => {
                let result = self.emit_binary_rt_call("__esc_rt_eq_strict", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::NeStrict => {
                let result = self.emit_binary_rt_call("__esc_rt_ne_strict", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::EqAbstract => {
                let result = self.emit_binary_rt_call("__esc_rt_eq_abstract", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::NeAbstract => {
                let result = self.emit_binary_rt_call("__esc_rt_ne_abstract", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::LtJS => {
                let result = self.emit_binary_rt_call("__esc_rt_lt_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::LeJS => {
                let result = self.emit_binary_rt_call("__esc_rt_le_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GtJS => {
                let result = self.emit_binary_rt_call("__esc_rt_gt_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GeJS => {
                let result = self.emit_binary_rt_call("__esc_rt_ge_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::EqI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, a, b, "eq_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::NeI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::NE, a, b, "ne_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::LtI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, a, b, "lt_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::LeI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::SLE, a, b, "le_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::GtI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::SGT, a, b, "gt_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::GeI32 => {
                let (a, b) = self.get_two_i32_operands(&inst.operands)?;
                let cmp = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, a, b, "ge_i32")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }
            Op::EqF64 | Op::NeF64 | Op::LtF64 | Op::LeF64 | Op::GtF64 | Op::GeF64 => {
                let (a, b) = self.get_two_f64_operands(&inst.operands)?;
                let pred = match &inst.op {
                    Op::EqF64 => inkwell::FloatPredicate::OEQ,
                    Op::NeF64 => inkwell::FloatPredicate::ONE,
                    Op::LtF64 => inkwell::FloatPredicate::OLT,
                    Op::LeF64 => inkwell::FloatPredicate::OLE,
                    Op::GtF64 => inkwell::FloatPredicate::OGT,
                    Op::GeF64 => inkwell::FloatPredicate::OGE,
                    _ => unreachable!(),
                };
                let cmp = self
                    .builder
                    .build_float_compare(pred, a, b, "fcmp")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, cmp);
                self.set_value(vid, boxed);
            }

            // === Type conversions ===
            Op::ToNumber => {
                let result = self.emit_unary_rt_call("__esc_rt_to_number", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ToBoolean => {
                let result = self.emit_unary_rt_call("__esc_rt_to_boolean_js", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ToString => {
                let result = self.emit_unary_rt_call("__esc_rt_to_string", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ToNumeric
            | Op::ToObject
            | Op::ToPrimitive
            | Op::ToPropertyKey
            | Op::ToInt32
            | Op::ToUint32 => {
                let name = match &inst.op {
                    Op::ToNumeric => "__esc_rt_to_numeric",
                    Op::ToObject => "__esc_rt_to_object",
                    Op::ToPrimitive => "__esc_rt_to_primitive",
                    Op::ToPropertyKey => "__esc_rt_to_property_key",
                    Op::ToInt32 => "__esc_rt_to_int32",
                    Op::ToUint32 => "__esc_rt_to_uint32",
                    _ => unreachable!(),
                };
                let result = self.emit_unary_rt_call(name, &inst.operands)?;
                self.set_value(vid, result);
            }

            // === NaN-boxing ===
            Op::BoxI32 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::BoxUnsignedI32 => {
                // Treat the i32 as unsigned: zero-extend to i64, convert to
                // f64, then box as f64 to preserve the full u32 range.
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                let i64_ty = self.ctx.i64_type();
                let f64_ty = self.ctx.f64_type();
                let zext = self
                    .builder
                    .build_int_z_extend(unboxed, i64_ty, "box_u32_ext")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let f64_val = self
                    .builder
                    .build_unsigned_int_to_float(zext, f64_ty, "box_u32_f64")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, f64_val);
                self.set_value(vid, boxed);
            }
            Op::BoxF64 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_f64(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::BoxBool => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_bool(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::BoxNull => {
                let val = nanbox_emit::emit_box_null(self.ctx);
                self.set_value(vid, val);
            }
            Op::BoxUndefined => {
                let val = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, val);
            }
            Op::BoxString => {
                let a = self.get_value(inst.operands[0].0)?;
                let boxed = nanbox_emit::emit_box_string(self.builder, self.ctx, a);
                self.set_value(vid, boxed);
            }
            Op::BoxObject => {
                let a = self.get_value(inst.operands[0].0)?;
                let boxed = nanbox_emit::emit_box_object(self.builder, self.ctx, a);
                self.set_value(vid, boxed);
            }
            Op::BoxSymbol => {
                // Symbol boxing: treat as i32 payload
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                // Reuse box_i32 but with symbol tag -- emit inline
                let tag_bits = self
                    .ctx
                    .i64_type()
                    .const_int(0x7FF8_0000_0000_0000 | (0x0007 << 48), false);
                let extended = self
                    .builder
                    .build_int_z_extend(unboxed, self.ctx.i64_type(), "box_sym_ext")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let boxed = self
                    .builder
                    .build_or(tag_bits, extended, "box_symbol")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                self.set_value(vid, boxed);
            }
            Op::UnboxI32 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::UnboxF64 => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_f64(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_f64(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::UnboxBool => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_bool(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_bool(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::UnboxString => {
                let a = self.get_value(inst.operands[0].0)?;
                let result = nanbox_emit::emit_unbox_string(self.builder, self.ctx, a);
                self.set_value(vid, result);
            }
            Op::UnboxObject => {
                let a = self.get_value(inst.operands[0].0)?;
                let result = nanbox_emit::emit_unbox_object(self.builder, self.ctx, a);
                self.set_value(vid, result);
            }
            Op::UnboxSymbol => {
                let a = self.get_value(inst.operands[0].0)?;
                let unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
                let boxed = nanbox_emit::emit_box_i32(self.builder, self.ctx, unboxed);
                self.set_value(vid, boxed);
            }
            Op::TypeofBoxed | Op::IsNullish | Op::IsFalsy => {
                let name = match &inst.op {
                    Op::TypeofBoxed => "__esc_rt_typeof_boxed",
                    Op::IsNullish => "__esc_rt_is_nullish",
                    Op::IsFalsy => "__esc_rt_is_falsy",
                    _ => unreachable!(),
                };
                let result = self.emit_unary_rt_call(name, &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Control flow ===
            Op::Br => {
                let target = inst.block_targets[0];
                let bb = self.block_map.get(&target.0).ok_or_else(|| {
                    LlvmCodegenError::Module(format!("missing block bb{}", target.0))
                })?;
                self.builder
                    .build_unconditional_branch(*bb)
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            }
            Op::BrIf => {
                let cond_val = self.get_value(inst.operands[0].0)?;
                // Compare NaN-boxed bool: check if payload is nonzero
                let zero = self.ctx.i64_type().const_zero();
                let cond = self
                    .builder
                    .build_int_compare(IntPredicate::NE, cond_val, zero, "brif_cond")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let then_bb = self
                    .block_map
                    .get(&inst.block_targets[0].0)
                    .ok_or_else(|| {
                        LlvmCodegenError::Module(format!(
                            "missing then block bb{}",
                            inst.block_targets[0].0
                        ))
                    })?;
                let else_bb = self
                    .block_map
                    .get(&inst.block_targets[1].0)
                    .ok_or_else(|| {
                        LlvmCodegenError::Module(format!(
                            "missing else block bb{}",
                            inst.block_targets[1].0
                        ))
                    })?;
                self.builder
                    .build_conditional_branch(cond, *then_bb, *else_bb)
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            }
            Op::Ret => {
                if inst.operands.is_empty() {
                    self.builder
                        .build_return(None)
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                } else {
                    let val = self.get_value(inst.operands[0].0)?;
                    self.builder
                        .build_return(Some(&val))
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                }
            }
            Op::Unreachable => {
                self.builder
                    .build_unreachable()
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            }
            Op::Switch => {
                // Switch: operand[0] is the discriminant, block_targets[0..n-1] are cases,
                // block_targets[last] is the default.
                let disc = self.get_value(inst.operands[0].0)?;
                let default_bb = self
                    .block_map
                    .get(
                        &inst
                            .block_targets
                            .last()
                            .ok_or_else(|| {
                                LlvmCodegenError::Module("switch with no targets".into())
                            })?
                            .0,
                    )
                    .ok_or_else(|| {
                        LlvmCodegenError::Module("missing switch default block".into())
                    })?;
                let cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = inst.block_targets
                    [..inst.block_targets.len() - 1]
                    .iter()
                    .enumerate()
                    .map(|(i, bt)| {
                        let case_val = self.ctx.i64_type().const_int(i as u64, false);
                        let bb = self.block_map[&bt.0];
                        (case_val, bb)
                    })
                    .collect();
                let case_refs: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = cases;
                self.builder
                    .build_switch(disc, *default_bb, &case_refs)
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            }

            // === SSA Phi ===
            Op::Phi => {
                let phi = self
                    .builder
                    .build_phi(self.ctx.i64_type(), &format!("phi_v{vid}"))
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                self.set_value(vid, phi.as_basic_value().into_int_value());
                // Collect incoming: operands and block_targets are paired
                let incoming: Vec<(ValueId, ir::BlockId)> = inst
                    .operands
                    .iter()
                    .zip(inst.block_targets.iter())
                    .map(|(v, b)| (*v, *b))
                    .collect();
                self.pending_phis.push((vid, phi, incoming));
            }

            // === Function parameters ===
            Op::LoadParam(idx) => {
                let param = llvm_func
                    .get_nth_param(*idx)
                    .ok_or_else(|| LlvmCodegenError::Module(format!("missing param {idx}")))?;
                self.set_value(vid, param.into_int_value());
            }

            // === Calls ===
            Op::Call => {
                // operands[0] is the function reference (index), rest are args
                // For direct calls to module functions, use func_values
                if !inst.operands.is_empty() {
                    // Try to resolve as a direct call to a module function
                    // The first operand is a ConstI32 func index in most cases
                    let callee_val = self.get_value(inst.operands[0].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[1..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    // Use indirect call via runtime
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let call_fn = self
                        .runtime
                        .get_call_variadic("__esc_rt_call_indirect", self.module);
                    // Spill args to stack
                    let argv = self.spill_args_to_stack(&args)?;
                    let result = self
                        .builder
                        .build_call(
                            call_fn,
                            &[callee_val.into(), argc.into(), argv.into()],
                            "call_result",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| LlvmCodegenError::Module("call returned void".into()))?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }
            Op::CallRuntime => {
                self.emit_call_runtime_dispatch(inst, vid)?;
            }
            Op::CallMethod => {
                // operands: [obj, key, ...args]
                if inst.operands.len() >= 2 {
                    let obj = self.get_value(inst.operands[0].0)?;
                    let key = self.get_value(inst.operands[1].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[2..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let argv = self.spill_args_to_stack(&args)?;
                    let method_fn = self
                        .runtime
                        .get_call_method("__esc_rt_call_method", self.module);
                    let result = self
                        .builder
                        .build_call(
                            method_fn,
                            &[obj.into(), key.into(), argc.into(), argv.into()],
                            "call_method",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| {
                            LlvmCodegenError::Module("call_method returned void".into())
                        })?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }
            Op::CallNew => {
                // operands: [constructor, ...args]
                if !inst.operands.is_empty() {
                    let ctor = self.get_value(inst.operands[0].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[1..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let argv = self.spill_args_to_stack(&args)?;
                    let new_fn = self
                        .runtime
                        .get_call_variadic("__esc_rt_call_new", self.module);
                    let result = self
                        .builder
                        .build_call(new_fn, &[ctor.into(), argc.into(), argv.into()], "call_new")
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| LlvmCodegenError::Module("call_new returned void".into()))?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }

            // === Property access ===
            Op::GetProp => {
                let result = self.emit_binary_rt_call("__esc_rt_get_prop", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ICGetProp => {
                // obj, key, ic_id → value
                let obj = self.get_value(inst.operands[0].0)?;
                let key = self.get_value(inst.operands[1].0)?;
                let ic_id = self.get_value(inst.operands[2].0)?;
                let func = self.runtime.get_ic_get_prop(self.module);
                let result = self
                    .builder
                    .build_call(func, &[obj.into(), key.into(), ic_id.into()], "ic_get_prop")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("ic_get_prop returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::ICSetProp => {
                // obj, key, val, ic_id → void
                let obj = self.get_value(inst.operands[0].0)?;
                let key = self.get_value(inst.operands[1].0)?;
                let val = self.get_value(inst.operands[2].0)?;
                let ic_id = self.get_value(inst.operands[3].0)?;
                let func = self.runtime.get_ic_set_prop(self.module);
                self.builder
                    .build_call(
                        func,
                        &[obj.into(), key.into(), val.into(), ic_id.into()],
                        "ic_set_prop",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::SetProp => {
                let result = self.emit_ternary_rt_call("__esc_rt_set_prop", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetPropStrict => {
                let result =
                    self.emit_ternary_rt_call("__esc_rt_set_prop_strict", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GetElem => {
                let result = self.emit_binary_rt_call("__esc_rt_get_elem", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetElem => {
                let result = self.emit_ternary_rt_call("__esc_rt_set_elem", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::HasProp => {
                let result = self.emit_binary_rt_call("__esc_rt_has_prop", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::DeleteProp => {
                let result = self.emit_binary_rt_call("__esc_rt_delete_prop", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::DeleteElem => {
                let result = self.emit_binary_rt_call("__esc_rt_delete_elem", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Object creation ===
            Op::CreateObject => {
                let create_fn = self
                    .runtime
                    .get_create_object("__esc_rt_create_object", self.module);
                let result = self
                    .builder
                    .build_call(create_fn, &[], "create_obj")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("create_object returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::CreateObjectLiteral => {
                // Operands: [key0, val0, key1, val1, ...] — interleaved kvpairs
                let pair_count = inst.operands.len() / 2;
                let mut args: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(inst.operands.len());
                for &op in &inst.operands {
                    let val = self.get_value(op.0)?;
                    args.push(val.into());
                }
                let kvpairs_ptr = self.spill_args_to_stack(&args)?;
                let count_val = self.ctx.i32_type().const_int(pair_count as u64, false);
                let create_fn = self
                    .runtime
                    .get_i32_i64_to_i64("__esc_rt_create_object_literal", self.module);
                let result = self
                    .builder
                    .build_call(
                        create_fn,
                        &[count_val.into(), kvpairs_ptr.into()],
                        "create_obj_lit",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| {
                        LlvmCodegenError::Module("create_object_literal returned void".into())
                    })?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::CreateArray => {
                let len = self
                    .ctx
                    .i32_type()
                    .const_int(inst.operands.len() as u64, false);
                let create_fn = self
                    .runtime
                    .get_create_array("__esc_rt_create_array", self.module);
                let result = self
                    .builder
                    .build_call(create_fn, &[len.into()], "create_arr")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("create_array returned void".into()))?
                    .into_int_value();
                // Push elements
                for (i, op) in inst.operands.iter().enumerate() {
                    let elem = self.get_value(op.0)?;
                    let idx_boxed = nanbox_emit::emit_box_i32(
                        self.builder,
                        self.ctx,
                        self.ctx.i32_type().const_int(i as u64, false),
                    );
                    let set_elem_fn = self
                        .runtime
                        .get_ternary_js_op("__esc_rt_set_elem", self.module);
                    self.builder
                        .build_call(
                            set_elem_fn,
                            &[result.into(), idx_boxed.into(), elem.into()],
                            "",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                }
                self.set_value(vid, result);
            }

            // === Closure creation ===
            Op::CreateClosure => {
                // operands: [func_idx_const, env, flags]
                if inst.operands.len() >= 3 {
                    let func_idx = self.get_value(inst.operands[0].0)?;
                    let func_idx_i32 =
                        nanbox_emit::emit_unbox_i32(self.builder, self.ctx, func_idx);
                    let env = self.get_value(inst.operands[1].0)?;
                    let flags = self.get_value(inst.operands[2].0)?;
                    let flags_i32 = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, flags);
                    let closure_fn = self
                        .runtime
                        .get_create_closure("__esc_rt_create_closure", self.module);
                    let result = self
                        .builder
                        .build_call(
                            closure_fn,
                            &[func_idx_i32.into(), env.into(), flags_i32.into()],
                            "create_closure",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| {
                            LlvmCodegenError::Module("create_closure returned void".into())
                        })?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }

            // === Environment (closure scope chain) ===
            Op::EnvCreate => {
                // operands: [slot_count_as_const_i32]
                // ABI: __esc_rt_env_create(parent: i64, slot_count: i32) -> i64
                let slot_count = self.get_value(inst.operands[0].0)?;
                let slot_count_i32 =
                    nanbox_emit::emit_unbox_i32(self.builder, self.ctx, slot_count);
                let null_parent = nanbox_emit::emit_box_null(self.ctx);
                let env_create_fn = self
                    .runtime
                    .get_env_create("__esc_rt_env_create", self.module);
                let result = self
                    .builder
                    .build_call(
                        env_create_fn,
                        &[null_parent.into(), slot_count_i32.into()],
                        "env_create",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("env_create returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::EnvLoad => {
                // operands: [env, slot_idx_as_const_i32]
                // ABI: __esc_rt_env_load(env: i64, depth: i32, slot: i32) -> i64
                let env = self.get_value(inst.operands[0].0)?;
                let slot = self.get_value(inst.operands[1].0)?;
                let slot_i32 = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, slot);
                let zero_depth = self.ctx.i32_type().const_int(0, false);
                let env_load_fn = self.runtime.get_env_load("__esc_rt_env_load", self.module);
                let result = self
                    .builder
                    .build_call(
                        env_load_fn,
                        &[env.into(), zero_depth.into(), slot_i32.into()],
                        "env_load",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("env_load returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::EnvStore => {
                // operands: [env, slot_idx_as_const_i32, val]
                // ABI: __esc_rt_env_store(env: i64, depth: i32, slot: i32, val: i64) -> void
                let env = self.get_value(inst.operands[0].0)?;
                let slot = self.get_value(inst.operands[1].0)?;
                let slot_i32 = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, slot);
                let val = self.get_value(inst.operands[2].0)?;
                let zero_depth = self.ctx.i32_type().const_int(0, false);
                let env_store_fn = self
                    .runtime
                    .get_env_store("__esc_rt_env_store", self.module);
                self.builder
                    .build_call(
                        env_store_fn,
                        &[env.into(), zero_depth.into(), slot_i32.into(), val.into()],
                        "",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::EnvExtend => {
                // operands: [outer, slot_count_as_const_i32]
                // ABI: __esc_rt_env_create(parent: i64, slot_count: i32) -> i64
                // Reuses env_create with outer env as parent (matching Cranelift)
                let outer = self.get_value(inst.operands[0].0)?;
                let slot_count = self.get_value(inst.operands[1].0)?;
                let slot_count_i32 =
                    nanbox_emit::emit_unbox_i32(self.builder, self.ctx, slot_count);
                let env_create_fn = self
                    .runtime
                    .get_env_create("__esc_rt_env_create", self.module);
                let result = self
                    .builder
                    .build_call(
                        env_create_fn,
                        &[outer.into(), slot_count_i32.into()],
                        "env_extend",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("env_extend returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }

            Op::EnvLookup => {
                let result = self.emit_binary_rt_call("__esc_rt_esc_env_lookup", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::EnvLookupStore => {
                let result = self.emit_ternary_rt_call("__esc_rt_esc_env_store", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === JsBox ops ===
            Op::AllocBox => {
                let result = self.emit_unary_rt_call("__esc_rt_alloc_box", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::BoxLoad => {
                let result = self.emit_unary_rt_call("__esc_rt_box_load", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::BoxStore => {
                // ABI: __esc_rt_box_store(box_ptr: i64, val: i64) -> void
                let box_ptr = self.get_value(inst.operands[0].0)?;
                let val = self.get_value(inst.operands[1].0)?;
                let box_store_fn = self
                    .runtime
                    .get_void_binary("__esc_rt_box_store", self.module);
                self.builder
                    .build_call(box_store_fn, &[box_ptr.into(), val.into()], "")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === Exception handling ===
            Op::Throw => {
                let val = self.get_value(inst.operands[0].0)?;
                let throw_fn = self.runtime.get_void_unary("__esc_rt_throw", self.module);
                self.builder
                    .build_call(throw_fn, &[val.into()], "")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                self.builder
                    .build_unreachable()
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            }
            Op::TryBegin | Op::TryEnd | Op::Catch | Op::Finally | Op::Rethrow => {
                // ESC-19: Exception handling is not fully wired on the LLVM
                // backend. Refuse rather than emit a silently-wrong binary.
                return Err(LlvmCodegenError::UnsupportedOpcode(format!(
                    "{:?}: exception handling (try/catch/finally) is not yet supported on the \
                     LLVM backend. Use the Cranelift backend (default) or track ESC-19.",
                    inst.op
                )));
            }
            Op::IsException => {
                let result = self.emit_unary_rt_call("__esc_rt_is_exception", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GetException => {
                let result = self.emit_unary_rt_call("__esc_rt_get_exception", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Iterators ===
            Op::IterInit => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_init", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ForInInit => {
                let result = self.emit_unary_rt_call("__esc_rt_for_in_init", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::IterInitAsync => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_init_async", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::IterNext => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_next", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::IterDone => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_done", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::IterValue => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_value", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::IterClose => {
                let result = self.emit_unary_rt_call("__esc_rt_iter_close", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === String operations ===
            Op::StringConcat => {
                let result = self.emit_binary_rt_call("__esc_rt_string_concat", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::StringLength => {
                let result = self.emit_unary_rt_call("__esc_rt_string_length", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::StringCharAt => {
                let result = self.emit_binary_rt_call("__esc_rt_string_char_at", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::StringCompare => {
                let result = self.emit_binary_rt_call("__esc_rt_string_compare", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Locals ===
            Op::LoadLocal | Op::StoreLocal => {
                // Locals are lowered as direct value copies (SSA).
                // LoadLocal: operand[0] = value to load from
                // StoreLocal: operand[0] = value to store (result unused)
                if !inst.operands.is_empty() {
                    let val = self.get_value(inst.operands[0].0)?;
                    self.set_value(vid, val);
                } else {
                    let undef = nanbox_emit::emit_box_undefined(self.ctx);
                    self.set_value(vid, undef);
                }
            }

            // === Object operations ===
            Op::ObjectDefineProperty => {
                // (obj, key, value)
                let _result =
                    self.emit_ternary_rt_call("__esc_rt_object_define_property", &inst.operands)?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::ObjectGetPrototype => {
                let result =
                    self.emit_unary_rt_call("__esc_rt_object_get_prototype", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::ObjectSetPrototype => {
                let _result =
                    self.emit_binary_rt_call("__esc_rt_object_set_prototype", &inst.operands)?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === Dynamic/super/private property access ===
            Op::GetPropDynamic => {
                let result =
                    self.emit_binary_rt_call("__esc_rt_get_prop_dynamic", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetPropDynamic => {
                let result =
                    self.emit_ternary_rt_call("__esc_rt_set_prop_dynamic", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetPropDynamicStrict => {
                let result =
                    self.emit_ternary_rt_call("__esc_rt_set_prop_strict", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GetSuper => {
                let result = self.emit_binary_rt_call("__esc_rt_get_super", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetSuper => {
                let result = self.emit_ternary_rt_call("__esc_rt_set_super", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::GetPrivate => {
                let result = self.emit_binary_rt_call("__esc_rt_get_private", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::SetPrivate => {
                let result = self.emit_ternary_rt_call("__esc_rt_set_private", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::PrivateFieldGet => {
                let result =
                    self.emit_binary_rt_call("__esc_rt_private_field_get", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::PrivateFieldSet => {
                let result =
                    self.emit_ternary_rt_call("__esc_rt_private_field_set", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::PrivateFieldHas => {
                let result =
                    self.emit_binary_rt_call("__esc_rt_private_field_has", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::InstallPrivateField => {
                let result =
                    self.emit_ternary_rt_call("__esc_rt_install_private_field", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Additional call variants ===
            Op::Invoke => {
                // Like Call, but within a try block; for now treat same as Call
                if !inst.operands.is_empty() {
                    let callee_val = self.get_value(inst.operands[0].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[1..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let call_fn = self
                        .runtime
                        .get_call_variadic("__esc_rt_call_indirect", self.module);
                    let argv = self.spill_args_to_stack(&args)?;
                    let result = self
                        .builder
                        .build_call(
                            call_fn,
                            &[callee_val.into(), argc.into(), argv.into()],
                            "invoke_result",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| LlvmCodegenError::Module("invoke returned void".into()))?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }
            Op::CallVarargs | Op::TailCall | Op::CallEval | Op::CallEvalDirect => {
                // All use the same indirect call mechanism
                if !inst.operands.is_empty() {
                    let callee_val = self.get_value(inst.operands[0].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[1..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let call_fn = self
                        .runtime
                        .get_call_variadic("__esc_rt_call_indirect", self.module);
                    let argv = self.spill_args_to_stack(&args)?;
                    let result = self
                        .builder
                        .build_call(
                            call_fn,
                            &[callee_val.into(), argc.into(), argv.into()],
                            "call_result",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| LlvmCodegenError::Module("call returned void".into()))?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }

            // === Promises & async ===
            Op::PromiseCreate => {
                let create_fn = self
                    .runtime
                    .get_create_object("__esc_rt_promise_create", self.module);
                let result = self
                    .builder
                    .build_call(create_fn, &[], "promise")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("promise_create returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::PromiseResolve => {
                let result =
                    self.emit_binary_rt_call("__esc_rt_promise_resolve", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::PromiseReject => {
                let result = self.emit_binary_rt_call("__esc_rt_promise_reject", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::Await => {
                let result = self.emit_unary_rt_call("__esc_rt_await", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Generators ===
            Op::GeneratorCreate => {
                let create_fn = self
                    .runtime
                    .get_create_object("__esc_rt_generator_create", self.module);
                let result = self
                    .builder
                    .build_call(create_fn, &[], "generator")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| {
                        LlvmCodegenError::Module("generator_create returned void".into())
                    })?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::Yield => {
                let result = self.emit_unary_rt_call("__esc_rt_yield", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::YieldDelegate => {
                let result = self.emit_unary_rt_call("__esc_rt_yield_delegate", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Type checks ===
            Op::InstanceOf => {
                let result = self.emit_binary_rt_call("__esc_rt_instanceof", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Guards & shapes ===
            Op::GuardType | Op::GuardShape | Op::GuardTruthiness => {
                // Guards pass through the value (deopt not yet wired)
                let val = self.get_value(inst.operands[0].0)?;
                self.set_value(vid, val);
            }
            Op::ShapeCheck | Op::ShapeTransition => {
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === TDZ (temporal dead zone) ===
            Op::TdzCheck => {
                let val = self.get_value(inst.operands[0].0)?;
                self.set_value(vid, val);
            }
            Op::TdzInit => {
                let val = self.get_value(inst.operands[0].0)?;
                self.set_value(vid, val);
            }

            // === Drop flags ===
            Op::DropFlagSet | Op::DropFlagCheck => {
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === Memory allocation ===
            Op::AllocZone | Op::AllocHeap | Op::AllocStack | Op::AllocArray => {
                let result = self.emit_unary_rt_call("__esc_rt_alloc", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::FreeZone => {
                let val = self.get_value(inst.operands[0].0)?;
                let free_fn = self
                    .runtime
                    .get_void_unary("__esc_rt_free_zone", self.module);
                self.builder
                    .build_call(free_fn, &[val.into()], "")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === Reference counting ===
            Op::IncRef | Op::RcIncStrong | Op::RcIncWeak => {
                let val = self.get_value(inst.operands[0].0)?;
                let inc_fn = self.runtime.get_void_unary("__esc_rt_inc_ref", self.module);
                self.builder
                    .build_call(inc_fn, &[val.into()], "")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                self.set_value(vid, val);
            }
            Op::DecRef | Op::RcDecStrong | Op::RcDecWeak => {
                let val = self.get_value(inst.operands[0].0)?;
                let dec_fn = self.runtime.get_void_unary("__esc_rt_dec_ref", self.module);
                self.builder
                    .build_call(dec_fn, &[val.into()], "")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::RcIsUnique => {
                let result = self.emit_unary_rt_call("__esc_rt_rc_is_unique", &inst.operands)?;
                self.set_value(vid, result);
            }

            // === Field/element access ===
            Op::LoadField => {
                let result = self.emit_binary_rt_call("__esc_rt_load_field", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::StoreField => {
                let _result = self.emit_ternary_rt_call("__esc_rt_store_field", &inst.operands)?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::LoadElement => {
                let result = self.emit_binary_rt_call("__esc_rt_load_element", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::StoreElement => {
                let _result =
                    self.emit_ternary_rt_call("__esc_rt_store_element", &inst.operands)?;
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }

            // === Misc ===
            Op::Nop | Op::Debugger => {
                let undef = nanbox_emit::emit_box_undefined(self.ctx);
                self.set_value(vid, undef);
            }
            Op::ThisValue => {
                let result = self.emit_unary_rt_call("__esc_rt_this_value", &inst.operands)?;
                self.set_value(vid, result);
            }
            Op::NewTarget => {
                let func = self
                    .runtime
                    .get_void_i64("__esc_rt_new_target", self.module);
                let result = self
                    .builder
                    .build_call(func, &[], "new_target")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("new_target returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::ImportMeta => {
                // Pass the module path as a NaN-boxed string if available
                // (from operands), otherwise pass undefined (0).
                let path_arg = if !inst.operands.is_empty() {
                    self.get_value(inst.operands[0].0)?
                } else {
                    self.module.get_context().i64_type().const_zero()
                };
                let import_meta_fn = self
                    .runtime
                    .get_unary_js_op("__esc_rt_import_meta", self.module);
                let result = self
                    .builder
                    .build_call(import_meta_fn, &[path_arg.into()], "import_meta")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module("import_meta returned void".into()))?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::CreateArguments => {
                let create_args_fn = self
                    .runtime
                    .get_create_object("__esc_rt_create_arguments", self.module);
                let result = self
                    .builder
                    .build_call(create_args_fn, &[], "create_arguments")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| {
                        LlvmCodegenError::Module("create_arguments returned void".into())
                    })?
                    .into_int_value();
                self.set_value(vid, result);
            }
            Op::SuperCall => {
                // operands: [callee, ...args] — same layout as CallNew
                if !inst.operands.is_empty() {
                    let callee = self.get_value(inst.operands[0].0)?;
                    let args: Vec<BasicValueEnum<'ctx>> = inst.operands[1..]
                        .iter()
                        .map(|op| self.get_value(op.0).map(|v| v.into()))
                        .collect::<Result<Vec<_>, _>>()?;
                    let argc = self.ctx.i32_type().const_int(args.len() as u64, false);
                    let argv = self.spill_args_to_stack(&args)?;
                    let super_fn = self
                        .runtime
                        .get_call_variadic("__esc_rt_super_call", self.module);
                    let result = self
                        .builder
                        .build_call(
                            super_fn,
                            &[callee.into(), argc.into(), argv.into()],
                            "super_call",
                        )
                        .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                        .try_as_basic_value()
                        .left()
                        .ok_or_else(|| LlvmCodegenError::Module("super_call returned void".into()))?
                        .into_int_value();
                    self.set_value(vid, result);
                }
            }
            Op::WithScope | Op::CreateRegExp => {
                // ESC-19: Not yet supported on the LLVM backend.
                return Err(LlvmCodegenError::UnsupportedOpcode(format!(
                    "{:?}: not yet supported on the LLVM backend. \
                     Use the Cranelift backend (default) or track ESC-19.",
                    inst.op
                )));
            }
        }

        Ok(())
    }

    /// Emit a binary runtime call: `fn(i64, i64) -> i64`.
    fn emit_binary_rt_call(
        &mut self,
        name: &str,
        operands: &[ValueId],
    ) -> Result<IntValue<'ctx>, LlvmCodegenError> {
        let a = self.get_value(operands[0].0)?;
        let b = self.get_value(operands[1].0)?;
        let func = self.runtime.get_binary_js_op(name, self.module);
        let result = self
            .builder
            .build_call(func, &[a.into(), b.into()], &format!("{name}_result"))
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
            .try_as_basic_value()
            .left()
            .ok_or_else(|| LlvmCodegenError::Module(format!("{name} returned void")))?
            .into_int_value();
        Ok(result)
    }

    /// Emit a unary runtime call: `fn(i64) -> i64`.
    fn emit_unary_rt_call(
        &mut self,
        name: &str,
        operands: &[ValueId],
    ) -> Result<IntValue<'ctx>, LlvmCodegenError> {
        let a = self.get_value(operands[0].0)?;
        let func = self.runtime.get_unary_js_op(name, self.module);
        let result = self
            .builder
            .build_call(func, &[a.into()], &format!("{name}_result"))
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
            .try_as_basic_value()
            .left()
            .ok_or_else(|| LlvmCodegenError::Module(format!("{name} returned void")))?
            .into_int_value();
        Ok(result)
    }

    /// Emit a ternary runtime call: `fn(i64, i64, i64) -> i64`.
    fn emit_ternary_rt_call(
        &mut self,
        name: &str,
        operands: &[ValueId],
    ) -> Result<IntValue<'ctx>, LlvmCodegenError> {
        let a = self.get_value(operands[0].0)?;
        let b = self.get_value(operands[1].0)?;
        let c = self.get_value(operands[2].0)?;
        let func = self.runtime.get_ternary_js_op(name, self.module);
        let result = self
            .builder
            .build_call(
                func,
                &[a.into(), b.into(), c.into()],
                &format!("{name}_result"),
            )
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
            .try_as_basic_value()
            .left()
            .ok_or_else(|| LlvmCodegenError::Module(format!("{name} returned void")))?
            .into_int_value();
        Ok(result)
    }

    /// Emit a CallRuntime dispatch matching the Cranelift backend's logic.
    ///
    /// operands[0] is a ConstString ValueId referencing the runtime function name
    /// in the string table. Remaining operands are the JS arguments.
    fn emit_call_runtime_dispatch(
        &mut self,
        inst: &ir::TypedInstruction,
        vid: u32,
    ) -> Result<(), LlvmCodegenError> {
        // Resolve operands[0] → string table index → function name
        let str_idx = self
            .const_string_indices
            .get(&inst.operands[0].0)
            .copied()
            .ok_or(LlvmCodegenError::UndefinedValue(inst.operands[0].0))?;

        let fn_name = self
            .string_table
            .get(str_idx as usize)
            .ok_or_else(|| {
                LlvmCodegenError::Module(format!("string index {str_idx} out of range"))
            })?
            .clone();

        if fn_name.starts_with("__esc_rt_console_") {
            // Console ABI: (argc: i32, argv_ptr: *const u64) -> void
            let js_args: Vec<BasicValueEnum<'ctx>> = inst
                .operands
                .iter()
                .skip(1)
                .map(|op| self.get_value(op.0).map(|v| v.into()))
                .collect::<Result<Vec<_>, _>>()?;

            let argc = self.ctx.i32_type().const_int(js_args.len() as u64, false);
            let argv = self.spill_args_to_stack(&js_args)?;
            let console_fn = self.runtime.get_console_fn(&fn_name, self.module);
            self.builder
                .build_call(console_fn, &[argc.into(), argv.into()], "")
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            let undef = nanbox_emit::emit_box_undefined(self.ctx);
            self.set_value(vid, undef);
        } else {
            let operand_count = inst.operands.len();
            if operand_count == 5 {
                // name + 4 args: quaternary JS op (e.g., __esc_rt_define_accessor)
                let a = self.get_value(inst.operands[1].0)?;
                let b = self.get_value(inst.operands[2].0)?;
                let c = self.get_value(inst.operands[3].0)?;
                let d = self.get_value(inst.operands[4].0)?;
                let func = self.runtime.get_quaternary_js_op(&fn_name, self.module);
                let result = self
                    .builder
                    .build_call(
                        func,
                        &[a.into(), b.into(), c.into(), d.into()],
                        "rt_quaternary",
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module(format!("{fn_name} returned void")))?
                    .into_int_value();
                self.set_value(vid, result);
            } else if operand_count == 4 {
                // name + 3 args: ternary JS op
                let a = self.get_value(inst.operands[1].0)?;
                let b = self.get_value(inst.operands[2].0)?;
                let c = self.get_value(inst.operands[3].0)?;
                let func = self.runtime.get_ternary_js_op(&fn_name, self.module);
                let result = self
                    .builder
                    .build_call(func, &[a.into(), b.into(), c.into()], "rt_ternary")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module(format!("{fn_name} returned void")))?
                    .into_int_value();
                self.set_value(vid, result);
            } else if operand_count == 3 {
                // name + 2 args: binary JS op
                let lhs = self.get_value(inst.operands[1].0)?;
                let rhs = self.get_value(inst.operands[2].0)?;
                let func = self.runtime.get_binary_js_op(&fn_name, self.module);
                let result = self
                    .builder
                    .build_call(func, &[lhs.into(), rhs.into()], "rt_binary")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module(format!("{fn_name} returned void")))?
                    .into_int_value();
                self.set_value(vid, result);
            } else if operand_count == 2 {
                // name + 1 arg: unary JS op
                let operand = self.get_value(inst.operands[1].0)?;
                let func = self.runtime.get_unary_js_op(&fn_name, self.module);
                let result = self
                    .builder
                    .build_call(func, &[operand.into()], "rt_unary")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module(format!("{fn_name} returned void")))?
                    .into_int_value();
                self.set_value(vid, result);
            } else {
                // 0 JS args → () -> i64 (e.g., __esc_rt_get_global_this)
                let func = self.runtime.get_void_i64(&fn_name, self.module);
                let result = self
                    .builder
                    .build_call(func, &[], "rt_void")
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| LlvmCodegenError::Module(format!("{fn_name} returned void")))?
                    .into_int_value();
                self.set_value(vid, result);
            }
        }

        Ok(())
    }

    /// Unbox two i32 operands from NaN-boxed values.
    fn get_two_i32_operands(
        &self,
        operands: &[ValueId],
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), LlvmCodegenError> {
        let a = self.get_value(operands[0].0)?;
        let b = self.get_value(operands[1].0)?;
        let a_unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, a);
        let b_unboxed = nanbox_emit::emit_unbox_i32(self.builder, self.ctx, b);
        Ok((a_unboxed, b_unboxed))
    }

    /// Unbox two f64 operands from NaN-boxed values.
    fn get_two_f64_operands(
        &self,
        operands: &[ValueId],
    ) -> Result<
        (
            inkwell::values::FloatValue<'ctx>,
            inkwell::values::FloatValue<'ctx>,
        ),
        LlvmCodegenError,
    > {
        let a = self.get_value(operands[0].0)?;
        let b = self.get_value(operands[1].0)?;
        let a_unboxed = nanbox_emit::emit_unbox_f64(self.builder, self.ctx, a);
        let b_unboxed = nanbox_emit::emit_unbox_f64(self.builder, self.ctx, b);
        Ok((a_unboxed, b_unboxed))
    }

    /// Spill arguments to a stack-allocated array and return the pointer as i64.
    fn spill_args_to_stack(
        &self,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<IntValue<'ctx>, LlvmCodegenError> {
        if args.is_empty() {
            return Ok(self.ctx.i64_type().const_zero());
        }
        let i64_ty = self.ctx.i64_type();
        let arr_ty = i64_ty.array_type(args.len() as u32);
        let alloca = self
            .builder
            .build_alloca(arr_ty, "argv")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        for (i, arg) in args.iter().enumerate() {
            // SAFETY: build_in_bounds_gep is marked unsafe in inkwell because LLVM requires
            // the resulting pointer to stay within the allocated object. Here, `alloca` points
            // to an array of `args.len()` i64 elements allocated on the current stack frame,
            // and `i` is bounded by `args.len()`, so the GEP index is always in bounds.
            let gep = unsafe {
                self.builder.build_in_bounds_gep(
                    arr_ty,
                    alloca,
                    &[
                        self.ctx.i32_type().const_zero(),
                        self.ctx.i32_type().const_int(i as u64, false),
                    ],
                    &format!("argv_{i}"),
                )
            }
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

            self.builder
                .build_store(gep, arg.into_int_value())
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        }

        // Cast the alloca pointer to i64
        self.builder
            .build_ptr_to_int(alloca, i64_ty, "argv_ptr")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))
    }
}
