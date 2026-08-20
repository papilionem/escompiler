//! Consolidated unit tests for the types crate.
//!
//! Covers: lattice operations, type inference, trust categories, narrowing,
//! and type specialization.

use ir::ValueId;
use ir::builder::TypedIrBuilder;
use ir::types::{IrType, Op};

use crate::inference::{infer_function, infer_module};
use crate::lattice::{InferredType, is_subtype, join, meet};
use crate::narrowing::{narrow_nullish, narrow_truthiness, narrow_typeof};
use crate::specialize::{specialize_function, specialize_module};
use crate::trust::TrustCategory;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Helper: build a single-block function with the given body.
/// Returns the TypedFunction.
fn build_single_block(
    body: impl FnOnce(&mut TypedIrBuilder) -> Vec<ValueId>,
) -> (ir::builder::TypedFunction, Vec<ValueId>) {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let values = body(&mut b);
    b.ret(None);
    b.end_function();
    let module = b.finish();
    (module.functions.into_iter().next().unwrap(), values)
}

// ===========================================================================
// Inference tests
// ===========================================================================

#[test]
fn test_const_types() {
    let (func, vals) = build_single_block(|b| {
        let v0 = b.const_i32(42);
        let v1 = b.const_f64(2.5);
        let v2 = b.const_bool(true);
        vec![v0, v1, v2]
    });

    let ann = infer_function(&func);
    assert_eq!(ann.get_type(vals[0]), &InferredType::Concrete(IrType::I32));
    assert_eq!(ann.get_type(vals[1]), &InferredType::Concrete(IrType::F64));
    assert_eq!(ann.get_type(vals[2]), &InferredType::Concrete(IrType::Bool));
    assert_eq!(ann.get_trust(vals[0]), TrustCategory::Provable);
    assert_eq!(ann.get_trust(vals[1]), TrustCategory::Provable);
    assert_eq!(ann.get_trust(vals[2]), TrustCategory::Provable);
}

#[test]
fn test_arithmetic_types() {
    let (func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let sum_i32 = b.add_i32(a, c);
        let x = b.const_f64(1.0);
        let y = b.const_f64(2.0);
        let sum_f64 = b.add_f64(x, y);
        let p = b.const_null();
        let q = b.const_null();
        let sum_js = b.add_js(p, q);
        vec![sum_i32, sum_f64, sum_js]
    });

    let ann = infer_function(&func);
    assert_eq!(ann.get_type(vals[0]), &InferredType::Concrete(IrType::I32));
    assert_eq!(ann.get_type(vals[1]), &InferredType::Concrete(IrType::F64));
    assert_eq!(
        ann.get_type(vals[2]),
        &InferredType::Concrete(IrType::JSValue)
    );
}

#[test]
fn test_comparison_types() {
    let (func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let eq = b.eq_i32(a, c);
        let lt = b.lt_js(a, c);
        let strict_eq = b.eq_strict(a, c);
        vec![eq, lt, strict_eq]
    });

    let ann = infer_function(&func);
    for v in &vals {
        assert_eq!(ann.get_type(*v), &InferredType::Concrete(IrType::Bool));
    }
}

#[test]
fn test_phi_join() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_phi", vec![], IrType::Void);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    // bb0: branch to bb1 or bb2
    b.switch_to_block(bb0);
    let cond = b.const_bool(true);
    let i32_val = b.const_i32(42);
    b.write_variable(0, i32_val);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    // bb1: define var 0 as f64
    b.switch_to_block(bb1);
    b.add_predecessor(bb1, bb0);
    let f64_val = b.const_f64(2.5);
    b.write_variable(0, f64_val);
    b.br(bb2);
    b.seal_block(bb1);

    // bb2: phi merge
    b.switch_to_block(bb2);
    b.add_predecessor(bb2, bb0);
    b.add_predecessor(bb2, bb1);
    let phi_val = b.read_variable(0, IrType::JSValue);
    b.ret(None);
    b.seal_block(bb2);

    b.end_function();
    let module = b.finish();
    let func = &module.functions[0];
    let ann = infer_function(func);

    let phi_type = ann.get_type(phi_val);
    // The phi merges I32 and F64, so it should be a Union.
    match phi_type {
        InferredType::Union(types) => {
            assert!(types.contains(&IrType::I32));
            assert!(types.contains(&IrType::F64));
        }
        _ => panic!("expected Union, got {phi_type:?}"),
    }
}

#[test]
fn test_narrowing_guard_type() {
    let (func, vals) = build_single_block(|b| {
        let val = b.const_null();
        let tag = b.const_i32(1); // tag for "number" guard
        let guarded = b.guard_type(val, tag);
        vec![guarded]
    });

    let ann = infer_function(&func);
    match ann.get_type(vals[0]) {
        InferredType::Narrowed(_) => {} // expected
        other => panic!("expected Narrowed, got {other:?}"),
    }
}

#[test]
fn test_call_return_type() {
    let (func, vals) = build_single_block(|b| {
        let callee = b.const_null(); // placeholder
        let result = b.call(callee, vec![]);
        vec![result]
    });

    let ann = infer_function(&func);
    assert_eq!(ann.get_type(vals[0]), &InferredType::Unknown);
    assert_eq!(ann.get_trust(vals[0]), TrustCategory::Untyped);
}

