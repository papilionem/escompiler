//! Unit tests for the Cranelift code generation backend.

use crate::CraneliftBackend;
use crate::context::CompilationContext;
use crate::error::CodegenError;
use crate::runtime_calls::RuntimeCalls;
use crate::types::ir_type_to_cranelift;
use ::ir::IrType;
use ::ir::builder::TypedIrBuilder;
use cranelift_codegen::ir::types;

// ---------------------------------------------------------------------------
// Helper: build a simple module with one function
// ---------------------------------------------------------------------------

/// Build a module with a single void function containing the given body.
fn build_void_module(
    body: impl FnOnce(&mut TypedIrBuilder, ::ir::BlockId),
) -> ::ir::builder::TypedModule {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    body(&mut b, bb);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    b.finish()
}

/// Build a module with a single i32-returning function containing the given body.
fn build_i32_module(
    body: impl FnOnce(&mut TypedIrBuilder, ::ir::BlockId),
) -> ::ir::builder::TypedModule {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    body(&mut b, bb);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    b.finish()
}

// ===========================================================================
// Context tests
// ===========================================================================

#[test]
fn test_context_creation_succeeds() {
    let ctx = CompilationContext::new();
    assert!(
        ctx.is_ok(),
        "CompilationContext::new() should succeed on host"
    );
}

#[test]
fn test_context_has_pointer_type() {
    let ctx = CompilationContext::new().unwrap();
    let ptr_ty = ctx.isa.pointer_type();
    // On 64-bit systems, pointer type should be I64
    assert!(
        ptr_ty == types::I64 || ptr_ty == types::I32,
        "pointer type should be I32 or I64, got {ptr_ty:?}"
    );
}

// ===========================================================================
// Type mapping tests
// ===========================================================================

#[test]
fn test_type_map_void() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::Void, &*ctx.isa).unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_type_map_bool() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::Bool, &*ctx.isa).unwrap();
    assert_eq!(result, Some(types::I8));
}

#[test]
fn test_type_map_i32() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::I32, &*ctx.isa).unwrap();
    assert_eq!(result, Some(types::I32));
}

#[test]
fn test_type_map_i64() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::I64, &*ctx.isa).unwrap();
    assert_eq!(result, Some(types::I64));
}

#[test]
fn test_type_map_f64() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::F64, &*ctx.isa).unwrap();
    assert_eq!(result, Some(types::F64));
}

#[test]
fn test_type_map_jsvalue_is_i64() {
    let ctx = CompilationContext::new().unwrap();
    let result = ir_type_to_cranelift(&IrType::JSValue, &*ctx.isa).unwrap();
    assert_eq!(result, Some(types::I64));
}

#[test]
fn test_type_map_js_types_all_i64() {
    let ctx = CompilationContext::new().unwrap();
    let js_types = [
        IrType::JSString,
        IrType::JSObject,
        IrType::JSArray,
        IrType::JSFunction,
        IrType::JSSymbol,
    ];
    for ty in &js_types {
        let result = ir_type_to_cranelift(ty, &*ctx.isa).unwrap();
        assert_eq!(result, Some(types::I64), "expected I64 for {ty:?}");
    }
}

#[test]
fn test_type_map_ptr_types() {
    let ctx = CompilationContext::new().unwrap();
    let ptr_type = ctx.isa.pointer_type();
    let ptr_types = [IrType::Ptr, IrType::ZonePtr, IrType::HeapPtr];
    for ty in &ptr_types {
        let result = ir_type_to_cranelift(ty, &*ctx.isa).unwrap();
        assert_eq!(result, Some(ptr_type), "expected pointer type for {ty:?}");
    }
}

// ===========================================================================
// Backend creation test
// ===========================================================================

#[test]
fn test_backend_creation_succeeds() {
    let backend = CraneliftBackend::new();
    assert!(backend.is_ok(), "CraneliftBackend::new() should succeed");
}

// ===========================================================================
// Constant lowering tests
// ===========================================================================

#[test]
fn test_lower_const_i32() {
    let module = build_i32_module(|b, _bb| {
        let v = b.const_i32(42);
        b.ret(Some(v));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "const_i32 should compile: {result:?}");
}

#[test]
fn test_lower_const_f64() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::F64);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_f64(2.5);
    b.ret(Some(v));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "const_f64 should compile: {result:?}");
}

#[test]
fn test_lower_const_bool() {
    let mut b = TypedIrBuilder::new();
    // ConstBool now produces NaN-boxed i64, so return type must be JSValue
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_bool(true);
    b.ret(Some(v));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "const_bool should compile: {result:?}");
}

#[test]
fn test_lower_const_null() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "const_null should compile: {result:?}");
}

#[test]
fn test_lower_const_undefined() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_undefined();
    b.ret(Some(v));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "const_undefined should compile: {result:?}");
}

#[test]
fn test_lower_const_string() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSString);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_string(0);
    b.ret(Some(v));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let string_table = vec!["hello".to_string()];
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &string_table);
    assert!(result.is_ok(), "const_string should compile: {result:?}");
}

// ===========================================================================
// Arithmetic tests
// ===========================================================================

#[test]
fn test_lower_add_i32() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let sum = b.add_i32(a, c);
        b.ret(Some(sum));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "add_i32 should compile: {result:?}");
}

#[test]
fn test_lower_sub_i32() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(10);
        let c = b.const_i32(3);
        let diff = b.sub_i32(a, c);
        b.ret(Some(diff));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "sub_i32 should compile: {result:?}");
}

