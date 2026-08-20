//! Calling convention: converts IR function signatures to Cranelift
//! [`Signature`]s with platform-appropriate calling conventions.

use ::ir::IrType;
use cranelift_codegen::ir::{AbiParam, Signature};
use cranelift_codegen::isa::{CallConv, TargetIsa};

use crate::error::CodegenError;
use crate::types::ir_type_to_cranelift;

/// Build a Cranelift [`Signature`] from IR parameter and return types.
///
/// Uses `SystemV` on non-Windows platforms and `WindowsFastcall` on Windows.
/// Each parameter and the return type are mapped via [`ir_type_to_cranelift`].
pub fn build_signature(
    params: &[(String, IrType)],
    return_type: &IrType,
    isa: &dyn TargetIsa,
) -> Result<Signature, CodegenError> {
    let call_conv = if cfg!(target_os = "windows") {
        CallConv::WindowsFastcall
    } else {
        CallConv::SystemV
    };

    let mut sig = Signature::new(call_conv);

    for (_name, ty) in params {
        if let Some(cl_ty) = ir_type_to_cranelift(ty, isa)? {
            sig.params.push(AbiParam::new(cl_ty));
        }
    }

    if let Some(ret_ty) = ir_type_to_cranelift(return_type, isa)? {
        sig.returns.push(AbiParam::new(ret_ty));
    }

    Ok(sig)
}