#[test]
fn test_box_unbox_types() {
    let (func, vals) = build_single_block(|b| {
        let i = b.const_i32(7);
        let boxed = b.box_i32(i);
        let unboxed = b.unbox_i32(boxed);
        let f = b.const_f64(2.0);
        let boxed_f = b.box_f64(f);
        let unboxed_f = b.unbox_f64(boxed_f);
        vec![boxed, unboxed, boxed_f, unboxed_f]
    });

    let ann = infer_function(&func);
    assert_eq!(
        ann.get_type(vals[0]),
        &InferredType::Concrete(IrType::JSValue)
    );
    assert_eq!(ann.get_type(vals[1]), &InferredType::Concrete(IrType::I32));
    assert_eq!(
        ann.get_type(vals[2]),
        &InferredType::Concrete(IrType::JSValue)
    );
    assert_eq!(ann.get_type(vals[3]), &InferredType::Concrete(IrType::F64));
    // Box is Provable, unbox is External (may fail at runtime).
    assert_eq!(ann.get_trust(vals[0]), TrustCategory::Provable);
    assert_eq!(ann.get_trust(vals[1]), TrustCategory::External);
}

#[test]
fn test_unknown_propagation() {
    let (func, vals) = build_single_block(|b| {
        // Call returns Unknown.
        let callee = b.const_null();
        let unknown_val = b.call(callee, vec![]);
        // Comparison with unknown still produces Bool (known result type).
        let cmp = b.eq_strict(unknown_val, unknown_val);
        vec![unknown_val, cmp]
    });

    let ann = infer_function(&func);
    assert_eq!(ann.get_type(vals[0]), &InferredType::Unknown);
    assert_eq!(ann.get_type(vals[1]), &InferredType::Concrete(IrType::Bool));
}

#[test]
fn test_trust_categories() {
    let (func, vals) = build_single_block(|b| {
        let constant = b.const_i32(10);
        let callee = b.const_null();
        let from_call = b.call(callee, vec![]);
        vec![constant, from_call]
    });

    let ann = infer_function(&func);
    assert_eq!(ann.get_trust(vals[0]), TrustCategory::Provable);
    assert_eq!(ann.get_trust(vals[1]), TrustCategory::Untyped);
}

#[test]
fn test_create_object_array_closure() {
    let (func, vals) = build_single_block(|b| {
        let obj = b.create_object();
        let arr = b.create_array(vec![]);
        let fn_val = b.const_null();
        let env = b.const_null();
        let flags = b.const_i32(0);
        let closure = b.create_closure(fn_val, env, flags);
        vec![obj, arr, closure]
    });

    let ann = infer_function(&func);
    assert_eq!(
        ann.get_type(vals[0]),
        &InferredType::Concrete(IrType::JSObject)
    );
    assert_eq!(
        ann.get_type(vals[1]),
        &InferredType::Concrete(IrType::JSArray)
    );
    assert_eq!(
        ann.get_type(vals[2]),
        &InferredType::Concrete(IrType::JSFunction)
    );
}

#[test]
fn test_string_operations() {
    let (func, vals) = build_single_block(|b| {
        let s1 = b.const_string(0);
        let s2 = b.const_string(1);
        let cat = b.string_concat(s1, s2);
        let len = b.string_length(s1);
        vec![s1, cat, len]
    });

    let ann = infer_function(&func);
    assert_eq!(
        ann.get_type(vals[0]),
        &InferredType::Concrete(IrType::JSString)
    );
    assert_eq!(
        ann.get_type(vals[1]),
        &InferredType::Concrete(IrType::JSString)
    );
    assert_eq!(ann.get_type(vals[2]), &InferredType::Concrete(IrType::I32));
}

