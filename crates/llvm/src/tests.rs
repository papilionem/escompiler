//! Tests for the LLVM backend.
//!
//! Covers type mapping, NaN-boxing emit, runtime call declarations,
//! error types, backend construction, and end-to-end IR-to-object
//! compilation through [`LlvmBackend`].

use inkwell::context::Context;

use crate::codegen::LlvmBackend;
use crate::error::LlvmCodegenError;
use crate::nanbox_emit;
use crate::runtime_calls::RuntimeCalls;
use crate::types::ir_type_to_llvm;

use inkwell::targets::{InitializationConfig, Target, TargetTriple};
use ir::IrType;
use ir::builder::TypedIrBuilder;

// ---------------------------------------------------------------------------
// Type mapping tests
// ---------------------------------------------------------------------------

#[test]
fn test_ir_type_void_maps_to_none() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::Void, &ctx).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_ir_type_i32_maps_to_i32() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::I32, &ctx).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_int_type());
}

#[test]
fn test_ir_type_f64_maps_to_f64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::F64, &ctx).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_float_type());
}

#[test]
fn test_ir_type_bool_maps_to_bool() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::Bool, &ctx).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_int_type());
}

#[test]
fn test_ir_type_jsvalue_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::JSValue, &ctx).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_int_type());
}

#[test]
fn test_ir_type_jsstring_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::JSString, &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_jsobject_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::JSObject, &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_jsarray_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::JSArray, &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_ptr_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::Ptr, &ctx).unwrap();
    assert!(result.is_some());
}

// ---------------------------------------------------------------------------
// NaN-boxing tests
// ---------------------------------------------------------------------------

#[test]
fn test_nanbox_box_null_is_constant() {
    let ctx = Context::create();
    let val = nanbox_emit::emit_box_null(&ctx);
    // Should be a constant i64
    assert!(val.is_const());
    assert_eq!(
        val.get_zero_extended_constant(),
        Some(0x7FF8_0000_0000_0000 | (0x0003 << 48))
    );
}

#[test]
fn test_nanbox_box_undefined_is_constant() {
    let ctx = Context::create();
    let val = nanbox_emit::emit_box_undefined(&ctx);
    assert!(val.is_const());
    assert_eq!(
        val.get_zero_extended_constant(),
        Some(0x7FF8_0000_0000_0000 | (0x0004 << 48))
    );
}

#[test]
fn test_nanbox_null_not_equal_undefined() {
    let ctx = Context::create();
    let null_val = nanbox_emit::emit_box_null(&ctx);
    let undef_val = nanbox_emit::emit_box_undefined(&ctx);
    assert_ne!(
        null_val.get_zero_extended_constant(),
        undef_val.get_zero_extended_constant()
    );
}

#[test]
fn test_nanbox_box_i32_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let i32_val = ctx.i32_type().const_int(42, false);
    let boxed = nanbox_emit::emit_box_i32(&builder, &ctx, i32_val);
    // The result should be an i64 value (possibly constant-folded)
    assert!(boxed.get_type() == ctx.i64_type());
}

#[test]
fn test_nanbox_box_f64_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let f64_val = ctx.f64_type().const_float(2.5);
    let boxed = nanbox_emit::emit_box_f64(&builder, &ctx, f64_val);
    assert!(boxed.get_type() == ctx.i64_type());
}

#[test]
fn test_nanbox_box_bool_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let bool_val = ctx.bool_type().const_int(1, false);
    let boxed = nanbox_emit::emit_box_bool(&builder, &ctx, bool_val);
    assert!(boxed.get_type() == ctx.i64_type());
}

#[test]
fn test_nanbox_unbox_i32_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let i64_val = ctx.i64_type().const_int(0x7FF9_0000_0000_002A, false);
    let unboxed = nanbox_emit::emit_unbox_i32(&builder, &ctx, i64_val);
    assert!(unboxed.get_type() == ctx.i32_type());
}

#[test]
fn test_nanbox_unbox_f64_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let bits = 2.5f64.to_bits();
    let i64_val = ctx.i64_type().const_int(bits, false);
    let unboxed = nanbox_emit::emit_unbox_f64(&builder, &ctx, i64_val);
    assert!(unboxed.get_type() == ctx.f64_type());
}

