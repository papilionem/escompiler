//! Cranelift code generation backend for the compiler.
//!
//! Translates typed IR ([`::ir::builder::TypedModule`]) into native
//! machine code via Cranelift. The main entry point is [`CraneliftBackend`],
//! which compiles an entire module and produces an object file (ELF/Mach-O)
//! as a `Vec<u8>`.
//!
//! # Architecture
//!
//! - [`context`] — ISA setup, ObjectModule creation
//! - [`lowering`] — Per-function instruction lowering (the core loop)
//! - [`abi`] — Function signature construction
//! - [`nanbox_emit`] — NaN-boxing encode/decode sequences
//! - [`constants`] — String constant pool
//! - [`control_flow`] — Block mapping and phi→block-param translation
//! - [`runtime_calls`] — External `__esc_rt_*` function declarations
//! - [`types`] — IrType → Cranelift type mapping
//! - [`error`] — Error types

pub mod abi;
pub mod constants;
pub mod context;
pub mod control_flow;
pub mod error;
pub mod lowering;
pub mod nanbox_emit;
pub mod runtime_calls;
pub mod types;

#[cfg(test)]
mod tests;

use ::ir::builder::TypedModule;
use cranelift_codegen::Context;
use cranelift_codegen::ir;
use cranelift_codegen::ir::types as cl_types;
use cranelift_codegen::ir::{AbiParam, Function as CraneliftFunction, InstBuilder, Signature};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};

use crate::abi::build_signature;
use crate::constants::ConstantPool;
use crate::context::CompilationContext;
use crate::control_flow::BlockMap;
use crate::error::CodegenError;
use crate::lowering::FunctionLowerer;
use crate::runtime_calls::RuntimeCalls;

/// The Cranelift code generation backend.
///
/// Compiles a [`TypedModule`] into a native object file. Create one instance
/// per compilation unit and call [`compile_module`](Self::compile_module) to
/// produce the object bytes.
pub struct CraneliftBackend {
    ctx: CompilationContext,
}

impl CraneliftBackend {
    /// Create a new backend targeting the host machine.
    pub fn new() -> Result<Self, CodegenError> {
        Ok(Self {
            ctx: CompilationContext::new()?,
        })
    }