#[test]
fn test_infer_module() {
    let mut b = TypedIrBuilder::new();

    b.begin_function("fn1", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    b.const_i32(1);
    b.ret(None);
    b.end_function();

    b.begin_function("fn2", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    b.const_f64(2.0);
    b.ret(None);
    b.end_function();

    let module = b.finish();
    let annotations = infer_module(&module);
    assert_eq!(annotations.len(), 2);
}

// ===========================================================================
// Trust category tests
// ===========================================================================

#[test]
fn test_trust_ordering() {
    assert!(TrustCategory::Provable > TrustCategory::Annotated);
    assert!(TrustCategory::Annotated > TrustCategory::External);
    assert!(TrustCategory::External > TrustCategory::Untyped);
}

#[test]
fn test_is_trusted() {
    assert!(TrustCategory::Provable.is_trusted());
    assert!(TrustCategory::Annotated.is_trusted());
    assert!(!TrustCategory::External.is_trusted());
    assert!(!TrustCategory::Untyped.is_trusted());
}

#[test]
fn test_can_skip_check() {
    assert!(TrustCategory::Provable.can_skip_check());
    assert!(!TrustCategory::Annotated.can_skip_check());
    assert!(!TrustCategory::External.can_skip_check());
    assert!(!TrustCategory::Untyped.can_skip_check());
}

#[test]
fn test_merge_conservative() {
    assert_eq!(
        TrustCategory::merge(TrustCategory::Provable, TrustCategory::External),
        TrustCategory::External
    );
    assert_eq!(
        TrustCategory::merge(TrustCategory::Annotated, TrustCategory::Provable),
        TrustCategory::Annotated
    );
    assert_eq!(
        TrustCategory::merge(TrustCategory::Untyped, TrustCategory::Provable),
        TrustCategory::Untyped
    );
    assert_eq!(
        TrustCategory::merge(TrustCategory::External, TrustCategory::External),
        TrustCategory::External
    );
}

// ===========================================================================
// Lattice tests
// ===========================================================================

#[test]
fn test_lattice_join_same_concrete() {
    let a = InferredType::Concrete(IrType::I32);
    let b = InferredType::Concrete(IrType::I32);
    assert_eq!(join(&a, &b), InferredType::Concrete(IrType::I32));
}

#[test]
fn test_lattice_join_different_concrete() {
    let a = InferredType::Concrete(IrType::I32);
    let b = InferredType::Concrete(IrType::F64);
    assert_eq!(
        join(&a, &b),
        InferredType::Union(vec![IrType::I32, IrType::F64])
    );
}

#[test]
fn test_lattice_join_unknown() {
    let a = InferredType::Unknown;
    let b = InferredType::Concrete(IrType::I32);
    assert_eq!(join(&a, &b), InferredType::Unknown);
    assert_eq!(join(&b, &a), InferredType::Unknown);
}

#[test]
fn test_lattice_unreachable() {
    let a = InferredType::Unreachable;
    let b = InferredType::Concrete(IrType::F64);
    assert_eq!(join(&a, &b), InferredType::Concrete(IrType::F64));
    assert_eq!(join(&b, &a), InferredType::Concrete(IrType::F64));
}

#[test]
fn test_lattice_union_join_concrete() {
    let a = InferredType::Union(vec![IrType::I32, IrType::F64]);
    let b = InferredType::Concrete(IrType::Bool);
    let result = join(&a, &b);
    assert_eq!(
        result,
        InferredType::Union(vec![IrType::I32, IrType::F64, IrType::Bool])
    );
}

#[test]
fn test_lattice_union_join_concrete_already_present() {
    let a = InferredType::Union(vec![IrType::I32, IrType::F64]);
    let b = InferredType::Concrete(IrType::I32);
    let result = join(&a, &b);
    assert_eq!(result, InferredType::Union(vec![IrType::I32, IrType::F64]));
}

#[test]
fn test_lattice_meet_same() {
    let a = InferredType::Concrete(IrType::I32);
    assert_eq!(meet(&a, &a), InferredType::Concrete(IrType::I32));
}

#[test]
fn test_lattice_meet_different() {
    let a = InferredType::Concrete(IrType::I32);
    let b = InferredType::Concrete(IrType::F64);
    assert_eq!(meet(&a, &b), InferredType::Unreachable);
}

#[test]
fn test_lattice_meet_unknown() {
    let a = InferredType::Unknown;
    let b = InferredType::Concrete(IrType::I32);
    assert_eq!(meet(&a, &b), InferredType::Concrete(IrType::I32));
}

#[test]
fn test_subtype_concrete() {
    let a = InferredType::Concrete(IrType::I32);
    let b = InferredType::Concrete(IrType::I32);
    assert!(is_subtype(&a, &b));
}

#[test]
fn test_subtype_concrete_of_union() {
    let a = InferredType::Concrete(IrType::I32);
    let b = InferredType::Union(vec![IrType::I32, IrType::F64]);
    assert!(is_subtype(&a, &b));
}

#[test]
fn test_subtype_unreachable_of_anything() {
    assert!(is_subtype(
        &InferredType::Unreachable,
        &InferredType::Concrete(IrType::I32)
    ));
    assert!(is_subtype(
        &InferredType::Unreachable,
        &InferredType::Unknown
    ));
}

#[test]
fn test_subtype_anything_of_unknown() {
    assert!(is_subtype(
        &InferredType::Concrete(IrType::F64),
        &InferredType::Unknown
    ));
}

#[test]
fn test_is_concrete() {
    assert!(InferredType::Concrete(IrType::I32).is_concrete());
    assert!(!InferredType::Unknown.is_concrete());
}

#[test]
fn test_is_unknown() {
    assert!(InferredType::Unknown.is_unknown());
    assert!(!InferredType::Concrete(IrType::I32).is_unknown());
}

#[test]
fn test_is_unreachable() {
    assert!(InferredType::Unreachable.is_unreachable());
    assert!(!InferredType::Unknown.is_unreachable());
}

// ===========================================================================
// Narrowing tests
// ===========================================================================

#[test]
fn test_narrow_typeof_number() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "number"),
        InferredType::Concrete(IrType::F64)
    );
}

#[test]
fn test_narrow_typeof_string() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "string"),
        InferredType::Concrete(IrType::JSString)
    );
}

#[test]
fn test_narrow_typeof_boolean() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "boolean"),
        InferredType::Concrete(IrType::Bool)
    );
}

#[test]
fn test_narrow_typeof_object() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "object"),
        InferredType::Concrete(IrType::JSObject)
    );
}

#[test]
fn test_narrow_typeof_function() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "function"),
        InferredType::Concrete(IrType::JSFunction)
    );
}

#[test]
fn test_narrow_typeof_undefined() {
    let ty = InferredType::Unknown;
    assert_eq!(
        narrow_typeof(&ty, "undefined"),
        InferredType::Concrete(IrType::JSValue)
    );
}

#[test]
fn test_narrow_typeof_unknown_string() {
    let ty = InferredType::Unknown;
    // Unknown typeof result should not change the type.
    assert_eq!(narrow_typeof(&ty, "bigint_custom"), InferredType::Unknown);
}

