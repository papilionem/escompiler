//! String constant pool and data section emission.
//!
//! Stores string literals referenced by `ConstString` instructions in the
//! object file's data section, and provides a way to reference them as
//! global values in Cranelift IR.

use std::collections::HashMap;

use cranelift_codegen::ir::{GlobalValue, InstBuilder};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, DataId, Linkage, Module};
use cranelift_object::ObjectModule;

use crate::error::CodegenError;

/// Manages string constants emitted into the object file's data section.
///
/// Each unique string index is declared once and cached; subsequent references
/// reuse the same [`DataId`].
pub struct ConstantPool {
    /// Maps string-table index to the declared DataId.
    data_ids: HashMap<u32, DataId>,
}

impl Default for ConstantPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstantPool {
    /// Create an empty constant pool.
    pub fn new() -> Self {
        Self {
            data_ids: HashMap::new(),
        }
    }

    /// Declare a string constant in the object module's data section.
    ///
    /// Returns the [`DataId`] for the string at the given index. The string
    /// bytes (including a NUL terminator) are stored as read-only data.
    pub fn declare_string(
        &mut self,
        idx: u32,
        string_table: &[String],
        module: &mut ObjectModule,
    ) -> Result<DataId, CodegenError> {
        if let Some(&id) = self.data_ids.get(&idx) {
            return Ok(id);
        }

        let s = string_table
            .get(idx as usize)
            .ok_or_else(|| CodegenError::Module(format!("string index {idx} out of range")))?;

        let name = format!("__esc_str_{idx}");
        let data_id = module.declare_data(&name, Linkage::Local, false, false)?;

        let mut desc = DataDescription::new();
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0); // NUL terminator
        desc.define(bytes.into_boxed_slice());

        module
            .define_data(data_id, &desc)
            .map_err(|e| CodegenError::Module(e.to_string()))?;

        self.data_ids.insert(idx, data_id);
        Ok(data_id)
    }

    /// Emit a reference to a string constant as a Cranelift global value,
    /// returning the pointer value in the current function.
    pub fn emit_string_ref(
        &mut self,
        idx: u32,
        string_table: &[String],
        module: &mut ObjectModule,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<cranelift_codegen::ir::Value, CodegenError> {
        let data_id = self.declare_string(idx, string_table, module)?;
        let gv = module.declare_data_in_func(data_id, builder.func);
        let ptr_ty = module.target_config().pointer_type();
        Ok(builder.ins().symbol_value(ptr_ty, gv))
    }

    /// Emit a reference to a sentinel string constant (from the generator
    /// transform) as a Cranelift global value.
    ///
    /// Sentinel strings use indices near `u32::MAX` and are not in the
    /// normal string table. This method declares the string data inline.
    pub fn emit_sentinel_string_ref(
        &mut self,
        idx: u32,
        content: &str,
        module: &mut ObjectModule,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<cranelift_codegen::ir::Value, CodegenError> {
        if let Some(&id) = self.data_ids.get(&idx) {
            let gv = module.declare_data_in_func(id, builder.func);
            let ptr_ty = module.target_config().pointer_type();
            return Ok(builder.ins().symbol_value(ptr_ty, gv));
        }

        let name = format!("__esc_sentinel_str_{idx}");
        let data_id = module.declare_data(&name, Linkage::Local, false, false)?;

        let mut desc = DataDescription::new();
        let mut bytes = content.as_bytes().to_vec();
        bytes.push(0); // NUL terminator
        desc.define(bytes.into_boxed_slice());

        module
            .define_data(data_id, &desc)
            .map_err(|e| CodegenError::Module(e.to_string()))?;

        self.data_ids.insert(idx, data_id);

        let gv = module.declare_data_in_func(data_id, builder.func);
        let ptr_ty = module.target_config().pointer_type();
        Ok(builder.ins().symbol_value(ptr_ty, gv))
    }

    /// Emit a reference to a previously declared data ID as a [`GlobalValue`]
    /// in the given function.
    pub fn declare_in_func(
        data_id: DataId,
        module: &mut ObjectModule,
        builder: &mut FunctionBuilder<'_>,
    ) -> GlobalValue {
        module.declare_data_in_func(data_id, builder.func)
    }
}
