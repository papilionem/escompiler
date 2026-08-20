//! Cranelift JIT compiler for eval/Function runtime code generation.
//!
//! [`JitEval`] compiles JavaScript source to native code at runtime using
//! Cranelift's JIT infrastructure. The pipeline is: parse (oxc) -> lower
//! (desugar) -> JIT compile (Cranelift) -> execute.

use std::collections::HashMap;
use std::sync::Arc;

use ::ir::builder::{TypedFunction, TypedModule};
use ::ir::{Op, ValueId};
use cranelift_codegen::Context;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, Signature};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};

use crate::error::EvalError;

/// NaN-boxing constants (must match nanbox / cranelift::nanbox_emit).
const QNAN: u64 = 0x7FF8_0000_0000_0000;
const TAG_INT: u64 = 0x0001;
const TAG_BOOL: u64 = 0x0002;
const TAG_NULL: u64 = 0x0003;
const TAG_UNDEFINED: u64 = 0x0004;
const TAG_SHIFT: u64 = 48;

/// Self-hosted JIT compiler for `eval()` and `new Function()`.
///
/// Wraps a Cranelift [`JITModule`] and provides `eval()` which parses,
/// lowers, compiles, and executes JavaScript source at runtime.
pub struct JitEval {
    /// The Cranelift JIT module.
    module: JITModule,
    /// Target ISA for signature construction.
    isa: Arc<dyn TargetIsa>,
}

impl JitEval {
    /// Create a new JIT eval context targeting the host machine.
    pub fn new() -> Result<Self, EvalError> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .map_err(|e| EvalError::Jit {
                message: e.to_string(),
            })?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| EvalError::Jit {
                message: e.to_string(),
            })?;

        let isa_builder =
            cranelift_codegen::isa::lookup(target_lexicon::Triple::host()).map_err(|e| {
                EvalError::Jit {
                    message: e.to_string(),
                }
            })?;

        let flags = settings::Flags::new(flag_builder);
        let isa = isa_builder.finish(flags).map_err(|e| EvalError::Jit {
            message: e.to_string(),
        })?;

        let mut jit_builder =
            JITBuilder::with_isa(isa.clone(), cranelift_module::default_libcall_names());

        register_runtime_symbols(&mut jit_builder);

        let module = JITModule::new(jit_builder);

        Ok(Self { module, isa })
    }

    /// Parse, lower, compile, and execute a JavaScript source string.
    ///
    /// Returns the result as a raw NaN-boxed u64 value.
    pub fn eval(&mut self, source: &str) -> Result<u64, EvalError> {
        if source.trim().is_empty() {
            return Ok(QNAN | (TAG_UNDEFINED << TAG_SHIFT));
        }

        // Step 1: Lower source to IR (as script/cjs, not ESM)
        let lowering_result =
            desugar::lower_source(source, oxc_span::SourceType::cjs()).map_err(|errors| {
                EvalError::Lowering {
                    message: errors
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; "),
                }
            })?;

        let ir_module = &lowering_result.module;
        let string_table = &lowering_result.string_table;

        // Step 2: Compile all functions
        let entry_idx = ir_module.entry.ok_or_else(|| EvalError::Jit {
            message: "no entry function in lowered module".to_string(),
        })?;

        let func_ids = self.declare_functions(ir_module)?;

        let mut fb_ctx = FunctionBuilderContext::new();
        for (i, func) in ir_module.functions.iter().enumerate() {
            self.compile_function(func, func_ids[i], &func_ids, string_table, &mut fb_ctx)?;
        }

        self.module
            .finalize_definitions()
            .map_err(|e| EvalError::Jit {
                message: format!("finalize error: {e}"),
            })?;

        // Step 3: Get pointer to entry function and call it
        let entry_ptr = self.module.get_finalized_function(func_ids[entry_idx]);

        // SAFETY: The function was just compiled by Cranelift with signature () -> i64.
        let code_fn: extern "C" fn() -> u64 = unsafe { std::mem::transmute(entry_ptr) };
        let result = code_fn();

        Ok(result)
    }

    /// Parse, lower, compile, and execute JavaScript source in direct eval mode.
    ///
    /// Unlike [`eval`](Self::eval), this method bridges the caller's scope:
    /// - `lex_env` is the caller's lexical environment (EscEnvironment pointer).
    /// - `var_env` is the caller's variable environment (for sloppy `var` leaking).
    /// - `this_value` is the caller's `this` binding.
    /// - `is_strict` indicates whether the eval is in strict mode.
    ///
    /// In strict mode, `var` declarations are confined to the eval's own scope.
    /// In sloppy mode, `var` declarations leak to the caller's `var_env`.
    ///
    /// Returns the result as a raw NaN-boxed u64 value.
    pub fn eval_direct(
        &mut self,
        source: &str,
        lex_env: u64,
        var_env: u64,
        this_value: u64,
        is_strict: bool,
    ) -> Result<u64, EvalError> {
        if source.trim().is_empty() {
            return Ok(QNAN | (TAG_UNDEFINED << TAG_SHIFT));
        }

        // Step 1: Detect var declarations for sloppy mode leaking
        let var_decls = if !is_strict {
            collect_var_declarations(source)
        } else {
            Vec::new()
        };

        // Step 2: Lower source to IR (as script/cjs, not ESM)
        let lowering_result =
            desugar::lower_source(source, oxc_span::SourceType::cjs()).map_err(|errors| {
                EvalError::Lowering {
                    message: errors
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; "),
                }
            })?;

        let ir_module = &lowering_result.module;
        let string_table = &lowering_result.string_table;

        // Step 3: Compile all functions (entry function gets env params)
        let entry_idx = ir_module.entry.ok_or_else(|| EvalError::Jit {
            message: "no entry function in lowered module".to_string(),
        })?;

        let func_ids = self.declare_functions_direct(ir_module, entry_idx)?;

        let mut fb_ctx = FunctionBuilderContext::new();
        for (i, func) in ir_module.functions.iter().enumerate() {
            if i == entry_idx {
                self.compile_function_direct(
                    func,
                    func_ids[i],
                    &func_ids,
                    string_table,
                    &mut fb_ctx,
                )?;
            } else {
                self.compile_function(func, func_ids[i], &func_ids, string_table, &mut fb_ctx)?;
            }
        }

        self.module
            .finalize_definitions()
            .map_err(|e| EvalError::Jit {
                message: format!("finalize error: {e}"),
            })?;

        // Step 4: Leak var declarations to var_env in sloppy mode
        if !is_strict {
            for name in &var_decls {
                leak_var_to_env(var_env, name);
            }
        }

        // Step 5: Execute with env pointers
        let entry_ptr = self.module.get_finalized_function(func_ids[entry_idx]);

        // SAFETY: The function was just compiled by Cranelift with signature
        // (i64, i64, i64) -> i64 representing (lex_env, var_env, this_value).
        let code_fn: extern "C" fn(u64, u64, u64) -> u64 =
            unsafe { std::mem::transmute(entry_ptr) };
        let result = code_fn(lex_env, var_env, this_value);

        Ok(result)
    }

    /// Declare functions for direct eval mode.
    ///
    /// The entry function gets an extended signature: `(lex_env: i64, var_env: i64,
    /// this_value: i64) -> i64`. Other functions are declared normally.
    fn declare_functions_direct(
        &mut self,
        ir_module: &TypedModule,
        entry_idx: usize,
    ) -> Result<Vec<FuncId>, EvalError> {
        let mut func_ids = Vec::with_capacity(ir_module.functions.len());

        for (i, func) in ir_module.functions.iter().enumerate() {
            let linkage = if i == entry_idx {
                Linkage::Export
            } else {
                Linkage::Local
            };
            let name = if i == entry_idx {
                "__esc_eval_direct_main".to_string()
            } else {
                format!("__esc_eval_direct_{}_{}", func.name, i)
            };

            let sig = if i == entry_idx {
                // Entry function takes (lex_env, var_env, this_value) and returns JSValue
                let call_conv = if cfg!(target_os = "windows") {
                    CallConv::WindowsFastcall
                } else {
                    CallConv::SystemV
                };
                let mut sig = Signature::new(call_conv);
                sig.params.push(AbiParam::new(types::I64)); // lex_env
                sig.params.push(AbiParam::new(types::I64)); // var_env
                sig.params.push(AbiParam::new(types::I64)); // this_value
                sig.returns.push(AbiParam::new(types::I64)); // result
                sig
            } else {
                self.build_signature(func)?
            };

            let id = self
                .module
                .declare_function(&name, linkage, &sig)
                .map_err(|e| EvalError::Jit {
                    message: format!("declare function '{}': {e}", func.name),
                })?;

            func_ids.push(id);
        }

        Ok(func_ids)
    }

    /// Compile the entry function for direct eval mode.
    ///
    /// The entry function receives `(lex_env, var_env, this_value)` parameters
    /// which are threaded through the lowerer so environment lookups can access
    /// the caller's scope.
    fn compile_function_direct(
        &mut self,
        func: &TypedFunction,
        func_id: FuncId,
        all_func_ids: &[FuncId],
        string_table: &[String],
        fb_ctx: &mut FunctionBuilderContext,
    ) -> Result<(), EvalError> {
        // Build the direct eval entry signature
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };
        let mut sig = Signature::new(call_conv);
        sig.params.push(AbiParam::new(types::I64)); // lex_env
        sig.params.push(AbiParam::new(types::I64)); // var_env
        sig.params.push(AbiParam::new(types::I64)); // this_value
        sig.returns.push(AbiParam::new(types::I64)); // result

        let mut cl_func = cranelift_codegen::ir::Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32()),
            sig,
        );

        {
            let builder = FunctionBuilder::new(&mut cl_func, fb_ctx);
            let mut lowerer = JitFunctionLowerer::new(all_func_ids);
            lowerer.set_direct_eval_mode();

            lowerer.lower(func, builder, &mut self.module, string_table)?;
        }

        if let Err(errors) = cranelift_codegen::verify_function(&cl_func, self.isa.as_ref()) {
            return Err(EvalError::Jit {
                message: format!(
                    "verifier error in '{}': {}\nIR:\n{}",
                    func.name,
                    errors,
                    cl_func.display()
                ),
            });
        }

        let mut codegen_ctx = Context::for_function(cl_func);
        self.module
            .define_function(func_id, &mut codegen_ctx)
            .map_err(|e| EvalError::Jit {
                message: format!("define function '{}': {e}", func.name),
            })?;

        Ok(())
    }

    /// Declare all functions from the IR module in the JIT module.
    fn declare_functions(&mut self, ir_module: &TypedModule) -> Result<Vec<FuncId>, EvalError> {
        let mut func_ids = Vec::with_capacity(ir_module.functions.len());

        for (i, func) in ir_module.functions.iter().enumerate() {
            let sig = self.build_signature(func)?;
            let linkage = if ir_module.entry == Some(i) {
                Linkage::Export
            } else {
                Linkage::Local
            };
            let name = if ir_module.entry == Some(i) {
                "__esc_eval_main".to_string()
            } else {
                format!("__esc_eval_{}_{}", func.name, i)
            };

            let id = self
                .module
                .declare_function(&name, linkage, &sig)
                .map_err(|e| EvalError::Jit {
                    message: format!("declare function '{}': {e}", func.name),
                })?;

            func_ids.push(id);
        }

        Ok(func_ids)
    }

    /// Build a Cranelift signature from an IR function.
    fn build_signature(&self, func: &TypedFunction) -> Result<Signature, EvalError> {
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };

        let mut sig = Signature::new(call_conv);

        for (_name, ty) in &func.params {
            if let Some(cl_ty) = self.ir_type_to_cl(ty)? {
                sig.params.push(AbiParam::new(cl_ty));
            }
        }

        if let Some(ret_ty) = self.ir_type_to_cl(&func.return_type)? {
            sig.returns.push(AbiParam::new(ret_ty));
        }

        Ok(sig)
    }

    /// Map an IR type to a Cranelift type.
    fn ir_type_to_cl(
        &self,
        ty: &::ir::IrType,
    ) -> Result<Option<cranelift_codegen::ir::Type>, EvalError> {
        use ::ir::IrType;
        match ty {
            IrType::Void => Ok(None),
            IrType::Bool => Ok(Some(types::I8)),
            IrType::I32 => Ok(Some(types::I32)),
            IrType::I64 => Ok(Some(types::I64)),
            IrType::F64 => Ok(Some(types::F64)),
            IrType::JSValue
            | IrType::JSString
            | IrType::JSObject
            | IrType::JSArray
            | IrType::JSFunction
            | IrType::JSSymbol => Ok(Some(types::I64)),
            IrType::Ptr | IrType::ZonePtr | IrType::HeapPtr => Ok(Some(self.isa.pointer_type())),
            IrType::Struct(_)
            | IrType::Array(_, _)
            | IrType::CompletionRecord
            | IrType::IteratorRecord => Ok(Some(self.isa.pointer_type())),
        }
    }

    /// Compile a single IR function into the JIT module.
    fn compile_function(
        &mut self,
        func: &TypedFunction,
        func_id: FuncId,
        all_func_ids: &[FuncId],
        string_table: &[String],
        fb_ctx: &mut FunctionBuilderContext,
    ) -> Result<(), EvalError> {
        let sig = self.build_signature(func)?;

        let mut cl_func = cranelift_codegen::ir::Function::with_name_signature(
            cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32()),
            sig,
        );

        {
            let builder = FunctionBuilder::new(&mut cl_func, fb_ctx);
            let mut lowerer = JitFunctionLowerer::new(all_func_ids);

            lowerer.lower(func, builder, &mut self.module, string_table)?;
        }

        if let Err(errors) = cranelift_codegen::verify_function(&cl_func, self.isa.as_ref()) {
            return Err(EvalError::Jit {
                message: format!(
                    "verifier error in '{}': {}\nIR:\n{}",
                    func.name,
                    errors,
                    cl_func.display()
                ),
            });
        }

        let mut codegen_ctx = Context::for_function(cl_func);
        self.module
            .define_function(func_id, &mut codegen_ctx)
            .map_err(|e| EvalError::Jit {
                message: format!("define function '{}': {e}", func.name),
            })?;

        Ok(())
    }
}