#[test]
fn test_narrow_typeof_unreachable() {
    assert_eq!(
        narrow_typeof(&InferredType::Unreachable, "number"),
        InferredType::Unreachable
    );
}

#[test]
fn test_narrow_truthiness_union_truthy() {
    let ty = InferredType::Union(vec![IrType::I32, IrType::JSValue, IrType::F64]);
    let result = narrow_truthiness(&ty, true);
    assert_eq!(result, InferredType::Union(vec![IrType::I32, IrType::F64]));
}

#[test]
fn test_narrow_truthiness_unreachable() {
    assert_eq!(
        narrow_truthiness(&InferredType::Unreachable, true),
        InferredType::Unreachable
    );
}

#[test]
fn test_narrow_nullish_not_nullish() {
    let ty = InferredType::Union(vec![IrType::I32, IrType::JSValue]);
    let result = narrow_nullish(&ty, false);
    assert_eq!(result, InferredType::Concrete(IrType::I32));
}

#[test]
fn test_narrow_nullish_is_nullish() {
    let ty = InferredType::Union(vec![IrType::I32, IrType::JSValue]);
    let result = narrow_nullish(&ty, true);
    assert_eq!(result, InferredType::Concrete(IrType::JSValue));
}

// ===========================================================================
// Specialization tests
// ===========================================================================

#[test]
fn test_specialize_add_js_to_add_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(2.0);
        let sum = b.add_js(a, c);
        vec![a, c, sum]
    });

    let ann = infer_function(&func);
    // Before specialization: AddJS
    let add_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[2])
        .expect("add instruction not found");
    assert_eq!(add_inst.op, Op::AddJS);
    assert_eq!(add_inst.ty, IrType::JSValue);

    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 1);

    // After specialization: AddF64
    let add_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[2])
        .expect("add instruction not found");
    assert_eq!(add_inst.op, Op::AddF64);
    assert_eq!(add_inst.ty, IrType::F64);
}

#[test]
fn test_specialize_sub_js_to_sub_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(10.0);
        let c = b.const_f64(3.0);
        let diff = b.sub_js(a, c);
        vec![diff]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 1);

    let sub_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("sub instruction not found");
    assert_eq!(sub_inst.op, Op::SubF64);
    assert_eq!(sub_inst.ty, IrType::F64);
}

#[test]
fn test_specialize_mul_div_mod_js_to_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(6.0);
        let c = b.const_f64(2.0);
        let mul = b.mul_js(a, c);
        let div = b.div_js(a, c);
        let modulo = b.mod_js(a, c);
        vec![mul, div, modulo]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 3);

    let ops: Vec<Op> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| vals.contains(&i.id))
        .map(|i| i.op.clone())
        .collect();
    assert_eq!(ops, vec![Op::MulF64, Op::DivF64, Op::ModF64]);
}

#[test]
fn test_i32_operands_do_not_specialize_arithmetic() {
    // Was `test_specialize_add_js_to_add_i32`, which asserted the opposite and so
    // certified an unsound rewrite as correct. AddI32 wraps at 32 bits; JS requires
    // the exact f64 sum. Two proven-I32 operands must leave AddJS alone.
    //
    // Kept as a NEGATIVE test rather than deleted: it now fails if anyone
    // reintroduces the I32 arithmetic arm. See docs/research/51-arithmetic-fix-scoping.md.
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let sum = b.add_js(a, c);
        vec![sum]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(
        stats.specialized_count, 0,
        "i32 operands must NOT specialize arithmetic — AddI32 wraps and JS does not"
    );

    let add_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("add instruction not found");
    assert_eq!(add_inst.op, Op::AddJS, "must remain the generic op");
    assert_ne!(
        add_inst.ty,
        IrType::I32,
        "and must not be typed I32 — the result of JS addition is a Number"
    );
}

#[test]
fn test_specialize_neg_js_to_neg_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(5.0);
        let neg = b.neg_js(a);
        vec![neg]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 1);

    let neg_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("neg instruction not found");
    assert_eq!(neg_inst.op, Op::NegF64);
    assert_eq!(neg_inst.ty, IrType::F64);
}

#[test]
fn test_specialize_comparison_lt_js_to_lt_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(2.0);
        let lt = b.lt_js(a, c);
        let le = b.le_js(a, c);
        let gt = b.gt_js(a, c);
        let ge = b.ge_js(a, c);
        vec![lt, le, gt, ge]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 4);

    let ops: Vec<Op> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| vals.contains(&i.id))
        .map(|i| i.op.clone())
        .collect();
    assert_eq!(ops, vec![Op::LtF64, Op::LeF64, Op::GtF64, Op::GeF64]);
}

#[test]
fn test_specialize_eq_abstract_same_type() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(1.0);
        let eq = b.eq_abstract(a, c);
        vec![eq]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 1);

    let eq_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("eq instruction not found");
    assert_eq!(eq_inst.op, Op::EqF64);
}

