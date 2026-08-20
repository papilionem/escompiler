//! External function declarations for `__esc_rt_*` runtime helpers.
//!
//! These are C ABI functions provided by `runtime` that handle dynamic
//! JS operations (e.g., `AddJS`, `ToNumber`, `StringConcat`). Each helper
//! is declared as an external function in the Cranelift [`ObjectModule`] and
//! resolved at link time.

use std::collections::HashMap;

use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, Signature};
use cranelift_codegen::isa::CallConv;
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

use crate::error::CodegenError;

/// Manages declarations of external runtime helper functions.
///
/// Lazily declares each helper the first time it is requested and caches the
/// resulting [`FuncId`] for subsequent uses within the same module.
pub struct RuntimeCalls {
    cache: HashMap<String, FuncId>,
    call_conv: CallConv,
}

impl Default for RuntimeCalls {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeCalls {
    /// Create a new runtime call manager.
    pub fn new() -> Self {
        let call_conv = if cfg!(target_os = "windows") {
            CallConv::WindowsFastcall
        } else {
            CallConv::SystemV
        };
        Self {
            cache: HashMap::new(),
            call_conv,
        }
    }

    /// Get or declare a binary JS arithmetic runtime helper: `(i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_add_js`, `__esc_rt_sub_js`, etc.
    pub fn get_binary_js_op(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a unary JS runtime helper: `(i64) -> i64`.
    ///
    /// Covers `__esc_rt_neg_js`, `__esc_rt_to_number`, etc.
    pub fn get_unary_js_op(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare `__esc_rt_to_boolean`: `(i64) -> i8`.
    pub fn get_to_boolean(&mut self, module: &mut ObjectModule) -> Result<FuncId, CodegenError> {
        let name = "__esc_rt_to_boolean";
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I8));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a string operation that takes two i64 args and returns i64.
    ///
    /// Covers `__esc_rt_string_concat`.
    pub fn get_string_binary_op(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        self.get_binary_js_op(name, module)
    }

    /// Get or declare `__esc_rt_string_length`: `(i64) -> i32`.
    pub fn get_string_length(&mut self, module: &mut ObjectModule) -> Result<FuncId, CodegenError> {
        let name = "__esc_rt_string_length";
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Declare a console function: `(i32, i64) -> void` (argc, argv_ptr).
    ///
    /// Used for `__esc_rt_console_log`, `__esc_rt_console_warn`, etc.
    pub fn get_console_fn(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I32)); // argc
        sig.params.push(AbiParam::new(types::I64)); // argv_ptr
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Declare a void→void function (for `__esc_rt_init`, `__esc_rt_shutdown`).
    pub fn get_void_void(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let sig = Signature::new(self.call_conv);
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Declare a void→i32 function (for `__esc_rt_has_pending_exception`).
    pub fn get_void_i32(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.returns.push(AbiParam::new(types::I32));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Declare a void→i64 function (for calling `__esc_main` which returns JSValue).
    pub fn get_void_i64(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a ternary runtime helper: `(i64, i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_set_prop`, `__esc_rt_set_elem`, `__esc_rt_object_define_property`, etc.
    pub fn get_ternary_js_op(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a quaternary runtime helper: `(i64, i64, i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_define_accessor` and other 4-arg runtime calls.
    pub fn get_quaternary_js_op(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a void-returning unary helper: `(i64) -> void`.
    ///
    /// Covers `__esc_rt_throw`, `__esc_rt_iter_close`, etc.
    pub fn get_void_unary(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a void-returning binary helper: `(i64, i64) -> void`.
    ///
    /// Covers `__esc_rt_promise_resolve`, `__esc_rt_promise_reject`, etc.
    pub fn get_void_binary(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a void-returning ternary helper: `(i64, i64, i64) -> void`.
    ///
    /// Covers `__esc_rt_set_prop`, `__esc_rt_set_elem`, etc (void-returning variants).
    pub fn get_void_ternary(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare `__esc_rt_ic_get_prop`: `(i64, i64, i32) -> i64`.
    ///
    /// Inline-cached property get: obj, key, ic_id → value.
    pub fn get_ic_get_prop(&mut self, module: &mut ObjectModule) -> Result<FuncId, CodegenError> {
        let name = "__esc_rt_ic_get_prop";
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // obj
        sig.params.push(AbiParam::new(types::I64)); // key
        sig.params.push(AbiParam::new(types::I32)); // ic_id
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare `__esc_rt_ic_set_prop`: `(i64, i64, i64, i32) -> void`.
    ///
    /// Inline-cached property set: obj, key, val, ic_id → void.
    pub fn get_ic_set_prop(&mut self, module: &mut ObjectModule) -> Result<FuncId, CodegenError> {
        let name = "__esc_rt_ic_set_prop";
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // obj
        sig.params.push(AbiParam::new(types::I64)); // key
        sig.params.push(AbiParam::new(types::I64)); // val
        sig.params.push(AbiParam::new(types::I32)); // ic_id
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a variadic call helper: `(i64, i32, i64) -> i64`.
    ///
    /// Used for `__esc_rt_call_new`, `__esc_rt_call_method` — callee/obj, argc, argv_ptr.
    pub fn get_call_variadic(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // callee or obj
        sig.params.push(AbiParam::new(types::I32)); // argc
        sig.params.push(AbiParam::new(types::I64)); // argv_ptr
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a method call helper: `(i64, i64, i32, i64) -> i64`.
    ///
    /// Used for `__esc_rt_call_method` — obj, key, argc, argv_ptr.
    pub fn get_call_method(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // obj
        sig.params.push(AbiParam::new(types::I64)); // key
        sig.params.push(AbiParam::new(types::I32)); // argc
        sig.params.push(AbiParam::new(types::I64)); // argv_ptr
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare an env_store helper: `(i64, i32, i32, i64) -> void`.
    ///
    /// Used for `__esc_rt_env_store(env, depth, slot, val)`.
    pub fn get_env_store(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // env
        sig.params.push(AbiParam::new(types::I32)); // depth
        sig.params.push(AbiParam::new(types::I32)); // slot
        sig.params.push(AbiParam::new(types::I64)); // val
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare an env_create helper: `(i64, i32) -> i64`.
    ///
    /// Used for `__esc_rt_env_create(parent, slot_count)`.
    pub fn get_env_create(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // parent
        sig.params.push(AbiParam::new(types::I32)); // slot_count
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare an env_load helper: `(i64, i32, i32) -> i64`.
    ///
    /// Used for `__esc_rt_env_load(env, depth, slot)`.
    pub fn get_env_load(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I64)); // env
        sig.params.push(AbiParam::new(types::I32)); // depth
        sig.params.push(AbiParam::new(types::I32)); // slot
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a create_closure helper: `(i32, i64, i32) -> i64`.
    ///
    /// Used for `__esc_rt_create_closure(func_idx, env, flags)`.
    pub fn get_create_closure(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I32)); // func_idx
        sig.params.push(AbiParam::new(types::I64)); // env
        sig.params.push(AbiParam::new(types::I32)); // flags
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a `(i32, i64) -> i64` helper.
    ///
    /// Used for `__esc_rt_create_object_literal(count, kvpairs_ptr)`.
    pub fn get_i32_i64_to_i64(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I32)); // count
        sig.params.push(AbiParam::new(types::I64)); // ptr
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Get or declare a create_array helper: `(i32) -> i64`.
    ///
    /// Used for `__esc_rt_create_array(len)`.
    pub fn get_create_array(
        &mut self,
        name: &str,
        module: &mut ObjectModule,
    ) -> Result<FuncId, CodegenError> {
        if let Some(&id) = self.cache.get(name) {
            return Ok(id);
        }
        let mut sig = Signature::new(self.call_conv);
        sig.params.push(AbiParam::new(types::I32)); // len
        sig.returns.push(AbiParam::new(types::I64));
        let id = module.declare_function(name, Linkage::Import, &sig)?;
        self.cache.insert(name.to_owned(), id);
        Ok(id)
    }
}
