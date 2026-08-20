//! Function lowering: translates typed IR instructions into Cranelift IR.
//!
//! The [`FunctionLowerer`] walks each basic block in a [`TypedFunction`],
//! translating each [`::ir::TypedInstruction`] into Cranelift IR via a
//! [`FunctionBuilder`]. This is the core code generation loop.

use std::collections::HashMap;

use ::ir::builder::TypedFunction;
use ::ir::{BlockId, Op, ValueId};
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{self, InstBuilder, StackSlotData, StackSlotKind, TrapCode, Value};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{FuncId, Module};
use cranelift_object::ObjectModule;

use crate::constants::ConstantPool;
use crate::control_flow::BlockMap;
use crate::error::CodegenError;
use crate::nanbox_emit;
use crate::runtime_calls::RuntimeCalls;

/// Resolve a generator-transform sentinel string index to its content.
///
/// The generator transform uses sentinel `ConstString` indices to encode
/// runtime function names, error messages, and state property keys without
/// requiring entries in the string table. This function maps those sentinels
/// to their actual string content, returning `None` for normal indices that
/// should be looked up in the string table.
///
/// Index ranges:
/// - `0..3`: reserved property keys ("state_index", "resume_mode", "sent_value")
/// - `3..100`: parameter keys ("param_0", "param_1", ...)
/// - `100..1000`: slot keys ("slot_0", "slot_1", ...)
/// - `u32::MAX - N`: runtime function sentinels
fn resolve_sentinel_string(idx: u32) -> Option<String> {
    // High sentinel range (near u32::MAX)
    if idx == u32::MAX {
        return Some("__esc_rt_create_generator".to_string());
    }
    if idx == u32::MAX - 1 {
        return Some("Generator is already executing".to_string());
    }
    if idx == u32::MAX - 2 {
        return Some("__esc_rt_create_iter_result".to_string());
    }
    if idx == u32::MAX - 7 {
        return Some("__esc_rt_async_wrap".to_string());
    }
    if idx == u32::MAX - 8 {
        return Some("__esc_rt_create_async_generator".to_string());
    }
    if idx >= u32::MAX - 20 {
        // Reserve a range for future sentinels; treat as empty string
        // to avoid panics.
        return Some(String::new());
    }
    // None: use the normal string table
    None
}

/// Resolve a generator-transform property key index to its content.
///
/// The generator transform uses special `ConstString` index ranges for
/// state object property keys that bypass the normal string table:
/// - `0..3`: reserved keys ("state_index", "resume_mode", "sent_value")
/// - `3..100`: parameter keys ("param_0", "param_1", ...)
/// - `100..1000`: slot keys ("slot_0", "slot_1", ...)
///
/// Returns `None` if the index is in the normal string table range for
/// the given string table size.
fn resolve_generator_prop_key(idx: u32, string_table_len: usize) -> Option<String> {
    // Only apply if the index is NOT a valid string table entry
    if (idx as usize) < string_table_len {
        return None;
    }
    match idx {
        0 => Some("state_index".to_string()),
        1 => Some("resume_mode".to_string()),
        2 => Some("sent_value".to_string()),
        n if (3..100).contains(&n) => Some(format!("param_{}", n - 3)),
        n if (100..1000).contains(&n) => Some(format!("slot_{}", n - 100)),
        _ => None,
    }
}

/// Lowers a single [`TypedFunction`] into Cranelift IR.
pub struct FunctionLowerer<'a> {
    /// Map from IR ValueId to Cranelift Value.
    values: HashMap<u32, Value>,
    /// Block map (IR blocks -> Cranelift blocks + phi info).
    block_map: BlockMap,
    /// Runtime call declarations.
    runtime: &'a mut RuntimeCalls,
    /// String constant pool.
    constants: &'a mut ConstantPool,
    /// The object module (for declaring imports, data, etc.).
    module: &'a mut ObjectModule,
    /// Declared function IDs for intra-module calls.
    func_ids: &'a [FuncId],
    /// String table for ConstString resolution.
    string_table: &'a [String],
    /// Map from ConstString ValueId (raw u32) to string table index.
    const_string_indices: HashMap<u32, u32>,
    /// Map from phi result IR ValueId to Cranelift Variable (for cross-block access).
    phi_variables: HashMap<u32, cranelift_frontend::Variable>,
    /// Map from ConstI32 ValueId (raw u32) to the i32 literal value.
    const_i32_values: HashMap<u32, i32>,
    /// Stack of catch block targets for try/catch exception handling.
    try_catch_stack: Vec<ir::Block>,
    /// Precomputed catch targets for catch blocks. When a `throw` occurs inside
    /// a catch handler, this map tells us the enclosing (parent) catch block to
    /// jump to, rather than jumping back to the same catch block (infinite loop).
    catch_block_targets: HashMap<BlockId, Option<ir::Block>>,
    /// Precomputed catch target for every block in the function.
    ///
    /// Maps each IR block to the Cranelift catch block it should jump to when
    /// a throw/exception occurs. Unlike `try_catch_stack` (which depends on
    /// sequential block processing order), this map uses CFG reachability to
    /// correctly handle blocks inside try scopes regardless of IR ordering.
    block_catch_scope: HashMap<BlockId, ir::Block>,
    /// The IR block currently being lowered (used by throw to look up catch targets).
    current_ir_block: Option<BlockId>,
    /// Maximum parameter count across all functions in the module.
    /// Used to size argv stack slots for indirect calls so that missing
    /// parameters read pre-filled undefined values.
    pub max_func_params: usize,
}

impl<'a> FunctionLowerer<'a> {
    /// Create a new function lowerer.
    pub fn new(
        block_map: BlockMap,
        runtime: &'a mut RuntimeCalls,
        constants: &'a mut ConstantPool,
        module: &'a mut ObjectModule,
        func_ids: &'a [FuncId],
        string_table: &'a [String],
    ) -> Self {
        Self {
            values: HashMap::new(),
            block_map,
            runtime,
            constants,
            module,
            func_ids,
            string_table,
            phi_variables: HashMap::new(),
            const_string_indices: HashMap::new(),
            const_i32_values: HashMap::new(),
            try_catch_stack: Vec::new(),
            catch_block_targets: HashMap::new(),
            block_catch_scope: HashMap::new(),
            current_ir_block: None,
            max_func_params: 0,
        }
    }