#[test]
fn test_lower_mul_i32() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(6);
        let c = b.const_i32(7);
        let prod = b.mul_i32(a, c);
        b.ret(Some(prod));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "mul_i32 should compile: {result:?}");
}

#[test]
fn test_lower_div_i32() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(42);
        let c = b.const_i32(6);
        let quot = b.div_i32(a, c);
        b.ret(Some(quot));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "div_i32 should compile: {result:?}");
}

#[test]
fn test_lower_f64_arithmetic() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::F64);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_f64(1.5);
    let c = b.const_f64(2.5);
    let sum = b.add_f64(a, c);
    b.ret(Some(sum));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "f64 arithmetic should compile: {result:?}");
}

// ===========================================================================
// JS arithmetic tests (runtime calls)
// ===========================================================================

#[test]
fn test_lower_add_js() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_null();
    let c = b.const_undefined();
    let sum = b.add_js(a, c);
    b.ret(Some(sum));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "add_js should compile: {result:?}");
}

#[test]
fn test_lower_sub_js() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_null();
    let c = b.const_null();
    let diff = b.sub_js(a, c);
    b.ret(Some(diff));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "sub_js should compile: {result:?}");
}

// ===========================================================================
// NaN-boxing tests
// ===========================================================================

#[test]
fn test_lower_box_i32() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let val = b.const_i32(42);
    let boxed = b.box_i32(val);
    b.ret(Some(boxed));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "box_i32 should compile: {result:?}");
}

#[test]
fn test_lower_unbox_i32() {
    let module = build_i32_module(|b, _bb| {
        let boxed = b.const_null(); // pretend this is a boxed i32
        let val = b.unbox_i32(boxed);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "unbox_i32 should compile: {result:?}");
}

#[test]
fn test_lower_box_f64() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let val = b.const_f64(2.5);
    let boxed = b.box_f64(val);
    b.ret(Some(boxed));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "box_f64 should compile: {result:?}");
}

#[test]
fn test_lower_box_bool() {
    // ConstBool now NaN-boxes at construction, so BoxBool is used for
    // raw i8 values (e.g., comparison results). Test with a const i8.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    // Use eq_strict which produces a raw i8 comparison result
    let a = b.const_i32(1);
    let c = b.const_i32(1);
    let cmp = b.eq_strict(a, c);
    let boxed = b.box_bool(cmp);
    b.ret(Some(boxed));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "box_bool should compile: {result:?}");
}

#[test]
fn test_lower_box_null_and_undefined() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let _null = b.box_null();
    let undef = b.box_undefined();
    b.ret(Some(undef));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "box_null/undefined should compile: {result:?}"
    );
}

// ===========================================================================
// Control flow tests
// ===========================================================================

#[test]
fn test_lower_br_if_two_targets() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();

    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.seal_block(entry);

    b.switch_to_block(then_bb);
    let v1 = b.const_i32(1);
    b.ret(Some(v1));
    b.seal_block(then_bb);

    b.switch_to_block(else_bb);
    let v2 = b.const_i32(0);
    b.ret(Some(v2));
    b.seal_block(else_bb);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "brif should compile: {result:?}");
}

#[test]
fn test_lower_ret_void() {
    let module = build_void_module(|b, _bb| {
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "ret void should compile: {result:?}");
}

#[test]
fn test_lower_ret_with_value() {
    let module = build_i32_module(|b, _bb| {
        let v = b.const_i32(99);
        b.ret(Some(v));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "ret with value should compile: {result:?}");
}

// ===========================================================================
// Comparison tests
// ===========================================================================

#[test]
fn test_lower_eq_i32() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Bool);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_i32(1);
    let c = b.const_i32(2);
    let eq = b.eq_i32(a, c);
    b.ret(Some(eq));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "eq_i32 should compile: {result:?}");
}

#[test]
fn test_lower_lt_f64() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Bool);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_f64(1.0);
    let c = b.const_f64(2.0);
    let lt = b.lt_f64(a, c);
    b.ret(Some(lt));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "lt_f64 should compile: {result:?}");
}

// ===========================================================================
// Object file output tests
// ===========================================================================

#[test]
fn test_compile_module_produces_valid_elf() {
    let module = build_void_module(|b, _bb| {
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let bytes = backend.compile_module(&module, &[]).unwrap();

    assert!(bytes.len() > 4, "object file should have content");
    // ELF magic on Unix, COFF magic on Windows
    if cfg!(windows) {
        // COFF: first two bytes are machine type (x86_64 = 0x8664 little-endian)
        assert_eq!(
            &bytes[..2],
            &[0x64, 0x86],
            "object file should have COFF header (x86_64)"
        );
    } else {
        assert_eq!(
            &bytes[..4],
            &[0x7F, b'E', b'L', b'F'],
            "object file should have ELF magic header"
        );
    }
}

#[test]
fn test_empty_function_compiles() {
    let module = build_void_module(|b, _bb| {
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "empty function should compile");
}

#[test]
fn test_arithmetic_function_compiles() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(10);
        let c = b.const_i32(20);
        let sum = b.add_i32(a, c);
        let d = b.const_i32(5);
        let prod = b.mul_i32(sum, d);
        b.ret(Some(prod));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "arithmetic function should compile");
}

// ===========================================================================
// Unimplemented op emits trap (doesn't panic)
// ===========================================================================

#[test]
fn test_unimplemented_op_emits_trap() {
    // CreateObject is now implemented, so we test that it compiles
    // correctly when followed by a return.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let _obj = b.create_object();
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "CreateObject lowering should compile: {result:?}"
    );
}

