//! AST-to-IR lowering and desugaring.
//!
//! Transforms the oxc AST into SSA-form IR suitable for type inference, escape
//! analysis, and code generation. The main entry points are [`lower_program`]
//! (for ES modules / strict mode) and [`lower_script`] (for scripts / sloppy mode).
//!
//! Key types:
//! - [`lowerer::IrLowerer`] — the stateful lowering visitor that walks the AST
//! - [`LoweringError`] — errors produced during lowering

mod capture;
mod expr;
mod function;
pub mod globals;
pub mod lowerer;
pub mod scope;
pub mod scope_analysis;
mod stmt;
#[cfg(test)]
mod tests;

pub use lowerer::{
    ExportDeclKind, ExportInfo, ExportKind, LoweringError, LoweringResult, Refusal, lower_program,
    lower_script, lower_source, lower_source_with_build_mode,
};
pub use scope_analysis::{ScopeAnalysis, ScopeId, VarId, analyze_scopes};
