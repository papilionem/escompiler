//! Mapping from IR types to Cranelift IR types.

use ::ir::IrType;
use cranelift_codegen::ir::Type as CraneliftType;
use cranelift_codegen::ir::types;
use cranelift_codegen::isa::TargetIsa;

use crate::error::CodegenError;

/// Convert an [`IrType`] to the corresponding Cranelift type.
///
/// JS-level types (JSValue, JSString, JSObject, etc.) are represented as i64
/// because they use NaN-boxing. Pointer types use the target's native pointer
/// width. `IrType::Void` returns `None` (no Cranelift type).
pub fn ir_type_to_cranelift(
    ty: &IrType,
    isa: &dyn TargetIsa,
) -> Result<Option<CraneliftType>, CodegenError> {
    match ty {
        IrType::Void => Ok(None),
        IrType::Bool => Ok(Some(types::I8)),
        IrType::I32 => Ok(Some(types::I32)),
        IrType::I64 => Ok(Some(types::I64)),
        IrType::F64 => Ok(Some(types::F64)),
        // All JS-level types are NaN-boxed into 64-bit values
        IrType::JSValue
        | IrType::JSString
        | IrType::JSObject
        | IrType::JSArray
        | IrType::JSFunction
        | IrType::JSSymbol => Ok(Some(types::I64)),
        // Pointer types use the target's pointer width
        IrType::Ptr | IrType::ZonePtr | IrType::HeapPtr => Ok(Some(isa.pointer_type())),
        // Composite types are passed as pointers
        IrType::Struct(_)
        | IrType::Array(_, _)
        | IrType::CompletionRecord
        | IrType::IteratorRecord => Ok(Some(isa.pointer_type())),
    }
}
