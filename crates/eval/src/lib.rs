//! Self-hosted eval/Function JIT compiler using Cranelift.
//!
//! Provides [`eval_string`] for global eval and [`eval_direct_string`] for
//! direct eval with scope bridging. The main type is [`JitEval`], which manages
//! a Cranelift JIT module and can compile and execute JavaScript source
//! strings at runtime.
//!
//! # Global eval
//!
//! ```ignore
//! let mut jit = JitEval::new()?;
//! let result = jit.eval("2 + 3")?;
//! ```
//!
//! # Direct eval with scope bridging
//!
//! ```ignore
//! let mut jit = JitEval::new()?;
//! let result = jit.eval_direct("x + 1", lex_env, var_env, this_value, false)?;
//! ```

mod error;
mod jit;

#[cfg(test)]
mod tests;

pub use error::EvalError;
pub use jit::{JitEval, register_eval_runtime};

use common::{CompileError, SourceSpan};
use nanbox::JsValue;

/// Evaluate a JavaScript source string at runtime using the Cranelift JIT.
///
/// This is the high-level entry point for indirect `eval()`. It initializes the
/// runtime, creates a fresh JIT context, compiles the source, executes it,
/// shuts down the runtime, and returns the result as a [`JsValue`].
pub fn eval_string(source: &str) -> Result<JsValue, CompileError> {
    // Initialize runtime state (thread-locals, shape tables, etc.)
    runtime::rt_api::__esc_rt_init();

    let mut jit = JitEval::new().map_err(|e| {
        runtime::rt_api::__esc_rt_shutdown();
        CompileError::Runtime {
            message: e.to_string(),
            span: SourceSpan::new(common::FileId(0), 0, 0),
        }
    })?;

    let result_bits = jit.eval(source).map_err(|e| {
        runtime::rt_api::__esc_rt_shutdown();
        CompileError::Runtime {
            message: e.to_string(),
            span: SourceSpan::new(common::FileId(0), 0, 0),
        }
    })?;

    // Shutdown runtime
    runtime::rt_api::__esc_rt_shutdown();

    Ok(JsValue::from_raw_bits(result_bits))
}

/// Evaluate a JavaScript source string in direct eval mode with scope bridging.
///
/// This is the high-level entry point for direct `eval()` calls inside poisoned
/// functions. The caller provides the lexical and variable environments plus the
/// `this` value from the enclosing scope.
///
/// - `lex_env`: NaN-boxed pointer to the caller's lexical `EscEnvironment`.
/// - `var_env`: NaN-boxed pointer to the caller's variable `EscEnvironment`.
/// - `this_value`: NaN-boxed `this` binding from the caller.
/// - `is_strict`: Whether the eval should run in strict mode.
pub fn eval_direct_string(
    source: &str,
    lex_env: u64,
    var_env: u64,
    this_value: u64,
    is_strict: bool,
) -> Result<JsValue, CompileError> {
    let mut jit = JitEval::new().map_err(|e| CompileError::Runtime {
        message: e.to_string(),
        span: SourceSpan::new(common::FileId(0), 0, 0),
    })?;

    let result_bits = jit
        .eval_direct(source, lex_env, var_env, this_value, is_strict)
        .map_err(|e| CompileError::Runtime {
            message: e.to_string(),
            span: SourceSpan::new(common::FileId(0), 0, 0),
        })?;

    Ok(JsValue::from_raw_bits(result_bits))
}