#[test]
fn test_nanbox_unbox_bool_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let i64_val = ctx.i64_type().const_int(0x7FFA_0000_0000_0001, false);
    let unboxed = nanbox_emit::emit_unbox_bool(&builder, &ctx, i64_val);
    assert!(unboxed.get_type() == ctx.bool_type());
}

#[test]
fn test_nanbox_box_string_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let ptr_val = ctx.i64_type().const_int(0x1234, false);
    let boxed = nanbox_emit::emit_box_string(&builder, &ctx, ptr_val);
    assert!(boxed.get_type() == ctx.i64_type());
}

#[test]
fn test_nanbox_box_object_with_builder() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let builder = ctx.create_builder();
    let fn_ty = ctx.void_type().fn_type(&[], false);
    let func = module.add_function("test", fn_ty, None);
    let bb = ctx.append_basic_block(func, "entry");
    builder.position_at_end(bb);

    let ptr_val = ctx.i64_type().const_int(0x5678, false);
    let boxed = nanbox_emit::emit_box_object(&builder, &ctx, ptr_val);
    assert!(boxed.get_type() == ctx.i64_type());
}

// ---------------------------------------------------------------------------
// Runtime calls tests
// ---------------------------------------------------------------------------

#[test]
fn test_runtime_calls_binary_op_caching() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f1 = rt.get_binary_js_op("__esc_rt_add_js", &module);
    let f2 = rt.get_binary_js_op("__esc_rt_add_js", &module);
    // Same function should be returned (cached)
    assert_eq!(f1.get_name().to_str().unwrap(), "__esc_rt_add_js");
    assert_eq!(f2.get_name().to_str().unwrap(), "__esc_rt_add_js");
}

#[test]
fn test_runtime_calls_different_ops_distinct() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f1 = rt.get_binary_js_op("__esc_rt_add_js", &module);
    let f2 = rt.get_binary_js_op("__esc_rt_sub_js", &module);
    assert_ne!(
        f1.get_name().to_str().unwrap(),
        f2.get_name().to_str().unwrap()
    );
}

#[test]
fn test_runtime_calls_console_fn() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_console_fn("__esc_rt_console_log", &module);
    assert_eq!(f.get_name().to_str().unwrap(), "__esc_rt_console_log");
    // Should have 2 params: i32 (argc) and i64 (argv_ptr)
    assert_eq!(f.count_params(), 2);
}

#[test]
fn test_runtime_calls_void_void() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_void_void("__esc_rt_init", &module);
    assert_eq!(f.get_name().to_str().unwrap(), "__esc_rt_init");
    assert_eq!(f.count_params(), 0);
}

#[test]
fn test_runtime_calls_unary_op() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_unary_js_op("__esc_rt_neg_js", &module);
    assert_eq!(f.count_params(), 1);
}

#[test]
fn test_runtime_calls_ternary_op() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_ternary_js_op("__esc_rt_set_prop", &module);
    assert_eq!(f.count_params(), 3);
}

#[test]
fn test_runtime_calls_call_method() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_call_method("__esc_rt_call_method", &module);
    assert_eq!(f.count_params(), 4); // obj, key, argc, argv_ptr
}

#[test]
fn test_runtime_calls_create_object() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_create_object("__esc_rt_create_object", &module);
    assert_eq!(f.count_params(), 0);
}

#[test]
fn test_runtime_calls_create_array() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_create_array("__esc_rt_create_array", &module);
    assert_eq!(f.count_params(), 1);
}

// ---------------------------------------------------------------------------
// Error type tests
// ---------------------------------------------------------------------------

#[test]
fn test_error_display_module() {
    let err = LlvmCodegenError::Module("test error".into());
    assert_eq!(err.to_string(), "llvm module error: test error");
}

#[test]
fn test_error_display_undefined_value() {
    let err = LlvmCodegenError::UndefinedValue(42);
    assert_eq!(err.to_string(), "undefined value: v42");
}