#[test]
fn test_no_specialize_mixed_types() {
    let (mut func, _vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_i32(2);
        let sum = b.add_js(a, c);
        vec![sum]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    // Mixed F64 + I32 should NOT be specialized
    assert_eq!(stats.specialized_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

#[test]
fn test_no_specialize_unknown_operands() {
    let (mut func, _vals) = build_single_block(|b| {
        let callee = b.const_null();
        let unknown = b.call(callee, vec![]);
        let known = b.const_f64(1.0);
        let sum = b.add_js(unknown, known);
        vec![sum]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

#[test]
fn test_specialize_module_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    // Function 1: add two f64 constants
    b.begin_function("add_f64", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let a = b.const_f64(1.0);
    let c = b.const_f64(2.0);
    b.add_js(a, c);
    b.ret(None);
    b.end_function();

    // Function 2: add two i32 constants
    b.begin_function("add_i32", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let x = b.const_i32(10);
    let y = b.const_i32(20);
    b.add_js(x, y);
    b.ret(None);
    b.end_function();

    let mut module = b.finish();
    let stats = specialize_module(&mut module);
    // Only the f64 function specializes. The i32 one must not: AddI32 wraps at
    // 32 bits and JS arithmetic is exact f64. Was `2` when the unsound I32 arm
    // existed.
    assert_eq!(
        stats.specialized_count, 1,
        "the f64 add specializes; the i32 add must stay generic"
    );
}

#[test]
fn test_specialize_preserves_non_specializable_ops() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(2.0);
        // These are already typed ops — should not be touched
        let sum = b.add_f64(a, c);
        let eq = b.eq_strict(a, c);
        vec![sum, eq]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 0);
    assert_eq!(stats.skipped_count, 0);

    // Verify instructions are unchanged
    let add_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("add instruction not found");
    assert_eq!(add_inst.op, Op::AddF64);

    let eq_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[1])
        .expect("eq instruction not found");
    assert_eq!(eq_inst.op, Op::EqStrict);
}

#[test]
fn test_specialize_comparison_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let lt = b.lt_js(a, c);
        let ge = b.ge_js(a, c);
        vec![lt, ge]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 2);

    let ops: Vec<Op> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| vals.contains(&i.id))
        .map(|i| i.op.clone())
        .collect();
    assert_eq!(ops, vec![Op::LtI32, Op::GeI32]);
}

#[test]
fn test_i32_operand_does_not_specialize_negation() {
    // Was `test_specialize_neg_js_to_neg_i32`. NegI32 wraps at i32::MIN (negating
    // it yields itself) and cannot produce -0, which JS distinguishes from 0 via
    // `1 / -0 === -Infinity`. Negative test, for the same reason as the add case.
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(5);
        let neg = b.neg_js(a);
        vec![neg]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(
        stats.specialized_count, 0,
        "i32 operand must NOT specialize negation — NegI32 cannot represent -0"
    );

    let neg_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("neg instruction not found");
    assert_eq!(neg_inst.op, Op::NegJS, "must remain the generic op");
    assert_ne!(
        neg_inst.ty,
        IrType::I32,
        "and must not be typed I32 — NegI32 cannot represent -0"
    );
}

#[test]
fn test_specialize_eq_abstract_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(1);
        let eq = b.eq_abstract(a, c);
        let ne = b.ne_abstract(a, c);
        vec![eq, ne]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 2);

    let ops: Vec<Op> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| vals.contains(&i.id))
        .map(|i| i.op.clone())
        .collect();
    assert_eq!(ops, vec![Op::EqI32, Op::NeI32]);
}

#[test]
fn test_specialize_eq_abstract_string_to_strict() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_string(0);
        let c = b.const_string(1);
        let eq = b.eq_abstract(a, c);
        vec![eq]
    });

    let ann = infer_function(&func);
    let stats = specialize_function(&mut func, &ann);
    assert_eq!(stats.specialized_count, 1);

    let eq_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("eq instruction not found");
    // Strings: same type => abstract == strict, use EqStrict
    assert_eq!(eq_inst.op, Op::EqStrict);
}

// ===========================================================================
// Constant folding tests
// ===========================================================================

use crate::constfold::{constfold_function, constfold_module};

// ---------------------------------------------------------------------------
// Integer arithmetic folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_add_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(2);
        let c = b.const_i32(3);
        let sum = b.add_i32(a, c);
        vec![sum]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(5));
    assert_eq!(inst.ty, IrType::I32);
    assert!(inst.operands.is_empty());
}

#[test]
fn test_constfold_sub_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(3);
        let diff = b.sub_i32(a, c);
        vec![diff]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(7));
}

#[test]
fn test_constfold_mul_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(4);
        let c = b.const_i32(5);
        let prod = b.mul_i32(a, c);
        vec![prod]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(20));
}

#[test]
fn test_constfold_div_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(2);
        let quot = b.div_i32(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(5));
}

#[test]
fn test_constfold_mod_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(3);
        let rem = b.mod_i32(a, c);
        vec![rem]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(1));
}

#[test]
fn test_constfold_neg_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(5);
        let neg = b.neg_i32(a);
        vec![neg]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(-5));
}

// ---------------------------------------------------------------------------
// Float arithmetic folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_add_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(2.0);
        let c = b.const_f64(3.0);
        let sum = b.add_f64(a, c);
        vec![sum]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(5.0));
    assert_eq!(inst.ty, IrType::F64);
}

#[test]
fn test_constfold_sub_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(10.0);
        let c = b.const_f64(3.0);
        let diff = b.sub_f64(a, c);
        vec![diff]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(7.0));
}

#[test]
fn test_constfold_mul_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(4.0);
        let c = b.const_f64(5.0);
        let prod = b.mul_f64(a, c);
        vec![prod]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(20.0));
}

#[test]
fn test_constfold_div_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(10.0);
        let c = b.const_f64(2.0);
        let quot = b.div_f64(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(5.0));
}

