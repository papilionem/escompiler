//! Top-level LLVM code generation entry point.
//!
//! Compiles a [`TypedModule`] into native object file bytes via LLVM. The main
//! entry point is [`LlvmBackend`], which manages the LLVM context, module, and
//! builder, then delegates per-function lowering to [`FunctionLowerer`].

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Linkage;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use ir::builder::TypedModule;

use crate::debug_info::DebugInfoEmitter;
use crate::error::LlvmCodegenError;
use crate::lowering::FunctionLowerer;
use crate::runtime_calls::RuntimeCalls;
use crate::types::ir_type_to_llvm;

/// The LLVM code generation backend.
///
/// Compiles a [`TypedModule`] into a native object file. Create one instance
/// per compilation unit and call [`compile_module`](Self::compile_module) to
/// produce the object bytes.
pub struct LlvmBackend {
    /// Optimization level for the target machine.
    opt_level: OptimizationLevel,
}

impl LlvmBackend {
    /// Create a new backend targeting the host machine with the given optimization level.
    pub fn new(opt_level: OptimizationLevel) -> Self {
        Self { opt_level }
    }

    /// Create a new backend with default (aggressive) optimization.
    pub fn new_release() -> Self {
        Self {
            opt_level: OptimizationLevel::Aggressive,
        }
    }

    /// Create a new backend with no optimization (for debug builds).
    pub fn new_debug() -> Self {
        Self {
            opt_level: OptimizationLevel::None,
        }
    }

    /// Compile an IR module into object file bytes.
    ///
    /// The `string_table` provides the string literals referenced by `ConstString`
    /// instructions. The entry function (if any) is exported as `__esc_main`.
    pub fn compile_module(
        &self,
        module: &TypedModule,
        string_table: &[String],
    ) -> Result<Vec<u8>, LlvmCodegenError> {
        // Initialize LLVM native target
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| LlvmCodegenError::Target(e.to_string()))?;

        let context = Context::create();
        let llvm_module = context.create_module("cs_module");
        let builder = context.create_builder();

        let mut runtime = RuntimeCalls::new();

        // Phase 1: Declare all functions in the module
        let mut func_values = Vec::with_capacity(module.functions.len());
        for (i, func) in module.functions.iter().enumerate() {
            let ret_ty = ir_type_to_llvm(&func.return_type, &context)?;
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = func
                .params
                .iter()
                .map(|(_, ty)| {
                    ir_type_to_llvm(ty, &context)
                        .map(|opt| opt.unwrap_or_else(|| context.i64_type().into()).into())
                })
                .collect::<Result<Vec<_>, _>>()?;

            let fn_type = match ret_ty {
                Some(ret) => match ret {
                    inkwell::types::BasicTypeEnum::IntType(t) => t.fn_type(&param_types, false),
                    inkwell::types::BasicTypeEnum::FloatType(t) => t.fn_type(&param_types, false),
                    _ => context.i64_type().fn_type(&param_types, false),
                },
                None => context.void_type().fn_type(&param_types, false),
            };

            let linkage = if module.entry == Some(i) {
                Linkage::External
            } else {
                Linkage::Internal
            };

            let name = if module.entry == Some(i) {
                "__esc_main".to_string()
            } else {
                format!("{}_{}", func.name, i)
            };

            let fv = llvm_module.add_function(&name, fn_type, Some(linkage));
            func_values.push(fv);
        }

        // Phase 2: Lower each function
        for (i, func) in module.functions.iter().enumerate() {
            let mut lowerer = FunctionLowerer::new(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                &func_values,
                string_table,
            );
            lowerer.lower(func, func_values[i])?;
        }

        // Phase 3: Emit main() wrapper if there is an entry function
        if let Some(entry_idx) = module.entry {
            self.emit_main_wrapper(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                func_values[entry_idx],
            )?;
        }

        // Phase 4: Emit dispatch trampoline
        self.emit_dispatch_trampoline(&context, &builder, &llvm_module, &func_values, module)?;

        // Verify the LLVM module before emission — a verification failure is a
        // compiler bug, never a user-facing error (security.md §3, DG-3).
        llvm_module.verify().map_err(|e| {
            LlvmCodegenError::Module(format!("BUG: LLVM module verification failed: {e}"))
        })?;