/// Register known runtime symbols so JIT code can call them.
///
/// Maps JIT symbol names to the real `runtime::rt_api` functions so
/// that Cranelift-compiled code can invoke the runtime at execution time.
fn register_runtime_symbols(builder: &mut JITBuilder) {
    use runtime::rt_api;

    let symbols: &[(&str, *const u8)] = &[
        // Arithmetic
        ("__esc_rt_add_js", rt_api::__esc_rt_add_js as *const u8),
        ("__esc_rt_sub_js", rt_api::__esc_rt_sub_js as *const u8),
        ("__esc_rt_mul_js", rt_api::__esc_rt_mul_js as *const u8),
        ("__esc_rt_div_js", rt_api::__esc_rt_div_js as *const u8),
        ("__esc_rt_mod_js", rt_api::__esc_rt_mod_js as *const u8),
        ("__esc_rt_exp_js", rt_api::__esc_rt_exp_js as *const u8),
        ("__esc_rt_neg_js", rt_api::__esc_rt_neg_js as *const u8),
        // Conversion
        (
            "__esc_rt_to_number",
            rt_api::__esc_rt_to_number as *const u8,
        ),
        (
            "__esc_rt_to_boolean",
            rt_api::__esc_rt_to_boolean as *const u8,
        ),
        (
            "__esc_rt_to_string",
            rt_api::__esc_rt_to_string as *const u8,
        ),
        ("__esc_rt_typeof", rt_api::__esc_rt_typeof as *const u8),
        // Comparison
        (
            "__esc_rt_eq_strict",
            rt_api::__esc_rt_eq_strict as *const u8,
        ),
        (
            "__esc_rt_ne_strict",
            rt_api::__esc_rt_ne_strict as *const u8,
        ),
        ("__esc_rt_lt_js", rt_api::__esc_rt_lt_js as *const u8),
        ("__esc_rt_le_js", rt_api::__esc_rt_le_js as *const u8),
        ("__esc_rt_gt_js", rt_api::__esc_rt_gt_js as *const u8),
        ("__esc_rt_ge_js", rt_api::__esc_rt_ge_js as *const u8),
        // String creation (legacy name maps to real string_from_data)
        (
            "__esc_rt_create_string",
            rt_api::__esc_rt_string_from_data as *const u8,
        ),
        (
            "__esc_rt_string_from_data",
            rt_api::__esc_rt_string_from_data as *const u8,
        ),
        // Object / array creation
        (
            "__esc_rt_create_object",
            rt_api::__esc_rt_create_object as *const u8,
        ),
        (
            "__esc_rt_create_array",
            rt_api::__esc_rt_create_array as *const u8,
        ),
        // Property access (register under both legacy and current names)
        (
            "__esc_rt_set_property",
            rt_api::__esc_rt_set_prop as *const u8,
        ),
        ("__esc_rt_set_prop", rt_api::__esc_rt_set_prop as *const u8),
        (
            "__esc_rt_get_property",
            rt_api::__esc_rt_get_prop as *const u8,
        ),
        ("__esc_rt_get_prop", rt_api::__esc_rt_get_prop as *const u8),
        // Array operations
        (
            "__esc_rt_array_push",
            rt_api::__esc_rt_array_push as *const u8,
        ),
        // Lifecycle
        ("__esc_rt_init", rt_api::__esc_rt_init as *const u8),
        ("__esc_rt_shutdown", rt_api::__esc_rt_shutdown as *const u8),
        // Exceptions
        ("__esc_rt_throw", rt_api::__esc_rt_throw as *const u8),
        // Console I/O
        (
            "__esc_rt_console_log",
            rt_api::__esc_rt_console_log as *const u8,
        ),
        (
            "__esc_rt_console_error",
            rt_api::__esc_rt_console_error as *const u8,
        ),
        (
            "__esc_rt_console_warn",
            rt_api::__esc_rt_console_warn as *const u8,
        ),
        // Environment (closures)
        (
            "__esc_rt_env_create",
            rt_api::__esc_rt_env_create as *const u8,
        ),
        ("__esc_rt_env_load", rt_api::__esc_rt_env_load as *const u8),
        // Dynamic environment (EscEnvironment) for eval scope bridging
        (
            "__esc_rt_esc_env_lookup",
            rt_api::__esc_rt_esc_env_lookup as *const u8,
        ),
        (
            "__esc_rt_esc_env_store",
            rt_api::__esc_rt_esc_env_store as *const u8,
        ),
        (
            "__esc_rt_esc_env_add_binding",
            rt_api::__esc_rt_esc_env_add_binding as *const u8,
        ),
        (
            "__esc_rt_esc_env_create",
            rt_api::__esc_rt_esc_env_create as *const u8,
        ),
        (
            "__esc_rt_esc_env_get",
            rt_api::__esc_rt_esc_env_get as *const u8,
        ),
        (
            "__esc_rt_esc_env_set",
            rt_api::__esc_rt_esc_env_set as *const u8,
        ),
        (
            "__esc_rt_esc_env_populate_slot_map",
            rt_api::__esc_rt_esc_env_populate_slot_map as *const u8,
        ),
        (
            "__esc_rt_esc_env_get_boxed",
            rt_api::__esc_rt_esc_env_get_boxed as *const u8,
        ),
        (
            "__esc_rt_esc_env_set_boxed",
            rt_api::__esc_rt_esc_env_set_boxed as *const u8,
        ),
        // String creation for name-based lookups
        (
            "__esc_rt_string_from_data",
            rt_api::__esc_rt_string_from_data as *const u8,
        ),
        (
            "__esc_rt_create_string",
            rt_api::__esc_rt_string_from_data as *const u8,
        ),
    ];

    for &(name, ptr) in symbols {
        builder.symbol(name, ptr);
    }
}