    /// Compile an IR module into object file bytes (ELF on Linux, Mach-O on macOS).
    ///
    /// The `string_table` provides the string literals referenced by `ConstString`
    /// instructions. The entry function (if any) is exported as `__esc_main`.
    pub fn compile_module(
        mut self,
        module: &TypedModule,
        string_table: &[String],
    ) -> Result<Vec<u8>, CodegenError> {
        let mut runtime = RuntimeCalls::new();
        let mut constant_pool = ConstantPool::new();

        // Phase 1: Declare all functions in the module
        let mut func_ids: Vec<FuncId> = Vec::with_capacity(module.functions.len());
        for (i, func) in module.functions.iter().enumerate() {
            let sig = build_signature(&func.params, &func.return_type, &*self.ctx.isa)?;
            let linkage = if module.entry == Some(i) {
                Linkage::Export
            } else {
                Linkage::Local
            };
            let name = if module.entry == Some(i) {
                "__esc_main".to_string()
            } else {
                format!("{}_{}", func.name, i)
            };
            let id = self
                .ctx
                .object_module
                .declare_function(&name, linkage, &sig)?;
            func_ids.push(id);
        }

        // Compute max param count so indirect call sites can size argv slots.
        let max_func_params = module
            .functions
            .iter()
            .map(|f| f.params.len())
            .max()
            .unwrap_or(0);

        // Phase 2: Lower and define each function
        let mut fb_ctx = FunctionBuilderContext::new();
        for (i, func) in module.functions.iter().enumerate() {
            let sig = build_signature(&func.params, &func.return_type, &*self.ctx.isa)?;

            let mut cl_func = CraneliftFunction::with_name_signature(
                cranelift_codegen::ir::UserFuncName::user(0, i as u32),
                sig,
            );

            {
                let mut builder = FunctionBuilder::new(&mut cl_func, &mut fb_ctx);

                let block_map = BlockMap::build(func, &mut builder, &*self.ctx.isa)?;

                let mut lowerer = FunctionLowerer::new(
                    block_map,
                    &mut runtime,
                    &mut constant_pool,
                    &mut self.ctx.object_module,
                    &func_ids,
                    string_table,
                );
                lowerer.max_func_params = max_func_params;

                lowerer.lower(func, builder)?;
            }

            // Verify the Cranelift IR before compilation for better error messages
            if let Err(errors) = cranelift_codegen::verify_function(&cl_func, self.ctx.isa.as_ref())
            {
                return Err(CodegenError::Module(format!(
                    "verifier error in function '{}' (index {}):\n{}\nCranelift IR:\n{}",
                    func.name,
                    i,
                    errors,
                    cl_func.display()
                )));
            }

            // Compile the function
            let mut codegen_ctx = Context::for_function(cl_func);
            self.ctx
                .object_module
                .define_function(func_ids[i], &mut codegen_ctx)
                .map_err(|e| {
                    CodegenError::Module(format!(
                        "error in function '{}' (index {}): {e}",
                        func.name, i
                    ))
                })?;
        }

        // Phase 2.4: Emit dispatch trampoline for indirect calls
        self.emit_dispatch_trampoline(&mut runtime, &mut fb_ctx, &func_ids, module)?;

        // Phase 2.5: Emit main() wrapper if there's an entry function
        if let Some(entry_idx) = module.entry {
            self.emit_main_wrapper(&mut runtime, &mut fb_ctx, func_ids[entry_idx])?;
        }

        // Phase 3: Emit the object file
        let product = self.ctx.object_module.finish();
        product
            .emit()
            .map_err(|e| CodegenError::Module(e.to_string()))
    }

    /// Emit a C-style `main() -> i32` wrapper that calls runtime init/shutdown
    /// around the JS entry point.
    ///
    /// The generated function:
    /// 1. Calls `__esc_rt_init()` — initializes the runtime
    /// 2. Calls `__esc_main()` — runs the JS entry function (return value ignored)
    /// 3. Checks for unhandled exceptions
    /// 4. Calls `__esc_rt_shutdown()` — tears down the runtime
    /// 5. Returns 0 (success) or 1 (unhandled exception)
    fn emit_main_wrapper(
        &mut self,
        runtime: &mut RuntimeCalls,
        fb_ctx: &mut FunctionBuilderContext,
        entry_func_id: FuncId,
    ) -> Result<(), CodegenError> {
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };

        // Declare external runtime lifecycle functions
        let init_id = runtime.get_void_void("__esc_rt_init", &mut self.ctx.object_module)?;
        let microtask_drain_id =
            runtime.get_void_void("__esc_rt_microtask_drain", &mut self.ctx.object_module)?;
        let shutdown_id =
            runtime.get_void_void("__esc_rt_shutdown", &mut self.ctx.object_module)?;
        let has_exc_id = runtime.get_void_i32(
            "__esc_rt_has_pending_exception",
            &mut self.ctx.object_module,
        )?;

        // Declare main() with signature () -> i32, exported
        let mut main_sig = Signature::new(call_conv);
        main_sig.returns.push(AbiParam::new(cl_types::I32));
        let main_id =
            self.ctx
                .object_module
                .declare_function("main", Linkage::Export, &main_sig)?;