// ===========================================================================
// Error path tests
// ===========================================================================

#[test]
fn test_no_entry_function_still_compiles() {
    // A module without an entry function should still compile
    let mut b = TypedIrBuilder::new();
    b.begin_function("helper", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    // Don't set entry
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "module without entry should still compile");
}

#[test]
fn test_error_display() {
    let err = CodegenError::UndefinedValue(42);
    assert_eq!(err.to_string(), "undefined value: v42");

    let err = CodegenError::NoEntryFunction;
    assert_eq!(err.to_string(), "no entry function in module");

    let err = CodegenError::UnsupportedType("Widget".to_string());
    assert_eq!(err.to_string(), "unsupported type: Widget");
}

// ===========================================================================
// Bitwise operation tests
// ===========================================================================

#[test]
fn test_lower_bitwise_ops() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(0xFF);
        let c = b.const_i32(0x0F);
        let and = b.bitwise_and(a, c);
        let or = b.bitwise_or(and, c);
        let xor = b.bitwise_xor(or, a);
        let not = b.bitwise_not(xor);
        b.ret(Some(not));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "bitwise ops should compile: {result:?}");
}

// ===========================================================================
// Negation tests
// ===========================================================================

#[test]
fn test_lower_neg_i32() {
    let module = build_i32_module(|b, _bb| {
        let v = b.const_i32(42);
        let neg = b.neg_i32(v);
        b.ret(Some(neg));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "neg_i32 should compile: {result:?}");
}

#[test]
fn test_lower_neg_f64() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::F64);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_f64(2.5);
    let neg = b.neg_f64(v);
    b.ret(Some(neg));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "neg_f64 should compile: {result:?}");
}

// ===========================================================================
// Multi-block with branch test
// ===========================================================================

#[test]
fn test_lower_unconditional_branch() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let entry = b.create_block();
    let target = b.create_block();

    b.switch_to_block(entry);
    b.br(target);
    b.seal_block(entry);

    b.switch_to_block(target);
    let v = b.const_i32(99);
    b.ret(Some(v));
    b.seal_block(target);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "unconditional branch should compile: {result:?}"
    );
}

// ===========================================================================
// Multiple functions test
// ===========================================================================

#[test]
fn test_compile_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    // Function 0: helper
    b.begin_function("helper", vec![], IrType::I32);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_i32(42);
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: main (entry)
    b.begin_function("main", vec![], IrType::I32);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let v2 = b.const_i32(0);
    b.ret(Some(v2));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "multiple functions should compile: {result:?}"
    );
}

// ===========================================================================
// Shift operations test
// ===========================================================================

#[test]
fn test_lower_shift_ops() {
    let module = build_i32_module(|b, _bb| {
        let a = b.const_i32(0xFF);
        let c = b.const_i32(4);
        let shl = b.shift_left(a, c);
        let shr = b.shift_right(shl, c);
        let ushr = b.shift_right_unsigned(shr, c);
        b.ret(Some(ushr));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "shift ops should compile: {result:?}");
}

// ===========================================================================
// CallRuntime tests
// ===========================================================================

#[test]
fn test_call_runtime_console_log_compiles() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let name = b.const_string(0);
    let arg = b.const_null();
    let _result = b.call_runtime(name, vec![arg]);
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let string_table = vec!["__esc_rt_console_log".to_string()];
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &string_table);
    assert!(
        result.is_ok(),
        "call_runtime console_log should compile: {result:?}"
    );
}

#[test]
fn test_call_runtime_resolves_string_table() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let name = b.const_string(0);
    let arg = b.const_null();
    let _result = b.call_runtime(name, vec![arg]);
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let string_table = vec!["__esc_rt_console_log".to_string()];
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &string_table);
    assert!(
        result.is_ok(),
        "string table resolution should succeed: {result:?}"
    );
}

#[test]
fn test_call_runtime_binary_op() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let name = b.const_string(0);
    let lhs = b.const_null();
    let rhs = b.const_null();
    let result_val = b.call_runtime(name, vec![lhs, rhs]);
    b.ret(Some(result_val));
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let string_table = vec!["__esc_rt_add_js".to_string()];
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &string_table);
    assert!(
        result.is_ok(),
        "call_runtime binary op should compile: {result:?}"
    );
}

// ===========================================================================
// Main wrapper tests
// ===========================================================================

#[test]
fn test_main_wrapper_emitted() {
    let module = build_void_module(|b, _bb| {
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let bytes = backend.compile_module(&module, &[]).unwrap();
    assert!(
        bytes.len() > 4,
        "object file with main wrapper should have content"
    );
    // Verify object format magic
    if cfg!(windows) {
        assert_eq!(
            &bytes[..2],
            &[0x64, 0x86],
            "should produce valid COFF (x86_64)"
        );
    } else {
        assert_eq!(
            &bytes[..4],
            &[0x7F, b'E', b'L', b'F'],
            "should produce valid ELF"
        );
    }
}

#[test]
fn test_main_wrapper_not_emitted_without_entry() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("helper", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    // No entry set
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "module without entry should compile without main wrapper"
    );
}

// ===========================================================================
// RuntimeCalls cache tests
// ===========================================================================

#[test]
fn test_runtime_calls_string_cache() {
    let ctx = CompilationContext::new().unwrap();
    let mut module = ctx.object_module;
    let mut runtime = RuntimeCalls::new();

    let id1 = runtime
        .get_binary_js_op("__esc_rt_add_js", &mut module)
        .unwrap();
    let id2 = runtime
        .get_binary_js_op("__esc_rt_add_js", &mut module)
        .unwrap();
    assert_eq!(id1, id2, "cached FuncId should be the same");
}

// ===========================================================================
// Console log multi-arg test
// ===========================================================================

#[test]
fn test_console_log_multi_arg() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let name = b.const_string(0);
    let a1 = b.const_null();
    let a2 = b.const_undefined();
    let a3 = b.const_null();
    let _result = b.call_runtime(name, vec![a1, a2, a3]);
    b.ret(None);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let string_table = vec!["__esc_rt_console_log".to_string()];
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &string_table);
    assert!(
        result.is_ok(),
        "console.log with 3 args should compile: {result:?}"
    );
}