#[test]
fn test_constfold_mod_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(10.0);
        let c = b.const_f64(3.0);
        let rem = b.mod_f64(a, c);
        vec![rem]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(1.0));
}

#[test]
fn test_constfold_neg_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(5.0);
        let neg = b.neg_f64(a);
        vec![neg]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(-5.0));
}

// ---------------------------------------------------------------------------
// Float division edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_div_f64_by_zero() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(0.0);
        let quot = b.div_f64(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    // 1.0 / 0.0 = Infinity (IEEE 754)
    assert_eq!(inst.op, Op::ConstF64(f64::INFINITY));
}

#[test]
fn test_constfold_div_f64_zero_by_zero() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(0.0);
        let c = b.const_f64(0.0);
        let quot = b.div_f64(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    // 0.0 / 0.0 = NaN (IEEE 754)
    match &inst.op {
        Op::ConstF64(v) => assert!(v.is_nan()),
        other => panic!("expected ConstF64(NaN), got {other:?}"),
    }
}

#[test]
fn test_constfold_div_f64_neg_by_zero() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(-1.0);
        let c = b.const_f64(0.0);
        let quot = b.div_f64(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    // -1.0 / 0.0 = -Infinity (IEEE 754)
    assert_eq!(inst.op, Op::ConstF64(f64::NEG_INFINITY));
}

// ---------------------------------------------------------------------------
// Integer division edge cases (not folded)
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_div_i32_by_zero_not_folded() {
    let (mut func, _vals) = build_single_block(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(0);
        let quot = b.div_i32(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    // Division by zero for i32 is NOT folded (would trap at runtime).
    assert_eq!(stats.folded_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

#[test]
fn test_constfold_mod_i32_by_zero_not_folded() {
    let (mut func, _vals) = build_single_block(|b| {
        let a = b.const_i32(10);
        let c = b.const_i32(0);
        let rem = b.mod_i32(a, c);
        vec![rem]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

#[test]
fn test_constfold_div_i32_min_by_neg1_not_folded() {
    let (mut func, _vals) = build_single_block(|b| {
        let a = b.const_i32(i32::MIN);
        let c = b.const_i32(-1);
        let quot = b.div_i32(a, c);
        vec![quot]
    });

    let stats = constfold_function(&mut func);
    // i32::MIN / -1 overflows — not folded.
    assert_eq!(stats.folded_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

// ---------------------------------------------------------------------------
// Comparison folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_lt_i32() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(5);
        let c = b.const_i32(10);
        let lt = b.lt_i32(a, c);
        vec![lt]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
    assert_eq!(inst.ty, IrType::Bool);
}

#[test]
fn test_constfold_eq_i32_true() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(5);
        let c = b.const_i32(5);
        let eq = b.eq_i32(a, c);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
}

#[test]
fn test_constfold_eq_i32_false() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(5);
        let c = b.const_i32(10);
        let eq = b.eq_i32(a, c);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(false));
}

#[test]
fn test_constfold_lt_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(1.0);
        let c = b.const_f64(2.0);
        let lt = b.lt_f64(a, c);
        vec![lt]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
}

#[test]
fn test_constfold_eq_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(5.0);
        let c = b.const_f64(5.0);
        let eq = b.eq_f64(a, c);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
}

#[test]
fn test_constfold_all_i32_comparisons() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(3);
        let c = b.const_i32(5);
        let eq = b.eq_i32(a, c);
        let ne = b.ne_i32(a, c);
        let lt = b.lt_i32(a, c);
        let le = b.le_i32(a, c);
        let gt = b.gt_i32(a, c);
        let ge = b.ge_i32(a, c);
        vec![eq, ne, lt, le, gt, ge]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 6);

    let ops: Vec<Op> = func.blocks[0]
        .instructions
        .iter()
        .filter(|i| vals.contains(&i.id))
        .map(|i| i.op.clone())
        .collect();
    // 3 == 5 -> false, 3 != 5 -> true, 3 < 5 -> true, 3 <= 5 -> true, 3 > 5 -> false, 3 >= 5 -> false
    assert_eq!(
        ops,
        vec![
            Op::ConstBool(false),
            Op::ConstBool(true),
            Op::ConstBool(true),
            Op::ConstBool(true),
            Op::ConstBool(false),
            Op::ConstBool(false),
        ]
    );
}

// ---------------------------------------------------------------------------
// Boolean strict equality folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_eq_strict_bool_true_false() {
    // This is the pattern generated by `!true`: EqStrict(ConstBool(true), ConstBool(false))
    let (mut func, vals) = build_single_block(|b| {
        let t = b.const_bool(true);
        let f = b.const_bool(false);
        let eq = b.eq_strict(t, f);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(false));
}

#[test]
fn test_constfold_eq_strict_bool_true_true() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_bool(true);
        let c = b.const_bool(true);
        let eq = b.eq_strict(a, c);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
}

#[test]
fn test_constfold_ne_strict_bool() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_bool(true);
        let c = b.const_bool(false);
        let ne = b.ne_strict(a, c);
        vec![ne]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstBool(true));
}

