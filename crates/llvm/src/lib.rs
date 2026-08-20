//! LLVM backend: compiles typed IR to native code via LLVM.
//!
//! Translates typed IR ([`ir::builder::TypedModule`]) into native
//! machine code via LLVM (inkwell). The main entry point is [`LlvmBackend`],
//! which compiles an entire module and produces an object file as `Vec<u8>`.
//!
//! # Architecture
//!
//! - [`codegen`] — Top-level compilation entry point
//! - [`debug_info`] — DWARF debug information emission via DIBuilder
//! - [`lowering`] — Per-function instruction lowering (the core loop)
//! - [`nanbox_emit`] — NaN-boxing encode/decode as LLVM IR sequences
//! - [`runtime_calls`] — External `__esc_rt_*` function declarations
//! - [`types`] — IrType to LLVM type mapping
//! - [`error`] — Error types
//!
//! # The `inkwell` feature
//!
//! Everything below is behind `feature = "inkwell"`, which is **off by
//! default**. Without it this crate compiles to an empty library and pulls in
//! no `llvm-sys`, so `cargo build` works on a machine with no LLVM installed.
//!
//! The reason is ESC-127. This is a virtual workspace, so `cargo build` builds
//! every member; a hard dependency on inkwell here made a system LLVM
//! mandatory for the first command anyone runs, on a backend that is not the
//! supported one and that currently miscompiles.
//!
//! Enable it through `driver/llvm` or `cli/llvm` rather than directly. Dev CI
//! passes `--all-features`, so the backend and its tests still build there.

#[cfg(feature = "inkwell")]
pub mod codegen;
#[cfg(feature = "inkwell")]
pub mod debug_info;
#[cfg(feature = "inkwell")]
pub mod error;
#[cfg(feature = "inkwell")]
pub mod lowering;
#[cfg(feature = "inkwell")]
pub mod nanbox_emit;
#[cfg(feature = "inkwell")]
pub mod runtime_calls;
#[cfg(feature = "inkwell")]
pub mod types;

#[cfg(all(test, feature = "inkwell"))]
mod tests;
