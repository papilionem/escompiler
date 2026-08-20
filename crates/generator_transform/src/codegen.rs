//! Code generation for the generator state machine transform.
//!
//! This module generates the two functions that replace each generator/async
//! function:
//!
//! 1. **Ramp function** — replaces the original function, allocates the state
//!    object, saves parameters, and returns a generator/async-generator/promise.
//! 2. **Resume function** — the state machine, called on every `.next()`,
//!    `.throw()`, or `.return()` invocation.
//!
//! The ramp function's return value depends on the function type:
//! - **`function*`** (sync generator): returns a Generator object.
//! - **`async function`**: returns a Promise (via `async_wrap`).
//! - **`async function*`** (async generator): returns an AsyncGenerator object
//!   (via `create_async_generator`).
//!
//! The codegen phase consumes the analysis results ([`LivenessResult`],
//! [`SplitResult`]) and produces standard IR that both backends handle
//! unchanged.

use std::collections::{HashMap, HashSet};

use common::SourceSpan;
use ir::builder::{TypedBasicBlock, TypedFunction};
use ir::types::TypedInstruction;
use ir::{BlockId, IrType, Op, ValueId};

use crate::TransformError;
use crate::analysis::LivenessResult;
use crate::split::SplitResult;

// ---------------------------------------------------------------------------
// State index constants
// ---------------------------------------------------------------------------

/// State index: generator has completed (done).
const STATE_DONE: i32 = -2;
/// State index: generator is currently executing (re-entrancy guard).
const STATE_EXECUTING: i32 = -3;
/// State index: generator has not started yet.
const STATE_NOT_STARTED: i32 = -1;

// ---------------------------------------------------------------------------
// Property key constants (string table indices)
// ---------------------------------------------------------------------------

/// String table index for "state_index".
const KEY_STATE_INDEX: u32 = 0;
/// String table index for "resume_mode".
const KEY_RESUME_MODE: u32 = 1;
/// String table index for "sent_value".
const KEY_SENT_VALUE: u32 = 2;

// ---------------------------------------------------------------------------
// ID allocator
// ---------------------------------------------------------------------------

/// Helper for allocating sequential `ValueId` and `BlockId` values.
///
/// Tracks the next available value and block IDs, ensuring no collisions
/// with the original function's IDs.
struct IdAllocator {
    next_value: u32,
    next_block: u32,
}

impl IdAllocator {
    /// Create a new allocator starting from the given IDs.
    fn new(next_value: u32, next_block: u32) -> Self {
        Self {
            next_value,
            next_block,
        }
    }

    /// Allocate a fresh `ValueId`.
    fn alloc_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    /// Allocate a fresh `BlockId`.
    fn alloc_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// Instruction builder helpers
// ---------------------------------------------------------------------------

/// Emit a single instruction into a block's instruction list.
fn emit_inst(
    alloc: &mut IdAllocator,
    block: &mut Vec<TypedInstruction>,
    op: Op,
    ty: IrType,
    operands: Vec<ValueId>,
    block_targets: Vec<BlockId>,
) -> ValueId {
    let id = alloc.alloc_value();
    block.push(TypedInstruction {
        id,
        op,
        ty,
        operands,
        block_targets,
        span: SourceSpan::DUMMY,
    });
    id
}

/// Create a block with the given id, instructions, and predecessors.
fn make_block(
    id: BlockId,
    instructions: Vec<TypedInstruction>,
    predecessors: Vec<BlockId>,
) -> TypedBasicBlock {
    TypedBasicBlock {
        id,
        instructions,
        sealed: true,
        predecessors,
    }
}

// ---------------------------------------------------------------------------
// Ramp function generation
// ---------------------------------------------------------------------------

/// Rewrite the original generator function as a ramp function.
///
/// The ramp function:
/// 1. Creates a state object (`CreateObject`)
/// 2. Stores the initial state_index (-1), resume_mode (0), and parameters
/// 3. Calls the runtime to create a generator object
/// 4. Returns the generator object
///
/// The original function's blocks are replaced entirely.
///
/// # Parameters
///
/// - `func`: The original generator function (will be mutated in place)
/// - `liveness`: Liveness analysis results (for parameter count)
/// - `resume_func_idx`: Index of the resume function in the module
///
/// # Errors
///
/// Returns [`TransformError`] if the rewrite encounters invalid state.
pub fn rewrite_as_ramp(
    func: &mut TypedFunction,
    liveness: &LivenessResult,
    resume_func_idx: usize,
) -> Result<(), TransformError> {
    let _ = liveness; // Used for future param-saving extensions

    // Detect whether this function captures a closure environment. The env is
    // passed as an implicit parameter at index `params.len()` (see desugar's
    // `lower_function_inner`). If present, it must be saved into the state
    // object so the resume function can load captured variables.
    let env_param_index = func.params.len() as u32;
    let has_env = func.blocks.iter().any(|b| {
        b.instructions
            .iter()
            .any(|i| matches!(i.op, Op::LoadParam(idx) if idx >= env_param_index))
    });

    let mut alloc = IdAllocator::new(0, 0);
    let entry_id = alloc.alloc_block();
    let mut insts: Vec<TypedInstruction> = Vec::new();

    // state = CreateObject()
    let state = emit_inst(
        &mut alloc,
        &mut insts,
        Op::CreateObject,
        IrType::JSObject,
        vec![],
        vec![],
    );

    // key_state_index = ConstString(KEY_STATE_INDEX)
    let key_si = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstString(KEY_STATE_INDEX),
        IrType::JSString,
        vec![],
        vec![],
    );

