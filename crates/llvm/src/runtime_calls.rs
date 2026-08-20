//! External function declarations for `__esc_rt_*` runtime helpers.
//!
//! These are C ABI functions provided by `runtime` that handle dynamic
//! JS operations (e.g., `AddJS`, `ToNumber`, `StringConcat`). Each helper
//! is declared as an external function in the LLVM module and resolved at
//! link time.

use std::collections::HashMap;

use inkwell::module::Module;
use inkwell::values::FunctionValue;

/// Manages declarations of external runtime helper functions in the LLVM module.
///
/// Lazily declares each helper the first time it is requested and caches the
/// resulting [`FunctionValue`] for subsequent uses within the same module.
pub struct RuntimeCalls<'ctx> {
    cache: HashMap<String, FunctionValue<'ctx>>,
}

impl<'ctx> Default for RuntimeCalls<'ctx> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'ctx> RuntimeCalls<'ctx> {
    /// Create a new runtime call manager.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get or declare a binary JS arithmetic runtime helper: `(i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_add_js`, `__esc_rt_sub_js`, etc.
    pub fn get_binary_js_op(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a unary JS runtime helper: `(i64) -> i64`.
    ///
    /// Covers `__esc_rt_neg_js`, `__esc_rt_to_number`, etc.
    pub fn get_unary_js_op(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare `__esc_rt_to_boolean`: `(i64) -> i1`.
    pub fn get_to_boolean(&mut self, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        let name = "__esc_rt_to_boolean";
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.bool_type().fn_type(&[ctx.i64_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Declare a console function: `(i32, i64) -> void` (argc, argv_ptr).
    ///
    /// Used for `__esc_rt_console_log`, `__esc_rt_console_warn`, etc.
    pub fn get_console_fn(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx
            .void_type()
            .fn_type(&[ctx.i32_type().into(), ctx.i64_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Declare a void->void function (for `__esc_rt_init`, `__esc_rt_shutdown`).
    pub fn get_void_void(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let fn_ty = module.get_context().void_type().fn_type(&[], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Declare a void->i32 function (for `__esc_rt_has_pending_exception`).
    pub fn get_void_i32(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let fn_ty = module.get_context().i32_type().fn_type(&[], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a ternary runtime helper: `(i64, i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_set_prop`, `__esc_rt_set_elem`, etc.
    pub fn get_ternary_js_op(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a quaternary runtime helper: `(i64, i64, i64, i64) -> i64`.
    ///
    /// Covers `__esc_rt_define_accessor` and other 4-arg runtime calls.
    pub fn get_quaternary_js_op(
        &mut self,
        name: &str,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare `__esc_rt_ic_get_prop`: `(i64, i64, i32) -> i64`.
    ///
    /// Inline-cached property get: obj, key, ic_id -> value.
    pub fn get_ic_get_prop(&mut self, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        let name = "__esc_rt_ic_get_prop";
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i64_type().into(),
                ctx.i32_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare `__esc_rt_ic_set_prop`: `(i64, i64, i64, i32) -> void`.
    ///
    /// Inline-cached property set: obj, key, val, ic_id -> void.
    pub fn get_ic_set_prop(&mut self, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        let name = "__esc_rt_ic_set_prop";
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.void_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i64_type().into(),
                ctx.i64_type().into(),
                ctx.i32_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a variadic call helper: `(i64, i32, i64) -> i64`.
    ///
    /// Used for `__esc_rt_call_indirect` -- callee, argc, argv_ptr.
    pub fn get_call_variadic(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i32_type().into(),
                ctx.i64_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a method call helper: `(i64, i64, i32, i64) -> i64`.
    ///
    /// Used for `__esc_rt_call_method` -- obj, key, argc, argv_ptr.
    pub fn get_call_method(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i64_type().into(),
                ctx.i32_type().into(),
                ctx.i64_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a `(i32, i64) -> i64` helper.
    ///
    /// Used for `__esc_rt_create_object_literal(count, kvpairs_ptr)`.
    pub fn get_i32_i64_to_i64(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx
            .i64_type()
            .fn_type(&[ctx.i32_type().into(), ctx.i64_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a create_object helper: `() -> i64`.
    pub fn get_create_object(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(&[], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a create_array helper: `(i32) -> i64`.
    pub fn get_create_array(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(&[ctx.i32_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a void-returning unary helper: `(i64) -> void`.
    ///
    /// Covers `__esc_rt_throw`, etc.
    pub fn get_void_unary(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.void_type().fn_type(&[ctx.i64_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a create_closure helper: `(i32, i64, i32) -> i64`.
    ///
    /// Used for `__esc_rt_create_closure(func_idx, env, flags)`.
    pub fn get_create_closure(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.i32_type().into(),
                ctx.i64_type().into(),
                ctx.i32_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a void→i64 function (for 0-arg runtime calls returning a value).
    ///
    /// Used for `__esc_rt_get_global_this`, `__esc_rt_get_exception`, etc.
    pub fn get_void_i64(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let i64_ty = module.get_context().i64_type();
        let fn_ty = i64_ty.fn_type(&[], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a void-returning binary helper: `(i64, i64) -> void`.
    ///
    /// Covers `__esc_rt_box_store`, `__esc_rt_object_set_prototype`, etc.
    pub fn get_void_binary(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx
            .void_type()
            .fn_type(&[ctx.i64_type().into(), ctx.i64_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare an env_create helper: `(i64, i32) -> i64`.
    ///
    /// Used for `__esc_rt_env_create(parent, slot_count)`.
    pub fn get_env_create(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx
            .i64_type()
            .fn_type(&[ctx.i64_type().into(), ctx.i32_type().into()], false);
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare an env_load helper: `(i64, i32, i32) -> i64`.
    ///
    /// Used for `__esc_rt_env_load(env, depth, slot)`.
    pub fn get_env_load(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i32_type().into(),
                ctx.i32_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare an env_store helper: `(i64, i32, i32, i64) -> void`.
    ///
    /// Used for `__esc_rt_env_store(env, depth, slot, val)`.
    pub fn get_env_store(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.void_type().fn_type(
            &[
                ctx.i64_type().into(),
                ctx.i32_type().into(),
                ctx.i32_type().into(),
                ctx.i64_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }

    /// Get or declare a string-intern helper: `(i8*, i32) -> i64`.
    ///
    /// Used for `__esc_rt_string_intern(data, len)`.
    pub fn get_string_intern(&mut self, name: &str, module: &Module<'ctx>) -> FunctionValue<'ctx> {
        if let Some(&fv) = self.cache.get(name) {
            return fv;
        }
        let ctx = module.get_context();
        let fn_ty = ctx.i64_type().fn_type(
            &[
                ctx.ptr_type(inkwell::AddressSpace::default()).into(),
                ctx.i32_type().into(),
            ],
            false,
        );
        let fv = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        self.cache.insert(name.to_owned(), fv);
        fv
    }
}
