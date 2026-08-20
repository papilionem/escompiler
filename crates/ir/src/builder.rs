//! IR builder — helpers for constructing SSA IR functions.
//!
//! Contains two builders:
//! - [`IrBuilder`]: Legacy builder using `Type`/`Instruction`/`Function`.
//! - [`TypedIrBuilder`]: New builder using `IrType`/`Op`/`TypedInstruction` with
//!   Braun et al. SSA construction.

use std::collections::{HashMap, HashSet};

use common::{SourceSpan, StructTypeId};

use crate::{
    BasicBlock, BlockId, Function, FunctionId, Instruction, InstructionData, IrType, Op, Type,
    TypedInstruction, ValueId,
};

// ===========================================================================
// Legacy IrBuilder (unchanged)
// ===========================================================================

/// Builder for constructing an IR [`Function`] incrementally.
pub struct IrBuilder {
    name: String,
    params: Vec<Type>,
    return_type: Type,
    blocks: Vec<BasicBlock>,
    current_block: Option<usize>,
    next_value: u32,
    next_block: u32,
    local_count: u32,
}

impl IrBuilder {
    /// Create a new builder for a function with the given signature.
    pub fn new(name: &str, params: Vec<Type>, return_type: Type) -> Self {
        Self {
            name: name.to_string(),
            params,
            return_type,
            blocks: Vec::new(),
            current_block: None,
            next_value: 0,
            next_block: 0,
            local_count: 0,
        }
    }

    /// Allocate a new basic block and return its id.
    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.blocks.push(BasicBlock {
            id,
            instructions: Vec::new(),
            sealed: false,
        });
        id
    }

    /// Set the current insertion point to the given block.
    ///
    /// # Panics
    ///
    /// Panics if `block` was not created by this builder.
    pub fn switch_to_block(&mut self, block: BlockId) {
        let Some(idx) = self.blocks.iter().position(|b| b.id == block) else {
            panic!("BUG: switch_to_block: block not found ({block})");
        };
        self.current_block = Some(idx);
    }

    /// Append an instruction to the current block and return its value id.
    ///
    /// # Panics
    ///
    /// Panics if no block has been set via [`switch_to_block`](Self::switch_to_block).
    pub fn push(&mut self, ty: Type, inst: Instruction) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        let data = InstructionData { id, ty, inst };
        let Some(idx) = self.current_block else {
            panic!("BUG: push: no current block — call switch_to_block first");
        };
        self.blocks[idx].instructions.push(data);
        id
    }

    /// Set the number of local variable slots for this function.
    pub fn set_local_count(&mut self, count: u32) {
        self.local_count = count;
    }

    /// Seal a block, indicating no more predecessors will be added.
    ///
    /// # Panics
    ///
    /// Panics if `block` was not created by this builder.
    pub fn seal_block(&mut self, block: BlockId) {
        let Some(idx) = self.blocks.iter().position(|b| b.id == block) else {
            panic!("BUG: seal_block: block {block} not found");
        };
        self.blocks[idx].sealed = true;
    }

    /// Consume the builder and produce the completed function.
    pub fn finish(self) -> Function {
        Function {
            id: FunctionId(0), // Assigned by Module when added
            name: self.name,
            params: self.params,
            return_type: self.return_type,
            blocks: self.blocks,
            next_value: self.next_value,
            next_block: self.next_block,
            local_count: self.local_count,
        }
    }
}

// ===========================================================================
// New type system structures
// ===========================================================================

/// A basic block in the new typed IR.
///
/// Contains a linear sequence of [`TypedInstruction`]s and tracks
/// predecessor blocks for SSA phi resolution.
#[derive(Clone, Debug)]
pub struct TypedBasicBlock {
    /// This block's unique identifier.
    pub id: BlockId,
    /// Instructions in program order.
    pub instructions: Vec<TypedInstruction>,
    /// Whether all predecessors have been declared (enables phi resolution).
    pub sealed: bool,
    /// Predecessor blocks (used by the Braun SSA algorithm).
    pub predecessors: Vec<BlockId>,
}

/// A function in the new typed IR.
///
/// Produced by [`TypedIrBuilder`] via `begin_function` / `end_function`.
/// Contains named parameters with types, a return type, and a list of
/// basic blocks forming the function body.
#[derive(Clone, Debug)]
pub struct TypedFunction {
    /// Function name.
    pub name: String,
    /// Parameter names and types.
    pub params: Vec<(String, IrType)>,
    /// Return type.
    pub return_type: IrType,
    /// Basic blocks forming the function body (first is the entry block).
    pub blocks: Vec<TypedBasicBlock>,
    /// Next available value ID (for allocation tracking).
    pub next_value: u32,
    /// Next available block ID.
    pub next_block: u32,
    /// Whether this function is a generator (`function*`).
    pub is_generator: bool,
    /// Whether this function is async (`async function`).
    pub is_async: bool,
}

/// A module (top-level compilation unit) in the new typed IR.
///
/// This is the primary input to code generation backends (Cranelift, LLVM).
/// Produced by [`TypedIrBuilder::finish`].
#[derive(Clone, Debug)]
pub struct TypedModule {
    /// All functions defined in this module.
    pub functions: Vec<TypedFunction>,
    /// Named struct types (name, list of (field_name, field_type)).
    pub struct_types: Vec<(String, Vec<(String, IrType)>)>,
    /// Index of the entry function (top-level script code), if any.
    pub entry: Option<usize>,
}

// ===========================================================================
// TypedIrBuilder — new builder with Braun SSA
// ===========================================================================

/// Builder for constructing typed IR using [`Op`]/[`IrType`]/[`TypedInstruction`]
/// with Braun et al. SSA construction.
///
/// # Usage
///
/// ```ignore
/// let mut b = TypedIrBuilder::new();
/// b.begin_function("main", vec![], IrType::Void);
/// let entry = b.create_block();
/// b.switch_to_block(entry);
/// b.seal_block(entry);
/// b.ret(None);
/// b.end_function();
/// b.set_entry(0);
/// let module = b.finish();
/// ```
///
/// The builder manages module-level state (functions, struct types) and
/// function-level state (blocks, SSA variable tracking). Nested function
/// building is supported via [`suspend_function`](Self::suspend_function)
/// / [`resume_function`](Self::resume_function).
pub struct TypedIrBuilder {
    // Module-level state
    functions: Vec<TypedFunction>,
    current_function: Option<usize>,
    struct_types: Vec<(String, Vec<(String, IrType)>)>,
    entry: Option<usize>,

