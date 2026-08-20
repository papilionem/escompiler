//! Generator and async function state machine transform.
//!
//! This crate implements an IR-to-IR pass that rewrites generator (`function*`)
//! and async (`async function`) functions into explicit state machines. The
//! transform runs after desugaring (AST to IR lowering) and before type
//! inference, consuming `Yield`/`Await`/`YieldDelegate` opcodes and producing
//! standard IR that both backends (Cranelift, LLVM) handle unchanged.
//!
//! ## Key Types
//!
//! - [`analysis::SuspensionPoint`] — a yield/await location in the function
//! - [`analysis::LivenessResult`] — live variable analysis across suspension points
//! - [`split::Segment`] — a group of blocks that execute atomically
//! - [`split::SplitResult`] — the result of splitting blocks at yield points
//! - [`TransformError`] — errors that can occur during the transform
//!
//! ## Pipeline Position
//!
//! ```text
//! oxc parse -> AST -> desugar -> SSA IR
//!                                  |
//!                     *** generator_transform pass ***  <- this crate
//!                                  |
//!                            SSA IR (rewritten)
//!                                  |
//!              type inference -> escape analysis -> memory -> backend
//! ```

pub mod analysis;
pub mod codegen;
pub mod split;
pub mod yield_delegate;

#[cfg(test)]
mod tests;

use ir::builder::TypedModule;
use thiserror::Error;

pub use analysis::{LivenessResult, SuspensionPoint};
pub use codegen::{generate_resume_function, rewrite_as_ramp};
pub use split::{Segment, SplitResult};

/// Errors that can occur during the generator/async transform.
#[derive(Debug, Error)]
pub enum TransformError {
    /// A suspension point references a block that does not exist.
    #[error("suspension point {index} references non-existent block {block_id}")]
    InvalidBlock {
        /// The suspension point index.
        index: u32,
        /// The referenced block ID.
        block_id: u32,
    },

    /// A suspension point references an instruction index that is out of range.
    #[error(
        "suspension point {index} references instruction {instr_index} but block has {block_len} instructions"
    )]
    InvalidInstructionIndex {
        /// The suspension point index.
        index: u32,
        /// The referenced instruction index.
        instr_index: usize,
        /// The actual number of instructions in the block.
        block_len: usize,
    },

    /// Block splitting failed because a yield was in an unexpected position.
    #[error("block splitting failed at suspension point {index}: {reason}")]
    SplitFailed {
        /// The suspension point index.
        index: u32,
        /// Description of the failure.
        reason: String,
    },
}

/// Result of analyzing and splitting a single generator/async function.
///
/// Contains the liveness analysis and block split results for one function.
/// The codegen phase (Wave 2) will consume this to generate ramp and resume
/// functions.
pub struct FunctionTransformResult {
    /// Live variable analysis across suspension points.
    pub liveness: LivenessResult,
    /// Block splitting and segment identification results.
    pub split: SplitResult,
}

/// Transform all generator and async functions in a module.
///
/// Iterates over every function in the module. For functions marked as
/// generator or async, runs the full transform pipeline:
///
/// 1. Suspension point discovery
/// 2. Live variable analysis and slot assignment
/// 3. Block splitting at yield points
/// 4. Resume function generation (state machine)
/// 5. Ramp function rewrite (replaces original)
///
/// The original function is rewritten as a ramp function, and a new resume
/// function is added to the module for each transformed function.
///
/// # Errors
///
/// Returns [`TransformError`] if any phase encounters invalid IR.
pub fn transform_module(
    module: &mut TypedModule,
) -> Result<Vec<(usize, FunctionTransformResult)>, TransformError> {
    let mut results = Vec::new();
    let mut new_functions = Vec::new();
    let original_func_count = module.functions.len();

    for (func_idx, func) in module.functions.iter_mut().enumerate() {
        if !func.is_generator && !func.is_async {
            continue;
        }

        // Step 0.5: Desugar yield* into iteration loops with normal Yield
        yield_delegate::desugar_yield_delegate(func)?;

        // Step 1: Discover suspension points (Yield/Await/YieldDelegate)
        let suspension_points = analysis::discover_suspension_points(func);

        // Every generator/async function must be transformed, even with no
        // suspension points:
        // - `function*` with no yields still returns {value, done:true} on .next().
        // - `async function` with no awaits must still return a Promise (the ramp
        //   wraps the generator in `async_wrap`); otherwise the raw return value
        //   escapes and `.then()` has nothing to attach to.
        // - `async function*` with no yields/awaits must still return an
        //   AsyncGenerator object.

        // Step 2: Analyze live variables across suspension points
        let mut liveness = analysis::analyze_liveness(func, &suspension_points);

        // Step 3: Assign state struct slots to live variables
        analysis::assign_slots(&mut liveness);

        // Step 4: Split blocks at yield points and identify segments
        let split = split::split_and_identify(func, &liveness.suspension_points)?;

        // Step 5: Generate resume function (state machine)
        let resume = codegen::generate_resume_function(func, &liveness, &split)?;

        // Step 6: Rewrite original function as ramp
        let resume_func_idx = original_func_count + new_functions.len();
        codegen::rewrite_as_ramp(func, &liveness, resume_func_idx)?;

        new_functions.push(resume);
        results.push((func_idx, FunctionTransformResult { liveness, split }));
    }

    module.functions.extend(new_functions);

    Ok(results)
}