        // Build the main function body
        let mut cl_func = CraneliftFunction::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(1, 0),
            main_sig,
        );

        {
            let mut builder = FunctionBuilder::new(&mut cl_func, fb_ctx);
            let entry_block = builder.create_block();
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            // call __esc_rt_init()
            let init_ref = self
                .ctx
                .object_module
                .declare_func_in_func(init_id, builder.func);
            builder.ins().call(init_ref, &[]);

            // call __esc_main() — use the already-declared entry function ID
            let main_fn_ref = self
                .ctx
                .object_module
                .declare_func_in_func(entry_func_id, builder.func);
            builder.ins().call(main_fn_ref, &[]);

            // call __esc_rt_microtask_drain() — flush promise reactions
            let drain_ref = self
                .ctx
                .object_module
                .declare_func_in_func(microtask_drain_id, builder.func);
            builder.ins().call(drain_ref, &[]);

            // Check for unhandled exceptions: if pending, return 1
            let has_exc_ref = self
                .ctx
                .object_module
                .declare_func_in_func(has_exc_id, builder.func);
            let exc_call = builder.ins().call(has_exc_ref, &[]);
            let has_exc = builder.inst_results(exc_call)[0]; // i32

            let ok_block = builder.create_block();
            let err_block = builder.create_block();

            builder.ins().brif(has_exc, err_block, &[], ok_block, &[]);

            // Error path: shutdown and return 1
            builder.switch_to_block(err_block);
            builder.seal_block(err_block);
            let shutdown_ref_err = self
                .ctx
                .object_module
                .declare_func_in_func(shutdown_id, builder.func);
            builder.ins().call(shutdown_ref_err, &[]);
            let one = builder.ins().iconst(cl_types::I32, 1);
            builder.ins().return_(&[one]);

            // Success path: shutdown and return 0
            builder.switch_to_block(ok_block);
            builder.seal_block(ok_block);
            let shutdown_ref_ok = self
                .ctx
                .object_module
                .declare_func_in_func(shutdown_id, builder.func);
            builder.ins().call(shutdown_ref_ok, &[]);
            let zero = builder.ins().iconst(cl_types::I32, 0);
            builder.ins().return_(&[zero]);

            builder.finalize();
        }

        // Compile and define
        let mut codegen_ctx = Context::for_function(cl_func);
        self.ctx
            .object_module
            .define_function(main_id, &mut codegen_ctx)?;

        Ok(())
    }

    /// Emit the `__esc_dispatch` trampoline function.
    ///
    /// This exported function routes a runtime `func_idx` to the correct
    /// compiled function, enabling the runtime to call back into compiled code
    /// (e.g. when invoking a closure via `__esc_rt_call_indirect`).
    ///
    /// Signature: `(func_idx: i32, argc: i32, argv_ptr: i64) -> i64`
    ///
    /// Uses a Cranelift `Switch` (which compiles to a `br_table`) for O(1)
    /// dispatch instead of an O(n) if-chain. Each case block loads arguments
    /// from `argv_ptr`, calls the target function, and returns the result.
    /// If no function matches, returns NaN-boxed `undefined`.
    fn emit_dispatch_trampoline(
        &mut self,
        _runtime: &mut RuntimeCalls,
        fb_ctx: &mut FunctionBuilderContext,
        func_ids: &[FuncId],
        module: &TypedModule,
    ) -> Result<(), CodegenError> {
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };

        // Declare __esc_dispatch(func_idx: i32, argc: i32, argv: i64) -> i64
        let mut sig = Signature::new(call_conv);
        sig.params.push(AbiParam::new(cl_types::I32)); // func_idx
        sig.params.push(AbiParam::new(cl_types::I32)); // argc
        sig.params.push(AbiParam::new(cl_types::I64)); // argv_ptr
        sig.returns.push(AbiParam::new(cl_types::I64)); // result

        let dispatch_id =
            self.ctx
                .object_module
                .declare_function("__esc_dispatch", Linkage::Export, &sig)?;

        let mut cl_func = CraneliftFunction::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(2, 0),
            sig,
        );

        {
            let mut builder = FunctionBuilder::new(&mut cl_func, fb_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);

            let func_idx_param = builder.block_params(entry_block)[0];
            let _argc_param = builder.block_params(entry_block)[1];
            let argv_param = builder.block_params(entry_block)[2];

            // NaN-boxed undefined: QNAN | (TAG_UNDEFINED << TAG_SHIFT) = 0x7FFC...
            let undefined_bits: i64 = (0x7FF8_0000_0000_0000_u64 | (0x0004_u64 << 48)) as i64;

            if func_ids.is_empty() {
                builder.seal_block(entry_block);
                let undef = builder.ins().iconst(cl_types::I64, undefined_bits);
                builder.ins().return_(&[undef]);
            } else {
                // Build O(1) jump table via Cranelift Switch (compiles to br_table)
                let default_block = builder.create_block();
                let mut switch = cranelift_frontend::Switch::new();

                // Create a case block for each function and register with the Switch
                let mut case_blocks = Vec::with_capacity(func_ids.len());
                for i in 0..func_ids.len() {
                    let case_block = builder.create_block();
                    case_blocks.push(case_block);
                    switch.set_entry(i as u128, case_block);
                }

                // Emit the jump table from the entry block
                switch.emit(&mut builder, func_idx_param, default_block);

                // Seal entry block after the switch is emitted (predecessors are known)
                builder.seal_block(entry_block);

                // Emit each case block: load args, call function, return result
                for (i, &fid) in func_ids.iter().enumerate() {
                    builder.switch_to_block(case_blocks[i]);
                    builder.seal_block(case_blocks[i]);

                    let param_count = module.functions[i].params.len();
                    let argc_param = builder.block_params(entry_block)[1];
                    let mut args = Vec::with_capacity(param_count);
                    for p in 0..param_count {
                        // If p < argc, load from argv; otherwise use undefined.
                        // This prevents reading past the end of argv when a
                        // function is called with fewer arguments than parameters.
                        let p_val = builder.ins().iconst(cl_types::I32, p as i64);
                        let in_range = builder.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::SignedLessThan,
                            p_val,
                            argc_param,
                        );
                        let offset = (p * 8) as i32;
                        let loaded = builder.ins().load(
                            cl_types::I64,
                            ir::MemFlags::trusted(),
                            argv_param,
                            offset,
                        );
                        let undef_val = builder.ins().iconst(cl_types::I64, undefined_bits);
                        let arg = builder.ins().select(in_range, loaded, undef_val);
                        args.push(arg);
                    }

                    let func_ref = self
                        .ctx
                        .object_module
                        .declare_func_in_func(fid, builder.func);
                    let call = builder.ins().call(func_ref, &args);
                    let results = builder.inst_results(call);
                    if results.is_empty() {
                        // Void function — return undefined
                        let undef = builder.ins().iconst(cl_types::I64, undefined_bits);
                        builder.ins().return_(&[undef]);
                    } else {
                        let raw = results[0];
                        let raw_ty = builder.func.dfg.value_type(raw);
                        // Widen the result to i64 for the trampoline return
                        let widened = if raw_ty == cl_types::I64 {
                            raw
                        } else if raw_ty == cl_types::I32 || raw_ty == cl_types::I8 {
                            builder.ins().uextend(cl_types::I64, raw)
                        } else if raw_ty == cl_types::F64 {
                            builder
                                .ins()
                                .bitcast(cl_types::I64, ir::MemFlags::new(), raw)
                        } else {
                            // Unknown type — zero-extend as best effort
                            builder.ins().uextend(cl_types::I64, raw)
                        };
                        builder.ins().return_(&[widened]);
                    }
                }

                // Default block: return undefined for unknown func_idx
                builder.switch_to_block(default_block);
                builder.seal_block(default_block);
                let undef = builder.ins().iconst(cl_types::I64, undefined_bits);
                builder.ins().return_(&[undef]);
            }

            builder.finalize();
        }

        let mut codegen_ctx = Context::for_function(cl_func);
        self.ctx
            .object_module
            .define_function(dispatch_id, &mut codegen_ctx)?;
        Ok(())
    }
}