    // val_not_started = ConstI32(-1)
    let val_not_started = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstI32(STATE_NOT_STARTED),
        IrType::I32,
        vec![],
        vec![],
    );

    // BoxI32(val_not_started) -> JSValue
    let boxed_not_started = emit_inst(
        &mut alloc,
        &mut insts,
        Op::BoxI32,
        IrType::JSValue,
        vec![val_not_started],
        vec![],
    );

    // SetProp(state, "state_index", -1)
    emit_inst(
        &mut alloc,
        &mut insts,
        Op::SetProp,
        IrType::Void,
        vec![state, key_si, boxed_not_started],
        vec![],
    );

    // key_resume_mode = ConstString(KEY_RESUME_MODE)
    let key_rm = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstString(KEY_RESUME_MODE),
        IrType::JSString,
        vec![],
        vec![],
    );

    // val_zero = ConstI32(0)
    let val_zero = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstI32(0),
        IrType::I32,
        vec![],
        vec![],
    );

    // BoxI32(val_zero) -> JSValue
    let boxed_zero = emit_inst(
        &mut alloc,
        &mut insts,
        Op::BoxI32,
        IrType::JSValue,
        vec![val_zero],
        vec![],
    );

    // SetProp(state, "resume_mode", 0)
    emit_inst(
        &mut alloc,
        &mut insts,
        Op::SetProp,
        IrType::Void,
        vec![state, key_rm, boxed_zero],
        vec![],
    );

    // Save each parameter to state: SetProp(state, "param_N", param_N)
    for (param_idx, _param) in func.params.iter().enumerate() {
        // Load the parameter
        let param_val = emit_inst(
            &mut alloc,
            &mut insts,
            Op::LoadParam(param_idx as u32),
            IrType::JSValue,
            vec![],
            vec![],
        );

        // key = ConstString(param_key_index)
        // We use string table indices starting after the reserved keys (3+)
        let param_key_idx = 3 + param_idx as u32;
        let key_param = emit_inst(
            &mut alloc,
            &mut insts,
            Op::ConstString(param_key_idx),
            IrType::JSString,
            vec![],
            vec![],
        );

        // SetProp(state, "param_N", param_val)
        emit_inst(
            &mut alloc,
            &mut insts,
            Op::SetProp,
            IrType::Void,
            vec![state, key_param, param_val],
            vec![],
        );
    }

    // Save the closure environment to state if the function captures one, so
    // the resume function can reload captured variables. Stored under the key
    // `3 + params.len()` (one past the parameter keys `3..3+params.len()`).
    if has_env {
        let env_val = emit_inst(
            &mut alloc,
            &mut insts,
            Op::LoadParam(env_param_index),
            IrType::JSValue,
            vec![],
            vec![],
        );

        let env_key_idx = 3 + env_param_index;
        let key_env = emit_inst(
            &mut alloc,
            &mut insts,
            Op::ConstString(env_key_idx),
            IrType::JSString,
            vec![],
            vec![],
        );

        emit_inst(
            &mut alloc,
            &mut insts,
            Op::SetProp,
            IrType::Void,
            vec![state, key_env, env_val],
            vec![],
        );
    }

    // resume_func_idx_val = ConstI32(resume_func_idx) then BoxI32 for NaN-boxing
    let resume_idx_raw = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstI32(resume_func_idx as i32),
        IrType::I32,
        vec![],
        vec![],
    );

    let resume_idx_val = emit_inst(
        &mut alloc,
        &mut insts,
        Op::BoxI32,
        IrType::JSValue,
        vec![resume_idx_raw],
        vec![],
    );

    // gen_obj = CallRuntime("create_generator", [state, resume_func_idx_val])
    // The runtime function identifier is passed as a ConstString.
    let runtime_name = emit_inst(
        &mut alloc,
        &mut insts,
        Op::ConstString(u32::MAX), // sentinel for "create_generator" runtime
        IrType::JSString,
        vec![],
        vec![],
    );

    let gen_obj = emit_inst(
        &mut alloc,
        &mut insts,
        Op::CallRuntime,
        IrType::JSValue,
        vec![runtime_name, state, resume_idx_val],
        vec![],
    );

    // For async functions (is_async && !is_generator), wrap the generator
    // in an async wrapper that returns a Promise and drives the state machine
    // via microtask callbacks.
    // For plain generators (is_generator && !is_async), return the generator directly.
    // For async generators (is_async && is_generator), wrap in an AsyncGenerator
    // via __esc_rt_create_async_generator.
    let return_val = if func.is_async && !func.is_generator {
        // async_wrap_name = ConstString(u32::MAX - 7) — sentinel for "async_wrap"
        let async_wrap_name = emit_inst(
            &mut alloc,
            &mut insts,
            Op::ConstString(u32::MAX - 7), // sentinel for "async_wrap" runtime
            IrType::JSString,
            vec![],
            vec![],
        );

        // promise = CallRuntime("async_wrap", [gen_obj])
        emit_inst(
            &mut alloc,
            &mut insts,
            Op::CallRuntime,
            IrType::JSValue,
            vec![async_wrap_name, gen_obj],
            vec![],
        )
    } else if func.is_async && func.is_generator {
        // create_async_generator_name = ConstString(u32::MAX - 8) — sentinel
        let create_ag_name = emit_inst(
            &mut alloc,
            &mut insts,
            Op::ConstString(u32::MAX - 8), // sentinel for "create_async_generator"
            IrType::JSString,
            vec![],
            vec![],
        );

        // async_gen = CallRuntime("create_async_generator", [gen_obj])
        emit_inst(
            &mut alloc,
            &mut insts,
            Op::CallRuntime,
            IrType::JSValue,
            vec![create_ag_name, gen_obj],
            vec![],
        )
    } else {
        gen_obj
    };

    // Return the result (generator for function*, promise for async function)
    emit_inst(
        &mut alloc,
        &mut insts,
        Op::Ret,
        IrType::Void,
        vec![return_val],
        vec![],
    );

    // Replace the function's blocks
    func.blocks = vec![make_block(entry_id, insts, vec![])];
    func.next_value = alloc.next_value;
    func.next_block = alloc.next_block;

    Ok(())
}

// ---------------------------------------------------------------------------
// Resume function generation
// ---------------------------------------------------------------------------