    // Function-level state (active while inside begin_function..end_function)
    pub(crate) blocks: Vec<TypedBasicBlock>,
    current_block: Option<usize>,
    next_value: u32,
    next_block: u32,

    // SSA state (Braun et al.)
    current_def: HashMap<(u32, u32), ValueId>,
    sealed_blocks: HashSet<u32>,
    incomplete_phis: HashMap<u32, Vec<(u32, ValueId)>>,
}

/// Saved state for a suspended function, used to support nested function building.
pub struct SuspendedFunction {
    function_idx: usize,
    blocks: Vec<TypedBasicBlock>,
    current_block: Option<usize>,
    next_value: u32,
    next_block: u32,
    current_def: HashMap<(u32, u32), ValueId>,
    sealed_blocks: HashSet<u32>,
    incomplete_phis: HashMap<u32, Vec<(u32, ValueId)>>,
}

impl Default for TypedIrBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TypedIrBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            current_function: None,
            struct_types: Vec::new(),
            entry: None,
            blocks: Vec::new(),
            current_block: None,
            next_value: 0,
            next_block: 0,
            current_def: HashMap::new(),
            sealed_blocks: HashSet::new(),
            incomplete_phis: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Module-level API
    // -----------------------------------------------------------------------

    /// Begin building a new function. Must call [`end_function`](Self::end_function) when done.
    pub fn begin_function(&mut self, name: &str, params: Vec<(&str, IrType)>, return_type: IrType) {
        assert!(
            self.current_function.is_none(),
            "begin_function: already inside a function"
        );
        // Reset function-level state
        self.blocks.clear();
        self.current_block = None;
        self.next_value = 0;
        self.next_block = 0;
        self.current_def.clear();
        self.sealed_blocks.clear();
        self.incomplete_phis.clear();

        let idx = self.functions.len();
        self.functions.push(TypedFunction {
            name: name.to_string(),
            params: params
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            return_type,
            blocks: Vec::new(),
            next_value: 0,
            next_block: 0,
            is_generator: false,
            is_async: false,
        });
        self.current_function = Some(idx);
    }

    /// Mark the current function as a generator (`function*`).
    ///
    /// # Panics
    ///
    /// Panics if not currently inside a function.
    pub fn set_generator(&mut self, val: bool) {
        let Some(idx) = self.current_function else {
            panic!("BUG: set_generator: not inside a function");
        };
        self.functions[idx].is_generator = val;
    }

    /// Mark the current function as async (`async function`).
    ///
    /// # Panics
    ///
    /// Panics if not currently inside a function.
    pub fn set_async(&mut self, val: bool) {
        let Some(idx) = self.current_function else {
            panic!("BUG: set_async: not inside a function");
        };
        self.functions[idx].is_async = val;
    }

    /// Get a mutable reference to a previously-built function by index.
    ///
    /// This allows setting flags on functions after they have been completed
    /// with [`end_function`](Self::end_function).
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    pub fn function_mut(&mut self, index: usize) -> &mut TypedFunction {
        assert!(
            index < self.functions.len(),
            "BUG: function_mut: index {index} out of range (have {})",
            self.functions.len()
        );
        &mut self.functions[index]
    }

    /// End the current function, saving all blocks into it.
    ///
    /// # Panics
    ///
    /// Panics if not currently inside a function (no matching `begin_function`).
    pub fn end_function(&mut self) {
        let Some(idx) = self.current_function else {
            panic!("BUG: end_function: not inside a function");
        };
        self.functions[idx].blocks = std::mem::take(&mut self.blocks);
        self.functions[idx].next_value = self.next_value;
        self.functions[idx].next_block = self.next_block;
        self.current_function = None;
    }

    /// Suspend the current function, saving its state so a nested function can be built.
    /// Returns a token that must be passed to [`resume_function`](Self::resume_function).
    ///
    /// # Panics
    ///
    /// Panics if not currently inside a function.
    pub fn suspend_function(&mut self) -> SuspendedFunction {
        let Some(idx) = self.current_function else {
            panic!("BUG: suspend_function: not inside a function");
        };
        let saved = SuspendedFunction {
            function_idx: idx,
            blocks: std::mem::take(&mut self.blocks),
            current_block: self.current_block.take(),
            next_value: self.next_value,
            next_block: self.next_block,
            current_def: std::mem::take(&mut self.current_def),
            sealed_blocks: std::mem::take(&mut self.sealed_blocks),
            incomplete_phis: std::mem::take(&mut self.incomplete_phis),
        };
        self.current_function = None;
        self.next_value = 0;
        self.next_block = 0;
        saved
    }

    /// Resume a previously suspended function, restoring its state.
    pub fn resume_function(&mut self, saved: SuspendedFunction) {
        assert!(
            self.current_function.is_none(),
            "resume_function: already inside a function"
        );
        self.current_function = Some(saved.function_idx);
        self.blocks = saved.blocks;
        self.current_block = saved.current_block;
        self.next_value = saved.next_value;
        self.next_block = saved.next_block;
        self.current_def = saved.current_def;
        self.sealed_blocks = saved.sealed_blocks;
        self.incomplete_phis = saved.incomplete_phis;
    }

    /// Register a named struct type and return its id.
    pub fn add_struct_type(&mut self, name: &str, fields: Vec<(&str, IrType)>) -> StructTypeId {
        let id = StructTypeId(self.struct_types.len() as u32);
        self.struct_types.push((
            name.to_string(),
            fields
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
        ));
        id
    }

    /// Set the module entry function by index.
    pub fn set_entry(&mut self, func_index: usize) {
        self.entry = Some(func_index);
    }

    /// Consume the builder and produce a [`TypedModule`].
    pub fn finish(self) -> TypedModule {
        assert!(
            self.current_function.is_none(),
            "finish: still inside a function — call end_function first"
        );
        TypedModule {
            functions: self.functions,
            struct_types: self.struct_types,
            entry: self.entry,
        }
    }

    // -----------------------------------------------------------------------
    // Block management
    // -----------------------------------------------------------------------

    /// Create a new basic block and return its id.
    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.blocks.push(TypedBasicBlock {
            id,
            instructions: Vec::new(),
            sealed: false,
            predecessors: Vec::new(),
        });
        id
    }

    /// Set the insertion point to the given block.
    ///
    /// # Panics
    ///
    /// Panics if `block` was not created by this builder.
    pub fn switch_to_block(&mut self, block: BlockId) {
        let Some(idx) = self.blocks.iter().position(|b| b.id == block) else {
            panic!("BUG: switch_to_block: block not found ({block})");
        };
        self.current_block = Some(idx);
    }

    /// Seal a block, indicating no more predecessors will be added.
    /// Resolves any incomplete phi nodes for this block.
    pub fn seal_block(&mut self, block: BlockId) {
        let block_idx = block.0;
        self.sealed_blocks.insert(block_idx);

        // Resolve incomplete phis for this block
        if let Some(phis) = self.incomplete_phis.remove(&block_idx) {
            for (var, phi_val) in phis {
                let preds: Vec<BlockId> = self.blocks[block_idx as usize].predecessors.clone();
                self.add_phi_operands(var, phi_val, &preds);
            }
        }

        let idx = block_idx as usize;
        self.blocks[idx].sealed = true;
    }

    /// Record `pred` as a predecessor of `block`.
    ///
    /// # Panics
    ///
    /// Panics if `block` was not created by this builder.
    pub fn add_predecessor(&mut self, block: BlockId, pred: BlockId) {
        let Some(idx) = self.blocks.iter().position(|b| b.id == block) else {
            panic!("BUG: add_predecessor: block {block} not found");
        };
        if !self.blocks[idx].predecessors.contains(&pred) {
            self.blocks[idx].predecessors.push(pred);
        }
    }

    // -----------------------------------------------------------------------
    // SSA variable tracking (Braun et al.)
    // -----------------------------------------------------------------------

    /// Define `var` to have `value` in the current block.
    pub fn write_variable(&mut self, var: u32, value: ValueId) {
        let block_idx = self.current_block_idx();
        self.current_def.insert((var, block_idx as u32), value);
    }

    /// Read the current SSA value of `var`, inserting phi nodes as needed.
    pub fn read_variable(&mut self, var: u32, ty: IrType) -> ValueId {
        let block_idx = self.current_block_idx() as u32;
        self.read_variable_in_block(var, block_idx, ty)
    }

    fn read_variable_in_block(&mut self, var: u32, block_idx: u32, ty: IrType) -> ValueId {
        if let Some(&val) = self.current_def.get(&(var, block_idx)) {
            return val;
        }
        if !self.sealed_blocks.contains(&block_idx) {
            // Block not sealed — add incomplete phi in the target block
            let phi_val = self.emit_phi_in_block(block_idx as usize, ty);
            self.incomplete_phis
                .entry(block_idx)
                .or_default()
                .push((var, phi_val));
            self.current_def.insert((var, block_idx), phi_val);
            return phi_val;
        }
        let preds: Vec<BlockId> = self.blocks[block_idx as usize].predecessors.clone();
        if preds.len() == 1 && preds[0].0 < block_idx {
            // Single predecessor with lower index — no phi needed, recurse.
            // The predecessor is guaranteed to be processed before this block
            // by the backend (which iterates in creation order).
            let val = self.read_variable_in_block(var, preds[0].0, ty);
            self.current_def.insert((var, block_idx), val);
            val
        } else if preds.len() == 1 {
            // Single predecessor with higher index — must use phi to avoid
            // forward reference. The value from the predecessor block wouldn't
            // be available yet when the backend processes this block.
            let phi_val = self.emit_phi_in_block(block_idx as usize, ty.clone());
            self.current_def.insert((var, block_idx), phi_val);
            self.add_phi_operands(var, phi_val, &preds);
            phi_val
        } else {
            // Multiple predecessors — need phi in the target block
            let phi_val = self.emit_phi_in_block(block_idx as usize, ty.clone());
            // Insert before recursing to break cycles
            self.current_def.insert((var, block_idx), phi_val);
            self.add_phi_operands(var, phi_val, &preds);
            phi_val
        }
    }

    /// Emit a phi placeholder instruction at the beginning of the specified block.
    ///
    /// This is used by the Braun SSA algorithm to place phi nodes in the correct
    /// block (the one with multiple predecessors), not wherever the cursor happens to be.
    fn emit_phi_in_block(&mut self, block_idx: usize, ty: IrType) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        let inst = TypedInstruction {
            id,
            op: Op::Phi,
            ty,
            operands: Vec::new(),
            block_targets: Vec::new(),
            span: SourceSpan::DUMMY,
        };
        // Insert phi at the beginning of the target block (before other instructions)
        self.blocks[block_idx].instructions.insert(0, inst);
        id
    }

    fn add_phi_operands(&mut self, var: u32, phi_val: ValueId, preds: &[BlockId]) {
        let mut operands = Vec::new();
        let mut block_targets = Vec::new();
        for pred in preds {
            // Find the phi instruction's type so we can recurse
            let ty = self.find_instruction_type(phi_val);
            let val = self.read_variable_in_block(var, pred.0, ty);
            operands.push(val);
            block_targets.push(*pred);
        }
        // Patch the phi instruction
        self.patch_phi(phi_val, operands, block_targets);
    }

    fn find_instruction_type(&self, val: ValueId) -> IrType {
        for block in &self.blocks {
            for inst in &block.instructions {
                if inst.id == val {
                    return inst.ty.clone();
                }
            }
        }
        IrType::JSValue // fallback
    }

    fn patch_phi(&mut self, phi_val: ValueId, operands: Vec<ValueId>, block_targets: Vec<BlockId>) {
        for block in &mut self.blocks {
            for inst in &mut block.instructions {
                if inst.id == phi_val {
                    inst.operands = operands;
                    inst.block_targets = block_targets;
                    return;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal: emit instruction
    // -----------------------------------------------------------------------

    fn emit(
        &mut self,
        op: Op,
        ty: IrType,
        operands: Vec<ValueId>,
        block_targets: Vec<BlockId>,
    ) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        let inst = TypedInstruction {
            id,
            op,
            ty,
            operands,
            block_targets,
            span: SourceSpan::DUMMY,
        };
        let idx = self.current_block_idx();
        self.blocks[idx].instructions.push(inst);
        id
    }

    fn current_block_idx(&self) -> usize {
        let Some(idx) = self.current_block else {
            panic!("BUG: no current block — call switch_to_block first");
        };
        idx
    }

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    /// Emit `ConstI32`.
    pub fn const_i32(&mut self, val: i32) -> ValueId {
        self.emit(Op::ConstI32(val), IrType::I32, vec![], vec![])
    }

    /// Emit `ConstI64`.
    pub fn const_i64(&mut self, val: i64) -> ValueId {
        self.emit(Op::ConstI64(val), IrType::I64, vec![], vec![])
    }

    /// Emit `ConstF64`.
    pub fn const_f64(&mut self, val: f64) -> ValueId {
        self.emit(Op::ConstF64(val), IrType::F64, vec![], vec![])
    }

    /// Emit `ConstBool`.
    pub fn const_bool(&mut self, val: bool) -> ValueId {
        self.emit(Op::ConstBool(val), IrType::Bool, vec![], vec![])
    }

    /// Emit `ConstNull`.
    pub fn const_null(&mut self) -> ValueId {
        self.emit(Op::ConstNull, IrType::JSValue, vec![], vec![])
    }

    /// Emit `ConstUndefined`.
    pub fn const_undefined(&mut self) -> ValueId {
        self.emit(Op::ConstUndefined, IrType::JSValue, vec![], vec![])
    }

    /// Emit `ConstString` with a string table index.
    pub fn const_string(&mut self, idx: u32) -> ValueId {
        self.emit(Op::ConstString(idx), IrType::JSString, vec![], vec![])
    }

    /// Emit `LoadGlobal` to load a built-in global object by name.
    ///
    /// The `idx` is a string table index identifying the global name
    /// (e.g., "Array", "Object", "Math"). At runtime this emits a call
    /// to `__esc_rt_get_global(name_bits)` which returns the global object
    /// as a NaN-boxed JS value.
    pub fn load_global(&mut self, idx: u32) -> ValueId {
        self.emit(Op::LoadGlobal(idx), IrType::JSValue, vec![], vec![])
    }

    /// Emit `LoadParam` to load a function parameter by index.
    pub fn load_param(&mut self, index: u32) -> ValueId {
        self.emit(Op::LoadParam(index), IrType::JSValue, vec![], vec![])
    }

    // -----------------------------------------------------------------------
    // Arithmetic — i32
    // -----------------------------------------------------------------------

    /// Emit `AddI32`.
    pub fn add_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::AddI32, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `SubI32`.
    pub fn sub_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::SubI32, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `MulI32`.
    pub fn mul_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::MulI32, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `DivI32`.
    pub fn div_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::DivI32, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `ModI32`.
    pub fn mod_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ModI32, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `NegI32`.
    pub fn neg_i32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::NegI32, IrType::I32, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Arithmetic — f64
    // -----------------------------------------------------------------------

    /// Emit `AddF64`.
    pub fn add_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::AddF64, IrType::F64, vec![lhs, rhs], vec![])
    }

    /// Emit `SubF64`.
    pub fn sub_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::SubF64, IrType::F64, vec![lhs, rhs], vec![])
    }

    /// Emit `MulF64`.
    pub fn mul_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::MulF64, IrType::F64, vec![lhs, rhs], vec![])
    }

    /// Emit `DivF64`.
    pub fn div_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::DivF64, IrType::F64, vec![lhs, rhs], vec![])
    }

    /// Emit `ModF64`.
    pub fn mod_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ModF64, IrType::F64, vec![lhs, rhs], vec![])
    }

    /// Emit `NegF64`.
    pub fn neg_f64(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::NegF64, IrType::F64, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Arithmetic — JS coercing
    // -----------------------------------------------------------------------

    /// Emit `AddJS`.
    pub fn add_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::AddJS, IrType::JSValue, vec![lhs, rhs], vec![])
    }

    /// Emit `SubJS`.
    pub fn sub_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::SubJS, IrType::JSValue, vec![lhs, rhs], vec![])
    }

    /// Emit `MulJS`.
    pub fn mul_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::MulJS, IrType::JSValue, vec![lhs, rhs], vec![])
    }

    /// Emit `DivJS`.
    pub fn div_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::DivJS, IrType::JSValue, vec![lhs, rhs], vec![])
    }

    /// Emit `ModJS`.
    pub fn mod_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ModJS, IrType::JSValue, vec![lhs, rhs], vec![])
    }

    /// Emit `NegJS`.
    pub fn neg_js(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::NegJS, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `ExpJS`.
    pub fn exp_js(&mut self, base: ValueId, exp: ValueId) -> ValueId {
        self.emit(Op::ExpJS, IrType::JSValue, vec![base, exp], vec![])
    }

    // -----------------------------------------------------------------------
    // Arithmetic — bitwise
    // -----------------------------------------------------------------------

    /// Emit `BitwiseAnd`.
    pub fn bitwise_and(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::BitwiseAnd, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `BitwiseOr`.
    pub fn bitwise_or(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::BitwiseOr, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `BitwiseXor`.
    pub fn bitwise_xor(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::BitwiseXor, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `BitwiseNot`.
    pub fn bitwise_not(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BitwiseNot, IrType::I32, vec![val], vec![])
    }

    /// Emit `ShiftLeft`.
    pub fn shift_left(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ShiftLeft, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `ShiftRight`.
    pub fn shift_right(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ShiftRight, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `ShiftRightUnsigned`.
    pub fn shift_right_unsigned(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::ShiftRightUnsigned, IrType::I32, vec![lhs, rhs], vec![])
    }

    // -----------------------------------------------------------------------
    // Comparison
    // -----------------------------------------------------------------------

    /// Emit `EqI32`.
    pub fn eq_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::EqI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `EqF64`.
    pub fn eq_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::EqF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `EqStrict` (===).
    pub fn eq_strict(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::EqStrict, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `EqAbstract` (==).
    pub fn eq_abstract(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::EqAbstract, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `NeI32`.
    pub fn ne_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::NeI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `NeF64`.
    pub fn ne_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::NeF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `NeStrict` (!==).
    pub fn ne_strict(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::NeStrict, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `NeAbstract` (!=).
    pub fn ne_abstract(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::NeAbstract, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LtI32`.
    pub fn lt_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LtI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LtF64`.
    pub fn lt_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LtF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LtJS`.
    pub fn lt_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LtJS, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LeI32`.
    pub fn le_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LeI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LeF64`.
    pub fn le_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LeF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `LeJS`.
    pub fn le_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::LeJS, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GtI32`.
    pub fn gt_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GtI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GtF64`.
    pub fn gt_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GtF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GtJS`.
    pub fn gt_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GtJS, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GeI32`.
    pub fn ge_i32(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GeI32, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GeF64`.
    pub fn ge_f64(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GeF64, IrType::Bool, vec![lhs, rhs], vec![])
    }

    /// Emit `GeJS` (JS >=).
    pub fn ge_js(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::GeJS, IrType::Bool, vec![lhs, rhs], vec![])
    }

    // -----------------------------------------------------------------------
    // Type conversion
    // -----------------------------------------------------------------------

    /// Emit `ToNumber`.
    pub fn to_number(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToNumber, IrType::F64, vec![val], vec![])
    }

    /// Emit `ToBoolean`.
    pub fn to_boolean(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToBoolean, IrType::Bool, vec![val], vec![])
    }

    /// Emit `ToString`.
    pub fn to_js_string(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToString, IrType::JSString, vec![val], vec![])
    }

    /// Emit `ToObject`.
    pub fn to_object(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToObject, IrType::JSObject, vec![val], vec![])
    }

    /// Emit `ToInt32`.
    pub fn to_int32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToInt32, IrType::I32, vec![val], vec![])
    }

    /// Emit `ToUint32`.
    pub fn to_uint32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::ToUint32, IrType::I32, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // NaN-boxing
    // -----------------------------------------------------------------------

    /// Emit `BoxI32`.
    pub fn box_i32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxI32, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxUnsignedI32` — box an i32 as unsigned (f64 for values >= 2^31).
    pub fn box_unsigned_i32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxUnsignedI32, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxF64`.
    pub fn box_f64(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxF64, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxBool`.
    pub fn box_bool(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxBool, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxNull`.
    pub fn box_null(&mut self) -> ValueId {
        self.emit(Op::BoxNull, IrType::JSValue, vec![], vec![])
    }

    /// Emit `BoxUndefined`.
    pub fn box_undefined(&mut self) -> ValueId {
        self.emit(Op::BoxUndefined, IrType::JSValue, vec![], vec![])
    }

    /// Emit `BoxString`.
    pub fn box_string(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxString, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxObject`.
    pub fn box_object(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxObject, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `BoxSymbol`.
    pub fn box_symbol(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::BoxSymbol, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `UnboxI32`.
    pub fn unbox_i32(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxI32, IrType::I32, vec![val], vec![])
    }

    /// Emit `UnboxF64`.
    pub fn unbox_f64(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxF64, IrType::F64, vec![val], vec![])
    }

    /// Emit `UnboxBool`.
    pub fn unbox_bool(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxBool, IrType::Bool, vec![val], vec![])
    }

    /// Emit `UnboxString`.
    pub fn unbox_string(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxString, IrType::JSString, vec![val], vec![])
    }

    /// Emit `UnboxObject`.
    pub fn unbox_object(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxObject, IrType::JSObject, vec![val], vec![])
    }

    /// Emit `UnboxSymbol`.
    pub fn unbox_symbol(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::UnboxSymbol, IrType::JSSymbol, vec![val], vec![])
    }

    /// Emit `TypeofBoxed` — returns a NaN-boxed string JsValue.
    pub fn typeof_boxed(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::TypeofBoxed, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `IsNullish`.
    pub fn is_nullish(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::IsNullish, IrType::Bool, vec![val], vec![])
    }

    /// Emit `IsFalsy`.
    pub fn is_falsy(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::IsFalsy, IrType::Bool, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------------

    /// Emit unconditional branch.
    pub fn br(&mut self, target: BlockId) {
        self.emit(Op::Br, IrType::Void, vec![], vec![target]);
    }

    /// Emit conditional branch.
    pub fn br_if(&mut self, cond: ValueId, then_block: BlockId, else_block: BlockId) {
        self.emit(
            Op::BrIf,
            IrType::Void,
            vec![cond],
            vec![then_block, else_block],
        );
    }

    /// Emit return.
    pub fn ret(&mut self, val: Option<ValueId>) {
        let operands = val.into_iter().collect();
        self.emit(Op::Ret, IrType::Void, operands, vec![]);
    }

    /// Emit multi-way branch (switch).
    pub fn switch(&mut self, discriminant: ValueId, targets: Vec<BlockId>) {
        self.emit(Op::Switch, IrType::Void, vec![discriminant], targets);
    }

    /// Emit unreachable.
    pub fn unreachable(&mut self) {
        self.emit(Op::Unreachable, IrType::Void, vec![], vec![]);
    }

    // -----------------------------------------------------------------------
    // Memory allocation
    // -----------------------------------------------------------------------

    /// Emit `AllocZone`.
    pub fn alloc_zone(&mut self, ty: IrType) -> ValueId {
        self.emit(Op::AllocZone, ty, vec![], vec![])
    }

    /// Emit `AllocHeap`.
    pub fn alloc_heap(&mut self, ty: IrType) -> ValueId {
        self.emit(Op::AllocHeap, ty, vec![], vec![])
    }

    /// Emit `AllocStack`.
    pub fn alloc_stack(&mut self, ty: IrType) -> ValueId {
        self.emit(Op::AllocStack, ty, vec![], vec![])
    }

    /// Emit `IncRef`.
    pub fn inc_ref(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::IncRef, IrType::Void, vec![val], vec![])
    }

    /// Emit `DecRef`.
    pub fn dec_ref(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::DecRef, IrType::Void, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Field / element access
    // -----------------------------------------------------------------------

    /// Emit `LoadField`. The `field` index is encoded as a `ConstI32` operand.
    pub fn load_field(&mut self, obj: ValueId, field: u32) -> ValueId {
        let field_idx = self.const_i32(field as i32);
        self.emit(Op::LoadField, IrType::JSValue, vec![obj, field_idx], vec![])
    }

    /// Emit `StoreField`. The `field` index is encoded as a `ConstI32` operand.
    pub fn store_field(&mut self, obj: ValueId, field: u32, val: ValueId) {
        let field_idx = self.const_i32(field as i32);
        self.emit(
            Op::StoreField,
            IrType::Void,
            vec![obj, field_idx, val],
            vec![],
        );
    }

    /// Emit `LoadElement`.
    pub fn load_element(&mut self, arr: ValueId, idx: ValueId) -> ValueId {
        self.emit(Op::LoadElement, IrType::JSValue, vec![arr, idx], vec![])
    }

    /// Emit `StoreElement`.
    pub fn store_element(&mut self, arr: ValueId, idx: ValueId, val: ValueId) {
        self.emit(Op::StoreElement, IrType::Void, vec![arr, idx, val], vec![]);
    }

    // -----------------------------------------------------------------------
    // Property access
    // -----------------------------------------------------------------------

    /// Emit `GetProp`.
    pub fn get_prop(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::GetProp, IrType::JSValue, vec![obj, key], vec![])
    }

    /// Emit `SetProp`.
    pub fn set_prop(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        self.emit(Op::SetProp, IrType::Void, vec![obj, key, val], vec![]);
    }

    /// Emit `SetPropStrict` (throws TypeError on frozen/sealed/non-extensible objects).
    pub fn set_prop_strict(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        self.emit(Op::SetPropStrict, IrType::Void, vec![obj, key, val], vec![]);
    }

    /// Emit `HasProp`.
    pub fn has_prop(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::HasProp, IrType::Bool, vec![obj, key], vec![])
    }

    /// Emit `DeleteProp`.
    pub fn delete_prop(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::DeleteProp, IrType::Bool, vec![obj, key], vec![])
    }

    /// Emit `ICGetProp` — inline-cached property get.
    pub fn ic_get_prop(&mut self, obj: ValueId, key: ValueId, ic_id: ValueId) -> ValueId {
        self.emit(
            Op::ICGetProp,
            IrType::JSValue,
            vec![obj, key, ic_id],
            vec![],
        )
    }

    /// Emit `ICSetProp` — inline-cached property set.
    pub fn ic_set_prop(&mut self, obj: ValueId, key: ValueId, val: ValueId, ic_id: ValueId) {
        self.emit(
            Op::ICSetProp,
            IrType::Void,
            vec![obj, key, val, ic_id],
            vec![],
        );
    }

    /// Emit `GetElem`.
    pub fn get_elem(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::GetElem, IrType::JSValue, vec![obj, key], vec![])
    }

    /// Emit `SetElem`.
    pub fn set_elem(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        self.emit(Op::SetElem, IrType::Void, vec![obj, key, val], vec![]);
    }

    /// Emit `GetPrivate`.
    pub fn get_private(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::GetPrivate, IrType::JSValue, vec![obj, key], vec![])
    }

    /// Emit `SetPrivate`.
    pub fn set_private(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        self.emit(Op::SetPrivate, IrType::Void, vec![obj, key, val], vec![]);
    }

    /// Emit `PrivateFieldGet`: get a private field by compile-time ID.
    ///
    /// Operands: `(obj, private_id_const)`. TypeError if brand check fails.
    pub fn private_field_get(&mut self, obj: ValueId, private_id: ValueId) -> ValueId {
        self.emit(
            Op::PrivateFieldGet,
            IrType::JSValue,
            vec![obj, private_id],
            vec![],
        )
    }

    /// Emit `PrivateFieldSet`: set a private field by compile-time ID.
    ///
    /// Operands: `(obj, private_id_const, value)`. TypeError if brand check fails.
    pub fn private_field_set(&mut self, obj: ValueId, private_id: ValueId, val: ValueId) {
        self.emit(
            Op::PrivateFieldSet,
            IrType::Void,
            vec![obj, private_id, val],
            vec![],
        );
    }

    /// Emit `PrivateFieldHas`: check if object has a private field (`#x in obj`).
    ///
    /// Operands: `(obj, private_id_const)`. Returns bool, does not throw.
    pub fn private_field_has(&mut self, obj: ValueId, private_id: ValueId) -> ValueId {
        self.emit(
            Op::PrivateFieldHas,
            IrType::JSValue,
            vec![obj, private_id],
            vec![],
        )
    }

    /// Emit `InstallPrivateField`: install a private field during construction.
    ///
    /// Operands: `(obj, private_id_const, value)`. Bypasses extensibility checks.
    pub fn install_private_field(&mut self, obj: ValueId, private_id: ValueId, val: ValueId) {
        self.emit(
            Op::InstallPrivateField,
            IrType::Void,
            vec![obj, private_id, val],
            vec![],
        );
    }

    // -----------------------------------------------------------------------
    // Calls
    // -----------------------------------------------------------------------

    /// Emit `Call`.
    pub fn call(&mut self, func: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![func];
        operands.extend(args);
        self.emit(Op::Call, IrType::JSValue, operands, vec![])
    }

    /// Emit `CallMethod`.
    pub fn call_method(&mut self, obj: ValueId, method: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![obj, method];
        operands.extend(args);
        self.emit(Op::CallMethod, IrType::JSValue, operands, vec![])
    }

    /// Emit `CallNew`.
    pub fn call_new(&mut self, ctor: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![ctor];
        operands.extend(args);
        self.emit(Op::CallNew, IrType::JSObject, operands, vec![])
    }

    /// Emit `CallEval` — runtime eval call that could not be inlined at compile time.
    pub fn call_eval(&mut self, args: Vec<ValueId>) -> ValueId {
        self.emit(Op::CallEval, IrType::JSValue, args, vec![])
    }

    /// Emit `CallEvalDirect` — direct eval with scope bridging in poisoned functions.
    ///
    /// Operands: `(code, lex_env, var_env, this_value)`.
    pub fn call_eval_direct(
        &mut self,
        code: ValueId,
        lex_env: ValueId,
        var_env: ValueId,
        this_value: ValueId,
    ) -> ValueId {
        self.emit(
            Op::CallEvalDirect,
            IrType::JSValue,
            vec![code, lex_env, var_env, this_value],
            vec![],
        )
    }

    /// Emit `Invoke` — call that may throw (inside try).
    pub fn invoke(&mut self, func: ValueId, args: Vec<ValueId>, catch_block: BlockId) -> ValueId {
        let mut operands = vec![func];
        operands.extend(args);
        self.emit(Op::Invoke, IrType::JSValue, operands, vec![catch_block])
    }

    /// Emit `TailCall`.
    pub fn tail_call(&mut self, func: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![func];
        operands.extend(args);
        self.emit(Op::TailCall, IrType::JSValue, operands, vec![])
    }

    /// Emit `CallRuntime`.
    pub fn call_runtime(&mut self, func: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![func];
        operands.extend(args);
        self.emit(Op::CallRuntime, IrType::JSValue, operands, vec![])
    }

    // -----------------------------------------------------------------------
    // Object / Shape
    // -----------------------------------------------------------------------

    /// Emit `CreateObject`.
    pub fn create_object(&mut self) -> ValueId {
        self.emit(Op::CreateObject, IrType::JSObject, vec![], vec![])
    }

    /// Emit `CreateObjectLiteral` with interleaved key-value pairs.
    ///
    /// Operands: `[key0, val0, key1, val1, ...]`. Keys must be `ConstString`
    /// values. The runtime builds the shape chain lazily and caches it.
    pub fn create_object_literal(&mut self, kvpairs: Vec<ValueId>) -> ValueId {
        self.emit(Op::CreateObjectLiteral, IrType::JSObject, kvpairs, vec![])
    }

    /// Emit `CreateArray`.
    pub fn create_array(&mut self, elements: Vec<ValueId>) -> ValueId {
        self.emit(Op::CreateArray, IrType::JSArray, elements, vec![])
    }

    /// Emit `CreateClosure` with a flags operand.
    ///
    /// Operands: `[func_idx, env, flags]`.
    /// - Bit 0 of flags: `is_arrow` (skip .prototype, lexical this)
    /// - Bit 1 of flags: `is_strict` (sloppy this substitution check)
    /// - Bit 2 of flags: `is_generator` (generator function)
    pub fn create_closure(&mut self, func: ValueId, env: ValueId, flags: ValueId) -> ValueId {
        self.emit(
            Op::CreateClosure,
            IrType::JSFunction,
            vec![func, env, flags],
            vec![],
        )
    }

    /// Emit `CreateArguments`.
    pub fn create_arguments(&mut self) -> ValueId {
        self.emit(Op::CreateArguments, IrType::JSObject, vec![], vec![])
    }

    /// Emit `InstanceOf`.
    pub fn instance_of(&mut self, obj: ValueId, ctor: ValueId) -> ValueId {
        self.emit(Op::InstanceOf, IrType::Bool, vec![obj, ctor], vec![])
    }

    // -----------------------------------------------------------------------
    // Exception handling
    // -----------------------------------------------------------------------

    /// Emit `TryBegin`.
    pub fn try_begin(&mut self, catch_block: BlockId) {
        self.emit(Op::TryBegin, IrType::Void, vec![], vec![catch_block]);
    }

    /// Emit `TryEnd`.
    pub fn try_end(&mut self) {
        self.emit(Op::TryEnd, IrType::Void, vec![], vec![]);
    }

    /// Emit `Throw`.
    pub fn throw_(&mut self, val: ValueId) {
        self.emit(Op::Throw, IrType::Void, vec![val], vec![]);
    }

    /// Emit `Catch` — binds the caught exception.
    pub fn catch_(&mut self) -> ValueId {
        self.emit(Op::Catch, IrType::JSValue, vec![], vec![])
    }

    /// Emit `Rethrow`.
    pub fn rethrow(&mut self, val: ValueId) {
        self.emit(Op::Rethrow, IrType::Void, vec![val], vec![]);
    }

    /// Emit `Rethrow` with an explicit catch target block.
    ///
    /// Used by `emit_finally_completion` when the enclosing catch target is
    /// known at IR construction time. The Cranelift backend uses this target
    /// instead of relying on the sequential `try_catch_stack`.
    pub fn rethrow_to(&mut self, val: ValueId, catch_target: BlockId) {
        self.emit(Op::Rethrow, IrType::Void, vec![val], vec![catch_target]);
    }

    /// Emit `IsException`.
    pub fn is_exception(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::IsException, IrType::Bool, vec![val], vec![])
    }

    /// Emit `GetException`.
    pub fn get_exception(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::GetException, IrType::JSValue, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Closure environment
    // -----------------------------------------------------------------------

    /// Emit `EnvCreate`.
    pub fn env_create(&mut self, size: u32) -> ValueId {
        let sz = self.const_i32(size as i32);
        self.emit(Op::EnvCreate, IrType::Ptr, vec![sz], vec![])
    }

    /// Emit `EnvLoad`.
    pub fn env_load(&mut self, env: ValueId, slot: u32) -> ValueId {
        let slot_idx = self.const_i32(slot as i32);
        self.emit(Op::EnvLoad, IrType::JSValue, vec![env, slot_idx], vec![])
    }

    /// Emit `EnvStore`.
    pub fn env_store(&mut self, env: ValueId, slot: u32, val: ValueId) {
        let slot_idx = self.const_i32(slot as i32);
        self.emit(Op::EnvStore, IrType::Void, vec![env, slot_idx, val], vec![]);
    }

    /// Emit `EnvExtend`.
    pub fn env_extend(&mut self, outer: ValueId, size: u32) -> ValueId {
        let sz = self.const_i32(size as i32);
        self.emit(Op::EnvExtend, IrType::Ptr, vec![outer, sz], vec![])
    }

    /// Emit `EnvLookup` — dynamic name-based lookup through an `EscEnvironment` chain.
    ///
    /// Returns the value found, or `undefined` if the name is not bound.
    pub fn env_lookup(&mut self, env: ValueId, name: ValueId) -> ValueId {
        self.emit(Op::EnvLookup, IrType::JSValue, vec![env, name], vec![])
    }

    /// Emit `EnvLookupStore` — dynamic name-based store through an `EscEnvironment` chain.
    ///
    /// Returns NaN-boxed `true` if the store succeeded, `false` if the name was not found.
    pub fn env_lookup_store(&mut self, env: ValueId, name: ValueId, val: ValueId) -> ValueId {
        self.emit(
            Op::EnvLookupStore,
            IrType::JSValue,
            vec![env, name, val],
            vec![],
        )
    }

    // -----------------------------------------------------------------------
    // JsBox (heap-allocated variable cell)
    // -----------------------------------------------------------------------

    /// Emit `AllocBox` — allocate a JsBox initialized with `init_val`.
    pub fn alloc_box(&mut self, init_val: ValueId) -> ValueId {
        self.emit(Op::AllocBox, IrType::JSValue, vec![init_val], vec![])
    }

    /// Emit `BoxLoad` — load the current value from a JsBox.
    pub fn box_load(&mut self, box_ptr: ValueId) -> ValueId {
        self.emit(Op::BoxLoad, IrType::JSValue, vec![box_ptr], vec![])
    }

    /// Emit `BoxStore` — store a new value into a JsBox.
    pub fn box_store(&mut self, box_ptr: ValueId, new_val: ValueId) {
        self.emit(Op::BoxStore, IrType::Void, vec![box_ptr, new_val], vec![]);
    }

    // -----------------------------------------------------------------------
    // Iterator protocol
    // -----------------------------------------------------------------------

    /// Emit `IterInit` — get iterator via `Symbol.iterator`. Used by `for..of`.
    pub fn iter_init(&mut self, iterable: ValueId) -> ValueId {
        self.emit(Op::IterInit, IrType::IteratorRecord, vec![iterable], vec![])
    }

    /// Emit `ForInInit` — create a `for..in` property enumerator.
    ///
    /// Returns an iterator over the enumerable string-keyed own AND inherited
    /// properties per `EnumerateObjectProperties` (ES2024 §14.7.5.9).
    pub fn for_in_init(&mut self, obj: ValueId) -> ValueId {
        self.emit(Op::ForInInit, IrType::IteratorRecord, vec![obj], vec![])
    }

    /// Emit `IterInitAsync` — get async iterator via `Symbol.asyncIterator`,
    /// falling back to `Symbol.iterator` if not present.
    pub fn iter_init_async(&mut self, iterable: ValueId) -> ValueId {
        self.emit(
            Op::IterInitAsync,
            IrType::IteratorRecord,
            vec![iterable],
            vec![],
        )
    }

    /// Emit `IterNext`.
    pub fn iter_next(&mut self, iter: ValueId) -> ValueId {
        self.emit(Op::IterNext, IrType::JSValue, vec![iter], vec![])
    }

    /// Emit `IterDone`.
    pub fn iter_done(&mut self, iter: ValueId) -> ValueId {
        self.emit(Op::IterDone, IrType::Bool, vec![iter], vec![])
    }

    /// Emit `IterValue`.
    pub fn iter_value(&mut self, result: ValueId) -> ValueId {
        self.emit(Op::IterValue, IrType::JSValue, vec![result], vec![])
    }

    /// Emit `IterClose`.
    pub fn iter_close(&mut self, iter: ValueId) {
        self.emit(Op::IterClose, IrType::Void, vec![iter], vec![]);
    }

    // -----------------------------------------------------------------------
    // String operations
    // -----------------------------------------------------------------------

    /// Emit `StringConcat`.
    pub fn string_concat(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::StringConcat, IrType::JSString, vec![lhs, rhs], vec![])
    }

    /// Emit `StringLength`.
    pub fn string_length(&mut self, s: ValueId) -> ValueId {
        self.emit(Op::StringLength, IrType::I32, vec![s], vec![])
    }

    /// Emit `StringCompare`.
    pub fn string_compare(&mut self, lhs: ValueId, rhs: ValueId) -> ValueId {
        self.emit(Op::StringCompare, IrType::I32, vec![lhs, rhs], vec![])
    }

    /// Emit `StringCharAt`.
    pub fn string_char_at(&mut self, s: ValueId, idx: ValueId) -> ValueId {
        self.emit(Op::StringCharAt, IrType::JSString, vec![s, idx], vec![])
    }

    // -----------------------------------------------------------------------
    // Promise / Async
    // -----------------------------------------------------------------------

    /// Emit `PromiseCreate`.
    pub fn promise_create(&mut self) -> ValueId {
        self.emit(Op::PromiseCreate, IrType::JSObject, vec![], vec![])
    }

    /// Emit `PromiseResolve`.
    pub fn promise_resolve(&mut self, promise: ValueId, val: ValueId) {
        self.emit(Op::PromiseResolve, IrType::Void, vec![promise, val], vec![]);
    }

    /// Emit `PromiseReject`.
    pub fn promise_reject(&mut self, promise: ValueId, val: ValueId) {
        self.emit(Op::PromiseReject, IrType::Void, vec![promise, val], vec![]);
    }

    /// Emit `Await`.
    pub fn await_(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::Await, IrType::JSValue, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Generator
    // -----------------------------------------------------------------------

    /// Emit `GeneratorCreate`.
    pub fn generator_create(&mut self) -> ValueId {
        self.emit(Op::GeneratorCreate, IrType::JSObject, vec![], vec![])
    }

    /// Emit `Yield`.
    pub fn yield_(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::Yield, IrType::JSValue, vec![val], vec![])
    }

    // -----------------------------------------------------------------------
    // Miscellaneous
    // -----------------------------------------------------------------------

    /// Emit `Nop`.
    pub fn nop(&mut self) {
        self.emit(Op::Nop, IrType::Void, vec![], vec![]);
    }

    /// Emit `ThisValue`.
    pub fn this_value(&mut self) -> ValueId {
        self.emit(Op::ThisValue, IrType::JSValue, vec![], vec![])
    }

    /// Emit `NewTarget`.
    pub fn new_target(&mut self) -> ValueId {
        self.emit(Op::NewTarget, IrType::JSValue, vec![], vec![])
    }

    /// Emit `ImportMeta`.
    pub fn import_meta(&mut self) -> ValueId {
        self.emit(Op::ImportMeta, IrType::JSValue, vec![], vec![])
    }

    /// Emit `SuperCall` — call the parent constructor with the given arguments.
    ///
    /// The first operand is the parent constructor (callee); remaining operands
    /// are the arguments passed to `super(...)`.
    pub fn super_call(&mut self, callee: ValueId, args: Vec<ValueId>) -> ValueId {
        let mut operands = vec![callee];
        operands.extend(args);
        self.emit(Op::SuperCall, IrType::JSValue, operands, vec![])
    }

    /// Emit `GetSuper` — read a property from the parent prototype.
    ///
    /// `obj` is the receiver (`this`), `key` is the property name string.
    /// The runtime resolves the property on the parent prototype chain.
    pub fn get_super(&mut self, obj: ValueId, key: ValueId) -> ValueId {
        self.emit(Op::GetSuper, IrType::JSValue, vec![obj, key], vec![])
    }

    /// Emit `SetSuper` — set a property via the parent prototype.
    ///
    /// `obj` is the receiver (`this`), `key` is the property name, `val` is
    /// the value to set.
    pub fn set_super(&mut self, obj: ValueId, key: ValueId, val: ValueId) {
        self.emit(Op::SetSuper, IrType::Void, vec![obj, key, val], vec![]);
    }

    // -----------------------------------------------------------------------
    // Type guards
    // -----------------------------------------------------------------------

    /// Emit `GuardType`.
    pub fn guard_type(&mut self, val: ValueId, expected_tag: ValueId) -> ValueId {
        self.emit(
            Op::GuardType,
            IrType::JSValue,
            vec![val, expected_tag],
            vec![],
        )
    }

    // -----------------------------------------------------------------------
    // TDZ / Drop flags
    // -----------------------------------------------------------------------

    /// Emit `TdzCheck`.
    pub fn tdz_check(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::TdzCheck, IrType::JSValue, vec![val], vec![])
    }

    /// Emit `TdzInit`.
    pub fn tdz_init(&mut self, val: ValueId) {
        self.emit(Op::TdzInit, IrType::Void, vec![val], vec![]);
    }

    // -----------------------------------------------------------------------
    // RC operations
    // -----------------------------------------------------------------------

    /// Emit `RcIncStrong`.
    pub fn rc_inc_strong(&mut self, val: ValueId) {
        self.emit(Op::RcIncStrong, IrType::Void, vec![val], vec![]);
    }

    /// Emit `RcDecStrong`.
    pub fn rc_dec_strong(&mut self, val: ValueId) {
        self.emit(Op::RcDecStrong, IrType::Void, vec![val], vec![]);
    }

    /// Emit `RcIsUnique`.
    pub fn rc_is_unique(&mut self, val: ValueId) -> ValueId {
        self.emit(Op::RcIsUnique, IrType::Bool, vec![val], vec![])
    }
}

// ===========================================================================
// Tests — TypedIrBuilder
// ===========================================================================