        // Phase 5: Compile to object file
        let triple = TargetMachine::get_default_triple();
        let target =
            Target::from_triple(&triple).map_err(|e| LlvmCodegenError::Target(e.to_string()))?;
        let cpu = TargetMachine::get_host_cpu_name();
        let features = TargetMachine::get_host_cpu_features();
        let target_machine = target
            .create_target_machine(
                &triple,
                cpu.to_str()
                    .map_err(|e| LlvmCodegenError::Target(e.to_string()))?,
                features
                    .to_str()
                    .map_err(|e| LlvmCodegenError::Target(e.to_string()))?,
                self.opt_level,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| LlvmCodegenError::Target("failed to create target machine".into()))?;

        let buf = target_machine
            .write_to_memory_buffer(&llvm_module, FileType::Object)
            .map_err(|e| LlvmCodegenError::ObjectWrite(e.to_string()))?;

        Ok(buf.as_slice().to_vec())
    }

    /// Compile an IR module into object file bytes with DWARF debug info.
    ///
    /// Like [`compile_module`](Self::compile_module), but also emits DWARF debug
    /// information so debuggers (GDB, LLDB) can map native code back to JS source
    /// lines.
    ///
    /// * `source_filename` - Name of the source file (e.g., `"script.js"`).
    /// * `source_directory` - Directory containing the source file.
    /// * `source_text` - The full source text (for byte-offset-to-line mapping).
    pub fn compile_module_with_debug_info(
        &self,
        module: &TypedModule,
        string_table: &[String],
        source_filename: &str,
        source_directory: &str,
        source_text: &str,
    ) -> Result<Vec<u8>, LlvmCodegenError> {
        // Initialize LLVM native target
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| LlvmCodegenError::Target(e.to_string()))?;

        let is_optimized = matches!(self.opt_level, OptimizationLevel::Aggressive);
        let context = Context::create();
        let llvm_module = context.create_module("cs_module");
        let builder = context.create_builder();

        let mut runtime = RuntimeCalls::new();

        // Create debug info emitter
        let debug_emitter = DebugInfoEmitter::new(
            &llvm_module,
            source_filename,
            source_directory,
            source_text,
            is_optimized,
        );

        // Phase 1: Declare all functions in the module
        let mut func_values = Vec::with_capacity(module.functions.len());
        for (i, func) in module.functions.iter().enumerate() {
            let ret_ty = ir_type_to_llvm(&func.return_type, &context)?;
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = func
                .params
                .iter()
                .map(|(_, ty)| {
                    ir_type_to_llvm(ty, &context)
                        .map(|opt| opt.unwrap_or_else(|| context.i64_type().into()).into())
                })
                .collect::<Result<Vec<_>, _>>()?;

            let fn_type = match ret_ty {
                Some(ret) => match ret {
                    inkwell::types::BasicTypeEnum::IntType(t) => t.fn_type(&param_types, false),
                    inkwell::types::BasicTypeEnum::FloatType(t) => t.fn_type(&param_types, false),
                    _ => context.i64_type().fn_type(&param_types, false),
                },
                None => context.void_type().fn_type(&param_types, false),
            };

            let linkage = if module.entry == Some(i) {
                Linkage::External
            } else {
                Linkage::Internal
            };

            let name = if module.entry == Some(i) {
                "__esc_main".to_string()
            } else {
                format!("{}_{}", func.name, i)
            };

            let fv = llvm_module.add_function(&name, fn_type, Some(linkage));
            func_values.push(fv);
        }

        // Phase 2: Lower each function with debug info
        for (i, func) in module.functions.iter().enumerate() {
            let mut lowerer = FunctionLowerer::new_with_debug(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                &func_values,
                string_table,
                &debug_emitter,
            );
            lowerer.lower(func, func_values[i])?;
        }

        // Unset debug location before emitting generated wrappers
        builder.unset_current_debug_location();

        // Phase 3: Emit main() wrapper if there is an entry function
        if let Some(entry_idx) = module.entry {
            self.emit_main_wrapper(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                func_values[entry_idx],
            )?;
        }

        // Phase 4: Emit dispatch trampoline
        self.emit_dispatch_trampoline(&context, &builder, &llvm_module, &func_values, module)?;

        // Phase 5: Finalize debug info before verification/emission
        debug_emitter.finalize();

        // Verify the LLVM module after debug info finalization.
        llvm_module.verify().map_err(|e| {
            LlvmCodegenError::Module(format!("BUG: LLVM module verification failed: {e}"))
        })?;

        // Phase 6: Compile to object file
        let triple = TargetMachine::get_default_triple();
        let target =
            Target::from_triple(&triple).map_err(|e| LlvmCodegenError::Target(e.to_string()))?;
        let cpu = TargetMachine::get_host_cpu_name();
        let features = TargetMachine::get_host_cpu_features();
        let target_machine = target
            .create_target_machine(
                &triple,
                cpu.to_str()
                    .map_err(|e| LlvmCodegenError::Target(e.to_string()))?,
                features
                    .to_str()
                    .map_err(|e| LlvmCodegenError::Target(e.to_string()))?,
                self.opt_level,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| LlvmCodegenError::Target("failed to create target machine".into()))?;

        let buf = target_machine
            .write_to_memory_buffer(&llvm_module, FileType::Object)
            .map_err(|e| LlvmCodegenError::ObjectWrite(e.to_string()))?;

        Ok(buf.as_slice().to_vec())
    }

    /// Compile an IR module into object file bytes for a specific target triple.
    ///
    /// Initializes all LLVM targets and creates a target machine for the given
    /// triple (e.g., `"aarch64-unknown-linux-gnu"`). This enables cross-compilation
    /// to architectures other than the host.
    pub fn compile_module_for_target(
        &self,
        module: &TypedModule,
        string_table: &[String],
        target_triple: &str,
    ) -> Result<Vec<u8>, LlvmCodegenError> {
        // Initialize ALL LLVM targets for cross-compilation
        Target::initialize_all(&InitializationConfig::default());

        let context = Context::create();
        let llvm_module = context.create_module("cs_module");
        let builder = context.create_builder();

        let mut runtime = RuntimeCalls::new();

        // Phase 1: Declare all functions
        let mut func_values = Vec::with_capacity(module.functions.len());
        for (i, func) in module.functions.iter().enumerate() {
            let ret_ty = ir_type_to_llvm(&func.return_type, &context)?;
            let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> = func
                .params
                .iter()
                .map(|(_, ty)| {
                    ir_type_to_llvm(ty, &context)
                        .map(|opt| opt.unwrap_or_else(|| context.i64_type().into()).into())
                })
                .collect::<Result<Vec<_>, _>>()?;

            let fn_type = match ret_ty {
                Some(ret) => match ret {
                    inkwell::types::BasicTypeEnum::IntType(t) => t.fn_type(&param_types, false),
                    inkwell::types::BasicTypeEnum::FloatType(t) => t.fn_type(&param_types, false),
                    _ => context.i64_type().fn_type(&param_types, false),
                },
                None => context.void_type().fn_type(&param_types, false),
            };

            let linkage = if module.entry == Some(i) {
                Linkage::External
            } else {
                Linkage::Internal
            };

            let name = if module.entry == Some(i) {
                "__esc_main".to_string()
            } else {
                format!("{}_{}", func.name, i)
            };

            let fv = llvm_module.add_function(&name, fn_type, Some(linkage));
            func_values.push(fv);
        }

        // Phase 2: Lower each function
        for (i, func) in module.functions.iter().enumerate() {
            let mut lowerer = FunctionLowerer::new(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                &func_values,
                string_table,
            );
            lowerer.lower(func, func_values[i])?;
        }

        // Phase 3: Emit main() wrapper if there is an entry function
        if let Some(entry_idx) = module.entry {
            self.emit_main_wrapper(
                &context,
                &builder,
                &llvm_module,
                &mut runtime,
                func_values[entry_idx],
            )?;
        }

        // Phase 4: Emit dispatch trampoline
        self.emit_dispatch_trampoline(&context, &builder, &llvm_module, &func_values, module)?;

        // Verify the LLVM module before cross-compilation emission.
        llvm_module.verify().map_err(|e| {
            LlvmCodegenError::Module(format!("BUG: LLVM module verification failed: {e}"))
        })?;

        // Phase 5: Compile to object file for specified target
        let triple = TargetTriple::create(target_triple);
        llvm_module.set_triple(&triple);
        let target =
            Target::from_triple(&triple).map_err(|e| LlvmCodegenError::Target(e.to_string()))?;
        let target_machine = target
            .create_target_machine(
                &triple,
                "",
                "",
                self.opt_level,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| {
                LlvmCodegenError::Target(format!(
                    "failed to create target machine for {target_triple}"
                ))
            })?;

        let buf = target_machine
            .write_to_memory_buffer(&llvm_module, FileType::Object)
            .map_err(|e| LlvmCodegenError::ObjectWrite(e.to_string()))?;

        Ok(buf.as_slice().to_vec())
    }

    /// Emit a C-style `main() -> i32` wrapper that calls runtime init/shutdown.
    ///
    /// The generated function:
    /// 1. Calls `__esc_rt_init()` — initializes the runtime
    /// 2. Calls `__esc_main()` — runs the JS entry function
    /// 3. Calls `__esc_rt_microtask_drain()` — flushes promise reactions
    /// 4. Checks for unhandled exceptions
    /// 5. Calls `__esc_rt_shutdown()` — tears down the runtime
    /// 6. Returns 0 (success) or 1 (unhandled exception)
    fn emit_main_wrapper<'ctx>(
        &self,
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
        runtime: &mut RuntimeCalls<'ctx>,
        entry_fn: inkwell::values::FunctionValue<'ctx>,
    ) -> Result<(), LlvmCodegenError> {
        let main_ty = ctx.i32_type().fn_type(&[], false);
        let main_fn = module.add_function("main", main_ty, Some(Linkage::External));
        let entry_bb = ctx.append_basic_block(main_fn, "entry");
        builder.position_at_end(entry_bb);

        // call __esc_rt_init()
        let init_fn = runtime.get_void_void("__esc_rt_init", module);
        builder
            .build_call(init_fn, &[], "")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // call __esc_main() — pass zero-initialized args matching the entry function signature.
        // JS entry functions with params receive `undefined` at runtime, so zeros (NaN-box
        // undefined) are correct.
        let arg_count = entry_fn.count_params() as usize;
        let mut args: Vec<inkwell::values::BasicMetadataValueEnum> = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            let param_ty = ctx.i64_type();
            let zero = param_ty.const_zero();
            args.push(zero.into());
        }
        builder
            .build_call(entry_fn, &args, "")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // call __esc_rt_microtask_drain()
        let drain_fn = runtime.get_void_void("__esc_rt_microtask_drain", module);
        builder
            .build_call(drain_fn, &[], "")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // Check for unhandled exceptions: if pending, return 1
        let has_exc_fn = runtime.get_void_i32("__esc_rt_has_pending_exception", module);
        let exc_call = builder
            .build_call(has_exc_fn, &[], "has_exc")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        let has_exc = exc_call
            .try_as_basic_value()
            .left()
            .ok_or_else(|| LlvmCodegenError::Module("has_pending_exception returned void".into()))?
            .into_int_value();

        let ok_bb = ctx.append_basic_block(main_fn, "ok");
        let err_bb = ctx.append_basic_block(main_fn, "err");

        let is_zero = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                has_exc,
                ctx.i32_type().const_zero(),
                "exc_is_zero",
            )
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        builder
            .build_conditional_branch(is_zero, ok_bb, err_bb)
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // Error path: shutdown and return 1
        builder.position_at_end(err_bb);
        let shutdown_fn_err = runtime.get_void_void("__esc_rt_shutdown", module);
        builder
            .build_call(shutdown_fn_err, &[], "")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        let one = ctx.i32_type().const_int(1, false);
        builder
            .build_return(Some(&one))
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // Success path: shutdown and return 0
        builder.position_at_end(ok_bb);
        let shutdown_fn_ok = runtime.get_void_void("__esc_rt_shutdown", module);
        builder
            .build_call(shutdown_fn_ok, &[], "")
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        let zero = ctx.i32_type().const_zero();
        builder
            .build_return(Some(&zero))
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        Ok(())
    }

    /// Emit the `__esc_dispatch` trampoline function.
    ///
    /// Signature: `(func_idx: i32, argc: i32, argv_ptr: i64) -> i64`
    fn emit_dispatch_trampoline<'ctx>(
        &self,
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        module: &inkwell::module::Module<'ctx>,
        func_values: &[inkwell::values::FunctionValue<'ctx>],
        ir_module: &TypedModule,
    ) -> Result<(), LlvmCodegenError> {
        let i32_ty = ctx.i32_type();
        let i64_ty = ctx.i64_type();
        let fn_ty = i64_ty.fn_type(&[i32_ty.into(), i32_ty.into(), i64_ty.into()], false);
        let dispatch_fn = module.add_function("__esc_dispatch", fn_ty, Some(Linkage::External));

        let entry_bb = ctx.append_basic_block(dispatch_fn, "entry");
        builder.position_at_end(entry_bb);

        let func_idx_param = dispatch_fn
            .get_nth_param(0)
            .ok_or_else(|| LlvmCodegenError::Module("missing dispatch param 0".into()))?
            .into_int_value();
        let argv_param = dispatch_fn
            .get_nth_param(2)
            .ok_or_else(|| LlvmCodegenError::Module("missing dispatch param 2".into()))?
            .into_int_value();

        // NaN-boxed undefined: QNAN | (TAG_UNDEFINED << TAG_SHIFT)
        let undefined_bits = i64_ty.const_int(0x7FF8_0000_0000_0000 | (0x0004 << 48), false);

        if func_values.is_empty() {
            builder
                .build_return(Some(&undefined_bits))
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
            return Ok(());
        }

        let default_bb = ctx.append_basic_block(dispatch_fn, "default");

        // Build switch
        let cases: Vec<(
            inkwell::values::IntValue<'ctx>,
            inkwell::basic_block::BasicBlock<'ctx>,
        )> = func_values
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let bb = ctx.append_basic_block(dispatch_fn, &format!("case_{i}"));
                (i32_ty.const_int(i as u64, false), bb)
            })
            .collect();

        builder
            .build_switch(func_idx_param, default_bb, &cases)
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        // Emit each case block
        for (i, (_, case_bb)) in cases.iter().enumerate() {
            builder.position_at_end(*case_bb);

            let param_count = ir_module.functions[i].params.len();
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                Vec::with_capacity(param_count);

            // Convert argv_param (i64) to pointer for loading
            let argv_ptr = builder
                .build_int_to_ptr(
                    argv_param,
                    ctx.ptr_type(inkwell::AddressSpace::default()),
                    "argv_as_ptr",
                )
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

            let argc_param = dispatch_fn
                .get_nth_param(1)
                .ok_or_else(|| LlvmCodegenError::Module("missing dispatch param 1".into()))?
                .into_int_value();

            for p in 0..param_count {
                // Check if p < argc; if not, use undefined instead of reading past argv.
                let p_val = i32_ty.const_int(p as u64, false);
                let in_range = builder
                    .build_int_compare(
                        inkwell::IntPredicate::SLT,
                        p_val,
                        argc_param,
                        &format!("in_range_{p}"),
                    )
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

                // SAFETY: build_in_bounds_gep is marked unsafe in inkwell because LLVM
                // requires the resulting pointer to stay within the allocated object. Here,
                // argv_ptr points to a caller-provided array of i64 values with at least
                // `argc` elements. We only dereference when `p < argc`.
                let gep = unsafe {
                    builder.build_in_bounds_gep(
                        i64_ty,
                        argv_ptr,
                        &[i32_ty.const_int(p as u64, false)],
                        &format!("arg_{p}"),
                    )
                }
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

                let loaded = builder
                    .build_load(i64_ty, gep, &format!("load_arg_{p}"))
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?
                    .into_int_value();

                let arg = builder
                    .build_select(in_range, loaded, undefined_bits, &format!("arg_sel_{p}"))
                    .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
                args.push(arg.into());
            }

            let call = builder
                .build_call(func_values[i], &args, "dispatch_call")
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

            let result = call
                .try_as_basic_value()
                .left()
                .map(|v| v.into_int_value())
                .unwrap_or(undefined_bits);

            builder
                .build_return(Some(&result))
                .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;
        }

        // Default block: return undefined
        builder.position_at_end(default_bb);
        builder
            .build_return(Some(&undefined_bits))
            .map_err(|e| LlvmCodegenError::Module(e.to_string()))?;

        Ok(())
    }
}

impl Default for LlvmBackend {
    fn default() -> Self {
        Self::new_release()
    }
}
