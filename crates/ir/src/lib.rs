//! # ir — SSA Intermediate Representation
//!
//! This crate defines the core intermediate representation for the compiler.
//! The IR uses a block-based SSA form with explicit phi nodes (Braun algorithm),
//! typed instructions, and two-world pointer types (`ZonePtr` vs `HeapPtr`)
//! reflecting the dual allocation strategy (zones + per-object ARC).
//!
//! ## Key Types
//!
//! - [`builder::TypedIrBuilder`] — incremental SSA construction with Braun phi insertion
//! - [`builder::TypedModule`] / [`builder::TypedFunction`] — the new typed IR program model
//! - [`Op`] — the 178-opcode instruction set covering JS semantics
//! - [`IrType`] — the 18-variant type lattice (primitives, JS types, pointers, composites)
//! - [`TypedInstruction`] — pairs an [`Op`] with its type, operands, and source span
//! - [`Module`] / [`Function`] — legacy IR model (being replaced by Typed* variants)
//!
//! ## Modules
//!
//! - [`builder`] — IR construction (both legacy [`builder::IrBuilder`] and new [`builder::TypedIrBuilder`])
//! - [`printer`] — human-readable IR text output
//! - [`types`] — comprehensive type system and opcode definitions
//! - [`verify`] — 7-pass IR verifier

pub mod builder;
pub mod printer;
pub mod types;
pub mod verify;

pub use types::*;

use std::fmt;

// ---------------------------------------------------------------------------
// ID newtypes
// ---------------------------------------------------------------------------

/// SSA value identifier.
///
/// Each instruction produces a unique `ValueId` that can be referenced
/// as an operand in subsequent instructions within the same function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

impl fmt::Display for ValueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Basic block identifier.
///
/// Blocks are the fundamental unit of control flow in the IR. Each block
/// contains a sequence of instructions and ends with a terminator
/// (branch, return, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

/// Function identifier within a [`Module`].
///
/// Assigned sequentially as functions are added to a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

impl fmt::Display for FunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fn{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Legacy function type signature (parameter types + return type).
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    /// Parameter types.
    pub params: Vec<Type>,
    /// Return type.
    pub ret: Type,
}

/// Legacy IR type enum.
///
/// Superseded by [`IrType`] in the new typed IR. Retained for the legacy
/// [`IrBuilder`](builder::IrBuilder) / [`Function`] / [`Module`] API.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    /// No value.
    Void,
    /// Boolean value.
    Boolean,
    /// 32-bit signed integer.
    Int32,
    /// 64-bit IEEE 754 float.
    Float64,
    /// JS string.
    String,
    /// JS object.
    Object,
    /// Dynamic/unknown type.
    Any,
    /// Pointer to a zone-allocated object.
    ZonePtr(Box<Type>),
    /// Pointer to a heap-allocated (RC) object.
    HeapPtr(Box<Type>),
    /// Function type with signature.
    Function(Box<FunctionType>),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Void => write!(f, "void"),
            Type::Boolean => write!(f, "bool"),
            Type::Int32 => write!(f, "i32"),
            Type::Float64 => write!(f, "f64"),
            Type::String => write!(f, "string"),
            Type::Object => write!(f, "object"),
            Type::Any => write!(f, "any"),
            Type::ZonePtr(inner) => write!(f, "zone_ptr<{inner}>"),
            Type::HeapPtr(inner) => write!(f, "heap_ptr<{inner}>"),
            Type::Function(ft) => {
                write!(f, "fn(")?;
                for (i, p) in ft.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {}", ft.ret)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    /// Load a constant JS value.
    Const(ConstValue),
    /// Arithmetic: lhs + rhs
    Add(ValueId, ValueId),
    /// Arithmetic: lhs - rhs
    Sub(ValueId, ValueId),
    /// Arithmetic: lhs * rhs
    Mul(ValueId, ValueId),
    /// Arithmetic: lhs / rhs
    Div(ValueId, ValueId),
    /// Arithmetic: lhs % rhs
    Mod(ValueId, ValueId),
    /// Unary negation: -operand
    Neg(ValueId),
    /// Abstract equality: lhs == rhs
    Eq(ValueId, ValueId),
    /// Strict equality: lhs === rhs
    StrictEq(ValueId, ValueId),
    /// Less than: lhs < rhs
    Lt(ValueId, ValueId),
    /// Greater than: lhs > rhs
    Gt(ValueId, ValueId),
    /// Logical not: !operand
    Not(ValueId),
    /// Call function with arguments.
    Call(FunctionId, Vec<ValueId>),
    /// Reference to function parameter by index.
    Param(u32),
    /// Return from function (optionally with a value).
    Return(Option<ValueId>),
    /// Unconditional branch to a block.
    Branch(BlockId),
    /// Conditional branch: if cond then true_block else false_block.
    BranchIf(ValueId, BlockId, BlockId),
    /// SSA phi node: merge values from predecessor blocks.
    Phi(Vec<(BlockId, ValueId)>),
    /// Load a local variable by slot index.
    LoadLocal(u32),
    /// Store a value into a local variable slot.
    StoreLocal(u32, ValueId),
    /// No operation.
    Nop,
}

