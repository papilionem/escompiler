//! Standard library implementations for the ESCompiler JavaScript runtime.
//!
//! Provides built-in JavaScript functions and object prototypes:
//! - Global functions (`isNaN`, `isFinite`, `parseInt`, `parseFloat`)
//! - Built-in objects (`Math`, `JSON`, `Boolean`, `Number`, `String`, etc.)
//! - Console methods (`console.log`, `console.error`, etc.)
//! - Error types (`TypeError`, `RangeError`, etc.)
//! - A [`BuiltinRegistry`](registry::BuiltinRegistry) that maps names to native implementations.

pub mod array;
pub mod boolean;
pub mod console;
pub mod error_types;
pub mod function;
pub mod json;
pub mod map;
pub mod math;
pub mod number;
pub mod object;
pub mod process;
pub mod promise;
pub mod registry;
pub mod set;
pub mod string;
pub mod symbol;

#[cfg(test)]
mod tests;