#[test]
fn test_error_display_unsupported_type() {
    let err = LlvmCodegenError::UnsupportedType("FooType".into());
    assert_eq!(err.to_string(), "unsupported type: FooType");
}

#[test]
fn test_error_display_no_entry() {
    let err = LlvmCodegenError::NoEntryFunction;
    assert_eq!(err.to_string(), "no entry function in module");
}

#[test]
fn test_error_display_target() {
    let err = LlvmCodegenError::Target("bad target".into());
    assert_eq!(err.to_string(), "target error: bad target");
}

#[test]
fn test_error_display_unsupported_opcode() {
    let err = LlvmCodegenError::UnsupportedOpcode("FooOp".into());
    assert_eq!(err.to_string(), "unsupported opcode: FooOp");
}

#[test]
fn test_error_display_object_write() {
    let err = LlvmCodegenError::ObjectWrite("write failed".into());
    assert_eq!(err.to_string(), "object file write error: write failed");
}

// ---------------------------------------------------------------------------
// LlvmBackend construction tests
// ---------------------------------------------------------------------------

#[test]
fn test_backend_new_debug() {
    let _backend = LlvmBackend::new_debug();
}

#[test]
fn test_backend_new_release() {
    let _backend = LlvmBackend::new_release();
}

#[test]
fn test_backend_default() {
    let _backend = LlvmBackend::default();
}

// ---------------------------------------------------------------------------
// Integration tests: IR → LLVM object compilation
// ---------------------------------------------------------------------------

/// Helper: build a single-function module with one block, no params, void return.
fn build_simple_module(
    setup: impl FnOnce(&mut TypedIrBuilder),
) -> (ir::builder::TypedModule, Vec<String>) {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    setup(&mut b);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    (b.finish(), vec![])
}

#[test]
fn test_compile_empty_function() {
    let (module, strings) = build_simple_module(|_| {});
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty(), "object file should not be empty");
}