// ============================================================================
// Simplified function lowerer for JIT (Option A: duplicated lowering logic)
// ============================================================================

/// Simplified function lowerer for JIT eval.
///
/// Handles the core subset of opcodes needed for basic eval expressions:
/// constants, arithmetic, control flow, function calls, and returns.
struct JitFunctionLowerer<'a> {
    /// IR ValueId -> Cranelift Value.
    values: HashMap<u32, ir::Value>,
    /// IR BlockId -> Cranelift Block.
    blocks: HashMap<u32, ir::Block>,
    /// Phi Variables: IR ValueId -> Cranelift Variable.
    phi_variables: HashMap<u32, Variable>,
    /// Declared function IDs for intra-module calls.
    func_ids: &'a [FuncId],
    /// Runtime function cache.
    runtime_cache: HashMap<String, FuncId>,
    /// ConstI32 values cache (for resolving Call targets).
    const_i32_values: HashMap<u32, i32>,
    /// ConstString index cache (for resolving CallRuntime names).
    const_string_indices: HashMap<u32, u32>,
    /// Call convention.
    call_conv: CallConv,
    /// Whether this is a direct eval entry function.
    ///
    /// When true, the function receives `(lex_env, var_env, this_value)` as
    /// block params, stored in `env_params` after entry block setup.
    direct_eval_mode: bool,
    /// Cranelift values for the `(lex_env, var_env, this_value)` parameters
    /// in direct eval mode. Populated during entry block setup.
    env_params: Option<EvalEnvParams>,
}

/// Environment parameters available in direct eval mode.
///
/// These are the Cranelift values for the `(lex_env, var_env, this_value)`
/// parameters that are passed to the entry function. They are stored during
/// entry block setup and used by `EnvLookup`/`EnvLookupStore` instructions
/// when the lowered IR references variables from the caller's scope.
struct EvalEnvParams {
    /// The caller's lexical environment (EscEnvironment pointer, NaN-boxed).
    /// Used by `EnvLookup` and `EnvLookupStore` to resolve free variables.
    // Stored for use in future IR instructions that reference caller scope
    #[allow(dead_code)]
    lex_env: ir::Value,
    /// The caller's variable environment (EscEnvironment pointer, NaN-boxed).
    /// Used for sloppy mode `var` leaking via `__esc_rt_esc_env_add_binding`.
    // Stored for use in future IR instructions that reference caller scope
    #[allow(dead_code)]
    var_env: ir::Value,
    /// The caller's `this` value (NaN-boxed).
    // Stored for use when `this` bridging is fully implemented
    #[allow(dead_code)]
    this_value: ir::Value,
}