/// Generate the resume function (state machine) for a generator.
///
/// The resume function:
/// 1. Loads state_index from the state object
/// 2. Checks for re-entrancy (state_index == -3 => error)
/// 3. Checks for completion (state_index == -2 => done result)
/// 4. Marks as executing (state_index = -3)
/// 5. Dispatches to the correct segment based on state_index
/// 6. Each segment loads saved variables, executes IR, saves live vars at yield
///
/// # Parameters
///
/// - `original_func`: The original generator function (read-only reference)
/// - `liveness`: Liveness analysis results
/// - `split`: Block split and segment results
///
/// # Returns
///
/// A new [`TypedFunction`] representing the resume function.
///
/// # Errors
///
/// Returns [`TransformError`] if codegen encounters invalid state.
pub fn generate_resume_function(
    original_func: &TypedFunction,
    liveness: &LivenessResult,
    split: &SplitResult,
) -> Result<TypedFunction, TransformError> {
    let resume_name = format!("{}_resume", original_func.name);
    let mut alloc = IdAllocator::new(0, 0);

    // The resume function takes 3 params: (state, sent_value, resume_mode)
    let params = vec![
        ("state".to_string(), IrType::JSObject),
        ("sent_value".to_string(), IrType::JSValue),
        ("resume_mode".to_string(), IrType::JSValue),
    ];

    let mut blocks: Vec<TypedBasicBlock> = Vec::new();

    // -----------------------------------------------------------------------
    // Block 0: Entry — load state_index, check re-entrancy and completion
    // -----------------------------------------------------------------------
    let entry_block_id = alloc.alloc_block();
    let reentrant_block_id = alloc.alloc_block();
    let done_check_block_id = alloc.alloc_block();
    let done_block_id = alloc.alloc_block();
    // Mode check block: inserted between done_check and dispatch to handle
    // RESUME_RETURN and RESUME_THROW on suspended generators.
    let mode_check_block_id = alloc.alloc_block();
    let dispatch_block_id = alloc.alloc_block();

    // Pre-allocate segment entry blocks
    let mut segment_block_ids: Vec<BlockId> = Vec::new();
    for _ in &split.segments {
        segment_block_ids.push(alloc.alloc_block());
    }

    // Completion/done return block
    let completion_block_id = alloc.alloc_block();

    // --- Entry block ---
    let mut entry_insts: Vec<TypedInstruction> = Vec::new();

    // Load state param
    let state_param = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::LoadParam(0),
        IrType::JSValue,
        vec![],
        vec![],
    );

    // Load sent_value param
    let sent_value_param = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::LoadParam(1),
        IrType::JSValue,
        vec![],
        vec![],
    );

    // Load resume_mode param
    let resume_mode_param = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::LoadParam(2),
        IrType::JSValue,
        vec![],
        vec![],
    );

    // key_state_index = ConstString(KEY_STATE_INDEX)
    let key_si = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::ConstString(KEY_STATE_INDEX),
        IrType::JSString,
        vec![],
        vec![],
    );

    // state_idx = GetProp(state, "state_index")
    let state_idx = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::GetProp,
        IrType::JSValue,
        vec![state_param, key_si],
        vec![],
    );

    // Unbox state_idx to I32 for comparison
    let state_idx_i32 = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::UnboxI32,
        IrType::I32,
        vec![state_idx],
        vec![],
    );

    // Check re-entrancy: state_index == -3
    let executing_const = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::ConstI32(STATE_EXECUTING),
        IrType::I32,
        vec![],
        vec![],
    );

    let is_executing = emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::EqI32,
        IrType::Bool,
        vec![state_idx_i32, executing_const],
        vec![],
    );

    // BrIf(is_executing, reentrant_block, done_check_block)
    emit_inst(
        &mut alloc,
        &mut entry_insts,
        Op::BrIf,
        IrType::Void,
        vec![is_executing],
        vec![reentrant_block_id, done_check_block_id],
    );

    blocks.push(make_block(entry_block_id, entry_insts, vec![]));

    // --- Re-entrancy error block ---
    let mut reentrant_insts: Vec<TypedInstruction> = Vec::new();

    // Create a TypeError string and throw
    let err_msg = emit_inst(
        &mut alloc,
        &mut reentrant_insts,
        Op::ConstString(u32::MAX - 1), // sentinel for "Generator is already executing"
        IrType::JSString,
        vec![],
        vec![],
    );

    let boxed_err = emit_inst(
        &mut alloc,
        &mut reentrant_insts,
        Op::BoxString,
        IrType::JSValue,
        vec![err_msg],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut reentrant_insts,
        Op::Throw,
        IrType::Void,
        vec![boxed_err],
        vec![],
    );

    blocks.push(make_block(
        reentrant_block_id,
        reentrant_insts,
        vec![entry_block_id],
    ));

    // --- Done check block ---
    let mut done_check_insts: Vec<TypedInstruction> = Vec::new();

    let done_const = emit_inst(
        &mut alloc,
        &mut done_check_insts,
        Op::ConstI32(STATE_DONE),
        IrType::I32,
        vec![],
        vec![],
    );

    let is_done = emit_inst(
        &mut alloc,
        &mut done_check_insts,
        Op::EqI32,
        IrType::Bool,
        vec![state_idx_i32, done_const],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut done_check_insts,
        Op::BrIf,
        IrType::Void,
        vec![is_done],
        vec![done_block_id, mode_check_block_id],
    );

    blocks.push(make_block(
        done_check_block_id,
        done_check_insts,
        vec![entry_block_id],
    ));

    // --- Done block: handle resume_mode for completed generators ---
    // .next() on done → {undefined, true}
    // .return(val) on done → {val, true}
    // .throw(err) on done → throw err
    let done_return_block_id = alloc.alloc_block();
    let done_throw_block_id = alloc.alloc_block();
    let done_next_block_id = alloc.alloc_block();

    let mut done_insts: Vec<TypedInstruction> = Vec::new();

    // Unbox resume_mode to i32
    let done_mode_i32 = emit_inst(
        &mut alloc,
        &mut done_insts,
        Op::UnboxI32,
        IrType::I32,
        vec![resume_mode_param],
        vec![],
    );

    // Check if RESUME_RETURN (2)
    let return_const = emit_inst(
        &mut alloc,
        &mut done_insts,
        Op::ConstI32(2), // RESUME_RETURN
        IrType::I32,
        vec![],
        vec![],
    );
    let is_return = emit_inst(
        &mut alloc,
        &mut done_insts,
        Op::EqI32,
        IrType::Bool,
        vec![done_mode_i32, return_const],
        vec![],
    );
    emit_inst(
        &mut alloc,
        &mut done_insts,
        Op::BrIf,
        IrType::Void,
        vec![is_return],
        vec![done_return_block_id, done_throw_block_id],
    );
    blocks.push(make_block(
        done_block_id,
        done_insts,
        vec![done_check_block_id],
    ));

    // Done + RESUME_RETURN: return {sent_value, true}
    let mut done_ret_insts: Vec<TypedInstruction> = Vec::new();
    emit_done_return(&mut alloc, &mut done_ret_insts, sent_value_param);
    blocks.push(make_block(
        done_return_block_id,
        done_ret_insts,
        vec![done_block_id],
    ));

    // Done + RESUME_THROW: check if actually throw mode, else next
    let mut done_throw_check: Vec<TypedInstruction> = Vec::new();
    let throw_const = emit_inst(
        &mut alloc,
        &mut done_throw_check,
        Op::ConstI32(1), // RESUME_THROW
        IrType::I32,
        vec![],
        vec![],
    );
    let is_throw = emit_inst(
        &mut alloc,
        &mut done_throw_check,
        Op::EqI32,
        IrType::Bool,
        vec![done_mode_i32, throw_const],
        vec![],
    );
    emit_inst(
        &mut alloc,
        &mut done_throw_check,
        Op::BrIf,
        IrType::Void,
        vec![is_throw],
        vec![done_next_block_id, done_next_block_id], // throw → done_next (re-use for now)
                                                      // TODO: actually throw sent_value for RESUME_THROW on done generators
    );
    blocks.push(make_block(
        done_throw_block_id,
        done_throw_check,
        vec![done_block_id],
    ));

    // Done + RESUME_NEXT (default): return {undefined, true}
    let mut done_next_insts: Vec<TypedInstruction> = Vec::new();
    let undef_val = emit_inst(
        &mut alloc,
        &mut done_next_insts,
        Op::ConstUndefined,
        IrType::JSValue,
        vec![],
        vec![],
    );
    emit_done_return(&mut alloc, &mut done_next_insts, undef_val);
    blocks.push(make_block(
        done_next_block_id,
        done_next_insts,
        vec![done_throw_block_id],
    ));

    // --- Mode check block: handle resume_mode for suspended generators ---
    // .return(val) → set done, return {val, true} immediately
    // .throw(err) → set done, throw err immediately
    // .next(val) → proceed with dispatch
    // mode_check_block_id was pre-allocated above alongside the other control blocks.
    let mode_return_block_id = alloc.alloc_block();
    let mode_throw_block_id = alloc.alloc_block();

    let mut mode_insts: Vec<TypedInstruction> = Vec::new();
    let mc_mode_i32 = emit_inst(
        &mut alloc,
        &mut mode_insts,
        Op::UnboxI32,
        IrType::I32,
        vec![resume_mode_param],
        vec![],
    );
    let mc_return_const = emit_inst(
        &mut alloc,
        &mut mode_insts,
        Op::ConstI32(2), // RESUME_RETURN
        IrType::I32,
        vec![],
        vec![],
    );
    let mc_is_return = emit_inst(
        &mut alloc,
        &mut mode_insts,
        Op::EqI32,
        IrType::Bool,
        vec![mc_mode_i32, mc_return_const],
        vec![],
    );
    emit_inst(
        &mut alloc,
        &mut mode_insts,
        Op::BrIf,
        IrType::Void,
        vec![mc_is_return],
        vec![mode_return_block_id, mode_throw_block_id],
    );
    blocks.push(make_block(
        mode_check_block_id,
        mode_insts,
        vec![done_check_block_id],
    ));

    // Suspended + RESUME_RETURN: set done, return {sent_value, true}
    let mut mode_ret_insts: Vec<TypedInstruction> = Vec::new();
    emit_set_state_index(&mut alloc, &mut mode_ret_insts, state_param, STATE_DONE);
    emit_done_return(&mut alloc, &mut mode_ret_insts, sent_value_param);
    blocks.push(make_block(
        mode_return_block_id,
        mode_ret_insts,
        vec![mode_check_block_id],
    ));

    // Suspended + check THROW vs NEXT
    let mut mode_throw_insts: Vec<TypedInstruction> = Vec::new();
    let mc_throw_const = emit_inst(
        &mut alloc,
        &mut mode_throw_insts,
        Op::ConstI32(1), // RESUME_THROW
        IrType::I32,
        vec![],
        vec![],
    );
    let mc_is_throw = emit_inst(
        &mut alloc,
        &mut mode_throw_insts,
        Op::EqI32,
        IrType::Bool,
        vec![mc_mode_i32, mc_throw_const],
        vec![],
    );
    emit_inst(
        &mut alloc,
        &mut mode_throw_insts,
        Op::BrIf,
        IrType::Void,
        vec![mc_is_throw],
        vec![dispatch_block_id, dispatch_block_id],
        // TODO: for RESUME_THROW, should throw into the generator context
        // For now, just proceed with dispatch (throw handling deferred)
    );
    blocks.push(make_block(
        mode_throw_block_id,
        mode_throw_insts,
        vec![mode_check_block_id],
    ));

    // --- Dispatch block (mark executing + switch) ---
    let mut dispatch_insts: Vec<TypedInstruction> = Vec::new();

    // Mark as executing: SetProp(state, "state_index", -3)
    let key_si2 = emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::ConstString(KEY_STATE_INDEX),
        IrType::JSString,
        vec![],
        vec![],
    );

    let exec_val = emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::ConstI32(STATE_EXECUTING),
        IrType::I32,
        vec![],
        vec![],
    );

    let boxed_exec = emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::BoxI32,
        IrType::JSValue,
        vec![exec_val],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::SetProp,
        IrType::Void,
        vec![state_param, key_si2, boxed_exec],
        vec![],
    );

    // Store sent_value to state
    let key_sv = emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::ConstString(KEY_SENT_VALUE),
        IrType::JSString,
        vec![],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut dispatch_insts,
        Op::SetProp,
        IrType::Void,
        vec![state_param, key_sv, sent_value_param],
        vec![],
    );

    // Switch dispatch: map state_index to segment blocks.
    // state_index -1 => segment 0, 0 => segment 1, 1 => segment 2, ...
    // We use a chain of BrIf comparisons since state_index values are not
    // contiguous from 0 (they start at -1).
    let dispatch_params = DispatchParams {
        dispatch_block_id,
        state_idx_i32,
        fallthrough_block: completion_block_id,
        predecessor: mode_throw_block_id,
    };
    generate_dispatch_chain(
        &mut alloc,
        &mut blocks,
        &dispatch_params,
        dispatch_insts,
        &split.segments,
        &segment_block_ids,
    );

    // --- Generate segment blocks ---
    generate_segment_blocks(
        &mut alloc,
        &mut blocks,
        original_func,
        liveness,
        split,
        &segment_block_ids,
        completion_block_id,
        state_param,
        sent_value_param,
        resume_mode_param,
        dispatch_block_id,
    );

    // --- Completion block (state_index = -2, return {value, done: true}) ---
    let mut comp_insts: Vec<TypedInstruction> = Vec::new();

    // Mark as done
    let key_si3 = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::ConstString(KEY_STATE_INDEX),
        IrType::JSString,
        vec![],
        vec![],
    );

    let done_val = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::ConstI32(STATE_DONE),
        IrType::I32,
        vec![],
        vec![],
    );

    let boxed_done_val = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::BoxI32,
        IrType::JSValue,
        vec![done_val],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::SetProp,
        IrType::Void,
        vec![state_param, key_si3, boxed_done_val],
        vec![],
    );

    // Return {undefined, done: true}
    let undef_comp = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::ConstUndefined,
        IrType::JSValue,
        vec![],
        vec![],
    );

    let done_true_comp = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::ConstBool(true),
        IrType::Bool,
        vec![],
        vec![],
    );

    let rt_iter_comp = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::ConstString(u32::MAX - 2),
        IrType::JSString,
        vec![],
        vec![],
    );

    let boxed_done_comp = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::BoxBool,
        IrType::JSValue,
        vec![done_true_comp],
        vec![],
    );

    let comp_result = emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::CallRuntime,
        IrType::JSValue,
        vec![rt_iter_comp, undef_comp, boxed_done_comp],
        vec![],
    );

    emit_inst(
        &mut alloc,
        &mut comp_insts,
        Op::Ret,
        IrType::Void,
        vec![comp_result],
        vec![],
    );

    // Collect predecessor blocks for completion — any segment can branch here
    let comp_preds: Vec<BlockId> = segment_block_ids.clone();
    blocks.push(make_block(completion_block_id, comp_insts, comp_preds));

    Ok(TypedFunction {
        name: resume_name,
        params,
        return_type: IrType::JSValue,
        blocks,
        next_value: alloc.next_value,
        next_block: alloc.next_block,
        is_generator: false,
        is_async: false,
    })
}