// ===========================================================================
// Helper: build a JSValue-returning module
// ===========================================================================

/// Build a module with a single JSValue-returning function containing the given body.
fn build_jsvalue_module(
    body: impl FnOnce(&mut TypedIrBuilder, ::ir::BlockId),
) -> ::ir::builder::TypedModule {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    body(&mut b, bb);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    b.finish()
}

// ===========================================================================
// ConstI32 call resolution test
// ===========================================================================

#[test]
fn test_const_i32_call_resolves() {
    let mut b = TypedIrBuilder::new();

    // Function 0: helper that returns JSValue
    b.begin_function("helper", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry that calls function 0
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let func_ref = b.const_i32(0); // refers to function 0 (helper)
    let result_val = b.call(func_ref, vec![]);
    b.ret(Some(result_val));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "call with ConstI32 func index should compile: {result:?}"
    );
}

// ===========================================================================
// Property access tests (C1)
// ===========================================================================

#[test]
fn test_lower_get_prop() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null(); // placeholder key
        let val = b.get_prop(obj, key);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "GetProp should compile: {result:?}");
}

#[test]
fn test_lower_set_prop() {
    let module = build_void_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.const_null();
        b.set_prop(obj, key, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "SetProp should compile: {result:?}");
}

#[test]
fn test_lower_delete_prop() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.delete_prop(obj, key);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "DeleteProp should compile: {result:?}");
}

#[test]
fn test_lower_has_prop() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.has_prop(obj, key);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "HasProp should compile: {result:?}");
}

#[test]
fn test_lower_get_elem() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let idx = b.const_null();
        let val = b.get_elem(obj, idx);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "GetElem should compile: {result:?}");
}

#[test]
fn test_lower_set_elem() {
    let module = build_void_module(|b, _bb| {
        let obj = b.create_object();
        let idx = b.const_null();
        let val = b.const_null();
        b.set_elem(obj, idx, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "SetElem should compile: {result:?}");
}

#[test]
fn test_lower_get_private() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.get_private(obj, key);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "GetPrivate should compile: {result:?}");
}

#[test]
fn test_lower_set_private() {
    let module = build_void_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.const_null();
        b.set_private(obj, key, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "SetPrivate should compile: {result:?}");
}

#[test]
fn test_lower_private_field_get() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let pid = b.const_i32(0);
        let val = b.private_field_get(obj, pid);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "PrivateFieldGet should compile: {result:?}");
}

#[test]
fn test_lower_private_field_set() {
    let module = build_void_module(|b, _bb| {
        let obj = b.create_object();
        let pid = b.const_i32(0);
        let val = b.const_null();
        b.private_field_set(obj, pid, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "PrivateFieldSet should compile: {result:?}");
}

#[test]
fn test_lower_private_field_has() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let pid = b.const_i32(0);
        let val = b.private_field_has(obj, pid);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "PrivateFieldHas should compile: {result:?}");
}