// ---------------------------------------------------------------------------
// Non-constant operands should NOT be folded
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_non_constant_operand_not_folded() {
    let (mut func, _vals) = build_single_block(|b| {
        let callee = b.const_null();
        let unknown = b.call(callee, vec![]);
        let known = b.const_i32(5);
        let sum = b.add_i32(unknown, known);
        vec![sum]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 0);
    assert_eq!(stats.skipped_count, 1);
}

#[test]
fn test_constfold_preserves_non_foldable_ops() {
    let (mut func, vals) = build_single_block(|b| {
        let obj = b.create_object();
        let callee = b.const_null();
        let result = b.call(callee, vec![]);
        vec![obj, result]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 0);
    assert_eq!(stats.skipped_count, 0);

    // Instructions should be unchanged
    let obj_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(obj_inst.op, Op::CreateObject);
}

// ---------------------------------------------------------------------------
// Chained folding (result of one fold feeds into another)
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_chained() {
    // Tests that the result of one fold can be used by a subsequent fold:
    // a = 2 + 3 = 5, b = 5 * 4 = 20
    let (mut func, vals) = build_single_block(|b| {
        let c2 = b.const_i32(2);
        let c3 = b.const_i32(3);
        let sum = b.add_i32(c2, c3);
        let c4 = b.const_i32(4);
        let prod = b.mul_i32(sum, c4);
        vec![sum, prod]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 2);

    let sum_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("sum not found");
    assert_eq!(sum_inst.op, Op::ConstI32(5));

    let prod_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[1])
        .expect("prod not found");
    assert_eq!(prod_inst.op, Op::ConstI32(20));
}

// ---------------------------------------------------------------------------
// Module-level folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_module_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    // Function 1: 1.0 + 2.0
    b.begin_function("add_f64", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let a = b.const_f64(1.0);
    let c = b.const_f64(2.0);
    b.add_f64(a, c);
    b.ret(None);
    b.end_function();

    // Function 2: 10 - 3
    b.begin_function("sub_i32", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let x = b.const_i32(10);
    let y = b.const_i32(3);
    b.sub_i32(x, y);
    b.ret(None);
    b.end_function();

    let mut module = b.finish();
    let stats = constfold_module(&mut module);
    assert_eq!(stats.folded_count, 2);
}

// ---------------------------------------------------------------------------
// Specialization + constant folding integration
// ---------------------------------------------------------------------------

#[test]
fn test_specialize_then_constfold() {
    // AddJS(ConstF64(2.0), ConstF64(3.0))
    // -> specialization: AddF64(ConstF64(2.0), ConstF64(3.0))
    // -> constfold: ConstF64(5.0)
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(2.0);
        let c = b.const_f64(3.0);
        let sum = b.add_js(a, c);
        vec![sum]
    });

    // Specialize first
    let ann = infer_function(&func);
    let spec_stats = specialize_function(&mut func, &ann);
    assert_eq!(spec_stats.specialized_count, 1);

    // The op should now be AddF64
    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::AddF64);

    // Constant fold
    let fold_stats = constfold_function(&mut func);
    assert_eq!(fold_stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstF64(5.0));
    assert!(inst.operands.is_empty());
}

// ---------------------------------------------------------------------------
// NaN comparison folding
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_nan_eq_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(f64::NAN);
        let c = b.const_f64(f64::NAN);
        let eq = b.eq_f64(a, c);
        vec![eq]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    // NaN !== NaN per IEEE 754
    assert_eq!(inst.op, Op::ConstBool(false));
}

#[test]
fn test_constfold_nan_ne_f64() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_f64(f64::NAN);
        let c = b.const_f64(1.0);
        let ne = b.ne_f64(a, c);
        vec![ne]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    // NaN != 1.0 per IEEE 754
    assert_eq!(inst.op, Op::ConstBool(true));
}

// ---------------------------------------------------------------------------
// Wrapping integer arithmetic
// ---------------------------------------------------------------------------

#[test]
fn test_constfold_i32_overflow_wraps() {
    let (mut func, vals) = build_single_block(|b| {
        let a = b.const_i32(i32::MAX);
        let c = b.const_i32(1);
        let sum = b.add_i32(a, c);
        vec![sum]
    });

    let stats = constfold_function(&mut func);
    assert_eq!(stats.folded_count, 1);

    let inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.id == vals[0])
        .expect("instruction not found");
    assert_eq!(inst.op, Op::ConstI32(i32::MIN));
}

// ===========================================================================
// Proxy flag (may_be_proxy) tests
// ===========================================================================

#[test]
fn test_proxy_flag_object_literal_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let k = b.const_string(0);
        let v = b.const_i32(1);
        let obj = b.create_object_literal(vec![k, v]);
        vec![obj]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "object literal should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_array_literal_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let arr = b.create_array(vec![a, c]);
        vec![arr]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "array literal should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_function_parameter_is_proxy() {
    let (func, vals) = build_single_block(|b| {
        let param = b.load_param(0);
        vec![param]
    });

    let ann = infer_function(&func);
    assert!(
        ann.get_may_be_proxy(vals[0]),
        "function parameter should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_call_return_is_proxy() {
    let (func, vals) = build_single_block(|b| {
        let f = b.const_i32(0); // dummy function ref
        let result = b.call(f, vec![]);
        vec![result]
    });

    let ann = infer_function(&func);
    assert!(
        ann.get_may_be_proxy(vals[0]),
        "call return should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_primitive_constant_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let i = b.const_i32(42);
        let f = b.const_f64(2.5);
        let bo = b.const_bool(true);
        let s = b.const_string(0);
        let n = b.const_null();
        let u = b.const_undefined();
        vec![i, f, bo, s, n, u]
    });

    let ann = infer_function(&func);
    for (idx, val) in vals.iter().enumerate() {
        assert!(
            !ann.get_may_be_proxy(*val),
            "primitive constant {idx} should not be may_be_proxy"
        );
    }
}