// ---------------------------------------------------------------------------
// Dispatch chain generation
// ---------------------------------------------------------------------------

/// Parameters for dispatch chain generation.
struct DispatchParams {
    /// The block ID for the dispatch block.
    dispatch_block_id: BlockId,
    /// The state_index value to compare against (as I32).
    state_idx_i32: ValueId,
    /// Block to branch to if no segment matches.
    fallthrough_block: BlockId,
    /// Predecessor block for the dispatch block.
    predecessor: BlockId,
}

/// Generate a chain of BrIf comparisons for the dispatch switch.
///
/// Maps state_index values to segment blocks:
/// - state_index == -1 => segment 0 (initial entry)
/// - state_index == 0  => segment 1 (after first yield)
/// - state_index == N  => segment N+1
///
/// If no segment matches, branches to the completion block (error/done state).
fn generate_dispatch_chain(
    alloc: &mut IdAllocator,
    blocks: &mut Vec<TypedBasicBlock>,
    params: &DispatchParams,
    dispatch_insts: Vec<TypedInstruction>,
    segments: &[crate::split::Segment],
    segment_block_ids: &[BlockId],
) {
    if segments.is_empty() {
        // No segments — just branch to fallthrough
        let mut insts = dispatch_insts;
        emit_inst(
            alloc,
            &mut insts,
            Op::Br,
            IrType::Void,
            vec![],
            vec![params.fallthrough_block],
        );
        blocks.push(make_block(
            params.dispatch_block_id,
            insts,
            vec![params.predecessor],
        ));
        return;
    }

    // For each segment, the state_index that maps to it:
    // Segment 0 => state_index == -1 (not started)
    // Segment N (N>0) => state_index == N-1
    let mut state_values: Vec<i32> = Vec::new();
    for seg in segments {
        if seg.index == 0 {
            state_values.push(STATE_NOT_STARTED);
        } else {
            state_values.push(seg.index as i32 - 1);
        }
    }

    // Build if-else chain: for each segment, compare state_idx == expected_value
    // We embed the first comparison in the dispatch block, and create new blocks
    // for subsequent comparisons.

    let mut current_insts = dispatch_insts;
    let mut current_block_id = params.dispatch_block_id;
    let mut current_pred = params.predecessor;

    for (i, (&state_val, &seg_block_id)) in state_values
        .iter()
        .zip(segment_block_ids.iter())
        .enumerate()
    {
        let is_last = i == state_values.len() - 1;

        let cmp_val = emit_inst(
            alloc,
            &mut current_insts,
            Op::ConstI32(state_val),
            IrType::I32,
            vec![],
            vec![],
        );

        let cmp_result = emit_inst(
            alloc,
            &mut current_insts,
            Op::EqI32,
            IrType::Bool,
            vec![params.state_idx_i32, cmp_val],
            vec![],
        );

        if is_last {
            // Last segment: if match => segment, else => fallthrough
            emit_inst(
                alloc,
                &mut current_insts,
                Op::BrIf,
                IrType::Void,
                vec![cmp_result],
                vec![seg_block_id, params.fallthrough_block],
            );
            let finished_insts = std::mem::take(&mut current_insts);
            blocks.push(make_block(
                current_block_id,
                finished_insts,
                vec![current_pred],
            ));
        } else {
            // Not last: if match => segment, else => next comparison block
            let next_cmp_block = alloc.alloc_block();
            emit_inst(
                alloc,
                &mut current_insts,
                Op::BrIf,
                IrType::Void,
                vec![cmp_result],
                vec![seg_block_id, next_cmp_block],
            );
            let finished_insts = std::mem::take(&mut current_insts);
            blocks.push(make_block(
                current_block_id,
                finished_insts,
                vec![current_pred],
            ));

            current_pred = current_block_id;
            current_block_id = next_cmp_block;
        }
    }
}