#[test]
fn test_lower_install_private_field() {
    let module = build_void_module(|b, _bb| {
        let obj = b.create_object();
        let pid = b.const_i32(0);
        let val = b.const_i32(42);
        b.install_private_field(obj, pid, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "InstallPrivateField should compile: {result:?}"
    );
}

// ===========================================================================
// Array/object creation tests (C2)
// ===========================================================================

#[test]
fn test_lower_create_array() {
    let module = build_jsvalue_module(|b, _bb| {
        let arr = b.create_array(vec![]);
        b.ret(Some(arr));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "CreateArray should compile: {result:?}");
}

#[test]
fn test_lower_create_array_with_elements() {
    let module = build_jsvalue_module(|b, _bb| {
        let e1 = b.const_null();
        let e2 = b.const_undefined();
        let arr = b.create_array(vec![e1, e2]);
        b.ret(Some(arr));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "CreateArray with elements should compile: {result:?}"
    );
}

#[test]
fn test_lower_instance_of() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let ctor = b.const_null();
        let val = b.instance_of(obj, ctor);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "InstanceOf should compile: {result:?}");
}

// ===========================================================================
// Closure/environment tests (C3)
// ===========================================================================

#[test]
fn test_lower_create_closure() {
    let module = build_jsvalue_module(|b, _bb| {
        let func_idx = b.const_i32(0);
        let env = b.const_null();
        let flags = b.const_i32(0);
        let closure = b.create_closure(func_idx, env, flags);
        b.ret(Some(closure));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "CreateClosure should compile: {result:?}");
}

#[test]
fn test_lower_env_create() {
    let module = build_jsvalue_module(|b, _bb| {
        let env = b.env_create(4);
        b.ret(Some(env));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "EnvCreate should compile: {result:?}");
}

#[test]
fn test_lower_env_load() {
    let module = build_jsvalue_module(|b, _bb| {
        let env = b.env_create(4);
        let val = b.env_load(env, 0);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "EnvLoad should compile: {result:?}");
}

#[test]
fn test_lower_env_store() {
    let module = build_void_module(|b, _bb| {
        let env = b.env_create(4);
        let val = b.const_null();
        b.env_store(env, 0, val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "EnvStore should compile: {result:?}");
}

#[test]
fn test_lower_env_extend() {
    let module = build_jsvalue_module(|b, _bb| {
        let env = b.env_create(2);
        let extended = b.env_extend(env, 4);
        b.ret(Some(extended));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "EnvExtend should compile: {result:?}");
}

// ===========================================================================
// Call ops tests (C4)
// ===========================================================================

#[test]
fn test_lower_call_new() {
    let module = build_jsvalue_module(|b, _bb| {
        let ctor = b.const_null();
        let arg = b.const_null();
        let val = b.call_new(ctor, vec![arg]);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "CallNew should compile: {result:?}");
}

#[test]
fn test_lower_call_new_no_args() {
    let module = build_jsvalue_module(|b, _bb| {
        let ctor = b.const_null();
        let val = b.call_new(ctor, vec![]);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "CallNew with no args should compile: {result:?}"
    );
}

#[test]
fn test_lower_call_method() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let method = b.const_null();
        let arg = b.const_null();
        let val = b.call_method(obj, method, vec![arg]);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "CallMethod should compile: {result:?}");
}

#[test]
fn test_lower_tail_call() {
    let mut b = TypedIrBuilder::new();

    // Function 0: target
    b.begin_function("target", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry with tail call
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let func_ref = b.const_i32(0);
    let val = b.tail_call(func_ref, vec![]);
    b.ret(Some(val));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "TailCall should compile: {result:?}");
}

// ===========================================================================
// Exception handling tests (C5)
// ===========================================================================

#[test]
fn test_lower_try_begin_end() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let entry = b.create_block();
    let catch_bb = b.create_block();

    b.switch_to_block(entry);
    b.try_begin(catch_bb);
    b.try_end();
    b.ret(None);
    b.seal_block(entry);

    b.switch_to_block(catch_bb);
    let _ex = b.catch_();
    b.ret(None);
    b.seal_block(catch_bb);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "TryBegin/TryEnd/Catch should compile: {result:?}"
    );
}

#[test]
fn test_lower_throw() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let val = b.const_null();
    b.throw_(val);
    b.seal_block(bb);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "Throw should compile: {result:?}");
}

#[test]
fn test_lower_rethrow() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let entry = b.create_block();
    let catch_bb = b.create_block();

    b.switch_to_block(entry);
    b.try_begin(catch_bb);
    b.try_end();
    b.ret(None);
    b.seal_block(entry);

    b.switch_to_block(catch_bb);
    let ex = b.catch_();
    b.rethrow(ex);
    b.seal_block(catch_bb);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "Rethrow should compile: {result:?}");
}

#[test]
fn test_lower_is_exception() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let is_ex = b.is_exception(val);
        b.ret(Some(is_ex));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "IsException should compile: {result:?}");
}

#[test]
fn test_lower_get_exception() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let ex = b.get_exception(val);
        b.ret(Some(ex));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "GetException should compile: {result:?}");
}

// ===========================================================================
// Iterator tests (C6)
// ===========================================================================