/// Constant values that can appear in the IR.
/// We use our own enum here rather than importing JsValue from nanbox
/// to keep the IR self-contained and serializable.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Undefined,
    Null,
    Boolean(bool),
    Int32(i32),
    Float64(f64),
    String(std::string::String),
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstValue::Undefined => write!(f, "undefined"),
            ConstValue::Null => write!(f, "null"),
            ConstValue::Boolean(b) => write!(f, "{b}"),
            ConstValue::Int32(n) => write!(f, "{n}i"),
            ConstValue::Float64(n) => write!(f, "{n}f"),
            ConstValue::String(s) => write!(f, "\"{s}\""),
        }
    }
}

// ---------------------------------------------------------------------------
// Instruction data (instruction + metadata)
// ---------------------------------------------------------------------------

/// An instruction together with its SSA value ID and type.
///
/// This is the legacy instruction representation. The new typed IR uses
/// [`TypedInstruction`] instead.
#[derive(Clone, Debug)]
pub struct InstructionData {
    /// The SSA value produced by this instruction.
    pub id: ValueId,
    /// The result type of this instruction.
    pub ty: Type,
    /// The instruction opcode and operands.
    pub inst: Instruction,
}

// ---------------------------------------------------------------------------
// Basic blocks
// ---------------------------------------------------------------------------

/// A basic block in the legacy IR.
///
/// Contains a linear sequence of instructions. A block is "sealed" once
/// all its predecessors are known, enabling phi resolution.
#[derive(Clone, Debug)]
pub struct BasicBlock {
    /// This block's unique identifier.
    pub id: BlockId,
    /// Instructions in program order.
    pub instructions: Vec<InstructionData>,
    /// Whether all predecessors have been declared (enables phi resolution).
    pub sealed: bool,
}

// ---------------------------------------------------------------------------
// Function
// ---------------------------------------------------------------------------

/// A function in the legacy IR.
///
/// Contains a signature (name, parameters, return type) and a list of
/// basic blocks forming the function body. Produced by [`builder::IrBuilder`].
#[derive(Clone, Debug)]
pub struct Function {
    /// Function identifier within the module.
    pub id: FunctionId,
    /// Function name.
    pub name: String,
    /// Parameter types.
    pub params: Vec<Type>,
    /// Return type.
    pub return_type: Type,
    /// Basic blocks forming the function body (first is the entry block).
    pub blocks: Vec<BasicBlock>,
    /// Next available value ID (for allocation tracking).
    pub next_value: u32,
    /// Next available block ID.
    pub next_block: u32,
    /// Number of local variable slots.
    pub local_count: u32,
}

impl Function {
    /// Returns the entry block ID (the first block), or `None` if the function
    /// has no blocks.
    pub fn entry_block(&self) -> Option<BlockId> {
        self.blocks.first().map(|b| b.id)
    }
}

// ---------------------------------------------------------------------------
// Module (top-level compilation unit)
// ---------------------------------------------------------------------------

/// A module (top-level compilation unit) in the legacy IR.
///
/// Contains a list of functions and an optional entry function.
/// For the new typed IR, see [`builder::TypedModule`].
#[derive(Clone, Debug)]
pub struct Module {
    /// All functions defined in this module.
    pub functions: Vec<Function>,
    /// The entry function (equivalent to top-level script code).
    pub entry: Option<FunctionId>,
}

impl Module {
    /// Create an empty module with no functions and no entry point.
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            entry: None,
        }
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
