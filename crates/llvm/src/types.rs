//! Mapping from IR types to LLVM IR types.

use inkwell::context::Context;
use inkwell::types::BasicTypeEnum;
use ir::IrType;

use crate::error::LlvmCodegenError;

/// Convert an [`IrType`] to the corresponding LLVM basic type.
///
/// JS-level types (JSValue, JSString, JSObject, etc.) are represented as i64
/// because they use NaN-boxing. `IrType::Void` returns `None` (no LLVM type
/// for void return).
pub fn ir_type_to_llvm<'ctx>(
    ty: &IrType,
    ctx: &'ctx Context,
) -> Result<Option<BasicTypeEnum<'ctx>>, LlvmCodegenError> {
    match ty {
        IrType::Void => Ok(None),
        IrType::Bool => Ok(Some(ctx.bool_type().into())),
        IrType::I32 => Ok(Some(ctx.i32_type().into())),
        IrType::I64 => Ok(Some(ctx.i64_type().into())),
        IrType::F64 => Ok(Some(ctx.f64_type().into())),
        // All JS-level types are NaN-boxed into 64-bit values
        IrType::JSValue
        | IrType::JSString
        | IrType::JSObject
        | IrType::JSArray
        | IrType::JSFunction
        | IrType::JSSymbol => Ok(Some(ctx.i64_type().into())),
        // Pointer types use i64 (64-bit target assumed)
        IrType::Ptr | IrType::ZonePtr | IrType::HeapPtr => Ok(Some(ctx.i64_type().into())),
        // Composite types are passed as pointers (i64)
        IrType::Struct(_)
        | IrType::Array(_, _)
        | IrType::CompletionRecord
        | IrType::IteratorRecord => Ok(Some(ctx.i64_type().into())),
    }
}

/// Return the LLVM i64 type used for NaN-boxed JSValue.
pub fn jsvalue_type<'ctx>(ctx: &'ctx Context) -> BasicTypeEnum<'ctx> {
    ctx.i64_type().into()
}