#[test]
fn test_lower_iter_init() {
    let module = build_jsvalue_module(|b, _bb| {
        let iterable = b.const_null();
        let iter = b.iter_init(iterable);
        b.ret(Some(iter));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "IterInit should compile: {result:?}");
}

#[test]
fn test_lower_iter_next_done_value() {
    let module = build_jsvalue_module(|b, _bb| {
        let iterable = b.const_null();
        let iter = b.iter_init(iterable);
        let result_val = b.iter_next(iter);
        let _done = b.iter_done(result_val);
        let val = b.iter_value(result_val);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "IterNext/Done/Value should compile: {result:?}"
    );
}

#[test]
fn test_lower_iter_close() {
    let module = build_void_module(|b, _bb| {
        let iterable = b.const_null();
        let iter = b.iter_init(iterable);
        b.iter_close(iter);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "IterClose should compile: {result:?}");
}

#[test]
fn test_lower_iter_full_cycle() {
    let module = build_jsvalue_module(|b, _bb| {
        let iterable = b.const_null();
        let iter = b.iter_init(iterable);
        let next_result = b.iter_next(iter);
        let _done = b.iter_done(next_result);
        let val = b.iter_value(next_result);
        b.iter_close(iter);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Full iterator cycle should compile: {result:?}"
    );
}

// ===========================================================================
// Promise/async tests (C7)
// ===========================================================================

#[test]
fn test_lower_promise_create() {
    let module = build_jsvalue_module(|b, _bb| {
        let p = b.promise_create();
        b.ret(Some(p));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "PromiseCreate should compile: {result:?}");
}

#[test]
fn test_lower_promise_resolve_reject() {
    let module = build_void_module(|b, _bb| {
        let p = b.promise_create();
        let val = b.const_null();
        b.promise_resolve(p, val);
        let p2 = b.promise_create();
        let err = b.const_null();
        b.promise_reject(p2, err);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "PromiseResolve/Reject should compile: {result:?}"
    );
}

#[test]
fn test_lower_await() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let awaited = b.await_(val);
        b.ret(Some(awaited));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "Await should compile: {result:?}");
}

// ===========================================================================
// Misc ops tests (C8)
// ===========================================================================

#[test]
fn test_lower_switch_three_cases() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let entry = b.create_block();
    let case0 = b.create_block();
    let case1 = b.create_block();
    let case2 = b.create_block();
    let default = b.create_block();

    b.switch_to_block(entry);
    let disc = b.const_null(); // discriminant
    b.switch(disc, vec![case0, case1, case2, default]);
    b.seal_block(entry);

    b.switch_to_block(case0);
    let v0 = b.const_i32(0);
    b.ret(Some(v0));
    b.seal_block(case0);

    b.switch_to_block(case1);
    let v1 = b.const_i32(1);
    b.ret(Some(v1));
    b.seal_block(case1);

    b.switch_to_block(case2);
    let v2 = b.const_i32(2);
    b.ret(Some(v2));
    b.seal_block(case2);

    b.switch_to_block(default);
    let vd = b.const_i32(-1);
    b.ret(Some(vd));
    b.seal_block(default);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Switch with 3 cases should compile: {result:?}"
    );
}

#[test]
fn test_lower_this_value() {
    let module = build_jsvalue_module(|b, _bb| {
        let this = b.this_value();
        b.ret(Some(this));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "ThisValue should compile: {result:?}");
}

#[test]
fn test_lower_new_target() {
    let module = build_jsvalue_module(|b, _bb| {
        let nt = b.new_target();
        b.ret(Some(nt));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "NewTarget should compile: {result:?}");
}

#[test]
fn test_lower_nop() {
    let module = build_void_module(|b, _bb| {
        b.nop();
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "Nop should compile: {result:?}");
}

#[test]
fn test_lower_tdz_check() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let checked = b.tdz_check(val);
        b.ret(Some(checked));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "TdzCheck should compile: {result:?}");
}

#[test]
fn test_lower_tdz_init() {
    let module = build_void_module(|b, _bb| {
        let val = b.const_null();
        b.tdz_init(val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "TdzInit should compile: {result:?}");
}

#[test]
fn test_lower_string_char_at() {
    let module = build_jsvalue_module(|b, _bb| {
        let s = b.const_null();
        let idx = b.const_null();
        let ch = b.string_char_at(s, idx);
        b.ret(Some(ch));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "StringCharAt should compile: {result:?}");
}

#[test]
fn test_lower_string_compare() {
    let module = build_jsvalue_module(|b, _bb| {
        let a = b.const_null();
        let c = b.const_null();
        let cmp = b.string_compare(a, c);
        b.ret(Some(cmp));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "StringCompare should compile: {result:?}");
}

// ===========================================================================
// Multi-op integration tests
// ===========================================================================

#[test]
fn test_lower_closure_create_env_load_call() {
    let mut b = TypedIrBuilder::new();

    // Function 0: target function
    b.begin_function("target", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry — creates closure, loads from env, calls
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let env = b.env_create(2);
    let stored_val = b.const_null();
    b.env_store(env, 0, stored_val);
    let loaded = b.env_load(env, 0);
    let func_idx = b.const_i32(0);
    let flags = b.const_i32(0);
    let closure = b.create_closure(func_idx, env, flags);
    // Use loaded and closure values
    let _ = loaded;
    let _ = closure;
    let func_ref = b.const_i32(0);
    let call_result = b.call(func_ref, vec![]);
    b.ret(Some(call_result));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Closure+env+call integration should compile: {result:?}"
    );
}

#[test]
fn test_lower_object_get_set_delete_prop_chain() {
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let key = b.const_null();
        let val = b.const_null();
        b.set_prop(obj, key, val);
        let got = b.get_prop(obj, key);
        let _has = b.has_prop(obj, key);
        let _del = b.delete_prop(obj, key);
        b.ret(Some(got));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Object prop chain should compile: {result:?}"
    );
}

#[test]
fn test_lower_try_throw_catch_flow() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let entry = b.create_block();
    let try_body = b.create_block();
    let catch_bb = b.create_block();

    // Entry: jump to try body
    b.switch_to_block(entry);
    b.br(try_body);
    b.seal_block(entry);

    // Try body: begin try, throw
    b.switch_to_block(try_body);
    b.try_begin(catch_bb);
    let err = b.const_null();
    b.throw_(err);
    b.seal_block(try_body);

    // Catch: get exception, return it
    b.switch_to_block(catch_bb);
    let ex = b.catch_();
    b.ret(Some(ex));
    b.seal_block(catch_bb);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Try/throw/catch flow should compile: {result:?}"
    );
}

#[test]
fn test_lower_call_new_with_argv() {
    let module = build_jsvalue_module(|b, _bb| {
        let ctor = b.const_null();
        let a1 = b.const_null();
        let a2 = b.const_undefined();
        let a3 = b.const_null();
        let val = b.call_new(ctor, vec![a1, a2, a3]);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "CallNew with 3 args should compile: {result:?}"
    );
}

#[test]
fn test_lower_generator_create() {
    let module = build_jsvalue_module(|b, _bb| {
        let generator = b.generator_create();
        b.ret(Some(generator));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "GeneratorCreate should compile: {result:?}");
}

#[test]
fn test_lower_yield() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let result_val = b.yield_(val);
        b.ret(Some(result_val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "Yield should compile: {result:?}");
}

#[test]
fn test_lower_create_arguments() {
    let module = build_jsvalue_module(|b, _bb| {
        let args = b.create_arguments();
        b.ret(Some(args));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "CreateArguments should compile: {result:?}");
}

// ===========================================================================
// RuntimeCalls new signature tests (C9)
// ===========================================================================

#[test]
fn test_runtime_calls_ternary_cache() {
    let ctx = CompilationContext::new().unwrap();
    let mut module = ctx.object_module;
    let mut runtime = RuntimeCalls::new();

    let id1 = runtime
        .get_ternary_js_op("__esc_rt_set_prop", &mut module)
        .unwrap();
    let id2 = runtime
        .get_ternary_js_op("__esc_rt_set_prop", &mut module)
        .unwrap();
    assert_eq!(id1, id2, "cached ternary FuncId should be the same");
}

#[test]
fn test_runtime_calls_void_unary_cache() {
    let ctx = CompilationContext::new().unwrap();
    let mut module = ctx.object_module;
    let mut runtime = RuntimeCalls::new();

    let id1 = runtime
        .get_void_unary("__esc_rt_throw", &mut module)
        .unwrap();
    let id2 = runtime
        .get_void_unary("__esc_rt_throw", &mut module)
        .unwrap();
    assert_eq!(id1, id2, "cached void unary FuncId should be the same");
}

#[test]
fn test_runtime_calls_call_variadic_cache() {
    let ctx = CompilationContext::new().unwrap();
    let mut module = ctx.object_module;
    let mut runtime = RuntimeCalls::new();

    let id1 = runtime
        .get_call_variadic("__esc_rt_call_new", &mut module)
        .unwrap();
    let id2 = runtime
        .get_call_variadic("__esc_rt_call_new", &mut module)
        .unwrap();
    assert_eq!(id1, id2, "cached variadic FuncId should be the same");
}

#[test]
fn test_runtime_calls_env_store_cache() {
    let ctx = CompilationContext::new().unwrap();
    let mut module = ctx.object_module;
    let mut runtime = RuntimeCalls::new();

    let id1 = runtime
        .get_env_store("__esc_rt_env_store", &mut module)
        .unwrap();
    let id2 = runtime
        .get_env_store("__esc_rt_env_store", &mut module)
        .unwrap();
    assert_eq!(id1, id2, "cached env_store FuncId should be the same");
}

// ===========================================================================
// RC operations tests
// ===========================================================================

#[test]
fn test_lower_rc_inc_dec_strong() {
    let module = build_void_module(|b, _bb| {
        let val = b.const_null();
        b.rc_inc_strong(val);
        b.rc_dec_strong(val);
        b.ret(None);
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "RcIncStrong/RcDecStrong should compile: {result:?}"
    );
}

#[test]
fn test_lower_rc_is_unique() {
    let module = build_jsvalue_module(|b, _bb| {
        let val = b.const_null();
        let unique = b.rc_is_unique(val);
        b.ret(Some(unique));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(result.is_ok(), "RcIsUnique should compile: {result:?}");
}

// ===========================================================================
// Indirect call and dispatch trampoline tests
// ===========================================================================

#[test]
fn test_emit_call_direct_preserved() {
    // Regression test: a direct call with a ConstI32 function index still works.
    let mut b = TypedIrBuilder::new();

    // Function 0: helper that returns JSValue
    b.begin_function("helper", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry that calls function 0 via ConstI32
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let func_ref = b.const_i32(0);
    let result_val = b.call(func_ref, vec![]);
    b.ret(Some(result_val));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Direct call with ConstI32 func index should still compile: {result:?}"
    );
    assert!(
        !result.unwrap().is_empty(),
        "Direct call should produce non-empty object bytes"
    );
}

#[test]
fn test_emit_call_indirect_compiles() {
    // When the callee is a runtime closure value (not a ConstI32), the call
    // should fall back to __esc_rt_call_indirect.
    let mut b = TypedIrBuilder::new();

    // Function 0: target function
    b.begin_function("target", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry — creates a closure and calls it indirectly
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let env = b.env_create(1);
    let func_idx = b.const_i32(0);
    let flags = b.const_i32(0);
    let closure = b.create_closure(func_idx, env, flags);
    // Call the closure value — not a ConstI32, so indirect dispatch
    let arg = b.const_null();
    let call_result = b.call(closure, vec![arg]);
    b.ret(Some(call_result));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Indirect call through closure should compile: {result:?}"
    );
    assert!(
        !result.unwrap().is_empty(),
        "Indirect call should produce non-empty object bytes"
    );
}

#[test]
fn test_dispatch_trampoline_generated() {
    // Compile a module with 2 functions and verify the output is non-empty,
    // which means the __esc_dispatch trampoline was generated successfully.
    let mut b = TypedIrBuilder::new();

    // Function 0: simple helper
    b.begin_function("helper", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    b.switch_to_block(bb0);
    let v = b.const_null();
    b.ret(Some(v));
    b.seal_block(bb0);
    b.end_function();

    // Function 1: entry
    b.begin_function("main", vec![], IrType::JSValue);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    let r = b.const_null();
    b.ret(Some(r));
    b.seal_block(bb1);
    b.end_function();

    b.set_entry(1);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "Module with dispatch trampoline should compile: {result:?}"
    );
    let bytes = result.unwrap();
    assert!(
        !bytes.is_empty(),
        "Compiled module with trampoline should produce non-empty object bytes"
    );
}

#[test]
fn test_call_method_still_works() {
    // Regression test: CallMethod opcode still compiles correctly after
    // emit_call was modified to handle indirect calls.
    let module = build_jsvalue_module(|b, _bb| {
        let obj = b.create_object();
        let method = b.const_null();
        let arg1 = b.const_null();
        let arg2 = b.const_undefined();
        let val = b.call_method(obj, method, vec![arg1, arg2]);
        b.ret(Some(val));
    });
    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "CallMethod should still compile after emit_call changes: {result:?}"
    );
    assert!(
        !result.unwrap().is_empty(),
        "CallMethod should produce non-empty object bytes"
    );
}

// ===========================================================================
// Type coercion in specialized ops (regression tests for Cranelift verifier
// errors caused by phi nodes providing i64 values to specialized i32/f64 ops)
// ===========================================================================

#[test]
fn test_coerce_i32_add_with_phi_operand() {
    // Regression: specialization rewrites AddJS -> AddI32, but phi operands
    // are i64 (NaN-boxed). The codegen must unbox to i32 before iadd.
    //
    // IR pattern:
    //   bb0: x = const_i32 1; cond = const_bool(true); br_if cond, bb1, bb2
    //   bb1: phi_x = phi(x); y = const_i32 2; z = add_i32(phi_x, y); ret z
    //   bb2: ret const_i32 0
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(1);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_i32(2);
    // Emit AddI32 with phi operand (phi is jsvalue/i64 but operands are i32)
    let z = b.add_i32(phi_x, y);
    let boxed = b.box_i32(z);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_i32(0);
    let boxed_zero = b.box_i32(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "AddI32 with phi operand (i64 coercion) should compile: {result:?}"
    );
}

#[test]
fn test_coerce_f64_add_with_phi_operand() {
    // Regression: specialization rewrites AddJS -> AddF64, but phi operands
    // are i64 (NaN-boxed). The codegen must unbox to f64 before fadd.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_f64(1.5);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_f64(2.5);
    // Emit AddF64 with phi operand (phi is jsvalue/i64 but operands are f64)
    let z = b.add_f64(phi_x, y);
    let boxed = b.box_f64(z);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_f64(0.0);
    let boxed_zero = b.box_f64(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "AddF64 with phi operand (i64 coercion) should compile: {result:?}"
    );
}

#[test]
fn test_coerce_i32_sub_with_phi_operand() {
    // Regression: SubI32 with phi providing i64 values.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(10);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_i32(3);
    let z = b.sub_i32(phi_x, y);
    let boxed = b.box_i32(z);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_i32(0);
    let boxed_zero = b.box_i32(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "SubI32 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_f64_sub_with_phi_operand() {
    // Regression: SubF64 with phi providing i64 values.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_f64(5.0);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_f64(1.5);
    let z = b.sub_f64(phi_x, y);
    let boxed = b.box_f64(z);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_f64(0.0);
    let boxed_zero = b.box_f64(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "SubF64 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_i32_comparison_with_phi_operand() {
    // Regression: LtI32 with phi providing i64 values.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(5);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_i32(3);
    let cmp = b.lt_i32(phi_x, y);
    let boxed = b.box_bool(cmp);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let f = b.const_bool(false);
    b.ret(Some(f));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "LtI32 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_f64_comparison_with_phi_operand() {
    // Regression: LtF64 with phi providing i64 values.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_f64(2.5);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let y = b.const_f64(2.0);
    let cmp = b.lt_f64(phi_x, y);
    let boxed = b.box_bool(cmp);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let f = b.const_bool(false);
    b.ret(Some(f));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "LtF64 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_neg_i32_with_phi_operand() {
    // Regression: NegI32 with phi providing i64 value.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(42);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let neg = b.neg_i32(phi_x);
    let boxed = b.box_i32(neg);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_i32(0);
    let boxed_zero = b.box_i32(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "NegI32 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_neg_f64_with_phi_operand() {
    // Regression: NegF64 with phi providing i64 value.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_f64(2.71);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let neg = b.neg_f64(phi_x);
    let boxed = b.box_f64(neg);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_f64(0.0);
    let boxed_zero = b.box_f64(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "NegF64 with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_bitwise_not_with_phi_operand() {
    // Regression: BitwiseNot with phi providing i64 value.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(0xFF);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let not = b.bitwise_not(phi_x);
    let boxed = b.box_i32(not);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_i32(0);
    let boxed_zero = b.box_i32(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "BitwiseNot with phi operand should compile: {result:?}"
    );
}

#[test]
fn test_coerce_mul_div_i32_with_phi_operand() {
    // Regression: MulI32 and DivI32 with phi providing i64 values.
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.switch_to_block(bb0);
    let x = b.const_i32(6);
    b.write_variable(0, x);
    let cond = b.const_bool(true);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    b.switch_to_block(bb1);
    let phi_x = b.read_variable(0, IrType::JSValue);
    let three = b.const_i32(3);
    let mul = b.mul_i32(phi_x, three);
    let two = b.const_i32(2);
    let div = b.div_i32(mul, two);
    let boxed = b.box_i32(div);
    b.ret(Some(boxed));
    b.seal_block(bb1);

    b.switch_to_block(bb2);
    let zero = b.const_i32(0);
    let boxed_zero = b.box_i32(zero);
    b.ret(Some(boxed_zero));
    b.seal_block(bb2);

    b.end_function();
    b.set_entry(0);
    let module = b.finish();

    let backend = CraneliftBackend::new().unwrap();
    let result = backend.compile_module(&module, &[]);
    assert!(
        result.is_ok(),
        "MulI32/DivI32 with phi operand should compile: {result:?}"
    );
}