impl<'a> JitFunctionLowerer<'a> {
    /// Create a new JIT function lowerer.
    fn new(func_ids: &'a [FuncId]) -> Self {
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };
        Self {
            values: HashMap::new(),
            blocks: HashMap::new(),
            phi_variables: HashMap::new(),
            func_ids,
            runtime_cache: HashMap::new(),
            const_i32_values: HashMap::new(),
            const_string_indices: HashMap::new(),
            call_conv,
            direct_eval_mode: false,
            env_params: None,
        }
    }

    /// Enable direct eval mode for the entry function.
    ///
    /// When set, the entry function receives `(lex_env, var_env, this_value)` as
    /// block parameters and the lowerer will store these for environment access.
    fn set_direct_eval_mode(&mut self) {
        self.direct_eval_mode = true;
    }

    /// Lower an entire function.
    fn lower(
        &mut self,
        func: &TypedFunction,
        mut builder: FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        // Create all Cranelift blocks
        for bb in &func.blocks {
            let cl_block = builder.create_block();
            self.blocks.insert(bb.id.0, cl_block);
        }

        // Set up phi variables using Cranelift's declare_var (which returns Variable)
        for bb in &func.blocks {
            for inst in &bb.instructions {
                if matches!(inst.op, Op::Phi) {
                    let var = builder.declare_var(types::I64);
                    self.phi_variables.insert(inst.id.0, var);
                }
            }
        }

        // Entry block: append function parameters
        if let Some(first_bb) = func.blocks.first() {
            let entry = self.get_block(first_bb.id)?;
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);

            let params = builder.block_params(entry).to_vec();
            if self.direct_eval_mode && params.len() >= 3 {
                // In direct eval mode, the first 3 params are
                // (lex_env, var_env, this_value)
                self.env_params = Some(EvalEnvParams {
                    lex_env: params[0],
                    var_env: params[1],
                    this_value: params[2],
                });
                // The IR function's own params (if any) start at index 3
                for (i, &param_val) in params.iter().skip(3).enumerate() {
                    self.values.insert(i as u32 | 0x8000_0000, param_val);
                }
            } else {
                for (i, &param_val) in params.iter().enumerate() {
                    self.values.insert(i as u32 | 0x8000_0000, param_val);
                }
            }
        }

        // Initialize phi variables with undefined defaults
        for &var in self.phi_variables.values() {
            let undef = self.emit_box_undefined(&mut builder);
            builder.def_var(var, undef);
        }

        // Lower each block
        for (bb_idx, bb) in func.blocks.iter().enumerate() {
            let cl_block = self.get_block(bb.id)?;

            if bb_idx > 0 {
                builder.switch_to_block(cl_block);
            }

            // Resolve phi variables
            let phi_vars: Vec<(u32, Variable)> =
                self.phi_variables.iter().map(|(&k, &v)| (k, v)).collect();
            for (vid, var) in &phi_vars {
                let val = builder.use_var(*var);
                self.values.insert(*vid, val);
            }

            // Lower each instruction
            for inst in &bb.instructions {
                if matches!(inst.op, Op::Phi) {
                    continue;
                }
                self.lower_instruction(inst, &mut builder, module, string_table)?;

                // If this instruction's result feeds into a phi, def_var it
                if let Some(&cl_val) = self.values.get(&inst.id.0) {
                    self.update_phi_bindings(func, inst.id, cl_val, &mut builder);
                }
            }
        }

        // Seal all blocks
        for bb in &func.blocks {
            let cl_block = self.get_block(bb.id)?;
            builder.seal_block(cl_block);
        }

        builder.finalize();
        Ok(())
    }

    /// Update phi variable bindings when a value is produced.
    fn update_phi_bindings(
        &self,
        func: &TypedFunction,
        produced_id: ValueId,
        cl_val: ir::Value,
        builder: &mut FunctionBuilder<'_>,
    ) {
        // Check all phi instructions to see if they reference this value
        for bb in &func.blocks {
            for inst in &bb.instructions {
                if matches!(inst.op, Op::Phi)
                    && inst.operands.contains(&produced_id)
                    && let Some(&var) = self.phi_variables.get(&inst.id.0)
                {
                    // Coerce to i64 if needed
                    let val_ty = builder.func.dfg.value_type(cl_val);
                    let coerced = if val_ty == types::I32 {
                        let tag = builder
                            .ins()
                            .iconst(types::I64, (QNAN | (TAG_INT << TAG_SHIFT)) as i64);
                        let ext = builder.ins().uextend(types::I64, cl_val);
                        builder.ins().bor(tag, ext)
                    } else if val_ty == types::F64 {
                        builder
                            .ins()
                            .bitcast(types::I64, ir::MemFlags::new(), cl_val)
                    } else if val_ty == types::I8 {
                        let tag = builder
                            .ins()
                            .iconst(types::I64, (QNAN | (TAG_BOOL << TAG_SHIFT)) as i64);
                        let ext = builder.ins().uextend(types::I64, cl_val);
                        builder.ins().bor(tag, ext)
                    } else {
                        cl_val
                    };
                    builder.def_var(var, coerced);
                }
            }
        }
    }

    /// Get a Cranelift block for an IR block.
    fn get_block(&self, id: ::ir::BlockId) -> Result<ir::Block, EvalError> {
        self.blocks
            .get(&id.0)
            .copied()
            .ok_or_else(|| EvalError::Jit {
                message: format!("unknown block: {:?}", id),
            })
    }

    /// Get a Cranelift value for an IR value.
    fn get_value(&self, id: ValueId) -> Result<ir::Value, EvalError> {
        self.values
            .get(&id.0)
            .copied()
            .ok_or_else(|| EvalError::Jit {
                message: format!("undefined value: {:?}", id),
            })
    }

    /// Emit a NaN-boxed undefined constant.
    fn emit_box_undefined(&self, builder: &mut FunctionBuilder<'_>) -> ir::Value {
        builder
            .ins()
            .iconst(types::I64, (QNAN | (TAG_UNDEFINED << TAG_SHIFT)) as i64)
    }

    /// Emit a NaN-boxed null constant.
    fn emit_box_null(&self, builder: &mut FunctionBuilder<'_>) -> ir::Value {
        builder
            .ins()
            .iconst(types::I64, (QNAN | (TAG_NULL << TAG_SHIFT)) as i64)
    }

    /// Emit NaN-boxed i32.
    fn emit_box_i32(&self, builder: &mut FunctionBuilder<'_>, val: ir::Value) -> ir::Value {
        let tag_bits = builder
            .ins()
            .iconst(types::I64, (QNAN | (TAG_INT << TAG_SHIFT)) as i64);
        let extended = builder.ins().uextend(types::I64, val);
        builder.ins().bor(tag_bits, extended)
    }

    /// Emit NaN-boxed f64.
    fn emit_box_f64(&self, builder: &mut FunctionBuilder<'_>, val: ir::Value) -> ir::Value {
        builder.ins().bitcast(types::I64, ir::MemFlags::new(), val)
    }

    /// Emit NaN-boxed bool.
    fn emit_box_bool(&self, builder: &mut FunctionBuilder<'_>, val: ir::Value) -> ir::Value {
        let tag_bits = builder
            .ins()
            .iconst(types::I64, (QNAN | (TAG_BOOL << TAG_SHIFT)) as i64);
        let extended = builder.ins().uextend(types::I64, val);
        builder.ins().bor(tag_bits, extended)
    }

    /// Ensure a value is i64 (NaN-boxed). If it's a narrower type, box it.
    fn ensure_nanboxed(&self, val: ir::Value, builder: &mut FunctionBuilder<'_>) -> ir::Value {
        let ty = builder.func.dfg.value_type(val);
        if ty == types::I64 {
            val
        } else if ty == types::I32 {
            self.emit_box_i32(builder, val)
        } else if ty == types::F64 {
            self.emit_box_f64(builder, val)
        } else if ty == types::I8 {
            self.emit_box_bool(builder, val)
        } else {
            builder.ins().uextend(types::I64, val)
        }
    }

    /// Get or declare a runtime function with signature (i64, i64) -> i64.
    fn get_rt_binary(&mut self, name: &str, module: &mut JITModule) -> Result<FuncId, EvalError> {
        if let Some(&id) = self.runtime_cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| EvalError::Jit {
                message: format!("declare runtime fn '{name}': {e}"),
            })?;
        self.runtime_cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a runtime function with signature (i64) -> i64.
    fn get_rt_unary(&mut self, name: &str, module: &mut JITModule) -> Result<FuncId, EvalError> {
        if let Some(&id) = self.runtime_cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| EvalError::Jit {
                message: format!("declare runtime fn '{name}': {e}"),
            })?;
        self.runtime_cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Emit a runtime binary call: result = rt_fn(lhs, rhs).
    fn emit_rt_binary(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let lhs = self.get_value(inst.operands[0])?;
        let rhs = self.get_value(inst.operands[1])?;
        let lhs = self.ensure_nanboxed(lhs, builder);
        let rhs = self.ensure_nanboxed(rhs, builder);
        let func_id = self.get_rt_binary(name, module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[lhs, rhs]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a runtime unary call: result = rt_fn(operand).
    fn emit_rt_unary(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let operand = self.get_value(inst.operands[0])?;
        let operand = self.ensure_nanboxed(operand, builder);
        let func_id = self.get_rt_unary(name, module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[operand]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Get or declare a runtime function with signature (i64, i64, i64) -> i64.
    fn get_rt_ternary(&mut self, name: &str, module: &mut JITModule) -> Result<FuncId, EvalError> {
        if let Some(&id) = self.runtime_cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| EvalError::Jit {
                message: format!("declare runtime fn '{name}': {e}"),
            })?;
        self.runtime_cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Lower a single instruction by dispatching to category-specific helpers.
    fn lower_instruction(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        match &inst.op {
            // Constants
            Op::ConstI32(_)
            | Op::ConstI64(_)
            | Op::ConstF64(_)
            | Op::ConstBool(_)
            | Op::ConstNull
            | Op::ConstUndefined
            | Op::ConstString(_) => self.lower_constant(inst, builder, module, string_table)?,

            // Typed i32 arithmetic
            Op::AddI32 | Op::SubI32 | Op::MulI32 | Op::DivI32 | Op::ModI32 | Op::NegI32 => {
                self.lower_i32_arithmetic(inst, builder)?;
            }

            // Typed f64 arithmetic
            Op::AddF64 | Op::SubF64 | Op::MulF64 | Op::DivF64 | Op::NegF64 => {
                self.lower_f64_arithmetic(inst, builder)?;
            }

            // JS arithmetic (runtime calls)
            Op::AddJS | Op::SubJS | Op::MulJS | Op::DivJS | Op::ModJS | Op::ExpJS | Op::NegJS => {
                self.lower_js_arithmetic(inst, builder, module)?;
            }

            // JS comparison
            Op::EqStrict | Op::NeStrict | Op::LtJS | Op::LeJS | Op::GtJS | Op::GeJS => {
                self.lower_comparison(inst, builder, module)?;
            }

            // Type conversions
            Op::ToNumber | Op::ToString | Op::ToBoolean => {
                self.lower_conversion(inst, builder, module)?;
            }

            // NaN-boxing
            Op::BoxI32
            | Op::BoxF64
            | Op::BoxBool
            | Op::BoxNull
            | Op::BoxUndefined
            | Op::BoxString
            | Op::BoxObject
            | Op::BoxSymbol => {
                self.lower_boxing(inst, builder)?;
            }

            // Control flow
            Op::Ret | Op::Br | Op::BrIf => {
                self.lower_control_flow(inst, builder)?;
            }

            // Function calls
            Op::Call | Op::CallRuntime => {
                self.lower_call(inst, builder, module, string_table)?;
            }

            // LoadParam
            Op::LoadParam(idx) => {
                let param_key = *idx | 0x8000_0000;
                if let Some(&val) = self.values.get(&param_key) {
                    self.values.insert(inst.id.0, val);
                } else {
                    let undef = self.emit_box_undefined(builder);
                    self.values.insert(inst.id.0, undef);
                }
            }

            // Phi (handled in the block-level loop)
            Op::Phi => {}

            // Unreachable
            Op::Unreachable => {
                builder.ins().trap(ir::TrapCode::unwrap_user(1));
            }

            // Dynamic environment ops (used by eval scope bridging and with statements)
            Op::EnvLookup => {
                self.lower_env_lookup(inst, builder, module)?;
            }
            Op::EnvLookupStore => {
                self.lower_env_lookup_store(inst, builder, module)?;
            }

            // Unsupported ops — emit undefined placeholder
            _ => {
                let undef = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
        }

        Ok(())
    }

    /// Lower `EnvLookup`: dynamic name-based lookup through an EscEnvironment chain.
    ///
    /// Operands: `(env, name_string)`. Calls `__esc_rt_esc_env_lookup(env, name)`.
    fn lower_env_lookup(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let env = self.get_value(inst.operands[0])?;
        let name = self.get_value(inst.operands[1])?;
        let env = self.ensure_nanboxed(env, builder);
        let name = self.ensure_nanboxed(name, builder);
        let func_id = self.get_rt_binary("__esc_rt_esc_env_lookup", module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[env, name]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Lower `EnvLookupStore`: dynamic name-based store through an EscEnvironment chain.
    ///
    /// Operands: `(env, name_string, value)`. Calls `__esc_rt_esc_env_store(env, name, val)`.
    fn lower_env_lookup_store(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let env = self.get_value(inst.operands[0])?;
        let name = self.get_value(inst.operands[1])?;
        let value = self.get_value(inst.operands[2])?;
        let env = self.ensure_nanboxed(env, builder);
        let name = self.ensure_nanboxed(name, builder);
        let value = self.ensure_nanboxed(value, builder);
        let func_id = self.get_rt_ternary("__esc_rt_esc_env_store", module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[env, name, value]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Lower constant opcodes: `ConstI32`, `ConstI64`, `ConstF64`, `ConstBool`,
    /// `ConstNull`, `ConstUndefined`, `ConstString`.
    fn lower_constant(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::ConstI32(val) => {
                let v = builder.ins().iconst(types::I32, i64::from(*val));
                self.values.insert(inst.id.0, v);
                self.const_i32_values.insert(inst.id.0, *val);
            }
            Op::ConstI64(val) => {
                let v = builder.ins().iconst(types::I64, *val);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstF64(val) => {
                let v = builder.ins().f64const(*val);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstBool(val) => {
                let v = builder.ins().iconst(types::I8, i64::from(*val));
                self.values.insert(inst.id.0, v);
            }
            Op::ConstNull => {
                let v = self.emit_box_null(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstUndefined => {
                let v = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::ConstString(idx) => {
                self.lower_const_string(inst, *idx, builder, module, string_table)?;
            }
            _ => unreachable!("lower_constant called with non-constant op"),
        }
        Ok(())
    }

    /// Lower a `ConstString` opcode: declare string data and emit a symbol reference.
    fn lower_const_string(
        &mut self,
        inst: &::ir::TypedInstruction,
        idx: u32,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        let s = string_table.get(idx as usize).cloned().unwrap_or_default();
        let data_id = module
            .declare_data(
                &format!("__esc_eval_str_{idx}"),
                Linkage::Local,
                false,
                false,
            )
            .map_err(|e| EvalError::Jit {
                message: format!("declare string data: {e}"),
            })?;

        let mut desc = cranelift_module::DataDescription::new();
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());
        module
            .define_data(data_id, &desc)
            .map_err(|e| EvalError::Jit {
                message: format!("define string data: {e}"),
            })?;

        let gv = module.declare_data_in_func(data_id, builder.func);
        let ptr_ty = module.target_config().pointer_type();
        let v = builder.ins().symbol_value(ptr_ty, gv);
        self.values.insert(inst.id.0, v);
        self.const_string_indices.insert(inst.id.0, idx);
        Ok(())
    }

    /// Lower typed i32 arithmetic: `AddI32`, `SubI32`, `MulI32`, `DivI32`,
    /// `ModI32`, `NegI32`.
    fn lower_i32_arithmetic(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::AddI32 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().iadd(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::SubI32 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().isub(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::MulI32 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().imul(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::DivI32 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().sdiv(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::ModI32 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().srem(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::NegI32 => {
                let operand = self.get_value(inst.operands[0])?;
                let zero = builder.ins().iconst(types::I32, 0);
                let v = builder.ins().isub(zero, operand);
                self.values.insert(inst.id.0, v);
            }
            _ => unreachable!("lower_i32_arithmetic called with non-i32 op"),
        }
        Ok(())
    }

    /// Lower typed f64 arithmetic: `AddF64`, `SubF64`, `MulF64`, `DivF64`, `NegF64`.
    fn lower_f64_arithmetic(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::AddF64 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().fadd(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::SubF64 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().fsub(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::MulF64 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().fmul(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::DivF64 => {
                let lhs = self.get_value(inst.operands[0])?;
                let rhs = self.get_value(inst.operands[1])?;
                let v = builder.ins().fdiv(lhs, rhs);
                self.values.insert(inst.id.0, v);
            }
            Op::NegF64 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = builder.ins().fneg(operand);
                self.values.insert(inst.id.0, v);
            }
            _ => unreachable!("lower_f64_arithmetic called with non-f64 op"),
        }
        Ok(())
    }

    /// Lower JS arithmetic runtime calls: `AddJS`, `SubJS`, `MulJS`, `DivJS`,
    /// `ModJS`, `ExpJS`, `NegJS`.
    fn lower_js_arithmetic(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::AddJS => self.emit_rt_binary(inst, "__esc_rt_add_js", builder, module),
            Op::SubJS => self.emit_rt_binary(inst, "__esc_rt_sub_js", builder, module),
            Op::MulJS => self.emit_rt_binary(inst, "__esc_rt_mul_js", builder, module),
            Op::DivJS => self.emit_rt_binary(inst, "__esc_rt_div_js", builder, module),
            Op::ModJS => self.emit_rt_binary(inst, "__esc_rt_mod_js", builder, module),
            Op::ExpJS => self.emit_rt_binary(inst, "__esc_rt_exp_js", builder, module),
            Op::NegJS => self.emit_rt_unary(inst, "__esc_rt_neg_js", builder, module),
            _ => unreachable!("lower_js_arithmetic called with non-JS-arithmetic op"),
        }
    }

    /// Lower JS comparison runtime calls: `EqStrict`, `NeStrict`, `LtJS`,
    /// `LeJS`, `GtJS`, `GeJS`.
    fn lower_comparison(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::EqStrict => self.emit_rt_binary(inst, "__esc_rt_eq_strict", builder, module),
            Op::NeStrict => self.emit_rt_binary(inst, "__esc_rt_ne_strict", builder, module),
            Op::LtJS => self.emit_rt_binary(inst, "__esc_rt_lt_js", builder, module),
            Op::LeJS => self.emit_rt_binary(inst, "__esc_rt_le_js", builder, module),
            Op::GtJS => self.emit_rt_binary(inst, "__esc_rt_gt_js", builder, module),
            Op::GeJS => self.emit_rt_binary(inst, "__esc_rt_ge_js", builder, module),
            _ => unreachable!("lower_comparison called with non-comparison op"),
        }
    }

    /// Lower type conversion opcodes: `ToNumber`, `ToString`, `ToBoolean`.
    fn lower_conversion(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::ToNumber => self.emit_rt_unary(inst, "__esc_rt_to_number", builder, module),
            Op::ToString => self.emit_rt_unary(inst, "__esc_rt_to_string", builder, module),
            Op::ToBoolean => self.lower_to_boolean(inst, builder, module),
            _ => unreachable!("lower_conversion called with non-conversion op"),
        }
    }

    /// Lower `ToBoolean` — returns i8 instead of the standard i64.
    fn lower_to_boolean(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let operand = self.get_value(inst.operands[0])?;
        let operand = self.ensure_nanboxed(operand, builder);
        // to_boolean returns i8
        let name = "__esc_rt_to_boolean";
        if let Some(&id) = self.runtime_cache.get(name) {
            let func_ref = module.declare_func_in_func(id, builder.func);
            let call = builder.ins().call(func_ref, &[operand]);
            let v = builder.inst_results(call)[0];
            self.values.insert(inst.id.0, v);
        } else {
            let mut sig = Signature::new(self.call_conv);
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I8));
            let id = module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| EvalError::Jit {
                    message: format!("declare runtime fn '{name}': {e}"),
                })?;
            self.runtime_cache.insert(name.to_owned(), id);
            let func_ref = module.declare_func_in_func(id, builder.func);
            let call = builder.ins().call(func_ref, &[operand]);
            let v = builder.inst_results(call)[0];
            self.values.insert(inst.id.0, v);
        }
        Ok(())
    }

    /// Lower NaN-boxing opcodes: `BoxI32`, `BoxF64`, `BoxBool`, `BoxNull`,
    /// `BoxUndefined`, `BoxString`, `BoxObject`, `BoxSymbol`.
    fn lower_boxing(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::BoxI32 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = self.emit_box_i32(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxF64 => {
                let operand = self.get_value(inst.operands[0])?;
                let v = self.emit_box_f64(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxBool => {
                let operand = self.get_value(inst.operands[0])?;
                let v = self.emit_box_bool(builder, operand);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxNull => {
                let v = self.emit_box_null(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxUndefined => {
                let v = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, v);
            }
            Op::BoxString | Op::BoxObject | Op::BoxSymbol => {
                // These types are already pointer-sized (i64), just pass through
                if let Ok(operand) = self.get_value(inst.operands[0]) {
                    self.values.insert(inst.id.0, operand);
                } else {
                    let undef = self.emit_box_undefined(builder);
                    self.values.insert(inst.id.0, undef);
                }
            }
            _ => unreachable!("lower_boxing called with non-boxing op"),
        }
        Ok(())
    }

    /// Lower control flow opcodes: `Ret`, `Br`, `BrIf`.
    fn lower_control_flow(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::Ret => {
                if inst.operands.is_empty() {
                    builder.ins().return_(&[]);
                } else {
                    let val = self.get_value(inst.operands[0])?;
                    let sig_ret = builder.func.signature.returns.first().map(|r| r.value_type);
                    let ret_val = if sig_ret == Some(types::I64) {
                        self.ensure_nanboxed(val, builder)
                    } else {
                        val
                    };
                    builder.ins().return_(&[ret_val]);
                }
            }
            Op::Br => {
                // Unconditional jump. Target is in block_targets[0].
                let target = self.get_block(inst.block_targets[0])?;
                builder.ins().jump(target, &[]);
            }
            Op::BrIf => {
                self.lower_br_if(inst, builder)?;
            }
            _ => unreachable!("lower_control_flow called with non-control-flow op"),
        }
        Ok(())
    }

    /// Lower a conditional branch (`BrIf`).
    fn lower_br_if(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), EvalError> {
        // Conditional branch. Operands[0] = condition.
        // block_targets[0] = then, block_targets[1] = else.
        let cond = self.get_value(inst.operands[0])?;
        let then_block = self.get_block(inst.block_targets[0])?;
        let else_block = self.get_block(inst.block_targets[1])?;

        let cond_ty = builder.func.dfg.value_type(cond);
        let cond_val = if cond_ty == types::I8 {
            cond
        } else if cond_ty == types::I32 {
            let zero = builder.ins().iconst(types::I32, 0);
            builder
                .ins()
                .icmp(ir::condcodes::IntCC::NotEqual, cond, zero)
        } else {
            let zero = builder.ins().iconst(types::I64, 0);
            builder
                .ins()
                .icmp(ir::condcodes::IntCC::NotEqual, cond, zero)
        };

        builder
            .ins()
            .brif(cond_val, then_block, &[], else_block, &[]);
        Ok(())
    }

    /// Lower function call opcodes: `Call`, `CallRuntime`.
    fn lower_call(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        match &inst.op {
            Op::Call => self.lower_call_direct(inst, builder, module),
            Op::CallRuntime => self.lower_call_runtime(inst, builder, module, string_table),
            _ => unreachable!("lower_call called with non-call op"),
        }
    }

    /// Lower a direct function `Call`.
    fn lower_call_direct(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        // operands[0] = function index (ConstI32), rest = args
        if let Some(&func_idx) = self.const_i32_values.get(&inst.operands[0].0) {
            let idx = func_idx as usize;
            if idx < self.func_ids.len() {
                let func_id = self.func_ids[idx];
                let mut args = Vec::with_capacity(inst.operands.len() - 1);
                for &op in &inst.operands[1..] {
                    let v = self.get_value(op)?;
                    args.push(self.ensure_nanboxed(v, builder));
                }
                let func_ref = module.declare_func_in_func(func_id, builder.func);
                let call = builder.ins().call(func_ref, &args);
                let results = builder.inst_results(call);
                if !results.is_empty() {
                    self.values.insert(inst.id.0, results[0]);
                } else {
                    let undef = self.emit_box_undefined(builder);
                    self.values.insert(inst.id.0, undef);
                }
            } else {
                let undef = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
        } else {
            // Indirect call — return undefined for now
            let undef = self.emit_box_undefined(builder);
            self.values.insert(inst.id.0, undef);
        }
        Ok(())
    }

    /// Lower a `CallRuntime` opcode.
    fn lower_call_runtime(
        &mut self,
        inst: &::ir::TypedInstruction,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
        string_table: &[String],
    ) -> Result<(), EvalError> {
        // operands[0] = ConstString index for function name, rest = args
        let str_idx = self.const_string_indices.get(&inst.operands[0].0).copied();
        if let Some(idx) = str_idx {
            let fn_name = string_table.get(idx as usize).cloned().unwrap_or_default();

            if fn_name.starts_with("__esc_rt_console_") {
                // Console functions: (argc: i32, argv_ptr: i64) -> void
                // For JIT stub, just emit undefined result
                let undef = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            } else if inst.operands.len() > 3 {
                // 3+ args after the name: ternary call
                self.emit_rt_ternary_raw(
                    inst,
                    &fn_name,
                    inst.operands[1],
                    inst.operands[2],
                    inst.operands[3],
                    builder,
                    module,
                )?;
            } else if inst.operands.len() > 2 {
                // 2 args after the name: binary call
                self.emit_rt_binary_raw(
                    inst,
                    &fn_name,
                    inst.operands[1],
                    inst.operands[2],
                    builder,
                    module,
                )?;
            } else if inst.operands.len() == 2 {
                // 1 arg after the name: unary call
                self.emit_rt_unary_raw(inst, &fn_name, inst.operands[1], builder, module)?;
            } else {
                let undef = self.emit_box_undefined(builder);
                self.values.insert(inst.id.0, undef);
            }
        } else {
            let undef = self.emit_box_undefined(builder);
            self.values.insert(inst.id.0, undef);
        }
        Ok(())
    }

    /// Emit a runtime binary call with explicit operand ValueIds.
    fn emit_rt_binary_raw(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        lhs_id: ValueId,
        rhs_id: ValueId,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let lhs = self.get_value(lhs_id)?;
        let rhs = self.get_value(rhs_id)?;
        let lhs = self.ensure_nanboxed(lhs, builder);
        let rhs = self.ensure_nanboxed(rhs, builder);
        let func_id = self.get_rt_binary(name, module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[lhs, rhs]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a runtime ternary call with explicit operand ValueIds.
    // Mirrors emit_rt_binary_raw which also takes inst+name+operands+builder+module.
    // Splitting further would obscure the simple delegation pattern.
    #[allow(clippy::too_many_arguments)]
    fn emit_rt_ternary_raw(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        a_id: ValueId,
        b_id: ValueId,
        c_id: ValueId,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let a = self.get_value(a_id)?;
        let b = self.get_value(b_id)?;
        let c = self.get_value(c_id)?;
        let a = self.ensure_nanboxed(a, builder);
        let b = self.ensure_nanboxed(b, builder);
        let c = self.ensure_nanboxed(c, builder);
        let func_id = self.get_rt_ternary(name, module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[a, b, c]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }

    /// Emit a runtime unary call with explicit operand ValueId.
    fn emit_rt_unary_raw(
        &mut self,
        inst: &::ir::TypedInstruction,
        name: &str,
        op_id: ValueId,
        builder: &mut FunctionBuilder<'_>,
        module: &mut JITModule,
    ) -> Result<(), EvalError> {
        let operand = self.get_value(op_id)?;
        let operand = self.ensure_nanboxed(operand, builder);
        let func_id = self.get_rt_unary(name, module)?;
        let func_ref = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(func_ref, &[operand]);
        let v = builder.inst_results(call)[0];
        self.values.insert(inst.id.0, v);
        Ok(())
    }
}

// ============================================================================
// Eval scope bridging helpers
// ============================================================================

/// Collect `var` declaration names from a JavaScript source string.
///
/// Parses the source with oxc and walks the AST to find top-level `var`
/// declarations. Returns the list of variable names. These need to be leaked
/// to the caller's `VariableEnvironment` in sloppy mode.
///
/// Only collects top-level `var` declarations — not `let`/`const` (which are
/// confined to the eval's own scope) and not `var` declarations nested inside
/// functions (which are scoped to those functions).
pub(crate) fn collect_var_declarations(source: &str) -> Vec<String> {
    use oxc_ast::ast::{Statement, VariableDeclarationKind};

    let result = parser::parse_with(source, oxc_span::SourceType::cjs(), |program| {
        let mut var_names: Vec<String> = Vec::new();
        for stmt in &program.body {
            match stmt {
                Statement::VariableDeclaration(decl)
                    if decl.kind == VariableDeclarationKind::Var =>
                {
                    for declarator in &decl.declarations {
                        if let Some(name) = declarator.id.get_identifier_name() {
                            var_names.push(name.to_string());
                        }
                    }
                }
                // Function declarations also leak in sloppy eval
                Statement::FunctionDeclaration(func) => {
                    if let Some(id) = &func.id {
                        var_names.push(id.name.to_string());
                    }
                }
                _ => {}
            }
        }
        var_names
    });

    result.unwrap_or_default()
}

/// Leak a `var` declaration from eval to the caller's `VariableEnvironment`.
///
/// In sloppy mode, `var` declarations inside `eval()` are added to the
/// enclosing function's `VariableEnvironment` (or the global scope). This
/// function calls `__esc_rt_esc_env_add_binding` to create the binding.
fn leak_var_to_env(var_env: u64, name: &str) {
    use nanbox::JsValue;

    // Skip if no var_env is provided
    if var_env == 0 || var_env == JsValue::undefined().raw_bits() {
        return;
    }

    // Create a NaN-boxed runtime string for the variable name
    let rt_str = Box::new(runtime::string_ops::RtString::new(name.to_string()));
    let ptr = Box::into_raw(rt_str) as *const ();
    let name_bits = JsValue::string(ptr).raw_bits();

    // Add the binding with undefined as initial value
    runtime::rt_api::__esc_rt_esc_env_add_binding(
        var_env,
        name_bits,
        JsValue::undefined().raw_bits(),
    );
}

/// The callback function registered with the runtime for direct eval.
///
/// This is the implementation that `__esc_rt_call_eval_direct` calls through.
/// It creates a fresh [`JitEval`] context and delegates to [`JitEval::eval_direct`].
fn eval_direct_callback(
    code: &str,
    lex_env: u64,
    var_env: u64,
    this_value: u64,
    is_strict: bool,
) -> u64 {
    use nanbox::JsValue;

    let mut jit = match JitEval::new() {
        Ok(jit) => jit,
        Err(_) => return JsValue::undefined().raw_bits(),
    };

    match jit.eval_direct(code, lex_env, var_env, this_value, is_strict) {
        Ok(result) => result,
        Err(_) => JsValue::undefined().raw_bits(),
    }
}

/// Global registry of JIT-compiled function pointers.
///
/// Maps a unique function ID to the raw function pointer and arity. When a
/// JIT-constructed function is called, the trampoline looks up the pointer
/// here and invokes it with the caller's arguments.
static JIT_FUNC_REGISTRY: std::sync::Mutex<Vec<JitFuncEntry>> = std::sync::Mutex::new(Vec::new());

/// Entry in the JIT function registry.
struct JitFuncEntry {
    /// Raw function pointer (from Cranelift JIT compilation).
    ptr: *const u8,
    /// Number of formal parameters.
    param_count: u32,
    /// The JitEval context that owns the compiled code. We must keep it alive
    /// so the function pointer remains valid (JIT memory is owned by JITModule).
    _jit: Box<JitEval>,
}

// SAFETY: The JitEval/function pointer are only accessed from the single JS
// thread (single-threaded runtime). The Mutex is for safe registration only.
unsafe impl Send for JitFuncEntry {}
unsafe impl Sync for JitFuncEntry {}

/// Trampoline for calling a JIT-constructed function.
///
/// `context` is the index into `JIT_FUNC_REGISTRY`. The trampoline reads
/// the function pointer, collects the caller's arguments from the thread-local
/// `CURRENT_ARGC`/`CURRENT_ARGV`, and calls the JIT'd function.
fn jit_func_trampoline(context: u64) -> u64 {
    use nanbox::JsValue;

    let idx = context as usize;
    let registry = match JIT_FUNC_REGISTRY.lock() {
        Ok(guard) => guard,
        Err(_) => return JsValue::undefined().raw_bits(),
    };

    let Some(entry) = registry.get(idx) else {
        return JsValue::undefined().raw_bits();
    };

    let ptr = entry.ptr;
    let param_count = entry.param_count;

    // Read the arguments from the thread-local CURRENT_ARGC/CURRENT_ARGV
    let argc = runtime::rt_api::CURRENT_ARGC.with(|cell| cell.get());
    let argv = runtime::rt_api::CURRENT_ARGV.with(|cell| cell.get());

    // Collect the arguments, padding with undefined if needed
    let mut args: Vec<u64> = Vec::with_capacity(param_count as usize);
    for i in 0..param_count as usize {
        if i < argc as usize && !argv.is_null() {
            // SAFETY: argv has at least argc elements, and i < argc.
            args.push(unsafe { *argv.add(i) });
        } else {
            args.push(JsValue::undefined().raw_bits());
        }
    }

    // Drop the registry guard before calling JIT'd code to avoid deadlock
    drop(registry);

    // Call the JIT'd function with the collected arguments
    // SAFETY: ptr was compiled by Cranelift with the matching signature.
    // Each parameter is a NaN-boxed u64 (i64), and the return is i64.
    unsafe { call_jit_func_ptr(ptr, &args) }
}

/// Call a JIT'd function pointer with the given arguments.
///
/// The function was compiled by Cranelift with a signature of
/// `(i64, i64, ...) -> i64` where each i64 is a NaN-boxed JsValue.
///
/// # Safety
///
/// `ptr` must be a valid function pointer compiled by Cranelift with
/// a signature matching the number of arguments in `args`.
unsafe fn call_jit_func_ptr(ptr: *const u8, args: &[u64]) -> u64 {
    // Dispatch based on argument count
    match args.len() {
        0 => {
            let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(ptr) };
            f()
        }
        1 => {
            let f: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            f(args[0])
        }
        2 => {
            let f: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            f(args[0], args[1])
        }
        3 => {
            let f: extern "C" fn(u64, u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            f(args[0], args[1], args[2])
        }
        4 => {
            let f: extern "C" fn(u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(ptr) };
            f(args[0], args[1], args[2], args[3])
        }
        5 => {
            let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            f(args[0], args[1], args[2], args[3], args[4])
        }
        _ => {
            // For > 5 args, call with just the first 5 (limitation)
            let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                unsafe { std::mem::transmute(ptr) };
            f(args[0], args[1], args[2], args[3], args[4])
        }
    }
}

/// The callback function registered with the runtime for the `Function()` constructor.
///
/// Creates a function from parameter names and a body string by:
/// 1. Constructing `function anonymous(params) { body }`
/// 2. Parsing, lowering, and JIT-compiling it
/// 3. Creating a `NativeFunc` wrapper that invokes the JIT'd code
///
/// The created function has global scope (not the caller's scope) per ES spec.
fn function_constructor_callback(params: &[&str], body: &str) -> Result<u64, String> {
    use nanbox::JsValue;

    // Construct the full function source
    let params_str = params.join(", ");
    let source = format!("function anonymous({params_str}) {{ {body} }}");

    // Parse to validate syntax
    let parse_result = parser::parse_with(&source, oxc_span::SourceType::cjs(), |_program| {
        // Parsing succeeded
    });
    if let Err(errors) = parse_result {
        let msg = errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }

    // Lower to IR
    let lowering_result =
        desugar::lower_source(&source, oxc_span::SourceType::cjs()).map_err(|errors| {
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })?;

    let ir_module = &lowering_result.module;
    let string_table = &lowering_result.string_table;

    // Find the `anonymous` function (should be the non-entry function)
    let anon_idx = ir_module
        .functions
        .iter()
        .position(|f| f.name == "anonymous")
        .ok_or_else(|| "no anonymous function found in lowered module".to_string())?;

    // Create a JIT context and compile all functions
    let mut jit = JitEval::new().map_err(|e| e.to_string())?;
    let func_ids = jit
        .declare_functions(ir_module)
        .map_err(|e| e.to_string())?;

    let mut fb_ctx = FunctionBuilderContext::new();
    for (i, func) in ir_module.functions.iter().enumerate() {
        jit.compile_function(func, func_ids[i], &func_ids, string_table, &mut fb_ctx)
            .map_err(|e| e.to_string())?;
    }

    jit.module
        .finalize_definitions()
        .map_err(|e| format!("finalize error: {e}"))?;

    // Get the compiled function pointer for `anonymous`
    let func_ptr = jit.module.get_finalized_function(func_ids[anon_idx]);

    // Register the JIT context and function pointer in the global registry
    let registry_idx = {
        let mut registry = JIT_FUNC_REGISTRY
            .lock()
            .map_err(|_| "JIT function registry lock poisoned".to_string())?;
        let idx = registry.len();
        registry.push(JitFuncEntry {
            ptr: func_ptr,
            param_count: params.len() as u32,
            _jit: Box::new(jit),
        });
        idx
    };

    // Create a NativeFunc that calls the trampoline with the registry index
    use runtime::internal_data::UnifiedObject;
    use runtime::tagged_obj::{ObjTag, TaggedObj};

    let func_obj = TaggedObj::boxed(
        ObjTag::Unified,
        UnifiedObject::native_func(jit_func_trampoline, registry_idx as u64),
    );

    // Set name = "anonymous" on the function object
    let name_bits = runtime::rt_api::make_rt_string("anonymous".to_string());
    let name_key = runtime::rt_api::make_rt_string("name".to_string());
    runtime::rt_api::__esc_rt_set_prop(func_obj, name_key, name_bits);

    // Set length = param count
    let length_bits = JsValue::int(params.len() as i32).raw_bits();
    let length_key = runtime::rt_api::make_rt_string("length".to_string());
    runtime::rt_api::__esc_rt_set_prop(func_obj, length_key, length_bits);

    // Set .prototype property (non-arrow function)
    let proto = runtime::rt_api::__esc_rt_create_object();
    let ctor_key = runtime::rt_api::make_rt_string("constructor".to_string());
    runtime::rt_api::__esc_rt_set_prop(proto, ctor_key, func_obj);
    let proto_key = runtime::rt_api::make_rt_string("prototype".to_string());
    runtime::rt_api::__esc_rt_set_prop(func_obj, proto_key, proto);

    Ok(func_obj)
}

/// Register the eval direct callback with the runtime.
///
/// Must be called during initialization (before any eval'd code runs) to wire
/// up the self-hosted eval pipeline. Without this registration, direct eval
/// returns `undefined`.
pub fn register_eval_runtime() {
    runtime::rt_api::register_eval_direct(eval_direct_callback);
    runtime::rt_api::register_function_constructor(function_constructor_callback);
}