// ---------------------------------------------------------------------------
// Segment block generation
// ---------------------------------------------------------------------------

/// Compute all blocks reachable from a given entry point through non-yield,
/// non-return control flow. Each segment gets its own copy of reachable blocks,
/// which correctly handles loop back-edges by duplicating loop headers/bodies.
///
/// Returns block IDs sorted with entry first, then by position in
/// `modified_blocks` (preserving the natural block ordering).
fn compute_reachable_blocks(entry: BlockId, modified_blocks: &[TypedBasicBlock]) -> Vec<BlockId> {
    use std::collections::VecDeque;

    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut worklist = VecDeque::new();
    worklist.push_back(entry);

    // BFS from entry. The result is in BFS discovery order, which ensures
    // that for each block, its non-back-edge predecessors on the entry path
    // have already been visited. This is critical for Phi degeneration:
    // when a Phi references a value from a predecessor, that predecessor's
    // values must already be in the value_map.
    while let Some(block_id) = worklist.pop_front() {
        if !visited.insert(block_id) {
            continue;
        }
        result.push(block_id);

        let Some(block) = modified_blocks.iter().find(|b| b.id == block_id) else {
            continue;
        };

        // Yield blocks are included but we don't follow their successors
        // (the yield terminates this segment's execution path).
        if block.instructions.iter().any(|i| is_yield_op(&i.op)) {
            continue;
        }

        // Return blocks are included but we don't follow successors.
        if block.instructions.iter().any(|i| matches!(i.op, Op::Ret)) {
            continue;
        }

        // Follow all successor edges (including loop back-edges).
        for inst in &block.instructions {
            if inst.op.is_terminator() {
                for &target in &inst.block_targets {
                    worklist.push_back(target);
                }
            }
        }
    }

    result
}

