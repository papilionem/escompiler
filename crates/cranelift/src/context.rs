//! Compilation context: ISA setup, ObjectModule, and settings for Cranelift.

use std::sync::Arc;

use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::error::CodegenError;

/// Holds the Cranelift compilation state: ISA configuration and the object
/// module that accumulates compiled functions and data.
pub struct CompilationContext {
    /// The target ISA (instruction set architecture).
    pub isa: Arc<dyn TargetIsa>,
    /// The object module that collects compiled code.
    pub object_module: ObjectModule,
}

impl CompilationContext {
    /// Create a new compilation context targeting the host machine.
    ///
    /// Sets up Cranelift with `opt_level = speed` and creates an
    /// [`ObjectModule`] backed by the host triple.
    pub fn new() -> Result<Self, CodegenError> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| CodegenError::Isa(e.to_string()))?;
        // Enable position-independent code for shared library compatibility
        flag_builder
            .set("is_pic", "true")
            .map_err(|e| CodegenError::Isa(e.to_string()))?;

        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::Triple::host())
            .map_err(|e| CodegenError::Isa(e.to_string()))?;

        let flags = settings::Flags::new(flag_builder);
        let isa = isa_builder
            .finish(flags)
            .map_err(|e| CodegenError::Isa(e.to_string()))?;

        let obj_builder = ObjectBuilder::new(isa.clone(), "cs_module", default_libcall_names())
            .map_err(|e| CodegenError::Module(e.to_string()))?;

        let object_module = ObjectModule::new(obj_builder);

        Ok(Self { isa, object_module })
    }
}