#[test]
fn test_proxy_flag_property_read_is_proxy() {
    let (func, vals) = build_single_block(|b| {
        let obj = b.create_object();
        let key = b.const_string(0);
        let prop = b.get_prop(obj, key);
        vec![prop]
    });

    let ann = infer_function(&func);
    assert!(
        ann.get_may_be_proxy(vals[0]),
        "property read should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_phi_with_mixed_inputs() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_phi_proxy", vec![], IrType::Void);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    // bb0: branch to bb1 or bb2
    b.switch_to_block(bb0);
    let cond = b.const_bool(true);
    let obj = b.create_object(); // may_be_proxy = false
    b.write_variable(0, obj);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    // bb1: define var 0 as a parameter load (may_be_proxy = true)
    b.switch_to_block(bb1);
    b.add_predecessor(bb1, bb0);
    let param = b.load_param(0);
    b.write_variable(0, param);
    b.br(bb2);
    b.seal_block(bb1);

    // bb2: phi merge
    b.switch_to_block(bb2);
    b.add_predecessor(bb2, bb0);
    b.add_predecessor(bb2, bb1);
    let phi_val = b.read_variable(0, IrType::JSValue);
    b.ret(None);
    b.seal_block(bb2);

    b.end_function();
    let module = b.finish();
    let func = &module.functions[0];
    let ann = infer_function(func);

    // Phi merges a non-proxy (object literal) with a proxy-possible (param).
    // Conservative: the result should be may_be_proxy = true.
    assert!(
        ann.get_may_be_proxy(phi_val),
        "phi with mixed inputs should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_constructor_result_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let ctor = b.const_i32(0); // dummy constructor ref
        let obj = b.call_new(ctor, vec![]);
        vec![obj]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "constructor result should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_global_read_via_env_load_is_proxy() {
    let (func, vals) = build_single_block(|b| {
        let env = b.env_create(1);
        let val = b.env_load(env, 0);
        vec![val]
    });

    let ann = infer_function(&func);
    assert!(
        ann.get_may_be_proxy(vals[0]),
        "env load (closure capture) should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_binary_operator_result_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let sum = b.add_i32(a, c);
        let fa = b.const_f64(1.0);
        let fb = b.const_f64(2.0);
        let fsum = b.add_f64(fa, fb);
        vec![sum, fsum]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "i32 add result should not be may_be_proxy"
    );
    assert!(
        !ann.get_may_be_proxy(vals[1]),
        "f64 add result should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_create_closure_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let func_idx = b.const_i32(0);
        let env = b.env_create(0);
        let flags = b.const_i32(0);
        let closure = b.create_closure(func_idx, env, flags);
        vec![closure]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "closure should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_comparison_result_not_proxy() {
    let (func, vals) = build_single_block(|b| {
        let a = b.const_i32(1);
        let c = b.const_i32(2);
        let eq = b.eq_i32(a, c);
        vec![eq]
    });

    let ann = infer_function(&func);
    assert!(
        !ann.get_may_be_proxy(vals[0]),
        "comparison result should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_phi_all_non_proxy_inputs() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_phi_no_proxy", vec![], IrType::Void);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    // bb0: branch to bb1 or bb2
    b.switch_to_block(bb0);
    let cond = b.const_bool(true);
    let obj1 = b.create_object(); // may_be_proxy = false
    b.write_variable(0, obj1);
    b.br_if(cond, bb1, bb2);
    b.seal_block(bb0);

    // bb1: define var 0 as another non-proxy object
    b.switch_to_block(bb1);
    b.add_predecessor(bb1, bb0);
    let obj2 = b.create_array(vec![]); // may_be_proxy = false
    b.write_variable(0, obj2);
    b.br(bb2);
    b.seal_block(bb1);

    // bb2: phi merge
    b.switch_to_block(bb2);
    b.add_predecessor(bb2, bb0);
    b.add_predecessor(bb2, bb1);
    let phi_val = b.read_variable(0, IrType::JSValue);
    b.ret(None);
    b.seal_block(bb2);

    b.end_function();
    let module = b.finish();
    let func = &module.functions[0];
    let ann = infer_function(func);

    // Phi merges two non-proxy values — result should also be non-proxy.
    assert!(
        !ann.get_may_be_proxy(phi_val),
        "phi with all non-proxy inputs should not be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_box_load_is_proxy() {
    let (func, vals) = build_single_block(|b| {
        let init = b.const_i32(0);
        let bx = b.alloc_box(init);
        let loaded = b.box_load(bx);
        vec![loaded]
    });

    let ann = infer_function(&func);
    assert!(
        ann.get_may_be_proxy(vals[0]),
        "box load should be may_be_proxy"
    );
}

#[test]
fn test_proxy_flag_default_for_unknown_value() {
    // get_may_be_proxy should return true (conservative) for an unknown value ID.
    let ann = infer_function(&ir::builder::TypedFunction {
        name: "empty".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        is_generator: false,
        is_async: false,
    });

    // Query a value ID that doesn't exist — should return true (conservative default).
    assert!(
        ann.get_may_be_proxy(ValueId(999)),
        "unknown value should default to may_be_proxy = true"
    );
}