#[test]
fn test_compile_const_i32() {
    let (module, strings) = build_simple_module(|b| {
        b.const_i32(42);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_const_f64() {
    let (module, strings) = build_simple_module(|b| {
        b.const_f64(2.5);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_const_bool() {
    let (module, strings) = build_simple_module(|b| {
        b.const_bool(true);
        b.const_bool(false);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_const_null_undefined() {
    let (module, strings) = build_simple_module(|b| {
        b.const_null();
        b.const_undefined();
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_const_string() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.const_string(0);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let strings = vec!["hello world".to_string()];
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_i32_arithmetic() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(3);
        b.add_i32(a, c);
        b.sub_i32(a, c);
        b.mul_i32(a, c);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_f64_arithmetic() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_f64(1.5);
        let c = b.const_f64(2.5);
        b.add_f64(a, c);
        b.sub_f64(a, c);
        b.mul_f64(a, c);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_js_arithmetic() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(5);
        let c = b.const_i32(3);
        b.add_js(a, c);
        b.sub_js(a, c);
        b.mul_js(a, c);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_comparison_ops() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(20);
        b.eq_i32(a, c);
        b.ne_i32(a, c);
        b.lt_i32(a, c);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_function_with_params() {
    let mut b = TypedIrBuilder::new();
    b.begin_function(
        "add",
        vec![("a", IrType::JSValue), ("b", IrType::JSValue)],
        IrType::JSValue,
    );
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.load_param(0);
    let p_b = b.load_param(1);
    let result = b.add_js(a, p_b);
    b.ret(Some(result));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &[]).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_branch_if_else() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();
    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.switch_to_block(then_bb);
    b.ret(None);
    b.switch_to_block(else_bb);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &[]).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_unconditional_branch() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let entry = b.create_block();
    let target = b.create_block();
    b.switch_to_block(entry);
    b.br(target);
    b.switch_to_block(target);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &[]).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    // Function 0: helper
    b.begin_function("helper", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();

    // Function 1: entry
    b.begin_function("main_fn", vec![], IrType::Void);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    b.ret(None);
    b.end_function();
    b.set_entry(1);

    let module = b.finish();
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &[]).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_nanbox_box_unbox_roundtrip() {
    let (module, strings) = build_simple_module(|b| {
        let i = b.const_i32(99);
        b.box_i32(i);
        let f = b.const_f64(2.75);
        b.box_f64(f);
        let t = b.const_bool(true);
        b.box_bool(t);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_nop_and_debugger() {
    let (module, strings) = build_simple_module(|b| {
        b.nop();
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_release_mode() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        b.add_i32(a, c);
    });
    let backend = LlvmBackend::new_release();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_runtime_calls_create_closure() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_create_closure("__esc_rt_create_closure", &module);
    assert_eq!(f.count_params(), 3); // func_idx: i32, env: i64, flags: i32
}

#[test]
fn test_runtime_calls_string_intern() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_string_intern("__esc_rt_string_intern", &module);
    assert_eq!(f.count_params(), 2); // data: ptr, len: i32
}

#[test]
fn test_runtime_calls_void_unary() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_void_unary("__esc_rt_throw", &module);
    assert_eq!(f.count_params(), 1);
}

#[test]
fn test_runtime_calls_call_variadic() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_call_variadic("__esc_rt_call_indirect", &module);
    assert_eq!(f.count_params(), 3); // callee: i64, argc: i32, argv: i64
}

// ---------------------------------------------------------------------------
// Environment & closure runtime call signature tests
// ---------------------------------------------------------------------------

#[test]
fn test_runtime_calls_env_create() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_env_create("__esc_rt_env_create", &module);
    assert_eq!(f.count_params(), 2); // parent: i64, slot_count: i32
    assert_eq!(f.get_name().to_str().unwrap(), "__esc_rt_env_create");
}

#[test]
fn test_runtime_calls_env_load() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_env_load("__esc_rt_env_load", &module);
    assert_eq!(f.count_params(), 3); // env: i64, depth: i32, slot: i32
}

#[test]
fn test_runtime_calls_env_store() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_env_store("__esc_rt_env_store", &module);
    assert_eq!(f.count_params(), 4); // env: i64, depth: i32, slot: i32, val: i64
}

#[test]
fn test_runtime_calls_void_binary() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_void_binary("__esc_rt_box_store", &module);
    assert_eq!(f.count_params(), 2); // ptr: i64, val: i64
}

#[test]
fn test_runtime_calls_void_i64() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f = rt.get_void_i64("__esc_rt_get_global_this", &module);
    assert_eq!(f.count_params(), 0);
    assert_eq!(f.get_name().to_str().unwrap(), "__esc_rt_get_global_this");
}

#[test]
fn test_runtime_calls_env_create_caching() {
    let ctx = Context::create();
    let module = ctx.create_module("test");
    let mut rt = RuntimeCalls::new();
    let f1 = rt.get_env_create("__esc_rt_env_create", &module);
    let f2 = rt.get_env_create("__esc_rt_env_create", &module);
    assert_eq!(
        f1.get_name().to_str().unwrap(),
        f2.get_name().to_str().unwrap()
    );
}

// ---------------------------------------------------------------------------
// LLVM CallRuntime dispatch compilation tests
// ---------------------------------------------------------------------------

#[test]
fn test_compile_call_runtime_binary() {
    // CallRuntime with binary pattern: name + 2 args
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_i32(5);
    let c = b.const_i32(3);
    let name = b.const_string(0);
    b.call_runtime(name, vec![a, c]);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let strings = vec!["__esc_rt_add_js".to_string()];
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_call_runtime_unary() {
    // CallRuntime with unary pattern: name + 1 arg
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_i32(5);
    let name = b.const_string(0);
    b.call_runtime(name, vec![a]);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let strings = vec!["__esc_rt_to_number".to_string()];
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_call_runtime_void_arg() {
    // CallRuntime with 0-arg pattern: name only (e.g., get_global_this)
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let name = b.const_string(0);
    b.call_runtime(name, vec![]);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let strings = vec!["__esc_rt_get_global_this".to_string()];
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_call_runtime_console_log() {
    // CallRuntime with console pattern
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let val = b.const_i32(42);
    let name = b.const_string(0);
    b.call_runtime(name, vec![val]);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let strings = vec!["__esc_rt_console_log".to_string()];
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

// ---------------------------------------------------------------------------
// LLVM Env ops compilation tests
// ---------------------------------------------------------------------------

#[test]
fn test_compile_env_create() {
    let (module, strings) = build_simple_module(|b| {
        b.env_create(3);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_env_load() {
    let (module, strings) = build_simple_module(|b| {
        let env = b.env_create(2);
        b.env_load(env, 0);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_env_store() {
    let (module, strings) = build_simple_module(|b| {
        let env = b.env_create(2);
        let val = b.const_i32(42);
        b.env_store(env, 0, val);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_env_extend() {
    let (module, strings) = build_simple_module(|b| {
        let outer = b.env_create(2);
        b.env_extend(outer, 3);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_box_store_void_return() {
    let (module, strings) = build_simple_module(|b| {
        let init = b.const_undefined();
        let bx = b.alloc_box(init);
        let val = b.const_i32(99);
        b.box_store(bx, val);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend.compile_module(&module, &strings).unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_ir_type_completion_record_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::CompletionRecord, &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_struct_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::Struct(common::StructTypeId(0)), &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_zone_ptr_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::ZonePtr, &ctx).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_ir_type_heap_ptr_maps_to_i64() {
    let ctx = Context::create();
    let result = ir_type_to_llvm(&IrType::HeapPtr, &ctx).unwrap();
    assert!(result.is_some());
}

// ---------------------------------------------------------------------------
// Cross-compilation tests: verify all LLVM target backends work
// ---------------------------------------------------------------------------

/// Verify that all LLVM target backends can be initialized.
#[test]
fn test_initialize_all_targets() {
    Target::initialize_all(&InitializationConfig::default());

    // Verify we can look up each major target by triple
    let triples = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "armv7-unknown-linux-gnueabihf",
        "riscv64-unknown-linux-gnu",
        "wasm32-unknown-unknown",
        "powerpc64le-unknown-linux-gnu",
        "mips64el-unknown-linux-gnuabi64",
        "s390x-unknown-linux-gnu",
    ];

    for triple_str in &triples {
        let triple = TargetTriple::create(triple_str);
        let result = Target::from_triple(&triple);
        assert!(
            result.is_ok(),
            "failed to find target for triple: {triple_str}"
        );
    }
}

/// Verify we can create a TargetMachine for each major architecture.
#[test]
fn test_create_target_machines_all_architectures() {
    Target::initialize_all(&InitializationConfig::default());

    let targets = [
        ("x86_64", "x86_64-unknown-linux-gnu"),
        ("aarch64", "aarch64-unknown-linux-gnu"),
        ("arm", "armv7-unknown-linux-gnueabihf"),
        ("riscv64", "riscv64-unknown-linux-gnu"),
        ("wasm32", "wasm32-unknown-unknown"),
        ("powerpc64le", "powerpc64le-unknown-linux-gnu"),
        ("mips64el", "mips64el-unknown-linux-gnuabi64"),
        ("s390x", "s390x-unknown-linux-gnu"),
        ("sparc64", "sparc64-unknown-linux-gnu"),
    ];

    for (arch, triple_str) in &targets {
        let triple = TargetTriple::create(triple_str);
        let target = Target::from_triple(&triple)
            .unwrap_or_else(|e| panic!("failed to find target for {arch}: {e}"));
        let machine = target.create_target_machine(
            &triple,
            "",
            "",
            inkwell::OptimizationLevel::None,
            inkwell::targets::RelocMode::PIC,
            inkwell::targets::CodeModel::Default,
        );
        assert!(
            machine.is_some(),
            "failed to create target machine for {arch} ({triple_str})"
        );
    }
}

/// Emit object files for multiple architectures from the same IR module.
#[test]
fn test_cross_compile_empty_function_all_targets() {
    let (module, strings) = build_simple_module(|_| {});
    let backend = LlvmBackend::new_debug();

    let targets = [
        ("x86_64", "x86_64-unknown-linux-gnu"),
        ("aarch64", "aarch64-unknown-linux-gnu"),
        ("arm", "armv7-unknown-linux-gnueabihf"),
        ("riscv64", "riscv64-unknown-linux-gnu"),
        ("wasm32", "wasm32-unknown-unknown"),
        ("powerpc64le", "powerpc64le-unknown-linux-gnu"),
    ];

    for (arch, triple) in &targets {
        let result = backend.compile_module_for_target(&module, &strings, triple);
        assert!(
            result.is_ok(),
            "cross-compilation to {arch} failed: {:?}",
            result.err()
        );
        let obj = result.unwrap();
        assert!(
            !obj.is_empty(),
            "object file for {arch} should not be empty"
        );
    }
}

/// Cross-compile a module with arithmetic and NaN-boxing to verify IR is target-independent.
#[test]
fn test_cross_compile_arithmetic_and_nanbox() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(42);
        let c = b.const_i32(7);
        b.add_js(a, c);
        b.sub_js(a, c);
        b.mul_js(a, c);
        let f = b.const_f64(2.5);
        b.box_f64(f);
        b.box_i32(a);
        b.const_null();
        b.const_undefined();
    });
    let backend = LlvmBackend::new_debug();

    let targets = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "riscv64-unknown-linux-gnu",
        "wasm32-unknown-unknown",
    ];

    for triple in &targets {
        let result = backend.compile_module_for_target(&module, &strings, triple);
        assert!(
            result.is_ok(),
            "cross-compile arithmetic to {triple} failed: {:?}",
            result.err()
        );
    }
}

/// Cross-compile a module with branching to verify control flow is target-independent.
#[test]
fn test_cross_compile_branching() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_fn", vec![], IrType::Void);
    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();
    let merge_bb = b.create_block();
    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.switch_to_block(then_bb);
    b.br(merge_bb);
    b.switch_to_block(else_bb);
    b.br(merge_bb);
    b.switch_to_block(merge_bb);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let backend = LlvmBackend::new_debug();

    for triple in &["aarch64-unknown-linux-gnu", "riscv64-unknown-linux-gnu"] {
        let result = backend.compile_module_for_target(&module, &[], triple);
        assert!(
            result.is_ok(),
            "cross-compile branching to {triple} failed: {:?}",
            result.err()
        );
    }
}

// ---------------------------------------------------------------------------
// Debug info integration tests
// ---------------------------------------------------------------------------

#[test]
fn test_compile_with_debug_info_empty_function() {
    let (module, strings) = build_simple_module(|_| {});
    let backend = LlvmBackend::new_debug();
    let obj = backend
        .compile_module_with_debug_info(&module, &strings, "test.js", ".", "var x = 1;\n")
        .unwrap();
    assert!(!obj.is_empty(), "object file should not be empty");
}

#[test]
fn test_compile_with_debug_info_arithmetic() {
    let source = "var a = 10;\nvar b = 3;\nvar c = a + b;\n";
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(3);
        b.add_i32(a, c);
    });
    let backend = LlvmBackend::new_debug();
    let obj = backend
        .compile_module_with_debug_info(&module, &strings, "arith.js", "/src", source)
        .unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_with_debug_info_multiple_functions() {
    let source = "function helper() {}\nfunction main() { helper(); }\n";
    let mut b = TypedIrBuilder::new();

    // Function 0: helper
    b.begin_function("helper", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();

    // Function 1: entry
    b.begin_function("main_fn", vec![], IrType::Void);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    b.ret(None);
    b.end_function();
    b.set_entry(1);

    let module = b.finish();
    let backend = LlvmBackend::new_debug();
    let obj = backend
        .compile_module_with_debug_info(&module, &[], "multi.js", ".", source)
        .unwrap();
    assert!(!obj.is_empty());
}

#[test]
fn test_compile_with_debug_info_release_mode() {
    let (module, strings) = build_simple_module(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        b.add_i32(a, c);
    });
    let backend = LlvmBackend::new_release();
    let obj = backend
        .compile_module_with_debug_info(&module, &strings, "release.js", ".", "1 + 2")
        .unwrap();
    assert!(!obj.is_empty());
}