/// Generate blocks for each segment of the resume function.
///
/// Each segment is processed **independently** with its own value_map and
/// block_map. Blocks reachable through loop back-edges are duplicated into
/// every segment that needs them, avoiding SSA dominance violations.
///
/// For each segment:
/// 1. Compute reachable blocks via BFS (including loop headers/bodies)
/// 2. Allocate fresh block IDs for every reachable block (duplication)
/// 3. Load params (segment 0) or saved live variables (segment N>0)
/// 4. Copy instructions with per-segment value remapping
/// 5. At Phi nodes: filter operands to in-segment predecessors only
/// 6. At yield points: save live variables, set state_index, return
/// 7. At return points: branch to completion block
#[allow(clippy::too_many_arguments)]
fn generate_segment_blocks(
    alloc: &mut IdAllocator,
    blocks: &mut Vec<TypedBasicBlock>,
    original_func: &TypedFunction,
    liveness: &LivenessResult,
    split: &SplitResult,
    segment_block_ids: &[BlockId],
    completion_block_id: BlockId,
    state_param: ValueId,
    sent_value_param: ValueId,
    _resume_mode_param: ValueId,
    dispatch_block_id: BlockId,
) {
    // Process each segment independently with its own value_map and block_map.
    // This is the key difference from the old approach: blocks shared across
    // segments (loop headers, loop bodies) are duplicated with fresh IDs.
    for (seg_idx, segment) in split.segments.iter().enumerate() {
        let seg_block_id = segment_block_ids[seg_idx];

        // 1. Compute all blocks reachable from this segment's entry.
        let reachable = compute_reachable_blocks(segment.entry_block, &split.modified_blocks);
        let reachable_set: HashSet<BlockId> = reachable.iter().copied().collect();

        if reachable.is_empty() {
            // Empty segment: emit a fallback block branching to completion.
            let mut fallback_insts: Vec<TypedInstruction> = Vec::new();
            emit_inst(
                alloc,
                &mut fallback_insts,
                Op::Br,
                IrType::Void,
                vec![],
                vec![completion_block_id],
            );
            blocks.push(make_block(
                seg_block_id,
                fallback_insts,
                vec![dispatch_block_id],
            ));
            continue;
        }

        // 2. Per-segment value_map and block_map (fresh for each segment).
        let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
        let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();

        // Allocate new block IDs. The first block (entry) uses the
        // pre-allocated segment_block_id from the dispatch chain.
        for (i, &orig_block_id) in reachable.iter().enumerate() {
            if i == 0 {
                block_map.insert(orig_block_id, seg_block_id);
            } else {
                let new_id = alloc.alloc_block();
                block_map.insert(orig_block_id, new_id);
            }
        }

        // 3. Generate preamble instructions.
        let mut preamble_insts: Vec<TypedInstruction> = Vec::new();

        if segment.index == 0 {
            // Segment 0: parameters and the closure environment are loaded
            // from the state object when their `LoadParam` instructions are
            // remapped in the copy loop below (see the `Op::LoadParam` arm).
        } else {
            // Segment N (N>0): load saved live variables from state slots.
            let sp_idx = segment.index - 1;
            if let Some(live_vars) = liveness.live_across.get(&sp_idx) {
                for &var in live_vars {
                    if let Some(&slot) = liveness.slot_assignment.get(&var) {
                        let slot_key_idx = 100 + slot;
                        let key = emit_inst(
                            alloc,
                            &mut preamble_insts,
                            Op::ConstString(slot_key_idx),
                            IrType::JSString,
                            vec![],
                            vec![],
                        );

                        let loaded = emit_inst(
                            alloc,
                            &mut preamble_insts,
                            Op::GetProp,
                            IrType::JSValue,
                            vec![state_param, key],
                            vec![],
                        );

                        value_map.insert(var, loaded);
                    }
                }
            }

            // Map the yield result to sent_value_param.
            if let Some(sp) = liveness
                .suspension_points
                .iter()
                .find(|sp| sp.index == sp_idx)
            {
                let yield_result_id = find_instruction_id_in_blocks(
                    &split.modified_blocks,
                    sp.block_id,
                    sp.instruction_index,
                );
                if let Some(yield_id) = yield_result_id {
                    value_map.insert(yield_id, sent_value_param);
                }
            }
        }

        // 4. Process each block in the segment.
        for (blk_idx, &orig_block_id) in reachable.iter().enumerate() {
            let new_block_id = block_map
                .get(&orig_block_id)
                .copied()
                .unwrap_or(seg_block_id);

            let is_first_block = blk_idx == 0;

            let Some(block) = split.modified_blocks.iter().find(|b| b.id == orig_block_id) else {
                continue;
            };

            let mut blk_insts: Vec<TypedInstruction> = Vec::new();

            // Prepend preamble to the first block.
            if is_first_block {
                blk_insts.append(&mut preamble_insts);
            }

            let mut block_has_yield = false;
            for inst in &block.instructions {
                if is_yield_op(&inst.op) {
                    // At a yield point: save live variables and return.
                    block_has_yield = true;

                    let sp_match = segment
                        .suspension_point
                        .and_then(|sp_idx| {
                            liveness
                                .suspension_points
                                .iter()
                                .find(|sp| sp.index == sp_idx)
                        })
                        .or_else(|| {
                            // Fallback: match by block_id and op.
                            liveness
                                .suspension_points
                                .iter()
                                .find(|sp| sp.block_id == orig_block_id && sp.op == inst.op)
                        });
                    if let Some(sp) = sp_match {
                        emit_save_live_vars(
                            alloc,
                            &mut blk_insts,
                            liveness,
                            sp.index,
                            state_param,
                            &value_map,
                        );
                        emit_set_state_index(alloc, &mut blk_insts, state_param, sp.index as i32);
                        let yield_value = resolve_value(inst.yield_value_operand(), &value_map);
                        emit_yield_return(alloc, &mut blk_insts, yield_value);
                    }
                    break;
                } else if matches!(inst.op, Op::Ret) {
                    // Return in original function → generator completion.
                    // Inline the completion: set state to done, return
                    // {value: returnValue, done: true}.
                    emit_set_state_index(alloc, &mut blk_insts, state_param, STATE_DONE);

                    // Get the return value (first operand of Ret), or undefined.
                    let ret_value = if let Some(&op) = inst.operands.first() {
                        value_map.get(&op).copied().unwrap_or(op)
                    } else {
                        emit_inst(
                            alloc,
                            &mut blk_insts,
                            Op::ConstUndefined,
                            IrType::JSValue,
                            vec![],
                            vec![],
                        )
                    };

                    emit_done_return(alloc, &mut blk_insts, ret_value);
                    break;
                } else if matches!(inst.op, Op::Phi) {
                    // 5. Phi handling: filter operands to in-segment predecessors.
                    // When a block is duplicated across segments, its predecessors
                    // may include blocks from other segments. We keep only the
                    // operands corresponding to predecessors within THIS segment.
                    let mut valid_operands: Vec<ValueId> = Vec::new();
                    for (i, &pred) in block.predecessors.iter().enumerate() {
                        if reachable_set.contains(&pred) && i < inst.operands.len() {
                            let remapped = value_map
                                .get(&inst.operands[i])
                                .copied()
                                .unwrap_or(inst.operands[i]);
                            valid_operands.push(remapped);
                        }
                    }

                    if valid_operands.len() == 1 {
                        // Single in-segment predecessor: Phi degenerates to a
                        // direct value mapping (no instruction emitted).
                        value_map.insert(inst.id, valid_operands[0]);
                    } else if valid_operands.is_empty() {
                        // No in-segment predecessors (shouldn't happen in
                        // well-formed IR). Use undefined as fallback.
                        let undef = emit_inst(
                            alloc,
                            &mut blk_insts,
                            Op::ConstUndefined,
                            IrType::JSValue,
                            vec![],
                            vec![],
                        );
                        value_map.insert(inst.id, undef);
                    } else {
                        // Multiple in-segment predecessors: emit Phi with
                        // filtered operands (e.g., intra-segment loop).
                        let new_id = alloc.alloc_value();
                        blk_insts.push(TypedInstruction {
                            id: new_id,
                            op: Op::Phi,
                            ty: inst.ty.clone(),
                            operands: valid_operands,
                            block_targets: vec![],
                            span: inst.span,
                        });
                        value_map.insert(inst.id, new_id);
                    }
                } else if let Op::LoadParam(idx) = &inst.op {
                    // The resume function receives (state, sent_value, resume_mode),
                    // so the original function's parameters and closure environment
                    // must be reloaded from the state object instead of LoadParam.
                    //   idx <  params.len()  => declared parameter, key 3 + idx
                    //   idx >= params.len()  => closure environment, key 3 + params.len()
                    let key_idx = if *idx < original_func.params.len() as u32 {
                        3 + *idx
                    } else {
                        3 + original_func.params.len() as u32
                    };
                    let key = emit_inst(
                        alloc,
                        &mut blk_insts,
                        Op::ConstString(key_idx),
                        IrType::JSString,
                        vec![],
                        vec![],
                    );
                    let loaded = emit_inst(
                        alloc,
                        &mut blk_insts,
                        Op::GetProp,
                        IrType::JSValue,
                        vec![state_param, key],
                        vec![],
                    );
                    value_map.insert(inst.id, loaded);
                } else {
                    // Regular instruction: copy with remapped operands and
                    // block targets.
                    let new_id = alloc.alloc_value();
                    let remapped_operands: Vec<ValueId> = inst
                        .operands
                        .iter()
                        .map(|&op| value_map.get(&op).copied().unwrap_or(op))
                        .collect();

                    let remapped_targets: Vec<BlockId> = inst
                        .block_targets
                        .iter()
                        .map(|&bt| block_map.get(&bt).copied().unwrap_or(bt))
                        .collect();

                    blk_insts.push(TypedInstruction {
                        id: new_id,
                        op: inst.op.clone(),
                        ty: inst.ty.clone(),
                        operands: remapped_operands,
                        block_targets: remapped_targets,
                        span: inst.span,
                    });

                    value_map.insert(inst.id, new_id);
                }
            }

            // Ensure every block ends with a terminator.
            if !block_has_yield && blk_insts.last().is_none_or(|i| !i.op.is_terminator()) {
                emit_inst(
                    alloc,
                    &mut blk_insts,
                    Op::Br,
                    IrType::Void,
                    vec![],
                    vec![completion_block_id],
                );
            }

            // Compute predecessors: only in-segment predecessors (remapped).
            let preds = if is_first_block {
                vec![dispatch_block_id]
            } else {
                block
                    .predecessors
                    .iter()
                    .filter_map(|&p| block_map.get(&p).copied())
                    .collect()
            };

            blocks.push(make_block(new_block_id, blk_insts, preds));
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Check if an op is a yield/await/yield_delegate.
fn is_yield_op(op: &Op) -> bool {
    matches!(op, Op::Yield | Op::Await | Op::YieldDelegate)
}

/// Find the ValueId of an instruction at a given position in a block.
fn find_instruction_id_in_blocks(
    blocks: &[TypedBasicBlock],
    block_id: BlockId,
    instr_index: usize,
) -> Option<ValueId> {
    let block = blocks.iter().find(|b| b.id == block_id)?;
    block.instructions.get(instr_index).map(|i| i.id)
}

/// Resolve a value through the value map, using the original if no mapping exists.
fn resolve_value(val: Option<ValueId>, value_map: &HashMap<ValueId, ValueId>) -> ValueId {
    match val {
        Some(v) => value_map.get(&v).copied().unwrap_or(v),
        None => ValueId(u32::MAX), // placeholder for undefined
    }
}

/// Emit instructions to save live variables to state slots at a yield point.
fn emit_save_live_vars(
    alloc: &mut IdAllocator,
    insts: &mut Vec<TypedInstruction>,
    liveness: &LivenessResult,
    sp_index: u32,
    state_param: ValueId,
    value_map: &HashMap<ValueId, ValueId>,
) {
    if let Some(live_vars) = liveness.live_across.get(&sp_index) {
        for &var in live_vars {
            if let Some(&slot) = liveness.slot_assignment.get(&var) {
                let slot_key_idx = 100 + slot;
                let key = emit_inst(
                    alloc,
                    insts,
                    Op::ConstString(slot_key_idx),
                    IrType::JSString,
                    vec![],
                    vec![],
                );

                let val = value_map.get(&var).copied().unwrap_or(var);
                emit_inst(
                    alloc,
                    insts,
                    Op::SetProp,
                    IrType::Void,
                    vec![state_param, key, val],
                    vec![],
                );
            }
        }
    }
}

/// Emit instructions to set state_index on the state object.
fn emit_set_state_index(
    alloc: &mut IdAllocator,
    insts: &mut Vec<TypedInstruction>,
    state_param: ValueId,
    index: i32,
) {
    let key = emit_inst(
        alloc,
        insts,
        Op::ConstString(KEY_STATE_INDEX),
        IrType::JSString,
        vec![],
        vec![],
    );

    let val = emit_inst(
        alloc,
        insts,
        Op::ConstI32(index),
        IrType::I32,
        vec![],
        vec![],
    );

    let boxed = emit_inst(alloc, insts, Op::BoxI32, IrType::JSValue, vec![val], vec![]);

    emit_inst(
        alloc,
        insts,
        Op::SetProp,
        IrType::Void,
        vec![state_param, key, boxed],
        vec![],
    );
}

/// Emit instructions to return a yield result {value, done: false}.
fn emit_yield_return(
    alloc: &mut IdAllocator,
    insts: &mut Vec<TypedInstruction>,
    yield_value: ValueId,
) {
    let done_false = emit_inst(
        alloc,
        insts,
        Op::ConstBool(false),
        IrType::Bool,
        vec![],
        vec![],
    );

    let rt_name = emit_inst(
        alloc,
        insts,
        Op::ConstString(u32::MAX - 2), // sentinel for "create_iter_result"
        IrType::JSString,
        vec![],
        vec![],
    );

    let boxed_done = emit_inst(
        alloc,
        insts,
        Op::BoxBool,
        IrType::JSValue,
        vec![done_false],
        vec![],
    );

    let result = emit_inst(
        alloc,
        insts,
        Op::CallRuntime,
        IrType::JSValue,
        vec![rt_name, yield_value, boxed_done],
        vec![],
    );

    emit_inst(alloc, insts, Op::Ret, IrType::Void, vec![result], vec![]);
}

/// Emit instructions to return a done result {value, done: true}.
///
/// Used when a generator completes (via return statement or falling off the end).
fn emit_done_return(
    alloc: &mut IdAllocator,
    insts: &mut Vec<TypedInstruction>,
    return_value: ValueId,
) {
    let done_true = emit_inst(
        alloc,
        insts,
        Op::ConstBool(true),
        IrType::Bool,
        vec![],
        vec![],
    );

    let rt_name = emit_inst(
        alloc,
        insts,
        Op::ConstString(u32::MAX - 2), // sentinel for "create_iter_result"
        IrType::JSString,
        vec![],
        vec![],
    );

    let boxed_done = emit_inst(
        alloc,
        insts,
        Op::BoxBool,
        IrType::JSValue,
        vec![done_true],
        vec![],
    );

    let result = emit_inst(
        alloc,
        insts,
        Op::CallRuntime,
        IrType::JSValue,
        vec![rt_name, return_value, boxed_done],
        vec![],
    );

    emit_inst(alloc, insts, Op::Ret, IrType::Void, vec![result], vec![]);
}

/// Trait extension for TypedInstruction to get the yield value operand.
trait YieldValueExt {
    /// Get the first operand (yield value), if any.
    fn yield_value_operand(&self) -> Option<ValueId>;
}

impl YieldValueExt for TypedInstruction {
    fn yield_value_operand(&self) -> Option<ValueId> {
        self.operands.first().copied()
    }
}
