//! Type inference engine for the compiler.
//!
//! Performs forward dataflow analysis over SSA IR to produce a side-table
//! mapping `ValueId` to `InferredType` and `TrustCategory`. The specialization
//! pass then rewrites generic JS opcodes to typed variants based on these
//! annotations.
//!
//! # Modules
//!
//! - [`lattice`] — Type lattice (join, meet, subtype).
//! - [`inference`] — Forward dataflow engine.
//! - [`narrowing`] — Type narrowing (typeof, truthiness, nullish).
//! - [`specialize`] — Opcode specialization (AddJS → AddF64 etc.).
//! - [`constfold`] — Constant folding (evaluate constant ops at compile time).
//! - [`trust`] — Trust categories for type provenance.

pub mod constfold;
pub mod inference;
pub mod lattice;
pub mod narrowing;
pub mod specialize;
pub mod trust;

pub use constfold::{ConstFoldStats, constfold_module};
pub use inference::{TypeAnnotations, infer_function, infer_module};
pub use lattice::InferredType;
pub use specialize::{SpecializationStats, specialize_module};
pub use trust::TrustCategory;

#[cfg(test)]
mod tests;