    /// Precompute catch targets for catch blocks and per-block try scopes.
    ///
    /// This does two things:
    /// 1. **Catch block parents:** When a `throw` happens inside a catch handler,
    ///    it should jump to the *enclosing* (parent) try scope's catch handler,
    ///    not back to itself.
    /// 2. **Block-level catch scope:** For every block reachable from within a try
    ///    body (between `TryBegin` and `TryEnd`), records the catch block it
    ///    should jump to on exception. This is needed because blocks inside a try
    ///    scope may appear after the catch handler in the IR block list, making
    ///    the sequential `try_catch_stack` unreliable for those blocks.
    fn precompute_catch_targets(&mut self, func: &TypedFunction) -> Result<(), CodegenError> {
        let mut scope_stack: Vec<BlockId> = Vec::new();

        // Collect branch targets for each block
        let mut block_successors: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for bb in &func.blocks {
            let mut succs = Vec::new();
            for inst in &bb.instructions {
                for &target in &inst.block_targets {
                    succs.push(target);
                }
            }
            block_successors.insert(bb.id, succs);
        }

        // Pass 1: linear walk to find TryBegin/TryEnd and build catch_block_targets.
        // Also build a mapping from (block_id, instruction_index) -> catch_block_id
        // for TryEnd instructions, so we know which try scope each TryEnd pops.
        let mut try_end_scopes: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for bb in &func.blocks {
            for inst in &bb.instructions {
                if inst.op == Op::TryBegin {
                    let catch_block_id = inst.block_targets[0];
                    let parent_catch = scope_stack.last().copied();
                    let cl_target = match parent_catch {
                        Some(bid) => Some(self.block_map.get(bid)?),
                        None => None,
                    };
                    self.catch_block_targets.insert(catch_block_id, cl_target);
                    scope_stack.push(catch_block_id);
                } else if inst.op == Op::TryEnd
                    && let Some(catch_bid) = scope_stack.pop()
                {
                    try_end_scopes.entry(bb.id).or_default().push(catch_bid);
                }
            }
        }

        // Collect all catch block IDs for skipping during BFS
        let all_catch_blocks: std::collections::HashSet<BlockId> =
            self.catch_block_targets.keys().copied().collect();

        // Pass 2: for each TryBegin, BFS from the block containing it to find
        // all blocks reachable within that try scope. A block is in the try
        // scope if it's reachable from the TryBegin block via branch targets
        // and is NOT the catch block itself, and doesn't go past a TryEnd.
        //
        // Process in forward order (outer TryBegin first). Inner try scopes
        // run later and overwrite via `insert`, ensuring innermost scope wins.
        for bb in &func.blocks {
            for inst in &bb.instructions {
                if inst.op == Op::TryBegin {
                    let catch_block_id = inst.block_targets[0];
                    let cl_catch = self.block_map.get(catch_block_id)?;

                    // BFS from the TryBegin block
                    let mut visited = std::collections::HashSet::new();
                    let mut queue = std::collections::VecDeque::new();
                    queue.push_back(bb.id);

                    while let Some(bid) = queue.pop_front() {
                        if !visited.insert(bid) {
                            continue;
                        }
                        // Skip catch blocks (handled by catch_block_targets).
                        // Skip both this scope's catch block and any nested
                        // catch blocks — their exception routing is handled
                        // separately via catch_block_targets.
                        if all_catch_blocks.contains(&bid) {
                            continue;
                        }
                        // Record this block's catch scope. Later (inner) BFS
                        // runs overwrite earlier (outer) entries, so the
                        // innermost try scope wins.
                        // Don't map the TryBegin's own block — the
                        // try_catch_stack handles catch routing within
                        // that block (empty before TryBegin, pushed after).
                        // Including it causes domination errors because
                        // exception checks for pre-TryBegin instructions
                        // incorrectly branch to the catch handler.
                        if bid != bb.id {
                            self.block_catch_scope.insert(bid, cl_catch);
                        }

                        // Check if this block has a TryEnd for THIS try scope.
                        // Only stop traversal for TryEnd that pops OUR scope;
                        // nested TryEnd instructions should not stop the outer
                        // BFS from continuing.
                        let has_our_try_end = try_end_scopes
                            .get(&bid)
                            .is_some_and(|scopes| scopes.contains(&catch_block_id));

                        // Don't traverse successors past our TryEnd (the scope
                        // ends here) but DO include the block itself.
                        if has_our_try_end {
                            continue;
                        }

                        // Enqueue successors
                        if let Some(succs) = block_successors.get(&bid) {
                            for &succ in succs {
                                if !visited.contains(&succ) {
                                    queue.push_back(succ);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Lower an entire function body.
    pub fn lower(
        &mut self,
        func: &TypedFunction,
        mut builder: FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        // Precompute catch targets for catch blocks so that throws inside
        // catch handlers jump to the enclosing scope, not back to themselves.
        self.precompute_catch_targets(func)?;

        // Entry block: append function parameters
        if let Some(first_bb) = func.blocks.first() {
            let entry = self.block_map.get(first_bb.id)?;
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);

            // Map function params to values.
            // With Variable-based phi handling, block params are only function params.
            let params = builder.block_params(entry).to_vec();
            for (i, &param_val) in params.iter().enumerate() {
                // We use a negative offset convention: function param i
                // is not mapped via ValueId, it is referenced by instructions.
                // Store with a sentinel prefix. Actually, params are referenced
                // by their position in the IR, not by ValueId. We need to figure
                // out how params are referenced.
                //
                // In the IR, there's no explicit Param instruction in the TypedIR.
                // Parameters are implicitly available. We store them by index.
                self.values.insert(i as u32 | 0x8000_0000, param_val);
            }
        }

        // Pre-register ALL phi Variables so they're available for cross-block
        // references regardless of block processing order.
        for bb in &func.blocks {
            let phis = self.block_map.get_phis(bb.id);
            for phi in phis {
                self.phi_variables.insert(phi.value_id.0, phi.variable);
                // Empty phis (unresolved closure captures) need a default value
                // so use_var has a definition to fall back on.
                if self.block_map.get_phi_bindings(phi.value_id).is_empty() {
                    let undef = nanbox_emit::emit_box_undefined(&mut builder);
                    builder.def_var(phi.variable, undef);
                }
            }
        }

        // Lower each block
        for (bb_idx, bb) in func.blocks.iter().enumerate() {
            let cl_block = self.block_map.get(bb.id)?;
            self.current_ir_block = Some(bb.id);

            if bb_idx > 0 {
                builder.switch_to_block(cl_block);
            }

            // Resolve phi Variables: use_var for each phi to get the merged value.
            let phis = self.block_map.get_phis(bb.id).to_vec();
            for phi in &phis {
                let val = builder.use_var(phi.variable);
                self.values.insert(phi.value_id.0, val);
            }
            // If a phi result is also used as a phi operand (self-referencing or
            // cross-phi), def_var it so Cranelift can propagate it to successors.
            for phi in &phis {
                let bindings = self.block_map.get_phi_bindings(phi.value_id).to_vec();
                if !bindings.is_empty()
                    && let Some(&cl_val) = self.values.get(&phi.value_id.0)
                {
                    for binding in &bindings {
                        builder.def_var(binding.variable, cl_val);
                    }
                }
            }
            // Pre-resolve ALL registered phi Variables in the current block.
            // This handles cross-block references where a phi result from another
            // block is used in this block (e.g., nested loops).
            let phi_vars: Vec<(u32, cranelift_frontend::Variable)> =
                self.phi_variables.iter().map(|(&k, &v)| (k, v)).collect();
            for (value_id, var) in &phi_vars {
                let val = builder.use_var(*var);
                self.values.insert(*value_id, val);
            }

            // Catch handler block setup: when entering a catch handler,
            // reset try_catch_stack to the parent scope and call catch_end
            // to pop the runtime catch frame.  This is needed because the
            // desugar does NOT emit TryEnd in catch handlers (doing so
            // would break the linear scope_stack scan in
            // precompute_catch_targets for nested try/catch).
            if let Some(parent_target) = self.catch_block_targets.get(&bb.id) {
                // Reset try_catch_stack so exception checks in the catch
                // handler body jump to the enclosing scope, not back to
                // this catch block.
                self.try_catch_stack.clear();
                if let Some(parent_bb) = parent_target {
                    self.try_catch_stack.push(*parent_bb);
                }
                // Call __esc_rt_catch_end to pop the runtime catch frame.
                let func_id = self
                    .runtime
                    .get_void_void("__esc_rt_catch_end", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[]);
            }

            // Lower each instruction
            for inst in &bb.instructions {
                // Skip phi instructions (handled via Cranelift Variables above)
                if matches!(inst.op, Op::Phi) {
                    continue;
                }
                self.lower_instruction(inst, &mut builder)?;

                // If this instruction's result feeds into any phi Variables,
                // def_var so Cranelift can propagate the value across blocks.
                let bindings = self.block_map.get_phi_bindings(inst.id).to_vec();
                if !bindings.is_empty()
                    && let Some(&cl_val) = self.values.get(&inst.id.0)
                {
                    let val_ty = builder.func.dfg.value_type(cl_val);
                    for binding in &bindings {
                        // Coerce type if needed (e.g. i32 → i64 for JSValue phis)
                        // Phi variables are always JSValue (I64) type.
                        let coerced = if val_ty == types::I32 {
                            let tag = builder.ins().iconst(types::I64, 0x7ff9_0000_0000_0000_i64);
                            let ext = builder.ins().uextend(types::I64, cl_val);
                            builder.ins().bor(tag, ext)
                        } else if val_ty == types::F64 {
                            builder
                                .ins()
                                .bitcast(types::I64, ir::MemFlags::new(), cl_val)
                        } else if val_ty == types::I8 {
                            let ext = builder.ins().uextend(types::I64, cl_val);
                            let tag = builder.ins().iconst(types::I64, 0x7ff9_0000_0000_0000_i64);
                            builder.ins().bor(tag, ext)
                        } else {
                            cl_val
                        };
                        builder.def_var(binding.variable, coerced);
                    }
                }
            }
        }

        // Seal all blocks at once — safe because all predecessors are now
        // defined. Eager per-block sealing is incorrect when a later-numbered
        // block is a predecessor of an earlier-numbered block (e.g. the merge
        // block in an if/else comes before the else branch in IR ordering).
        builder.seal_all_blocks();
        builder.finalize();
        Ok(())
    }

    /// Lower a single instruction.
    fn lower_instruction(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        match &inst.op {
            // === Constants ===
            Op::ConstI32(val) => {
                let v = builder.ins().iconst(types::I32, i64::from(*val));
                self.values.insert(inst.id.0, v);
                self.const_i32_values.insert(inst.id.0, *val);
            }
            Op::ConstI64(val) => {
                let v = builder.ins().iconst(types::I64, *val);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstF64(val) => {
                let v = builder.ins().f64const(*val);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstBool(val) => {
                let raw = builder.ins().iconst(types::I8, i64::from(*val));
                let boxed = nanbox_emit::emit_box_bool(builder, raw);
                self.values.insert(inst.id.0, boxed);
            }
            Op::ConstNull => {
                let v = nanbox_emit::emit_box_null(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstUndefined => {
                let v = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstString(idx) => {
                // Check for sentinel strings (runtime functions, error messages)
                // and generator property keys that bypass the string table.
                let resolved = resolve_sentinel_string(*idx)
                    .or_else(|| resolve_generator_prop_key(*idx, self.string_table.len()));

                if let Some(sentinel_str) = resolved {
                    // Sentinel/generator string — emit inline data.
                    let raw_ptr = self.constants.emit_sentinel_string_ref(
                        *idx,
                        &sentinel_str,
                        self.module,
                        builder,
                    )?;
                    let len = sentinel_str.len() as i64;
                    let len_val = builder.ins().iconst(types::I64, len);
                    let func_id = self
                        .runtime
                        .get_binary_js_op("__esc_rt_string_from_data", self.module)?;
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let result = builder.ins().call(func_ref, &[raw_ptr, len_val]);
                    let v = builder.inst_results(result)[0];
                    self.values.insert(inst.id.0, v);
                    self.const_string_indices.insert(inst.id.0, *idx);
                } else {
                    let raw_ptr = self.constants.emit_string_ref(
                        *idx,
                        self.string_table,
                        self.module,
                        builder,
                    )?;
                    // Create a proper runtime string by calling
                    // __esc_rt_string_from_data(ptr, len) which returns a NaN-boxed
                    // string JSValue. This ensures string values are always valid
                    // JSValues, even when flowing through phi nodes.
                    let str_data = self.string_table.get(*idx as usize).ok_or_else(|| {
                        CodegenError::Module(format!("string index {idx} out of range"))
                    })?;
                    let len = str_data.len() as i64;
                    let len_val = builder.ins().iconst(types::I64, len);
                    let func_id = self
                        .runtime
                        .get_binary_js_op("__esc_rt_string_from_data", self.module)?;
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let result = builder.ins().call(func_ref, &[raw_ptr, len_val]);
                    let v = builder.inst_results(result)[0];
                    self.values.insert(inst.id.0, v);
                    self.const_string_indices.insert(inst.id.0, *idx);
                }
            }

            // === LoadGlobal — resolve a built-in global by name ===
            Op::LoadGlobal(idx) => {
                // Build the name string as a NaN-boxed JSValue, then call
                // __esc_rt_get_global(name_bits) -> i64.
                let raw_ptr = self.constants.emit_string_ref(
                    *idx,
                    self.string_table,
                    self.module,
                    builder,
                )?;
                let str_data = self.string_table.get(*idx as usize).ok_or_else(|| {
                    CodegenError::Module(format!("string index {idx} out of range"))
                })?;
                let len = str_data.len() as i64;
                let len_val = builder.ins().iconst(types::I64, len);
                let from_data_id = self
                    .runtime
                    .get_binary_js_op("__esc_rt_string_from_data", self.module)?;
                let from_data_ref = self.module.declare_func_in_func(from_data_id, builder.func);
                let str_result = builder.ins().call(from_data_ref, &[raw_ptr, len_val]);
                let name_bits = builder.inst_results(str_result)[0];
                let get_global_id = self
                    .runtime
                    .get_unary_js_op("__esc_rt_get_global", self.module)?;
                let get_global_ref = self
                    .module
                    .declare_func_in_func(get_global_id, builder.func);
                let result = builder.ins().call(get_global_ref, &[name_bits]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
                // Also record the string index so CallRuntime and friends
                // can still resolve the name if this value flows into them.
                self.const_string_indices.insert(inst.id.0, *idx);
            }

            // === Typed i32 arithmetic ===
            Op::AddI32 => self.emit_binary_i32(inst, builder, BinaryI32Op::Add)?,
            Op::SubI32 => self.emit_binary_i32(inst, builder, BinaryI32Op::Sub)?,
            Op::MulI32 => self.emit_binary_i32(inst, builder, BinaryI32Op::Mul)?,
            Op::DivI32 => self.emit_binary_i32(inst, builder, BinaryI32Op::Div)?,
            Op::ModI32 => self.emit_binary_i32(inst, builder, BinaryI32Op::Mod)?,
            Op::NegI32 => {
                let operand = self.get_value(inst.operands[0])?;
                let operand = Self::coerce_to_i32(operand, builder);
                let zero = builder.ins().iconst(types::I32, 0);
                let v = builder.ins().isub(zero, operand);
                self.values.insert(inst.id.0, v);
            }

            // === Typed f64 arithmetic ===
            Op::AddF64 => self.emit_binary_f64(inst, builder, BinaryF64Op::Add)?,
            Op::SubF64 => self.emit_binary_f64(inst, builder, BinaryF64Op::Sub)?,
            Op::MulF64 => self.emit_binary_f64(inst, builder, BinaryF64Op::Mul)?,
            Op::DivF64 => self.emit_binary_f64(inst, builder, BinaryF64Op::Div)?,
            Op::ModF64 => {
                // Cranelift has no fmod; emit call to __esc_rt_fmod.
                // The runtime expects NaN-boxed i64 operands.
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let lhs = self.ensure_nanboxed(lhs, inst.operands[0], builder)?;
                let rhs = self.ensure_nanboxed(rhs, inst.operands[1], builder)?;
                let func_id = self.runtime.get_binary_js_op("fmod", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[lhs, rhs]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::NegF64 => {
                let operand = self.get_value(inst.operands[0])?;
                let operand = Self::coerce_to_f64(operand, builder);
                let v = builder.ins().fneg(operand);
                self.values.insert(inst.id.0, v);
            }

            // === JS arithmetic (runtime calls) ===
            Op::AddJS => self.emit_rt_binary(inst, "__esc_rt_add_js", builder)?,
            Op::SubJS => self.emit_rt_binary(inst, "__esc_rt_sub_js", builder)?,
            Op::MulJS => self.emit_rt_binary(inst, "__esc_rt_mul_js", builder)?,
            Op::DivJS => self.emit_rt_binary(inst, "__esc_rt_div_js", builder)?,
            Op::ModJS => self.emit_rt_binary(inst, "__esc_rt_mod_js", builder)?,
            Op::ExpJS => self.emit_rt_binary(inst, "__esc_rt_exp_js", builder)?,
            Op::NegJS => self.emit_rt_unary(inst, "__esc_rt_neg_js", builder)?,

            // === Bitwise ops ===
            Op::BitwiseAnd => self.emit_binary_i32(inst, builder, BinaryI32Op::Band)?,
            Op::BitwiseOr => self.emit_binary_i32(inst, builder, BinaryI32Op::Bor)?,
            Op::BitwiseXor => self.emit_binary_i32(inst, builder, BinaryI32Op::Bxor)?,
            Op::BitwiseNot => {
                let operand = self.get_value(inst.operands[0])?;
                let operand = Self::coerce_to_i32(operand, builder);
                let v = builder.ins().bnot(operand);
                self.values.insert(inst.id.0, v);
            }
            Op::ShiftLeft => self.emit_binary_i32(inst, builder, BinaryI32Op::Shl)?,
            Op::ShiftRight => self.emit_binary_i32(inst, builder, BinaryI32Op::Sshr)?,
            Op::ShiftRightUnsigned => self.emit_binary_i32(inst, builder, BinaryI32Op::Ushr)?,

            // === Comparison (i32) ===
            Op::EqI32 => self.emit_icmp(inst, builder, ir::condcodes::IntCC::Equal)?,
            Op::NeI32 => self.emit_icmp(inst, builder, ir::condcodes::IntCC::NotEqual)?,
            Op::LtI32 => self.emit_icmp(inst, builder, ir::condcodes::IntCC::SignedLessThan)?,
            Op::LeI32 => {
                self.emit_icmp(inst, builder, ir::condcodes::IntCC::SignedLessThanOrEqual)?;
            }
            Op::GtI32 => {
                self.emit_icmp(inst, builder, ir::condcodes::IntCC::SignedGreaterThan)?;
            }
            Op::GeI32 => {
                self.emit_icmp(
                    inst,
                    builder,
                    ir::condcodes::IntCC::SignedGreaterThanOrEqual,
                )?;
            }

            // === Comparison (f64) ===
            Op::EqF64 => self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::Equal)?,
            Op::NeF64 => self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::NotEqual)?,
            Op::LtF64 => self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::LessThan)?,
            Op::LeF64 => {
                self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::LessThanOrEqual)?;
            }
            Op::GtF64 => {
                self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::GreaterThan)?;
            }
            Op::GeF64 => {
                self.emit_fcmp(inst, builder, ir::condcodes::FloatCC::GreaterThanOrEqual)?;
            }

            // === JS comparison (runtime calls) ===
            Op::EqStrict => self.emit_rt_binary(inst, "__esc_rt_eq_strict", builder)?,
            Op::EqAbstract => self.emit_rt_binary(inst, "__esc_rt_eq_abstract", builder)?,
            Op::NeStrict => self.emit_rt_binary(inst, "__esc_rt_ne_strict", builder)?,
            Op::NeAbstract => self.emit_rt_binary(inst, "__esc_rt_ne_abstract", builder)?,
            Op::LtJS => self.emit_rt_binary(inst, "__esc_rt_lt_js", builder)?,
            Op::LeJS => self.emit_rt_binary(inst, "__esc_rt_le_js", builder)?,
            Op::GtJS => self.emit_rt_binary(inst, "__esc_rt_gt_js", builder)?,
            Op::GeJS => self.emit_rt_binary(inst, "__esc_rt_ge_js", builder)?,

            // === Type conversion ===
            Op::ToNumber => self.emit_rt_unary(inst, "__esc_rt_to_number", builder)?,
            Op::ToBoolean => {
                let operand = self.get_value(inst.operands[0])?;
                let operand = self.ensure_nanboxed(operand, inst.operands[0], builder)?;
                let func_id = self.runtime.get_to_boolean(self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[operand]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::ToNumeric => self.emit_rt_unary(inst, "__esc_rt_to_numeric", builder)?,
            Op::ToString => self.emit_rt_unary(inst, "__esc_rt_to_string", builder)?,
            Op::ToObject => self.emit_rt_unary(inst, "__esc_rt_to_object", builder)?,
            Op::ToPrimitive => self.emit_rt_unary(inst, "__esc_rt_to_primitive", builder)?,
            Op::ToPropertyKey => {
                self.emit_rt_unary(inst, "__esc_rt_to_property_key", builder)?;
            }
            Op::ToInt32 => {
                // Call the runtime to coerce to int32 (returns NaN-boxed i32),
                // then unbox to get a raw Cranelift i32 value.
                self.emit_rt_unary(inst, "__esc_rt_to_int32", builder)?;
                let boxed = self.get_value(inst.id)?;
                let raw_i32 = nanbox_emit::emit_unbox_i32(builder, boxed);
                self.values.insert(inst.id.0, raw_i32);
            }
            Op::ToUint32 => {
                // Call the runtime to coerce to uint32 (returns NaN-boxed i32),
                // then unbox to get a raw Cranelift i32 value.
                self.emit_rt_unary(inst, "__esc_rt_to_uint32", builder)?;
                let boxed = self.get_value(inst.id)?;
                let raw_i32 = nanbox_emit::emit_unbox_i32(builder, boxed);
                self.values.insert(inst.id.0, raw_i32);
            }

            // === NaN-boxing ===
            Op::BoxI32 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_i32(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxUnsignedI32 => {
                // Treat the i32 as unsigned: zero-extend to i64, convert to
                // f64, then box as f64.  This preserves the full u32 range
                // (e.g. -1 as i32 → 4294967295.0 as f64).
                let operand = self.get_value(inst.operands[0])?;
                let u64_val = builder.ins().uextend(types::I64, operand);
                let f64_val = builder.ins().fcvt_from_uint(types::F64, u64_val);
                let v = nanbox_emit::emit_box_f64(builder, f64_val);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxF64 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_f64(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxBool => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_bool(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxNull => {
                let v = nanbox_emit::emit_box_null(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxUndefined => {
                let v = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxString => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_string(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxObject => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_object(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxSymbol => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_box_symbol(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxI32 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_unbox_i32(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxF64 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_unbox_f64(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxBool => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_unbox_bool(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxString => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_unbox_string(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxObject => {
                let operand = self.get_value(inst.operands[0])?;
                let v = nanbox_emit::emit_unbox_object(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::UnboxSymbol | Op::TypeofBoxed | Op::IsNullish | Op::IsFalsy => {
                self.emit_rt_unary(
                    inst,
                    match &inst.op {
                        Op::UnboxSymbol => "__esc_rt_unbox_symbol",
                        Op::TypeofBoxed => "__esc_rt_typeof_boxed",
                        Op::IsNullish => "__esc_rt_is_nullish",
                        Op::IsFalsy => "__esc_rt_is_falsy",
                        _ => unreachable!(),
                    },
                    builder,
                )?;
            }

            // === Control flow ===
            Op::Br => {
                let target = self.block_map.get(inst.block_targets[0])?;
                // Phi args are handled via Cranelift Variables (def_var/use_var)
                builder.ins().jump(target, &[]);
            }
            Op::BrIf => {
                let cond = self.get_value(inst.operands[0])?;
                let then_block = self.block_map.get(inst.block_targets[0])?;
                let else_block = self.block_map.get(inst.block_targets[1])?;

                // NaN-boxed values have tag bits set, so they're always non-zero.
                // We must call __esc_rt_to_boolean to get a proper 0/1 result
                // before branching, unless the value is already a raw i8/i32
                // from a comparison.
                let cond_ty = builder.func.dfg.value_type(cond);
                let bool_val = if cond_ty == types::I64 {
                    // i64 = NaN-boxed JSValue — must coerce to boolean
                    let func_id = self.runtime.get_to_boolean(self.module)?;
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let call = builder.ins().call(func_ref, &[cond]);
                    builder.inst_results(call)[0]
                } else {
                    // i8 or i32 from a comparison — already a raw boolean
                    cond
                };
                builder
                    .ins()
                    .brif(bool_val, then_block, &[], else_block, &[]);
            }
            Op::Ret => {
                if inst.operands.is_empty() {
                    builder.ins().return_(&[]);
                } else {
                    let val = self.get_value(inst.operands[0])?;
                    // Only NaN-box if the function signature returns i64 (JSValue).
                    // Functions returning i32, f64, etc. should pass values through as-is.
                    let sig_ret = builder.func.signature.returns.first().map(|r| r.value_type);
                    let ret_val = if sig_ret == Some(types::I64) {
                        self.ensure_nanboxed(val, inst.operands[0], builder)?
                    } else {
                        val
                    };
                    builder.ins().return_(&[ret_val]);
                }
            }
            Op::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
            }
            Op::Switch => {
                self.emit_switch(inst, builder)?;
            }
            Op::Phi => {
                // Handled in block_map setup — nothing to emit here
            }

            // === Calls ===
            Op::Call => {
                // First operand is the function reference (as a ConstI32 func index),
                // remaining operands are arguments.
                // For intra-module calls, we use the func_ids table.
                self.emit_call(inst, builder)?;
            }
            Op::CallRuntime => {
                self.emit_call_runtime(inst, builder)?;
            }
            Op::Invoke => {
                // Invoke is like Call but with exception handling.
                // For now, lower as a regular call.
                // todo!("Phase D: implement invoke with landing pads")
                self.emit_call(inst, builder)?;
            }

            // === String ops (runtime calls) ===
            Op::StringConcat => {
                self.emit_rt_binary(inst, "__esc_rt_string_concat", builder)?;
            }
            Op::StringLength => {
                let operand = self.get_value(inst.operands[0])?;
                let func_id = self.runtime.get_string_length(self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[operand]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }

            // === LoadLocal / StoreLocal ===
            Op::LoadLocal | Op::StoreLocal => {
                // In SSA IR, locals are already in SSA form. LoadLocal/StoreLocal
                // are handled through the value mapping directly.
                // For now, treat them as pass-through.
                if !inst.operands.is_empty() {
                    let v = self.get_value(inst.operands[0])?;
                    self.values.insert(inst.id.0, v);
                }
            }

            // === Function parameters ===
            Op::LoadParam(idx) => {
                let param_key = *idx | 0x8000_0000;
                let v = if let Some(&val) = self.values.get(&param_key) {
                    val
                } else {
                    // Parameter index beyond declared params — this is the
                    // closure environment. Load it from the thread-local
                    // CURRENT_CLOSURE_ENV via __esc_rt_get_closure_env().
                    let func_id = self
                        .runtime
                        .get_void_i64("__esc_rt_get_closure_env", self.module)?;
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let call = builder.ins().call(func_ref, &[]);
                    builder.inst_results(call)[0]
                };
                self.values.insert(inst.id.0, v);
            }

            // === Object creation ===
            Op::CreateObject => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_create_object", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::CreateObjectLiteral => {
                // Operands: [key0, val0, key1, val1, ...] — interleaved kvpairs
                let pair_count = inst.operands.len() / 2;
                let slot_size = ((inst.operands.len() * 8) as u32).max(8);
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_size,
                    0,
                ));
                for (i, &operand_id) in inst.operands.iter().enumerate() {
                    let val = self.get_value(operand_id)?;
                    let boxed = self.ensure_nanboxed(val, operand_id, builder)?;
                    let offset = (i * 8) as i32;
                    builder.ins().stack_store(boxed, ss, offset);
                }
                let kvpairs_ptr = builder.ins().stack_addr(types::I64, ss, 0);
                let count_val = builder.ins().iconst(types::I32, pair_count as i64);
                let func_id = self
                    .runtime
                    .get_i32_i64_to_i64("__esc_rt_create_object_literal", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[count_val, kvpairs_ptr]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::CreateArray => {
                let len = inst.operands.len() as i64;
                let len_val = builder.ins().iconst(types::I32, len);
                let create_id = self
                    .runtime
                    .get_create_array("__esc_rt_create_array", self.module)?;
                let create_ref = self.module.declare_func_in_func(create_id, builder.func);
                let result = builder.ins().call(create_ref, &[len_val]);
                let arr = builder.inst_results(result)[0];

                // Push each element into the array
                if !inst.operands.is_empty() {
                    let push_id = self
                        .runtime
                        .get_binary_js_op("__esc_rt_array_push", self.module)?;
                    let push_ref = self.module.declare_func_in_func(push_id, builder.func);
                    for &op in &inst.operands {
                        let elem = self.get_value(op)?;
                        let boxed = self.ensure_nanboxed(elem, op, builder)?;
                        builder.ins().call(push_ref, &[arr, boxed]);
                    }
                }

                self.values.insert(inst.id.0, arr);
            }
            Op::ObjectDefineProperty => {
                // obj, key, desc → void
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let desc = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let desc = self.ensure_nanboxed(desc, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_object_define_property", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, desc]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::ObjectGetPrototype => {
                self.emit_rt_unary(inst, "__esc_rt_object_get_prototype", builder)?;
            }
            Op::ObjectSetPrototype => {
                // obj, proto → void
                let obj = self.get_value(inst.operands[0])?;
                let proto = self.get_value(inst.operands[1])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let proto = self.ensure_nanboxed(proto, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_void_binary("__esc_rt_object_set_prototype", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, proto]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }

            // === Property access ops ===
            Op::GetProp => {
                self.emit_rt_binary(inst, "__esc_rt_get_prop", builder)?;
            }
            Op::ICGetProp => {
                // obj, key, ic_id → value
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let ic_id = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let func_id = self.runtime.get_ic_get_prop(self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[obj, key, ic_id]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::ICSetProp => {
                // obj, key, val, ic_id → void
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let ic_id = self.get_value(inst.operands[3])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self.runtime.get_ic_set_prop(self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val, ic_id]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::SetProp | Op::SetPropStrict => {
                // obj, key, val → void
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let rt_name = if inst.op == Op::SetPropStrict {
                    "__esc_rt_set_prop_strict"
                } else {
                    "__esc_rt_set_prop"
                };
                let func_id = self.runtime.get_void_ternary(rt_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::DeleteProp => {
                self.emit_rt_binary(inst, "__esc_rt_delete_prop", builder)?;
            }
            Op::HasProp => {
                self.emit_rt_binary(inst, "__esc_rt_has_prop", builder)?;
            }
            Op::GetElem => {
                self.emit_rt_binary(inst, "__esc_rt_get_elem", builder)?;
            }
            Op::SetElem => {
                // obj, key, val → void
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_set_elem", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::DeleteElem => {
                self.emit_rt_binary(inst, "__esc_rt_delete_elem", builder)?;
            }
            Op::GetPropDynamic => {
                self.emit_rt_binary(inst, "__esc_rt_get_prop", builder)?;
            }
            Op::SetPropDynamic | Op::SetPropDynamicStrict => {
                // obj, key, val → void (same as SetProp but key is runtime value)
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let rt_name = if inst.op == Op::SetPropDynamicStrict {
                    "__esc_rt_set_prop_strict"
                } else {
                    "__esc_rt_set_prop"
                };
                let func_id = self.runtime.get_void_ternary(rt_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::GetSuper => {
                self.emit_rt_binary(inst, "__esc_rt_get_super", builder)?;
            }
            Op::SetSuper => {
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_set_super", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::GetPrivate => {
                self.emit_rt_binary(inst, "__esc_rt_get_private", builder)?;
            }
            Op::SetPrivate => {
                let obj = self.get_value(inst.operands[0])?;
                let key = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_set_private", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, key, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::PrivateFieldGet => {
                self.emit_rt_binary(inst, "__esc_rt_private_field_get", builder)?;
            }
            Op::PrivateFieldSet => {
                let obj = self.get_value(inst.operands[0])?;
                let pid = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let pid = self.ensure_nanboxed(pid, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_ternary_js_op("__esc_rt_private_field_set", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, pid, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }
            Op::PrivateFieldHas => {
                self.emit_rt_binary(inst, "__esc_rt_private_field_has", builder)?;
            }
            Op::InstallPrivateField => {
                let obj = self.get_value(inst.operands[0])?;
                let pid = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let pid = self.ensure_nanboxed(pid, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_ternary_js_op("__esc_rt_install_private_field", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, pid, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
                self.emit_exception_check_if_needed(builder)?;
            }

            // === Closure / environment ops ===
            Op::CreateClosure => {
                // operands: [func_idx, env, flags]
                let func_idx_val = self.get_value(inst.operands[0])?;
                let env = self.get_value(inst.operands[1])?;
                let env = self.ensure_nanboxed(env, inst.operands[1], builder)?;
                let flags_val = self.get_value(inst.operands[2])?;
                let func_id = self
                    .runtime
                    .get_create_closure("__esc_rt_create_closure", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder
                    .ins()
                    .call(func_ref, &[func_idx_val, env, flags_val]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::EnvCreate => {
                // operands: [slot_count_as_const_i32]
                let slot_count = self.get_value(inst.operands[0])?;
                let null_parent = nanbox_emit::emit_box_null(builder);
                let func_id = self
                    .runtime
                    .get_env_create("__esc_rt_env_create", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[null_parent, slot_count]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::EnvLoad => {
                // operands: [env, slot_idx_as_const_i32]
                let env = self.get_value(inst.operands[0])?;
                let env = self.ensure_nanboxed(env, inst.operands[0], builder)?;
                let slot = self.get_value(inst.operands[1])?;
                let zero_depth = builder.ins().iconst(types::I32, 0);
                let func_id = self
                    .runtime
                    .get_env_load("__esc_rt_env_load", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[env, zero_depth, slot]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::EnvStore => {
                // operands: [env, slot_idx_as_const_i32, val]
                let env = self.get_value(inst.operands[0])?;
                let env = self.ensure_nanboxed(env, inst.operands[0], builder)?;
                let slot = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let zero_depth = builder.ins().iconst(types::I32, 0);
                let func_id = self
                    .runtime
                    .get_env_store("__esc_rt_env_store", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[env, zero_depth, slot, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::EnvExtend => {
                // operands: [outer, slot_count_as_const_i32]
                let outer = self.get_value(inst.operands[0])?;
                let outer = self.ensure_nanboxed(outer, inst.operands[0], builder)?;
                let slot_count = self.get_value(inst.operands[1])?;
                let func_id = self
                    .runtime
                    .get_env_create("__esc_rt_env_create", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[outer, slot_count]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }

            Op::EnvLookup => {
                // operands: [env, name_string]
                self.emit_rt_binary(inst, "__esc_rt_esc_env_lookup", builder)?;
            }
            Op::EnvLookupStore => {
                // operands: [env, name_string, value]
                let env = self.get_value(inst.operands[0])?;
                let env = self.ensure_nanboxed(env, inst.operands[0], builder)?;
                let name = self.get_value(inst.operands[1])?;
                let name = self.ensure_nanboxed(name, inst.operands[1], builder)?;
                let val = self.get_value(inst.operands[2])?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_ternary_js_op("__esc_rt_esc_env_store", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[env, name, val]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
                self.emit_exception_check_if_needed(builder)?;
            }

            // === JsBox ops ===
            Op::AllocBox => {
                self.emit_rt_unary(inst, "__esc_rt_alloc_box", builder)?;
            }
            Op::BoxLoad => {
                self.emit_rt_unary(inst, "__esc_rt_box_load", builder)?;
            }
            Op::BoxStore => {
                let box_ptr = self.get_value(inst.operands[0])?;
                let new_val = self.get_value(inst.operands[1])?;
                let box_ptr = self.ensure_nanboxed(box_ptr, inst.operands[0], builder)?;
                let new_val = self.ensure_nanboxed(new_val, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_void_binary("__esc_rt_box_store", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[box_ptr, new_val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }

            // === Call ops ===
            Op::CallNew => {
                self.emit_call_with_argv(inst, "__esc_rt_call_new", builder)?;
            }
            Op::CallMethod => {
                self.emit_call_method(inst, builder)?;
            }
            Op::CallVarargs => {
                // Same layout as CallNew for now
                self.emit_call_with_argv(inst, "__esc_rt_call_varargs", builder)?;
            }
            Op::TailCall => {
                // TCO deferred — lower as regular call
                self.emit_call(inst, builder)?;
            }
            Op::CallEval => {
                self.emit_call_with_argv(inst, "__esc_rt_call_eval", builder)?;
            }
            Op::CallEvalDirect => {
                // Direct eval with scope bridging: operands are
                // (code, lex_env, var_env, this_value).
                // Forwards all four operands to the runtime helper.
                self.emit_call_with_argv(inst, "__esc_rt_call_eval_direct", builder)?;
            }

            // === Exception handling ops ===
            Op::TryBegin => {
                // Push the catch block target for this try scope
                let catch_bb = self.block_map.get(inst.block_targets[0])?;
                self.try_catch_stack.push(catch_bb);
                // Call catch_begin to push exception frame
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_catch_begin", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::TryEnd => {
                // Pop the catch target for this try scope
                self.try_catch_stack.pop();
                let func_id = self
                    .runtime
                    .get_void_void("__esc_rt_catch_end", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::Throw => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self.runtime.get_void_unary("__esc_rt_throw", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);

                // Determine catch target using three strategies:
                // 1. Use try_catch_stack when it has an entry that is NOT a
                //    self-reference (handles nested try blocks in catch bodies).
                // 2. If we're in a catch block and try_catch_stack is empty or
                //    self-referencing, use the precomputed parent catch target
                //    from catch_block_targets.
                // 3. Fall back to the precomputed block_catch_scope map (CFG
                //    reachability for blocks after the catch handler in IR).
                let catch_target = self.resolve_catch_target_for_throw();

                if let Some(catch_bb) = catch_target {
                    builder.ins().jump(catch_bb, &[]);
                } else {
                    // Uncaught throw — return from function so the main wrapper
                    // can detect the pending exception and exit cleanly (code 1)
                    // instead of trapping with SIGILL (ud2).
                    self.emit_exception_return(builder);
                }
            }
            Op::Catch => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_get_exception", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::Finally => {
                // Finally block marker — no code to emit, execution flows through
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::Rethrow => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self.runtime.get_void_unary("__esc_rt_throw", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);

                // If the Rethrow carries an explicit catch target (set by
                // emit_finally_completion via rethrow_to), use it directly.
                // Otherwise fall back to catch_block_targets, try_catch_stack,
                // then block_catch_scope.
                let catch_target = if !inst.block_targets.is_empty() {
                    let target_bid = inst.block_targets[0];
                    Some(self.block_map.get(target_bid)?)
                } else {
                    self.resolve_catch_target_for_throw()
                };

                if let Some(catch_bb) = catch_target {
                    builder.ins().jump(catch_bb, &[]);
                } else {
                    // Uncaught rethrow — return from function cleanly
                    self.emit_exception_return(builder);
                }
            }
            Op::IsException => {
                self.emit_rt_unary(inst, "__esc_rt_is_exception", builder)?;
            }
            Op::GetException => {
                self.emit_rt_unary(inst, "__esc_rt_get_exception", builder)?;
            }

            // === Iterator ops ===
            Op::IterInit => {
                self.emit_rt_unary(inst, "__esc_rt_iter_init", builder)?;
            }
            Op::ForInInit => {
                self.emit_rt_unary(inst, "__esc_rt_for_in_init", builder)?;
            }
            Op::IterInitAsync => {
                self.emit_rt_unary(inst, "__esc_rt_iter_init_async", builder)?;
            }
            Op::IterNext => {
                self.emit_rt_unary(inst, "__esc_rt_iter_next", builder)?;
            }
            Op::IterDone => {
                self.emit_rt_unary(inst, "__esc_rt_iter_done", builder)?;
            }
            Op::IterValue => {
                self.emit_rt_unary(inst, "__esc_rt_iter_value", builder)?;
            }
            Op::IterClose => {
                let operand = self.get_value(inst.operands[0])?;
                let operand = self.ensure_nanboxed(operand, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_iter_close", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[operand]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }

            // === Promise / Async ops ===
            Op::PromiseCreate => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_promise_create", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::PromiseResolve => {
                let promise = self.get_value(inst.operands[0])?;
                let val = self.get_value(inst.operands[1])?;
                let promise = self.ensure_nanboxed(promise, inst.operands[0], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_void_binary("__esc_rt_promise_resolve", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[promise, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::PromiseReject => {
                let promise = self.get_value(inst.operands[0])?;
                let val = self.get_value(inst.operands[1])?;
                let promise = self.ensure_nanboxed(promise, inst.operands[0], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_void_binary("__esc_rt_promise_reject", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[promise, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::Await => {
                self.emit_rt_unary(inst, "__esc_rt_await", builder)?;
            }

            // === Generator ops ===
            Op::GeneratorCreate => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_generator_create", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::Yield => {
                self.emit_rt_unary(inst, "__esc_rt_yield", builder)?;
            }
            Op::YieldDelegate => {
                self.emit_rt_unary(inst, "__esc_rt_yield_delegate", builder)?;
            }

            // === String ops ===
            Op::StringCharAt => {
                self.emit_rt_binary(inst, "__esc_rt_string_char_at", builder)?;
            }
            Op::StringCompare => {
                self.emit_rt_binary(inst, "__esc_rt_string_compare", builder)?;
            }

            // === InstanceOf ===
            Op::InstanceOf => {
                self.emit_rt_binary(inst, "__esc_rt_instanceof", builder)?;
            }

            // === Type guards ===
            Op::GuardType => {
                // val, expected_tag → val (deopt/trap on mismatch)
                let val = self.get_value(inst.operands[0])?;
                let tag = self.get_value(inst.operands[1])?;
                let val_boxed = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let tag_boxed = self.ensure_nanboxed(tag, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_binary_js_op("__esc_rt_guard_type", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[val_boxed, tag_boxed]);
                let check = builder.inst_results(result)[0];
                // If check is 0 (false), trap
                builder.ins().trapz(check, TrapCode::unwrap_user(1));
                self.values.insert(inst.id.0, val_boxed);
            }
            Op::GuardShape => {
                let val = self.get_value(inst.operands[0])?;
                let shape = self.get_value(inst.operands[1])?;
                let val_boxed = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let shape_boxed = self.ensure_nanboxed(shape, inst.operands[1], builder)?;
                let func_id = self
                    .runtime
                    .get_binary_js_op("__esc_rt_guard_shape", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[val_boxed, shape_boxed]);
                let check = builder.inst_results(result)[0];
                builder.ins().trapz(check, TrapCode::unwrap_user(1));
                self.values.insert(inst.id.0, val_boxed);
            }
            Op::GuardTruthiness => {
                let val = self.get_value(inst.operands[0])?;
                let val_boxed = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self.runtime.get_to_boolean(self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[val_boxed]);
                let check = builder.inst_results(result)[0];
                builder.ins().trapz(check, TrapCode::unwrap_user(1));
                self.values.insert(inst.id.0, val_boxed);
            }

            // === Shape ops ===
            Op::ShapeCheck => {
                self.emit_rt_binary(inst, "__esc_rt_shape_check", builder)?;
            }
            Op::ShapeTransition => {
                self.emit_rt_binary(inst, "__esc_rt_shape_transition", builder)?;
            }

            // === TDZ / Drop flags ===
            Op::TdzCheck => {
                self.emit_rt_unary(inst, "__esc_rt_tdz_check", builder)?;
            }
            Op::TdzInit => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_tdz_init", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::DropFlagSet => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_drop_flag_set", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::DropFlagCheck => {
                self.emit_rt_unary(inst, "__esc_rt_drop_flag_check", builder)?;
            }

            // === Memory allocation ops ===
            Op::AllocZone => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_alloc_zone", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::AllocHeap => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_alloc_heap", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::AllocStack => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_alloc_stack", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::AllocArray => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_alloc_array", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::FreeZone => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_free_zone", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::IncRef => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_inc_ref", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::DecRef => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let func_id = self
                    .runtime
                    .get_void_unary("__esc_rt_dec_ref", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }

            // === Field / element access ===
            Op::LoadField => {
                self.emit_rt_binary(inst, "__esc_rt_load_field", builder)?;
            }
            Op::StoreField => {
                let obj = self.get_value(inst.operands[0])?;
                let idx = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
                let idx = self.ensure_nanboxed(idx, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_store_field", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[obj, idx, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::LoadElement => {
                self.emit_rt_binary(inst, "__esc_rt_load_element", builder)?;
            }
            Op::StoreElement => {
                let arr = self.get_value(inst.operands[0])?;
                let idx = self.get_value(inst.operands[1])?;
                let val = self.get_value(inst.operands[2])?;
                let arr = self.ensure_nanboxed(arr, inst.operands[0], builder)?;
                let idx = self.ensure_nanboxed(idx, inst.operands[1], builder)?;
                let val = self.ensure_nanboxed(val, inst.operands[2], builder)?;
                let func_id = self
                    .runtime
                    .get_void_ternary("__esc_rt_store_element", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[arr, idx, val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }

            // === RC operations ===
            Op::RcIncStrong | Op::RcIncWeak => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let name = if matches!(inst.op, Op::RcIncStrong) {
                    "__esc_rt_rc_inc_strong"
                } else {
                    "__esc_rt_rc_inc_weak"
                };
                let func_id = self.runtime.get_void_unary(name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::RcDecStrong | Op::RcDecWeak => {
                let val = self.get_value(inst.operands[0])?;
                let val = self.ensure_nanboxed(val, inst.operands[0], builder)?;
                let name = if matches!(inst.op, Op::RcDecStrong) {
                    "__esc_rt_rc_dec_strong"
                } else {
                    "__esc_rt_rc_dec_weak"
                };
                let func_id = self.runtime.get_void_unary(name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                builder.ins().call(func_ref, &[val]);
                let undef = nanbox_emit::emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
            Op::RcIsUnique => {
                self.emit_rt_unary(inst, "__esc_rt_rc_is_unique", builder)?;
            }

            // === Miscellaneous ops ===
            Op::Nop => {
                // No-op: emit nothing
            }
            Op::Debugger => {
                // Debugger: no-op in production
            }
            Op::ThisValue => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_this_value", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::NewTarget => {
                let func_id = self
                    .runtime
                    .get_void_i64("__cs_rt_new_target", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::ImportMeta => {
                // Pass the module path as a NaN-boxed string if available
                // (from operands), otherwise pass undefined (0).
                let path_arg = if !inst.operands.is_empty() {
                    self.get_value(inst.operands[0])?
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                let func_id = self
                    .runtime
                    .get_unary_js_op("__esc_rt_import_meta", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[path_arg]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            Op::SuperCall => {
                self.emit_call_with_argv(inst, "__esc_rt_super_call", builder)?;
            }
            Op::WithScope => {
                self.emit_rt_unary(inst, "__esc_rt_with_scope", builder)?;
            }
            Op::CreateRegExp => {
                self.emit_rt_binary(inst, "__esc_rt_create_regexp", builder)?;
            }
            Op::CreateArguments => {
                let func_id = self
                    .runtime
                    .get_void_i64("__esc_rt_create_arguments", self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
        }
        Ok(())
    }

    /// Get the Cranelift value for an IR ValueId.
    /// Get the Cranelift value for an IR ValueId.
    fn get_value(&self, id: ValueId) -> Result<Value, CodegenError> {
        self.values
            .get(&id.0)
            .copied()
            .ok_or(CodegenError::UndefinedValue(id.0))
    }

    /// Emit a binary i32 operation.
    ///
    /// Operands may arrive as i64 (from phi nodes or NaN-boxed values) when the
    /// specialization pass has rewritten a generic JS op (e.g. `AddJS`) to a
    /// typed i32 op (e.g. `AddI32`). In that case the operands are unboxed to
    /// i32 before the operation.
    fn emit_binary_i32(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        op: BinaryI32Op,
    ) -> Result<(), CodegenError> {
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = Self::coerce_to_i32(lhs, builder);
        let rhs = Self::coerce_to_i32(rhs, builder);
        let v = match op {
            BinaryI32Op::Add => builder.ins().iadd(lhs, rhs),
            BinaryI32Op::Sub => builder.ins().isub(lhs, rhs),
            BinaryI32Op::Mul => builder.ins().imul(lhs, rhs),
            BinaryI32Op::Div => builder.ins().sdiv(lhs, rhs),
            BinaryI32Op::Mod => builder.ins().srem(lhs, rhs),
            BinaryI32Op::Band => builder.ins().band(lhs, rhs),
            BinaryI32Op::Bor => builder.ins().bor(lhs, rhs),
            BinaryI32Op::Bxor => builder.ins().bxor(lhs, rhs),
            BinaryI32Op::Shl => builder.ins().ishl(lhs, rhs),
            BinaryI32Op::Sshr => builder.ins().sshr(lhs, rhs),
            BinaryI32Op::Ushr => builder.ins().ushr(lhs, rhs),
        };
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a binary f64 operation.
    ///
    /// Operands may arrive as i64 (from phi nodes or NaN-boxed values) when the
    /// specialization pass has rewritten a generic JS op (e.g. `AddJS`) to a
    /// typed f64 op (e.g. `AddF64`). In that case the operands are unboxed to
    /// f64 before the operation.
    fn emit_binary_f64(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        op: BinaryF64Op,
    ) -> Result<(), CodegenError> {
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = Self::coerce_to_f64(lhs, builder);
        let rhs = Self::coerce_to_f64(rhs, builder);
        let v = match op {
            BinaryF64Op::Add => builder.ins().fadd(lhs, rhs),
            BinaryF64Op::Sub => builder.ins().fsub(lhs, rhs),
            BinaryF64Op::Mul => builder.ins().fmul(lhs, rhs),
            BinaryF64Op::Div => builder.ins().fdiv(lhs, rhs),
        };
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit an integer comparison.
    ///
    /// Both operands are coerced to i32 to handle cases where phi nodes or
    /// NaN-boxed values flow into a specialized i32 comparison.
    fn emit_icmp(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        cc: ir::condcodes::IntCC,
    ) -> Result<(), CodegenError> {
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = Self::coerce_to_i32(lhs, builder);
        let rhs = Self::coerce_to_i32(rhs, builder);
        let v = builder.ins().icmp(cc, lhs, rhs);
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a float comparison.
    ///
    /// Both operands are coerced to f64 to handle cases where phi nodes or
    /// NaN-boxed values flow into a specialized f64 comparison.
    fn emit_fcmp(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        cc: ir::condcodes::FloatCC,
    ) -> Result<(), CodegenError> {
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = Self::coerce_to_f64(lhs, builder);
        let rhs = Self::coerce_to_f64(rhs, builder);
        let v = builder.ins().fcmp(cc, lhs, rhs);
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a return instruction with a dummy value matching the function's
    /// return type. Used when an uncaught exception must propagate up the
    /// call chain without trapping (SIGILL).
    fn emit_exception_return(&self, builder: &mut FunctionBuilder<'_>) {
        let sig_ret = builder.func.signature.returns.first().map(|r| r.value_type);
        if sig_ret == Some(types::I64) {
            let undef = nanbox_emit::emit_box_undefined(builder);
            builder.ins().return_(&[undef]);
        } else if sig_ret == Some(types::I32) {
            let one = builder.ins().iconst(types::I32, 1);
            builder.ins().return_(&[one]);
        } else {
            builder.ins().return_(&[]);
        }
    }

    /// Resolve the catch target for a throw or exception check, avoiding
    /// self-references to the current catch block (which would loop).
    ///
    /// Priority order:
    /// 1. `try_catch_stack` top, if it doesn't self-reference the current
    ///    catch block (handles nested try inside catch body).
    /// 2. `catch_block_targets` parent catch (for rethrows in catch bodies
    ///    without nested try scopes).
    /// 3. `block_catch_scope` (CFG reachability fallback).
    fn resolve_catch_target_for_throw(&self) -> Option<ir::Block> {
        let is_catch_block = self
            .current_ir_block
            .is_some_and(|bid| self.catch_block_targets.contains_key(&bid));

        if is_catch_block {
            // In a catch block: use parent catch target from catch_block_targets,
            // unless try_catch_stack has a different (nested) target.
            let stack_top = self.try_catch_stack.last().copied();
            if let Some(target) = stack_top {
                let my_cl_block = self
                    .current_ir_block
                    .and_then(|bid| self.block_map.get(bid).ok());
                if my_cl_block == Some(target) {
                    // Self-reference — use parent
                    self.current_ir_block
                        .and_then(|bid| self.catch_block_targets.get(&bid))
                        .copied()
                        .flatten()
                } else {
                    Some(target)
                }
            } else {
                self.current_ir_block
                    .and_then(|bid| self.catch_block_targets.get(&bid))
                    .copied()
                    .flatten()
            }
        } else {
            // Not in a catch block: prefer precomputed block_catch_scope
            // (CFG-based, always correct) over try_catch_stack (fragile,
            // depends on block processing order).
            if let Some(scope_target) = self
                .current_ir_block
                .and_then(|bid| self.block_catch_scope.get(&bid).copied())
            {
                return Some(scope_target);
            }
            self.try_catch_stack.last().copied()
        }
    }

    /// Check for a pending runtime exception after a call.
    ///
    /// Calls `__esc_rt_has_pending_exception()` which returns a raw `i32`
    /// (0 or 1). If an exception is pending:
    /// - **Inside a try block:** branch to the catch handler
    /// - **Outside a try block:** return from the function so the exception
    ///   propagates to the caller (ultimately reaching the main wrapper which
    ///   exits with code 1 instead of SIGILL)
    fn emit_exception_check_if_needed(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        let func_id = self
            .runtime
            .get_void_i32("__esc_rt_has_pending_exception", self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder.ins().call(func_ref, &[]);
        let has_exc = builder.inst_results(result)[0]; // i32

        let continue_bb = builder.create_block();

        // Determine catch target, avoiding self-references to the current
        // catch block (which would cause infinite loops).
        let catch_target = self.resolve_catch_target_for_throw();

        if let Some(catch_bb) = catch_target {
            // Inside a try block: branch to the catch handler
            builder.ins().brif(has_exc, catch_bb, &[], continue_bb, &[]);
        } else {
            // Outside a try block: return from the function to propagate
            // the exception up the call chain
            let exc_return_bb = builder.create_block();
            builder
                .ins()
                .brif(has_exc, exc_return_bb, &[], continue_bb, &[]);
            builder.switch_to_block(exc_return_bb);
            builder.seal_block(exc_return_bb);
            self.emit_exception_return(builder);
        }

        builder.switch_to_block(continue_bb);
        builder.seal_block(continue_bb);

        Ok(())
    }

    fn emit_rt_binary(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.len() < 2 {
            return Err(CodegenError::Module(format!(
                "{name}: expected 2 operands, got {}",
                inst.operands.len()
            )));
        }
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = self.ensure_nanboxed(lhs, inst.operands[0], builder)?;
        let rhs = self.ensure_nanboxed(rhs, inst.operands[1], builder)?;
        let func_id = self.runtime.get_binary_js_op(name, self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder.ins().call(func_ref, &[lhs, rhs]);
        let v = builder.inst_results(result)[0];
        self.values.insert(inst.id.0, v);
        self.emit_exception_check_if_needed(builder)?;
        Ok(())
    }

    /// Emit a runtime call to a unary `(i64) -> i64` helper.
    ///
    /// The operand is NaN-boxed before being passed to the runtime function.
    fn emit_rt_unary(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.is_empty() {
            return Err(CodegenError::Module(format!(
                "{name}: expected 1 operand, got 0"
            )));
        }
        let operand = self.get_value(inst.operands[0])?;
        let operand = self.ensure_nanboxed(operand, inst.operands[0], builder)?;
        let func_id = self.runtime.get_unary_js_op(name, self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder.ins().call(func_ref, &[operand]);
        let v = builder.inst_results(result)[0];
        self.values.insert(inst.id.0, v);
        self.emit_exception_check_if_needed(builder)?;
        Ok(())
    }

    /// Emit a call instruction.
    ///
    /// When the callee is a compile-time constant function index (ConstI32), a
    /// direct intra-module call is emitted.  Otherwise — e.g. when the callee
    /// is a runtime closure value — the call is dispatched through the
    /// `__esc_rt_call_indirect(callee, argc, argv_ptr)` runtime helper.
    fn emit_call(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.is_empty() {
            return Err(CodegenError::Module(
                "Call: instruction has no operands (expected at least callee)".to_string(),
            ));
        }
        if self.func_ids.is_empty() {
            self.emit_trap(inst, builder);
            return Ok(());
        }

        // Try direct call first: callee is a const i32 (function index)
        if let Some(&func_idx) = self.const_i32_values.get(&inst.operands[0].0) {
            let func_id = *self.func_ids.get(func_idx as usize).ok_or_else(|| {
                CodegenError::Module(format!("function index {func_idx} out of range"))
            })?;

            let mut args = Vec::with_capacity(inst.operands.len() - 1);
            for &op in &inst.operands[1..] {
                let val = self.get_value(op)?;
                let boxed = self.ensure_nanboxed(val, op, builder)?;
                args.push(boxed);
            }

            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            let result = builder.ins().call(func_ref, &args);
            let results = builder.inst_results(result);
            if !results.is_empty() {
                self.values.insert(inst.id.0, results[0]);
            }
            self.emit_exception_check_if_needed(builder)?;
            return Ok(());
        }

        // Indirect call: callee is a runtime value (e.g. closure)
        // Call __esc_rt_call_indirect(callee, argc, argv_ptr)
        let callee_val = self.get_value(inst.operands[0])?;
        let callee_boxed = self.ensure_nanboxed(callee_val, inst.operands[0], builder)?;

        let js_args: Vec<ValueId> = inst.operands.iter().skip(1).copied().collect();
        let argc = js_args.len();
        // Allocate enough slots for max(argc, max_param_count) so that the
        // dispatch trampoline can safely read params beyond argc.  Missing
        // arguments are pre-filled with undefined (ES2024 §10.2.1.1 step 19).
        let max_params = self.max_func_params.max(argc);
        let slot_size = ((max_params * 8) as u32).max(8);

        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size,
            0,
        ));

        // Pre-fill all slots with undefined (correct NaN-boxed undefined tag)
        let undef_val = nanbox_emit::emit_box_undefined(builder);
        for p in 0..max_params {
            let offset = (p * 8) as i32;
            builder.ins().stack_store(undef_val, ss, offset);
        }

        // Overwrite with actual arguments
        for (i, &operand_id) in js_args.iter().enumerate() {
            let arg = self.get_value(operand_id)?;
            let boxed = self.ensure_nanboxed(arg, operand_id, builder)?;
            let offset = (i * 8) as i32;
            builder.ins().stack_store(boxed, ss, offset);
        }

        let argv_ptr = builder.ins().stack_addr(types::I64, ss, 0);
        let argc_val = builder.ins().iconst(types::I32, argc as i64);

        let func_id = self
            .runtime
            .get_call_variadic("__esc_rt_call_indirect", self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder
            .ins()
            .call(func_ref, &[callee_boxed, argc_val, argv_ptr]);
        let v = builder.inst_results(result)[0];
        self.values.insert(inst.id.0, v);
        self.emit_exception_check_if_needed(builder)?;
        Ok(())
    }

    /// Emit a runtime helper call (CallRuntime).
    ///
    /// The first operand is a ConstString ValueId referencing the runtime function
    /// name in the string table. Remaining operands are the JS arguments.
    fn emit_call_runtime(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.is_empty() {
            return Err(CodegenError::Module(
                "CallRuntime: instruction has no operands (expected function name)".to_string(),
            ));
        }
        // Resolve operands[0] → string table index → function name
        let str_idx = self
            .const_string_indices
            .get(&inst.operands[0].0)
            .copied()
            .ok_or(CodegenError::UndefinedValue(inst.operands[0].0))?;

        // Handle sentinel string indices from the generator transform
        let fn_name = if let Some(resolved) = resolve_sentinel_string(str_idx)
            .or_else(|| resolve_generator_prop_key(str_idx, self.string_table.len()))
        {
            resolved
        } else {
            self.string_table
                .get(str_idx as usize)
                .ok_or_else(|| {
                    CodegenError::Module(format!("string index {str_idx} out of range"))
                })?
                .clone()
        };

        if fn_name.starts_with("__esc_rt_console_") {
            // Console ABI: (argc: i32, argv_ptr: *const u64) -> void
            // All args must be NaN-boxed i64 values.
            let js_arg_operands: Vec<ValueId> = inst.operands.iter().skip(1).copied().collect();

            let argc = js_arg_operands.len();
            let slot_size = (argc * 8) as u32;
            let actual_slot_size = slot_size.max(8);

            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                actual_slot_size,
                0,
            ));

            // Store each arg as a NaN-boxed i64 into sequential positions
            for (i, &operand_id) in js_arg_operands.iter().enumerate() {
                let arg = self.get_value(operand_id)?;
                let boxed = self.ensure_nanboxed(arg, operand_id, builder)?;
                let offset = (i * 8) as i32;
                builder.ins().stack_store(boxed, ss, offset);
            }

            // Get the stack address as pointer and argc as i32
            let argv_ptr = builder.ins().stack_addr(types::I64, ss, 0);
            let argc_val = builder.ins().iconst(types::I32, argc as i64);

            let func_id = self.runtime.get_console_fn(&fn_name, self.module)?;
            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            builder.ins().call(func_ref, &[argc_val, argv_ptr]);

            // Console functions return void — store undefined for the instruction output
            let undef = nanbox_emit::emit_box_undefined(builder);
            self.values.insert(inst.id.0, undef);
            // Console functions don't throw — no exception check needed.
        } else {
            let operand_count = inst.operands.len();
            if operand_count == 5 {
                // name + 4 args: quaternary JS op (e.g., __esc_rt_define_accessor)
                let a = self.get_value(inst.operands[1])?;
                let a = self.ensure_nanboxed(a, inst.operands[1], builder)?;
                let b = self.get_value(inst.operands[2])?;
                let b = self.ensure_nanboxed(b, inst.operands[2], builder)?;
                let c = self.get_value(inst.operands[3])?;
                let c = self.ensure_nanboxed(c, inst.operands[3], builder)?;
                let d = self.get_value(inst.operands[4])?;
                let d = self.ensure_nanboxed(d, inst.operands[4], builder)?;
                let func_id = self.runtime.get_quaternary_js_op(&fn_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[a, b, c, d]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            } else if operand_count == 4 {
                // name + 3 args: ternary JS op (e.g., esc_env_set, populate_slot_map)
                let a = self.get_value(inst.operands[1])?;
                let a = self.ensure_nanboxed(a, inst.operands[1], builder)?;
                let b = self.get_value(inst.operands[2])?;
                let b = self.ensure_nanboxed(b, inst.operands[2], builder)?;
                let c = self.get_value(inst.operands[3])?;
                let c = self.ensure_nanboxed(c, inst.operands[3], builder)?;
                let func_id = self.runtime.get_ternary_js_op(&fn_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[a, b, c]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            } else if operand_count == 3 {
                // name + 2 args: binary JS op
                let lhs = self.get_value(inst.operands[1])?;
                let lhs = self.ensure_nanboxed(lhs, inst.operands[1], builder)?;
                let rhs = self.get_value(inst.operands[2])?;
                let rhs = self.ensure_nanboxed(rhs, inst.operands[2], builder)?;
                let func_id = self.runtime.get_binary_js_op(&fn_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[lhs, rhs]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            } else if operand_count == 2 {
                // name + 1 arg: unary JS op
                let operand = self.get_value(inst.operands[1])?;
                let operand = self.ensure_nanboxed(operand, inst.operands[1], builder)?;
                let func_id = self.runtime.get_unary_js_op(&fn_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[operand]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            } else {
                // Fallback: 0 JS args → () -> i64. Use void_i64 signature
                // because many 0-arg runtime calls (e.g., __esc_rt_get_global_this)
                // return a value. The result is NaN-boxed i64.
                let func_id = self.runtime.get_void_i64(&fn_name, self.module)?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                let result = builder.ins().call(func_ref, &[]);
                let v = builder.inst_results(result)[0];
                self.values.insert(inst.id.0, v);
            }
            // Non-console runtime calls may throw — check for pending exception.
            self.emit_exception_check_if_needed(builder)?;
        }

        Ok(())
    }

    /// Ensure a Cranelift value is NaN-boxed (i64).
    ///
    /// If the value is already i64 (from ConstNull, ConstUndefined, BoxI32, etc.),
    /// it is returned as-is unless it came from a ConstString (raw data pointer),
    /// in which case `__esc_rt_string_from_data` is called to create a proper
    /// NaN-boxed string value.
    ///
    /// For sub-i64 types (i8 for bools, i32 for ints, f64 for floats), the
    /// appropriate NaN-boxing sequence is emitted.
    fn ensure_nanboxed(
        &mut self,
        arg: Value,
        _operand_id: ValueId,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<Value, CodegenError> {
        let arg_type = builder.func.dfg.value_type(arg);
        if arg_type == types::I8 {
            // Bool → BoxBool
            Ok(nanbox_emit::emit_box_bool(builder, arg))
        } else if arg_type == types::I32 {
            // Integer → BoxI32
            Ok(nanbox_emit::emit_box_i32(builder, arg))
        } else if arg_type == types::F64 {
            // Float → BoxF64
            Ok(nanbox_emit::emit_box_f64(builder, arg))
        } else if arg_type == types::I64 {
            // Already NaN-boxed (ConstNull, ConstUndefined, ConstString, BoxI32
            // result, phi result, etc.)
            Ok(arg)
        } else {
            // Unknown type — pass through
            Ok(arg)
        }
    }

    /// Coerce a Cranelift value to i32.
    ///
    /// After the specialization pass rewrites generic JS ops (e.g. `AddJS`) to
    /// typed i32 ops (e.g. `AddI32`), operands may still be i64 (from phi
    /// nodes, which are always declared as `I64`/JSValue in Cranelift). This
    /// helper unboxes them: i64 values are NaN-unboxed via `ireduce`, f64
    /// values are converted via `fcvt_to_sint`, and i32 values pass through.
    fn coerce_to_i32(val: Value, builder: &mut FunctionBuilder<'_>) -> Value {
        let ty = builder.func.dfg.value_type(val);
        if ty == types::I32 {
            val
        } else if ty == types::I64 {
            // Unbox: extract lower 32 bits (NaN-boxed i32 payload)
            nanbox_emit::emit_unbox_i32(builder, val)
        } else if ty == types::F64 {
            // Convert f64 to i32 (truncation)
            builder.ins().fcvt_to_sint(types::I32, val)
        } else if ty == types::I8 {
            // Extend bool (i8) to i32
            builder.ins().uextend(types::I32, val)
        } else {
            // Unknown type — pass through (will fail at Cranelift verifier
            // but at least won't panic here).
            val
        }
    }

    /// Coerce a Cranelift value to f64.
    ///
    /// After the specialization pass rewrites generic JS ops (e.g. `AddJS`) to
    /// typed f64 ops (e.g. `AddF64`), operands may still be i64 (from phi
    /// nodes, which are always declared as `I64`/JSValue in Cranelift). This
    /// helper unboxes them: i64 values are NaN-unboxed via `bitcast`, i32
    /// values are converted via `fcvt_from_sint`, and f64 values pass through.
    fn coerce_to_f64(val: Value, builder: &mut FunctionBuilder<'_>) -> Value {
        let ty = builder.func.dfg.value_type(val);
        if ty == types::F64 {
            val
        } else if ty == types::I64 {
            // Unbox: bitcast i64 bits to f64 (NaN-boxed f64 is stored as raw bits)
            nanbox_emit::emit_unbox_f64(builder, val)
        } else if ty == types::I32 {
            // Convert i32 to f64
            builder.ins().fcvt_from_sint(types::F64, val)
        } else if ty == types::I8 {
            // Extend bool (i8) to i32, then to f64
            let ext = builder.ins().uextend(types::I32, val);
            builder.ins().fcvt_from_sint(types::F64, ext)
        } else {
            // Unknown type — pass through
            val
        }
    }

    /// Emit a trap for an unimplemented opcode.
    fn emit_trap(&self, inst: &::ir::TypedInstruction, builder: &mut FunctionBuilder<'_>) {
        // Emit a user trap — this will cause a runtime abort if executed.
        // The trap code identifies this as an unimplemented opcode.
        let _ = inst; // suppress unused warning
        builder.ins().trap(TrapCode::unwrap_user(1));
    }

    /// Emit a variadic call with stack-allocated argv array.
    ///
    /// The first operand is the callee; remaining operands are JS arguments.
    /// Arguments are NaN-boxed, stored to a stack slot, and passed as
    /// `(callee, argc, argv_ptr)`.
    fn emit_call_with_argv(
        &mut self,
        inst: &::ir::TypedInstruction,
        rt_name: &str,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.is_empty() {
            return Err(CodegenError::Module(format!(
                "{rt_name}: call instruction has no operands (expected at least callee)"
            )));
        }
        let callee = self.get_value(inst.operands[0])?;
        let callee = self.ensure_nanboxed(callee, inst.operands[0], builder)?;

        let js_args: Vec<ValueId> = inst.operands.iter().skip(1).copied().collect();
        let argc = js_args.len();
        let slot_size = ((argc * 8) as u32).max(8);

        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size,
            0,
        ));

        for (i, &operand_id) in js_args.iter().enumerate() {
            let arg = self.get_value(operand_id)?;
            let boxed = self.ensure_nanboxed(arg, operand_id, builder)?;
            let offset = (i * 8) as i32;
            builder.ins().stack_store(boxed, ss, offset);
        }

        let argv_ptr = builder.ins().stack_addr(types::I64, ss, 0);
        let argc_val = builder.ins().iconst(types::I32, argc as i64);

        let func_id = self.runtime.get_call_variadic(rt_name, self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder.ins().call(func_ref, &[callee, argc_val, argv_ptr]);
        let v = builder.inst_results(result)[0];
        self.values.insert(inst.id.0, v);
        self.emit_exception_check_if_needed(builder)?;
        Ok(())
    }

    /// Emit a method call: `(obj, key, argc, argv_ptr) -> i64`.
    ///
    /// Operands: `[obj, method_key, arg0, arg1, ...]`.
    fn emit_call_method(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.len() < 2 {
            return Err(CodegenError::Module(format!(
                "CallMethod: expected at least 2 operands (obj, key), got {}",
                inst.operands.len()
            )));
        }
        let obj = self.get_value(inst.operands[0])?;
        let obj = self.ensure_nanboxed(obj, inst.operands[0], builder)?;
        let key = self.get_value(inst.operands[1])?;
        let key = self.ensure_nanboxed(key, inst.operands[1], builder)?;

        let js_args: Vec<ValueId> = inst.operands.iter().skip(2).copied().collect();
        let argc = js_args.len();
        let slot_size = ((argc * 8) as u32).max(8);

        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size,
            0,
        ));

        for (i, &operand_id) in js_args.iter().enumerate() {
            let arg = self.get_value(operand_id)?;
            let boxed = self.ensure_nanboxed(arg, operand_id, builder)?;
            let offset = (i * 8) as i32;
            builder.ins().stack_store(boxed, ss, offset);
        }

        let argv_ptr = builder.ins().stack_addr(types::I64, ss, 0);
        let argc_val = builder.ins().iconst(types::I32, argc as i64);

        let func_id = self
            .runtime
            .get_call_method("__esc_rt_call_method", self.module)?;
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        let result = builder
            .ins()
            .call(func_ref, &[obj, key, argc_val, argv_ptr]);
        let v = builder.inst_results(result)[0];
        self.values.insert(inst.id.0, v);
        self.emit_exception_check_if_needed(builder)?;
        Ok(())
    }

    /// Emit a switch as a chain of `icmp`/`brif` comparisons.
    ///
    /// The first operand is the discriminant. Block targets are the case
    /// branches, with the last target being the default block.
    fn emit_switch(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), CodegenError> {
        if inst.operands.is_empty() {
            builder.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        let discriminant = self.get_value(inst.operands[0])?;

        if inst.block_targets.is_empty() {
            builder.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }

        // Last target is the default block
        let default_target = *inst.block_targets.last().ok_or(CodegenError::Module(
            "switch has no block targets".to_string(),
        ))?;
        let default_block = self.block_map.get(default_target)?;
        // Each non-default target corresponds to case value 0, 1, 2, ...
        let case_targets = &inst.block_targets[..inst.block_targets.len() - 1];

        for (i, &case_target) in case_targets.iter().enumerate() {
            let case_val = builder.ins().iconst(types::I64, i as i64);
            let cmp = builder
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, discriminant, case_val);

            let target_block = self.block_map.get(case_target)?;

            // Create a fall-through block for the next comparison
            let next_block = builder.create_block();
            // Phi args are handled via Cranelift Variables
            builder.ins().brif(cmp, target_block, &[], next_block, &[]);
            builder.seal_block(next_block);
            builder.switch_to_block(next_block);
        }

        // Fall through to default
        builder.ins().jump(default_block, &[]);
        Ok(())
    }
}

/// Binary i32 operation variants.
enum BinaryI32Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Band,
    Bor,
    Bxor,
    Shl,
    Sshr,
    Ushr,
}

/// Binary f64 operation variants.
enum BinaryF64Op {
    Add,
    Sub,
    Mul,
    Div,
}
