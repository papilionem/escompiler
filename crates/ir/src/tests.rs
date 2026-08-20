// Consolidated tests for the ir crate (Rule 7: one tests.rs per crate).
//
// Tests originally from: builder.rs, verify.rs, types.rs, printer.rs, lib.rs

use super::*;
use crate::builder::{IrBuilder, TypedBasicBlock, TypedFunction, TypedIrBuilder, TypedModule};
use crate::printer::{
    format_ir_type, format_ir_type_full, format_op_name, print_function, print_typed_function,
    print_typed_module,
};
use crate::verify::{
    VerifyError, VerifyErrorKind, verify_function, verify_typed_function, verify_typed_module,
};
use crate::{Instruction, Type};
use common::{SourceSpan, StructTypeId};
use std::collections::HashSet;

// ===========================================================================
// Builder tests (from builder.rs)
// ===========================================================================

// -- Helper: build a simple function and return its module ---------------

fn build_simple_module() -> TypedModule {
    let mut b = TypedIrBuilder::new();
    b.begin_function("main", vec![], IrType::Void);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    b.finish()
}

// -- 1. Constants -------------------------------------------------------

#[test]
fn typed_builder_constants() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("constants", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let v0 = b.const_i32(42);
    let v1 = b.const_i64(100);
    let v2 = b.const_f64(2.5);
    let v3 = b.const_bool(true);
    let v4 = b.const_null();
    let v5 = b.const_undefined();
    let v6 = b.const_string(0);

    assert_eq!(v0, ValueId(0));
    assert_eq!(v1, ValueId(1));
    assert_eq!(v2, ValueId(2));
    assert_eq!(v3, ValueId(3));
    assert_eq!(v4, ValueId(4));
    assert_eq!(v5, ValueId(5));
    assert_eq!(v6, ValueId(6));

    b.ret(None);
    b.end_function();
    let m = b.finish();
    // 7 constants + 1 ret = 8 instructions
    assert_eq!(m.functions[0].blocks[0].instructions.len(), 8);
}

// -- 2. Arithmetic ------------------------------------------------------

#[test]
fn typed_builder_arithmetic() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("arith", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let a = b.const_i32(10);
    let c = b.const_i32(20);
    let sum = b.add_i32(a, c);
    assert_eq!(sum, ValueId(2));

    let x = b.const_f64(1.5);
    let y = b.const_f64(2.5);
    let diff = b.sub_f64(x, y);
    assert_eq!(diff, ValueId(5));

    let p = b.const_null();
    let q = b.const_null();
    let js_mul = b.mul_js(p, q);
    assert_eq!(js_mul, ValueId(8));

    // Verify Op variants
    let instrs = &b.blocks[0].instructions;
    assert_eq!(instrs[2].op, Op::AddI32);
    assert_eq!(instrs[2].ty, IrType::I32);
    assert_eq!(instrs[5].op, Op::SubF64);
    assert_eq!(instrs[5].ty, IrType::F64);
    assert_eq!(instrs[8].op, Op::MulJS);
    assert_eq!(instrs[8].ty, IrType::JSValue);

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 3. Fibonacci -------------------------------------------------------

#[test]
fn typed_builder_fibonacci() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("fib", vec![("n", IrType::I32)], IrType::I32);

    let entry = b.create_block();
    let base_case = b.create_block();
    let recurse = b.create_block();

    // entry: if n < 2, goto base_case, else recurse
    b.switch_to_block(entry);
    let n = b.const_i32(0); // placeholder for param
    let two = b.const_i32(2);
    let cmp = b.lt_i32(n, two);
    b.br_if(cmp, base_case, recurse);

    // base_case: return n
    b.switch_to_block(base_case);
    b.ret(Some(n));

    // recurse: return fib(n-1) + fib(n-2)
    b.switch_to_block(recurse);
    let one = b.const_i32(1);
    let n_minus_1 = b.sub_i32(n, one);
    let n_minus_2 = b.sub_i32(n, two);
    // Simulate calls as add (simplified test)
    let result = b.add_i32(n_minus_1, n_minus_2);
    b.ret(Some(result));

    b.end_function();
    let m = b.finish();

    assert_eq!(m.functions[0].blocks.len(), 3);
    assert_eq!(m.functions[0].name, "fib");
    assert_eq!(m.functions[0].return_type, IrType::I32);
}

// -- 4. If/else with phi ------------------------------------------------

#[test]
fn typed_builder_if_else_phi() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("if_else", vec![], IrType::I32);

    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();
    let merge = b.create_block();

    // entry
    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.seal_block(entry);

    // then
    b.switch_to_block(then_bb);
    b.add_predecessor(then_bb, entry);
    let val_then = b.const_i32(1);
    b.write_variable(0, val_then);
    b.br(merge);
    b.seal_block(then_bb);

    // else
    b.switch_to_block(else_bb);
    b.add_predecessor(else_bb, entry);
    let val_else = b.const_i32(2);
    b.write_variable(0, val_else);
    b.br(merge);
    b.seal_block(else_bb);

    // merge — phi
    b.switch_to_block(merge);
    b.add_predecessor(merge, then_bb);
    b.add_predecessor(merge, else_bb);
    b.seal_block(merge);

    let result = b.read_variable(0, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    let m = b.finish();

    // merge block should have a phi + ret
    let merge_block = &m.functions[0].blocks[3];
    assert!(
        merge_block.instructions.iter().any(|i| i.op == Op::Phi),
        "merge block should contain a Phi instruction"
    );
    // Phi should have 2 operands (from then and else)
    let phi = merge_block
        .instructions
        .iter()
        .find(|i| i.op == Op::Phi)
        .unwrap();
    assert_eq!(phi.operands.len(), 2);
    assert_eq!(phi.block_targets.len(), 2);
}

// -- 5. For-loop --------------------------------------------------------

#[test]
fn typed_builder_for_loop() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("loop", vec![], IrType::I32);

    let entry = b.create_block();
    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    // entry: i = 0
    b.switch_to_block(entry);
    let zero = b.const_i32(0);
    b.write_variable(0, zero); // i = 0
    b.br(header);
    b.seal_block(entry);

    // header: if i < 10 goto body else exit
    b.switch_to_block(header);
    b.add_predecessor(header, entry);
    b.add_predecessor(header, body);
    // Don't seal yet, body hasn't been filled

    let i = b.read_variable(0, IrType::I32);
    let ten = b.const_i32(10);
    let cmp = b.lt_i32(i, ten);
    b.br_if(cmp, body, exit);

    // body: i = i + 1
    b.switch_to_block(body);
    b.add_predecessor(body, header);
    let i_in_body = b.read_variable(0, IrType::I32);
    let one = b.const_i32(1);
    let next_i = b.add_i32(i_in_body, one);
    b.write_variable(0, next_i);
    b.br(header);
    b.seal_block(body);

    // Now seal header (all predecessors known)
    b.seal_block(header);

    // exit
    b.switch_to_block(exit);
    b.add_predecessor(exit, header);
    b.seal_block(exit);
    let final_i = b.read_variable(0, IrType::I32);
    b.ret(Some(final_i));

    b.end_function();
    let m = b.finish();

    assert_eq!(m.functions[0].blocks.len(), 4);
    // Header should have a phi for variable 0
    let header_block = &m.functions[0].blocks[1];
    assert!(header_block.instructions.iter().any(|i| i.op == Op::Phi));
}

// -- 6. Control flow: br, br_if, ret ------------------------------------

#[test]
fn typed_builder_control_flow() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("cf", vec![], IrType::Void);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();
    let bb3 = b.create_block();

    b.switch_to_block(bb0);
    let cond = b.const_bool(false);
    b.br_if(cond, bb1, bb2);

    b.switch_to_block(bb1);
    b.br(bb3);

    b.switch_to_block(bb2);
    b.br(bb3);

    b.switch_to_block(bb3);
    b.ret(None);

    b.end_function();
    let m = b.finish();

    let blocks = &m.functions[0].blocks;
    // bb0 ends with BrIf
    assert!(blocks[0].instructions.last().unwrap().op.is_terminator());
    assert_eq!(blocks[0].instructions.last().unwrap().op, Op::BrIf);
    // bb1 ends with Br
    assert_eq!(blocks[1].instructions.last().unwrap().op, Op::Br);
    // bb3 ends with Ret
    assert_eq!(blocks[3].instructions.last().unwrap().op, Op::Ret);
}

// -- 7. Property access -------------------------------------------------

#[test]
fn typed_builder_property_access() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("props", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let obj = b.create_object();
    let key = b.const_string(0);
    let val = b.const_i32(42);
    let boxed = b.box_i32(val);
    b.set_prop(obj, key, boxed);
    let loaded = b.get_prop(obj, key);
    let has = b.has_prop(obj, key);
    b.ret(None);

    b.end_function();
    let m = b.finish();

    let instrs = &m.functions[0].blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::GetProp));
    assert!(instrs.iter().any(|i| i.op == Op::SetProp));
    assert!(instrs.iter().any(|i| i.op == Op::HasProp));

    // Verify loaded and has have correct types
    let get = instrs.iter().find(|i| i.op == Op::GetProp).unwrap();
    assert_eq!(get.id, loaded);
    assert_eq!(get.ty, IrType::JSValue);

    let has_inst = instrs.iter().find(|i| i.op == Op::HasProp).unwrap();
    assert_eq!(has_inst.id, has);
    assert_eq!(has_inst.ty, IrType::Bool);
}

// -- 8. Calls -----------------------------------------------------------

#[test]
fn typed_builder_calls() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("calls", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let func = b.const_i32(0); // placeholder func ref
    let arg1 = b.const_i32(1);
    let arg2 = b.const_i32(2);

    let direct = b.call(func, vec![arg1, arg2]);
    assert_eq!(direct, ValueId(3));

    let obj = b.create_object();
    let method = b.const_string(0);
    let method_result = b.call_method(obj, method, vec![arg1]);
    assert_ne!(method_result, direct);

    let ctor = b.const_i32(99);
    let new_result = b.call_new(ctor, vec![]);
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == new_result)
            .unwrap()
            .ty,
        IrType::JSObject
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 9. NaN-boxing round-trip -------------------------------------------

#[test]
fn typed_builder_nanbox_round_trip() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("nanbox", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let raw = b.const_i32(42);
    let boxed = b.box_i32(raw);
    let unboxed = b.unbox_i32(boxed);

    let instrs = &b.blocks[0].instructions;
    assert_eq!(instrs[0].ty, IrType::I32); // const_i32
    assert_eq!(instrs[1].ty, IrType::JSValue); // box_i32
    assert_eq!(instrs[2].ty, IrType::I32); // unbox_i32
    assert_eq!(instrs[1].op, Op::BoxI32);
    assert_eq!(instrs[2].op, Op::UnboxI32);
    assert_eq!(instrs[2].operands, vec![boxed]);
    assert_eq!(unboxed, ValueId(2));

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 10. Try/catch ------------------------------------------------------

#[test]
fn typed_builder_try_catch() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("try_catch", vec![], IrType::Void);

    let try_bb = b.create_block();
    let catch_bb = b.create_block();
    let after = b.create_block();

    b.switch_to_block(try_bb);
    b.try_begin(catch_bb);
    let val = b.const_i32(42);
    b.throw_(val);

    b.switch_to_block(catch_bb);
    let exc = b.catch_();
    assert_eq!(
        b.blocks[1]
            .instructions
            .iter()
            .find(|i| i.id == exc)
            .unwrap()
            .ty,
        IrType::JSValue
    );
    b.br(after);

    b.switch_to_block(after);
    b.ret(None);

    b.end_function();
    let m = b.finish();

    let try_instrs = &m.functions[0].blocks[0].instructions;
    assert!(try_instrs.iter().any(|i| i.op == Op::TryBegin));
    assert!(try_instrs.iter().any(|i| i.op == Op::Throw));

    let catch_instrs = &m.functions[0].blocks[1].instructions;
    assert!(catch_instrs.iter().any(|i| i.op == Op::Catch));
}

// -- 11. Closure environment --------------------------------------------

#[test]
fn typed_builder_closure_env() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("closure", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let env = b.env_create(3);
    let val = b.const_i32(42);
    b.env_store(env, 0, val);
    let loaded = b.env_load(env, 0);

    let instrs = &b.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::EnvCreate));
    assert!(instrs.iter().any(|i| i.op == Op::EnvStore));
    assert!(instrs.iter().any(|i| i.op == Op::EnvLoad));

    let load = instrs.iter().find(|i| i.op == Op::EnvLoad).unwrap();
    assert_eq!(load.id, loaded);
    assert_eq!(load.ty, IrType::JSValue);

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 12. Iterator protocol ----------------------------------------------

#[test]
fn typed_builder_iterators() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("iter", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let arr = b.create_array(vec![]);
    let iter = b.iter_init(arr);
    let next = b.iter_next(iter);
    let done = b.iter_done(iter);
    let val = b.iter_value(next);

    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == iter)
            .unwrap()
            .ty,
        IrType::IteratorRecord
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == done)
            .unwrap()
            .ty,
        IrType::Bool
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == val)
            .unwrap()
            .ty,
        IrType::JSValue
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 13. String ops -----------------------------------------------------

#[test]
fn typed_builder_string_ops() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("strings", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let s1 = b.const_string(0);
    let s2 = b.const_string(1);
    let concat = b.string_concat(s1, s2);
    let len = b.string_length(concat);

    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == concat)
            .unwrap()
            .ty,
        IrType::JSString
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == len)
            .unwrap()
            .ty,
        IrType::I32
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 14. Module building ------------------------------------------------

#[test]
fn typed_builder_module() {
    let m = build_simple_module();
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.functions[0].name, "main");
    assert_eq!(m.entry, Some(0));
}

// -- 15. Struct types ---------------------------------------------------

#[test]
fn typed_builder_struct_types() {
    let mut b = TypedIrBuilder::new();
    let s0 = b.add_struct_type("Point", vec![("x", IrType::F64), ("y", IrType::F64)]);
    let s1 = b.add_struct_type("Color", vec![("r", IrType::I32), ("g", IrType::I32)]);

    assert_eq!(s0, StructTypeId(0));
    assert_eq!(s1, StructTypeId(1));

    b.begin_function("use_struct", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();

    let m = b.finish();
    assert_eq!(m.struct_types.len(), 2);
    assert_eq!(m.struct_types[0].0, "Point");
    assert_eq!(m.struct_types[1].0, "Color");
    assert_eq!(m.struct_types[0].1.len(), 2);
}

// -- 16. SSA write/read variable ----------------------------------------

#[test]
fn typed_builder_ssa_write_read() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("ssa", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb); // no predecessors

    let val = b.const_i32(42);
    b.write_variable(0, val);
    let read = b.read_variable(0, IrType::I32);

    // Should return the same value (no phi needed, same block)
    assert_eq!(read, val);

    b.ret(Some(read));
    b.end_function();
    b.finish();
}

// -- 17. SSA phi insertion (multi-predecessor) --------------------------

#[test]
fn typed_builder_ssa_phi_multi_pred() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("phi", vec![], IrType::I32);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    // bb0: x = 10
    b.switch_to_block(bb0);
    b.seal_block(bb0);
    let ten = b.const_i32(10);
    b.write_variable(0, ten);
    b.br(bb2);

    // bb1: x = 20
    b.switch_to_block(bb1);
    b.seal_block(bb1);
    let twenty = b.const_i32(20);
    b.write_variable(0, twenty);
    b.br(bb2);

    // bb2: merge — should insert phi
    b.switch_to_block(bb2);
    b.add_predecessor(bb2, bb0);
    b.add_predecessor(bb2, bb1);
    b.seal_block(bb2);

    let result = b.read_variable(0, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    let m = b.finish();

    let merge_block = &m.functions[0].blocks[2];
    let phi = merge_block
        .instructions
        .iter()
        .find(|i| i.op == Op::Phi)
        .expect("should have phi");
    assert_eq!(phi.operands.len(), 2);
}

// -- 18. Block sealing resolves incomplete phis -------------------------

#[test]
fn typed_builder_seal_resolves_phis() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("seal", vec![], IrType::I32);

    let bb0 = b.create_block();
    let bb1 = b.create_block();

    // bb1 is NOT sealed yet
    b.switch_to_block(bb1);
    b.add_predecessor(bb1, bb0);
    // Read variable before sealing — should create incomplete phi
    let val = b.read_variable(0, IrType::I32);

    // Now fill bb0
    b.switch_to_block(bb0);
    b.seal_block(bb0);
    let def = b.const_i32(99);
    b.write_variable(0, def);
    b.br(bb1);

    // Seal bb1 — should resolve the incomplete phi
    b.seal_block(bb1);

    b.switch_to_block(bb1);
    b.ret(Some(val));

    b.end_function();
    let m = b.finish();

    // bb1 should have an instruction for the value
    let bb1_instrs = &m.functions[0].blocks[1].instructions;
    assert!(!bb1_instrs.is_empty());
}

// -- 19. Memory: alloc_zone, alloc_heap, load/store_field ---------------

#[test]
fn typed_builder_memory_ops() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("memory", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let zone_obj = b.alloc_zone(IrType::JSObject);
    let heap_obj = b.alloc_heap(IrType::JSObject);

    let val = b.const_i32(42);
    b.store_field(zone_obj, 0, val);
    let loaded = b.load_field(zone_obj, 1);

    let instrs = &b.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::AllocZone));
    assert!(instrs.iter().any(|i| i.op == Op::AllocHeap));
    assert!(instrs.iter().any(|i| i.op == Op::StoreField));
    assert!(instrs.iter().any(|i| i.op == Op::LoadField));

    let alloc_z = instrs.iter().find(|i| i.id == zone_obj).unwrap();
    assert_eq!(alloc_z.ty, IrType::JSObject);

    let alloc_h = instrs.iter().find(|i| i.id == heap_obj).unwrap();
    assert_eq!(alloc_h.ty, IrType::JSObject);

    let load = instrs.iter().find(|i| i.id == loaded).unwrap();
    assert_eq!(load.ty, IrType::JSValue);

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 20. Multiple functions in module -----------------------------------

#[test]
fn typed_builder_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    b.begin_function("foo", vec![("x", IrType::I32)], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let x = b.const_i32(0);
    b.ret(Some(x));
    b.end_function();

    b.begin_function("bar", vec![], IrType::Void);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    b.ret(None);
    b.end_function();

    let m = b.finish();
    assert_eq!(m.functions.len(), 2);
    assert_eq!(m.functions[0].name, "foo");
    assert_eq!(m.functions[1].name, "bar");
    // Each function has independent value numbering
    assert_eq!(m.functions[0].next_value, 2); // const + ret
    assert_eq!(m.functions[1].next_value, 1); // ret
}

// -- 21. Default trait --------------------------------------------------

#[test]
fn typed_builder_default() {
    let b = TypedIrBuilder::default();
    let m = b.finish();
    assert!(m.functions.is_empty());
    assert!(m.struct_types.is_empty());
    assert_eq!(m.entry, None);
}

// -- 22. Unreachable terminator -----------------------------------------

#[test]
fn typed_builder_unreachable() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("trap", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.unreachable();
    b.end_function();

    let m = b.finish();
    let last = m.functions[0].blocks[0].instructions.last().unwrap();
    assert_eq!(last.op, Op::Unreachable);
    assert!(last.op.is_terminator());
}

// -- 23. Bitwise ops ----------------------------------------------------

#[test]
fn typed_builder_bitwise() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("bits", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let a = b.const_i32(0xFF);
    let c = b.const_i32(0x0F);
    let and = b.bitwise_and(a, c);
    let or = b.bitwise_or(a, c);
    let xor = b.bitwise_xor(a, c);
    let not = b.bitwise_not(a);
    let shl = b.shift_left(a, c);
    let shr = b.shift_right(a, c);
    let shru = b.shift_right_unsigned(a, c);

    let instrs = &b.blocks[0].instructions;
    assert_eq!(
        instrs.iter().find(|i| i.id == and).unwrap().op,
        Op::BitwiseAnd
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == or).unwrap().op,
        Op::BitwiseOr
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == xor).unwrap().op,
        Op::BitwiseXor
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == not).unwrap().op,
        Op::BitwiseNot
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == shl).unwrap().op,
        Op::ShiftLeft
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == shr).unwrap().op,
        Op::ShiftRight
    );
    assert_eq!(
        instrs.iter().find(|i| i.id == shru).unwrap().op,
        Op::ShiftRightUnsigned
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 24. Comparison ops -------------------------------------------------

#[test]
fn typed_builder_comparisons() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("cmp", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let a = b.const_i32(1);
    let c = b.const_i32(2);
    let eq = b.eq_i32(a, c);
    let ne = b.ne_i32(a, c);
    let lt = b.lt_i32(a, c);
    let le = b.le_i32(a, c);
    let gt = b.gt_i32(a, c);
    let ge = b.ge_i32(a, c);

    // All comparison results should be Bool
    for id in [eq, ne, lt, le, gt, ge] {
        let inst = b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == id)
            .unwrap();
        assert_eq!(inst.ty, IrType::Bool, "comparison {id:?} should be Bool");
    }

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 25. Invoke (call in try context) -----------------------------------

#[test]
fn typed_builder_invoke() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("inv", vec![], IrType::Void);

    let normal = b.create_block();
    let catch_bb = b.create_block();

    b.switch_to_block(normal);
    let func = b.const_i32(0);
    let result = b.invoke(func, vec![], catch_bb);

    let instrs = &b.blocks[0].instructions;
    let inv = instrs.iter().find(|i| i.id == result).unwrap();
    assert_eq!(inv.op, Op::Invoke);
    assert_eq!(inv.block_targets, vec![catch_bb]);

    b.ret(None);

    b.switch_to_block(catch_bb);
    b.ret(None);

    b.end_function();
    b.finish();
}

// -- 26. Promise/async ops ----------------------------------------------

#[test]
fn typed_builder_promise() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("async_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let p = b.promise_create();
    let val = b.const_i32(42);
    b.promise_resolve(p, val);

    let instrs = &b.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::PromiseCreate));
    assert!(instrs.iter().any(|i| i.op == Op::PromiseResolve));
    assert_eq!(
        instrs.iter().find(|i| i.id == p).unwrap().ty,
        IrType::JSObject
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 27. Type conversion ops --------------------------------------------

#[test]
fn typed_builder_type_conversions() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("convert", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let val = b.const_null();
    let num = b.to_number(val);
    let boolean = b.to_boolean(val);
    let s = b.to_js_string(val);
    let i = b.to_int32(val);

    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|x| x.id == num)
            .unwrap()
            .ty,
        IrType::F64
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|x| x.id == boolean)
            .unwrap()
            .ty,
        IrType::Bool
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|x| x.id == s)
            .unwrap()
            .ty,
        IrType::JSString
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|x| x.id == i)
            .unwrap()
            .ty,
        IrType::I32
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 28. RC operations --------------------------------------------------

#[test]
fn typed_builder_rc_ops() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("rc", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let obj = b.alloc_heap(IrType::JSObject);
    b.rc_inc_strong(obj);
    b.rc_dec_strong(obj);
    let unique = b.rc_is_unique(obj);

    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == unique)
            .unwrap()
            .ty,
        IrType::Bool
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 29. Create closure + array -----------------------------------------

#[test]
fn typed_builder_create_closure_array() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("objects", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let func = b.const_i32(0);
    let env = b.env_create(2);
    let flags = b.const_i32(0);
    let closure = b.create_closure(func, env, flags);
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == closure)
            .unwrap()
            .ty,
        IrType::JSFunction
    );

    let e1 = b.const_i32(1);
    let e2 = b.const_i32(2);
    let arr = b.create_array(vec![e1, e2]);
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == arr)
            .unwrap()
            .ty,
        IrType::JSArray
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 30. Predecessor tracking -------------------------------------------

#[test]
fn typed_builder_predecessor_tracking() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("preds", vec![], IrType::Void);

    let bb0 = b.create_block();
    let bb1 = b.create_block();
    let bb2 = b.create_block();

    b.add_predecessor(bb2, bb0);
    b.add_predecessor(bb2, bb1);
    // Adding duplicate should not create duplicate
    b.add_predecessor(bb2, bb0);

    assert_eq!(b.blocks[2].predecessors.len(), 2);
    assert_eq!(b.blocks[2].predecessors[0], bb0);
    assert_eq!(b.blocks[2].predecessors[1], bb1);

    b.switch_to_block(bb0);
    b.br(bb2);
    b.switch_to_block(bb1);
    b.br(bb2);
    b.switch_to_block(bb2);
    b.ret(None);

    b.end_function();
    b.finish();
}

// -- 31. TDZ ops --------------------------------------------------------

#[test]
fn typed_builder_tdz() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("tdz", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let val = b.const_undefined();
    let checked = b.tdz_check(val);
    b.tdz_init(checked);

    let instrs = &b.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::TdzCheck));
    assert!(instrs.iter().any(|i| i.op == Op::TdzInit));

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 32. Function params ------------------------------------------------

#[test]
fn typed_builder_function_params() {
    let mut b = TypedIrBuilder::new();
    b.begin_function(
        "add",
        vec![("a", IrType::I32), ("b", IrType::I32)],
        IrType::I32,
    );
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_i32(0);
    let c = b.const_i32(1);
    let sum = b.add_i32(a, c);
    b.ret(Some(sum));
    b.end_function();

    let m = b.finish();
    assert_eq!(m.functions[0].params.len(), 2);
    assert_eq!(m.functions[0].params[0].0, "a");
    assert_eq!(m.functions[0].params[0].1, IrType::I32);
    assert_eq!(m.functions[0].params[1].0, "b");
}

// -- 33. Nop and misc ---------------------------------------------------

#[test]
fn typed_builder_nop_and_misc() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("misc", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    b.nop();
    let this = b.this_value();
    let nt = b.new_target();

    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == this)
            .unwrap()
            .ty,
        IrType::JSValue
    );
    assert_eq!(
        b.blocks[0]
            .instructions
            .iter()
            .find(|i| i.id == nt)
            .unwrap()
            .ty,
        IrType::JSValue
    );
    assert_eq!(b.blocks[0].instructions[0].op, Op::Nop);

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 34. SSA read before write (single pred, recursive) -----------------

#[test]
fn typed_builder_ssa_recursive_read() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("rec_read", vec![], IrType::I32);

    let bb0 = b.create_block();
    let bb1 = b.create_block();

    // bb0: define x=5, branch to bb1
    b.switch_to_block(bb0);
    b.seal_block(bb0);
    let five = b.const_i32(5);
    b.write_variable(0, five);
    b.br(bb1);

    // bb1: read x (single pred from bb0 — should not create phi)
    b.switch_to_block(bb1);
    b.add_predecessor(bb1, bb0);
    b.seal_block(bb1);
    let val = b.read_variable(0, IrType::I32);
    b.ret(Some(val));

    b.end_function();
    let m = b.finish();

    // bb1 should NOT have a phi, single pred
    let bb1_block = &m.functions[0].blocks[1];
    assert!(
        !bb1_block.instructions.iter().any(|i| i.op == Op::Phi),
        "single-predecessor read should not create phi"
    );
    // val should be the same as five
    assert_eq!(val, five);
}

// -- 35. Element access -------------------------------------------------

#[test]
fn typed_builder_element_access() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("elem", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let arr = b.create_array(vec![]);
    let idx = b.const_i32(0);
    let val = b.const_i32(42);
    b.store_element(arr, idx, val);
    let loaded = b.load_element(arr, idx);

    let instrs = &b.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::StoreElement));
    assert!(instrs.iter().any(|i| i.op == Op::LoadElement));
    assert_eq!(
        instrs.iter().find(|i| i.id == loaded).unwrap().ty,
        IrType::JSValue
    );

    b.ret(None);
    b.end_function();
    b.finish();
}

// -- 36. Switch statement -----------------------------------------------

#[test]
fn test_switch_statement() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("switch_fn", vec![], IrType::I32);

    let entry = b.create_block();
    let case0 = b.create_block();
    let case1 = b.create_block();
    let case2 = b.create_block();
    let default = b.create_block();

    b.switch_to_block(entry);
    let disc = b.const_i32(1);
    b.switch(disc, vec![case0, case1, case2, default]);

    b.switch_to_block(case0);
    let v0 = b.const_i32(10);
    b.ret(Some(v0));

    b.switch_to_block(case1);
    let v1 = b.const_i32(20);
    b.ret(Some(v1));

    b.switch_to_block(case2);
    let v2 = b.const_i32(30);
    b.ret(Some(v2));

    b.switch_to_block(default);
    let vd = b.const_i32(0);
    b.ret(Some(vd));

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 5);
    // Entry block ends with Switch
    assert_eq!(func.blocks[0].instructions.last().unwrap().op, Op::Switch);
    // Switch targets 4 blocks
    assert_eq!(
        func.blocks[0]
            .instructions
            .last()
            .unwrap()
            .block_targets
            .len(),
        4
    );
}

// -- 37. Nested try/catch -----------------------------------------------

#[test]
fn test_nested_try_catch() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("nested_try", vec![], IrType::Void);

    let outer_try = b.create_block();
    let inner_try = b.create_block();
    let inner_catch = b.create_block();
    let outer_catch = b.create_block();
    let after = b.create_block();

    // Outer try block
    b.switch_to_block(outer_try);
    b.try_begin(outer_catch);
    b.br(inner_try);

    // Inner try block
    b.switch_to_block(inner_try);
    b.try_begin(inner_catch);
    let err = b.const_string(0);
    b.throw_(err);

    // Inner catch
    b.switch_to_block(inner_catch);
    let exc = b.catch_();
    b.rethrow(exc);

    // Outer catch
    b.switch_to_block(outer_catch);
    let outer_exc = b.catch_();
    let _ = b.get_exception(outer_exc);
    b.try_end();
    b.br(after);

    // After
    b.switch_to_block(after);
    b.ret(None);

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 5);
    // Verify both TryBegin instructions exist
    let try_count = func
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| i.op == Op::TryBegin)
        .count();
    assert_eq!(try_count, 2);
    // Verify Rethrow exists
    assert!(
        func.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| i.op == Op::Rethrow)
    );
}

// -- 38. Generator yield ------------------------------------------------

#[test]
fn test_generator_yield() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("gen", vec![], IrType::JSObject);

    let bb = b.create_block();
    b.switch_to_block(bb);

    let generator = b.generator_create();
    let val1 = b.const_i32(1);
    let boxed1 = b.box_i32(val1);
    let _resumed1 = b.yield_(boxed1);
    let val2 = b.const_i32(2);
    let boxed2 = b.box_i32(val2);
    let _resumed2 = b.yield_(boxed2);
    b.ret(Some(generator));

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 1);
    // Verify GeneratorCreate and 2 Yields
    let instrs = &func.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::GeneratorCreate));
    let yield_count = instrs.iter().filter(|i| i.op == Op::Yield).count();
    assert_eq!(yield_count, 2);
}

// -- 39. Async/await ----------------------------------------------------

#[test]
fn test_async_await() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("async_fn", vec![], IrType::Void);

    let bb = b.create_block();
    b.switch_to_block(bb);

    let promise = b.promise_create();
    let fetch_result = b.const_string(0); // placeholder
    let awaited = b.await_(fetch_result);
    b.promise_resolve(promise, awaited);
    b.ret(None);

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 1);
    let instrs = &func.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::PromiseCreate));
    assert!(instrs.iter().any(|i| i.op == Op::Await));
    assert!(instrs.iter().any(|i| i.op == Op::PromiseResolve));
}

// -- 40. For-in pattern (iterator protocol) -----------------------------

#[test]
fn test_for_in_pattern() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("for_in", vec![], IrType::Void);

    let entry = b.create_block();
    let loop_header = b.create_block();
    let loop_body = b.create_block();
    let loop_exit = b.create_block();

    // entry: create iterator
    b.switch_to_block(entry);
    let obj = b.create_object();
    let iter = b.iter_init(obj);
    b.br(loop_header);
    b.seal_block(entry);

    // loop_header: check done
    b.switch_to_block(loop_header);
    b.add_predecessor(loop_header, entry);
    b.add_predecessor(loop_header, loop_body);
    let done = b.iter_done(iter);
    b.br_if(done, loop_exit, loop_body);

    // loop_body: get value
    b.switch_to_block(loop_body);
    b.add_predecessor(loop_body, loop_header);
    b.seal_block(loop_body);
    let next = b.iter_next(iter);
    let _val = b.iter_value(next);
    b.br(loop_header);

    // seal header after body
    b.seal_block(loop_header);

    // loop_exit: close iterator
    b.switch_to_block(loop_exit);
    b.add_predecessor(loop_exit, loop_header);
    b.seal_block(loop_exit);
    b.iter_close(iter);
    b.ret(None);

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 4);
    let all_instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    assert!(all_instrs.iter().any(|i| i.op == Op::IterInit));
    assert!(all_instrs.iter().any(|i| i.op == Op::IterNext));
    assert!(all_instrs.iter().any(|i| i.op == Op::IterDone));
    assert!(all_instrs.iter().any(|i| i.op == Op::IterValue));
    assert!(all_instrs.iter().any(|i| i.op == Op::IterClose));
}

// -- 41. Complex closure (env create/store/load/extend/create_closure) --

#[test]
fn test_complex_closure() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("complex_closure", vec![], IrType::JSFunction);

    let bb = b.create_block();
    b.switch_to_block(bb);

    // Create outer environment with 2 slots
    let outer_env = b.env_create(2);
    let x = b.const_i32(10);
    b.env_store(outer_env, 0, x);
    let y = b.const_i32(20);
    b.env_store(outer_env, 1, y);

    // Extend into inner environment with 1 additional slot
    let inner_env = b.env_extend(outer_env, 1);
    let z = b.const_i32(30);
    b.env_store(inner_env, 0, z);

    // Load from environments
    let _loaded_x = b.env_load(outer_env, 0);
    let _loaded_z = b.env_load(inner_env, 0);

    // Create closure capturing the inner env
    let func_ref = b.const_i32(0); // placeholder
    let flags = b.const_i32(0);
    let closure = b.create_closure(func_ref, inner_env, flags);
    b.ret(Some(closure));

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();

    assert!(verify_typed_function(&func).is_ok());
    assert_eq!(func.blocks.len(), 1);
    let instrs = &func.blocks[0].instructions;
    assert!(instrs.iter().any(|i| i.op == Op::EnvCreate));
    assert!(instrs.iter().any(|i| i.op == Op::EnvExtend));
    assert!(instrs.iter().any(|i| i.op == Op::EnvStore));
    assert!(instrs.iter().any(|i| i.op == Op::EnvLoad));
    assert!(instrs.iter().any(|i| i.op == Op::CreateClosure));
}

// ===========================================================================
// Verifier tests (from verify.rs)
// ===========================================================================

// -- Legacy verifier tests (keep existing) --

#[test]
fn empty_function_verifies() {
    let mut b = crate::builder::IrBuilder::new("empty", vec![], Type::Void);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.push(Type::Void, Instruction::Return(None));
    let func = b.finish();
    assert!(verify_function(&func).is_ok());
}

// -- Helper: build a valid simple function --

fn build_valid_function() -> TypedFunction {
    let mut b = TypedIrBuilder::new();
    b.begin_function("valid", vec![("x", IrType::I32)], IrType::I32);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let c = b.const_i32(42);
    b.ret(Some(c));
    b.end_function();
    b.finish().functions.into_iter().next().unwrap()
}

fn build_fibonacci() -> TypedFunction {
    let mut b = TypedIrBuilder::new();
    b.begin_function("fib", vec![("n", IrType::I32)], IrType::I32);

    let entry = b.create_block();
    let base_case = b.create_block();
    let recurse = b.create_block();

    b.switch_to_block(entry);
    let n = b.const_i32(0); // placeholder for param
    let two = b.const_i32(2);
    let cmp = b.lt_i32(n, two);
    b.br_if(cmp, base_case, recurse);

    b.switch_to_block(base_case);
    b.ret(Some(n));

    b.switch_to_block(recurse);
    let one = b.const_i32(1);
    let n_minus_1 = b.sub_i32(n, one);
    let n_minus_2 = b.sub_i32(n, two);
    let result = b.add_i32(n_minus_1, n_minus_2);
    b.ret(Some(result));

    b.end_function();
    b.finish().functions.into_iter().next().unwrap()
}

fn build_if_else_phi() -> TypedFunction {
    let mut b = TypedIrBuilder::new();
    b.begin_function("if_else", vec![], IrType::I32);

    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();
    let merge = b.create_block();

    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.seal_block(entry);

    b.switch_to_block(then_bb);
    b.add_predecessor(then_bb, entry);
    let val_then = b.const_i32(1);
    b.write_variable(0, val_then);
    b.br(merge);
    b.seal_block(then_bb);

    b.switch_to_block(else_bb);
    b.add_predecessor(else_bb, entry);
    let val_else = b.const_i32(2);
    b.write_variable(0, val_else);
    b.br(merge);
    b.seal_block(else_bb);

    b.switch_to_block(merge);
    b.add_predecessor(merge, then_bb);
    b.add_predecessor(merge, else_bb);
    b.seal_block(merge);

    let result = b.read_variable(0, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    b.finish().functions.into_iter().next().unwrap()
}

// -- 1. Valid function verifies OK --

#[test]
fn valid_function_verifies_ok() {
    let func = build_valid_function();
    assert!(verify_typed_function(&func).is_ok());
}

// -- 2. Empty function (no blocks) fails structural --

#[test]
fn empty_function_no_blocks_fails() {
    let func = TypedFunction {
        name: "empty".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
    assert!(err[0].message.contains("no blocks"));
}

// -- 3. Block without terminator fails --

#[test]
fn block_without_terminator_fails() {
    let func = TypedFunction {
        name: "no_term".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::ConstI32(42),
                ty: IrType::I32,
                operands: vec![],
                block_targets: vec![],
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::InvalidTerminator)
    );
}

// -- 4. Use of undefined value fails --

#[test]
fn undefined_value_fails() {
    let func = TypedFunction {
        name: "undef".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![
                TypedInstruction {
                    id: ValueId(0),
                    op: Op::AddI32,
                    ty: IrType::I32,
                    operands: vec![ValueId(99), ValueId(100)],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(1),
                    op: Op::Ret,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
            ],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 2,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::UndefinedValue)
    );
}

// -- 5. Invalid block target fails --

#[test]
fn invalid_block_target_fails() {
    let func = TypedFunction {
        name: "bad_target".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::Br,
                ty: IrType::Void,
                operands: vec![],
                block_targets: vec![BlockId(99)],
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
    assert!(
        err.iter()
            .any(|e| e.message.contains("invalid block target"))
    );
}

// -- 6. Phi with wrong operand count fails --

#[test]
fn phi_wrong_operand_count_fails() {
    let func = TypedFunction {
        name: "bad_phi".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![
            TypedBasicBlock {
                id: BlockId(0),
                instructions: vec![TypedInstruction {
                    id: ValueId(0),
                    op: Op::Br,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![BlockId(1)],
                    span: SourceSpan::DUMMY,
                }],
                sealed: true,
                predecessors: vec![],
            },
            TypedBasicBlock {
                id: BlockId(1),
                instructions: vec![
                    TypedInstruction {
                        id: ValueId(1),
                        op: Op::Phi,
                        ty: IrType::I32,
                        // 3 operands but only 1 predecessor
                        operands: vec![ValueId(0), ValueId(0), ValueId(0)],
                        block_targets: vec![],
                        span: SourceSpan::DUMMY,
                    },
                    TypedInstruction {
                        id: ValueId(2),
                        op: Op::Ret,
                        ty: IrType::Void,
                        operands: vec![],
                        block_targets: vec![],
                        span: SourceSpan::DUMMY,
                    },
                ],
                sealed: true,
                predecessors: vec![BlockId(0)],
            },
        ],
        next_value: 3,
        next_block: 2,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(err.iter().any(|e| e.kind == VerifyErrorKind::InvalidPhi));
    assert!(err.iter().any(|e| e.message.contains("operands")));
}

// -- 7. Module with invalid entry index fails --

#[test]
fn module_invalid_entry_fails() {
    let module = TypedModule {
        functions: vec![],
        struct_types: vec![],
        entry: Some(5),
    };
    let err = verify_typed_module(&module).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
    assert!(err.iter().any(|e| e.message.contains("entry index")));
}

// -- 8. Multiple errors collected --

#[test]
fn multiple_errors_collected() {
    // Block with undefined value AND missing terminator
    let func = TypedFunction {
        name: "multi_err".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::AddI32,
                ty: IrType::I32,
                operands: vec![ValueId(99), ValueId(100)],
                block_targets: vec![],
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    // Should have at least undefined value errors + missing terminator
    assert!(
        err.len() >= 2,
        "expected multiple errors, got {}",
        err.len()
    );
}

// -- 9. Valid fibonacci function passes --

#[test]
fn fibonacci_verifies_ok() {
    let func = build_fibonacci();
    assert!(verify_typed_function(&func).is_ok());
}

// -- 10. Valid if/else with phi passes --

#[test]
fn if_else_phi_verifies_ok() {
    let func = build_if_else_phi();
    assert!(verify_typed_function(&func).is_ok());
}

// -- 11. Module with valid entry verifies --

#[test]
fn valid_module_verifies() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("main", vec![], IrType::Void);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    assert!(verify_typed_module(&module).is_ok());
}

// -- 12. Module with no entry verifies --

#[test]
fn module_no_entry_verifies() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("helper", vec![], IrType::Void);
    let entry = b.create_block();
    b.switch_to_block(entry);
    b.ret(None);
    b.end_function();
    let module = b.finish();
    assert!(verify_typed_module(&module).is_ok());
}

// -- 13. Empty block fails structural --

#[test]
fn empty_block_fails_structural() {
    let func = TypedFunction {
        name: "empty_block".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 0,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
    assert!(err.iter().any(|e| e.message.contains("empty")));
}

// -- 14. Phi after non-phi instruction fails --

#[test]
fn phi_after_non_phi_fails() {
    let func = TypedFunction {
        name: "phi_order".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![
                TypedInstruction {
                    id: ValueId(0),
                    op: Op::ConstI32(1),
                    ty: IrType::I32,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(1),
                    op: Op::Phi,
                    ty: IrType::I32,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(2),
                    op: Op::Ret,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
            ],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 3,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let err = verify_typed_function(&func).unwrap_err();
    assert!(err.iter().any(|e| e.kind == VerifyErrorKind::InvalidPhi));
    assert!(err.iter().any(|e| e.message.contains("after non-phi")));
}

// -- 15. Module propagates function errors --

#[test]
fn module_propagates_function_errors() {
    let func = TypedFunction {
        name: "bad".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        is_generator: false,
        is_async: false,
    };
    let module = TypedModule {
        functions: vec![func],
        struct_types: vec![],
        entry: Some(0),
    };
    let err = verify_typed_module(&module).unwrap_err();
    assert!(
        err.iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
}

// -- 16. Throw as terminator is valid --

#[test]
fn throw_terminator_is_valid() {
    let func = TypedFunction {
        name: "throws".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![
                TypedInstruction {
                    id: ValueId(0),
                    op: Op::ConstNull,
                    ty: IrType::JSValue,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(1),
                    op: Op::Throw,
                    ty: IrType::Void,
                    operands: vec![ValueId(0)],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
            ],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 2,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    assert!(verify_typed_function(&func).is_ok());
}

// -- 17. Unreachable as terminator is valid --

#[test]
fn unreachable_terminator_is_valid() {
    let func = TypedFunction {
        name: "unreachable_fn".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::Unreachable,
                ty: IrType::Void,
                operands: vec![],
                block_targets: vec![],
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    assert!(verify_typed_function(&func).is_ok());
}

// -- 18. BrIf with valid targets passes --

#[test]
fn br_if_valid_targets_passes() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("branch", vec![], IrType::Void);

    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();

    b.switch_to_block(entry);
    let c = b.const_bool(true);
    b.br_if(c, then_bb, else_bb);

    b.switch_to_block(then_bb);
    b.ret(None);

    b.switch_to_block(else_bb);
    b.ret(None);

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();
    assert!(verify_typed_function(&func).is_ok());
}

// -- 19. VerifyError Display implementation --

#[test]
fn verify_error_display() {
    let err = VerifyError {
        kind: VerifyErrorKind::UndefinedValue,
        message: "test message".to_string(),
    };
    let display = format!("{err}");
    assert!(display.contains("UndefinedValue"));
    assert!(display.contains("test message"));
}

// -- 20. VerifyError is std::error::Error --

#[test]
fn verify_error_is_std_error() {
    let err = VerifyError {
        kind: VerifyErrorKind::StructuralError,
        message: "test".to_string(),
    };
    let _: &dyn std::error::Error = &err;
}

// -- 21. Function with multiple blocks, all valid --

#[test]
fn multi_block_function_verifies() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("multi", vec![], IrType::I32);

    let bb0 = b.create_block();
    let bb1 = b.create_block();

    b.switch_to_block(bb0);
    b.br(bb1);

    b.switch_to_block(bb1);
    let val = b.const_i32(0);
    b.ret(Some(val));

    b.end_function();
    let func = b.finish().functions.into_iter().next().unwrap();
    assert!(verify_typed_function(&func).is_ok());
}

// -- 22. Module with multiple functions --

#[test]
fn module_multiple_functions_verifies() {
    let mut b = TypedIrBuilder::new();

    b.begin_function("foo", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();

    b.begin_function("bar", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let v = b.const_i32(1);
    b.ret(Some(v));
    b.end_function();

    b.set_entry(0);
    let module = b.finish();
    assert!(verify_typed_module(&module).is_ok());
}

// ===========================================================================
// Types tests (from types.rs)
// ===========================================================================

// -- Op variant count ---------------------------------------------------

/// Helper: returns a list containing one representative of every `Op`
/// variant. Keeping this list in sync ensures the count assertion catches
/// newly added variants.
fn all_op_variants() -> Vec<Op> {
    vec![
        // Constants (7)
        Op::ConstI32(0),
        Op::ConstI64(0),
        Op::ConstF64(0.0),
        Op::ConstBool(false),
        Op::ConstNull,
        Op::ConstUndefined,
        Op::ConstString(0),
        // Arithmetic (26)
        Op::AddI32,
        Op::SubI32,
        Op::MulI32,
        Op::DivI32,
        Op::ModI32,
        Op::NegI32,
        Op::AddF64,
        Op::SubF64,
        Op::MulF64,
        Op::DivF64,
        Op::ModF64,
        Op::NegF64,
        Op::AddJS,
        Op::SubJS,
        Op::MulJS,
        Op::DivJS,
        Op::ModJS,
        Op::NegJS,
        Op::ExpJS,
        Op::BitwiseAnd,
        Op::BitwiseOr,
        Op::BitwiseXor,
        Op::BitwiseNot,
        Op::ShiftLeft,
        Op::ShiftRight,
        Op::ShiftRightUnsigned,
        // Comparison (20)
        Op::EqI32,
        Op::EqF64,
        Op::EqStrict,
        Op::EqAbstract,
        Op::NeI32,
        Op::NeF64,
        Op::NeStrict,
        Op::NeAbstract,
        Op::LtI32,
        Op::LtF64,
        Op::LtJS,
        Op::LeI32,
        Op::LeF64,
        Op::LeJS,
        Op::GtI32,
        Op::GtF64,
        Op::GtJS,
        Op::GeI32,
        Op::GeF64,
        Op::GeJS,
        // Type conversion (8)
        Op::ToNumber,
        Op::ToNumeric,
        Op::ToString,
        Op::ToBoolean,
        Op::ToObject,
        Op::ToPrimitive,
        Op::ToPropertyKey,
        Op::ToInt32,
        Op::ToUint32,
        // NaN-boxing (18)
        Op::BoxI32,
        Op::BoxUnsignedI32,
        Op::BoxF64,
        Op::BoxBool,
        Op::BoxNull,
        Op::BoxUndefined,
        Op::BoxString,
        Op::BoxObject,
        Op::BoxSymbol,
        Op::UnboxI32,
        Op::UnboxF64,
        Op::UnboxBool,
        Op::UnboxString,
        Op::UnboxObject,
        Op::UnboxSymbol,
        Op::TypeofBoxed,
        Op::IsNullish,
        Op::IsFalsy,
        // Control flow (5)
        Op::Br,
        Op::BrIf,
        Op::Switch,
        Op::Ret,
        Op::Unreachable,
        // SSA (1)
        Op::Phi,
        // Memory allocation (7)
        Op::AllocZone,
        Op::AllocHeap,
        Op::AllocStack,
        Op::AllocArray,
        Op::FreeZone,
        Op::IncRef,
        Op::DecRef,
        // Field/element access (7)
        Op::LoadField,
        Op::StoreField,
        Op::LoadElement,
        Op::StoreElement,
        Op::LoadLocal,
        Op::StoreLocal,
        Op::LoadParam(0),
        // RC operations (5)
        Op::RcIncStrong,
        Op::RcDecStrong,
        Op::RcIncWeak,
        Op::RcDecWeak,
        Op::RcIsUnique,
        // Property access (15)
        Op::GetProp,
        Op::SetProp,
        Op::SetPropStrict,
        Op::DeleteProp,
        Op::HasProp,
        Op::GetElem,
        Op::SetElem,
        Op::DeleteElem,
        Op::GetPropDynamic,
        Op::SetPropDynamic,
        Op::SetPropDynamicStrict,
        Op::GetSuper,
        Op::SetSuper,
        Op::GetPrivate,
        Op::SetPrivate,
        Op::PrivateFieldGet,
        Op::PrivateFieldSet,
        Op::PrivateFieldHas,
        Op::InstallPrivateField,
        // Calls (8)
        Op::Call,
        Op::CallMethod,
        Op::CallNew,
        Op::CallEval,
        Op::CallVarargs,
        Op::CallRuntime,
        Op::TailCall,
        Op::Invoke,
        // Object/Shape (12)
        Op::CreateObject,
        Op::CreateObjectLiteral,
        Op::CreateArray,
        Op::CreateRegExp,
        Op::CreateClosure,
        Op::CreateArguments,
        Op::ObjectDefineProperty,
        Op::ObjectGetPrototype,
        Op::ObjectSetPrototype,
        Op::ShapeCheck,
        Op::ShapeTransition,
        Op::InstanceOf,
        // Type guards (3)
        Op::GuardType,
        Op::GuardShape,
        Op::GuardTruthiness,
        // Exception handling (8)
        Op::TryBegin,
        Op::TryEnd,
        Op::Throw,
        Op::Catch,
        Op::Finally,
        Op::Rethrow,
        Op::IsException,
        Op::GetException,
        // TDZ / Drop flags (4)
        Op::TdzCheck,
        Op::TdzInit,
        Op::DropFlagSet,
        Op::DropFlagCheck,
        // Closure environment (4)
        Op::EnvCreate,
        Op::EnvLoad,
        Op::EnvStore,
        Op::EnvExtend,
        // Iterator protocol (6)
        Op::IterInit,
        Op::IterInitAsync,
        Op::IterNext,
        Op::IterDone,
        Op::IterValue,
        Op::IterClose,
        // Promise/Async (4)
        Op::PromiseCreate,
        Op::PromiseResolve,
        Op::PromiseReject,
        Op::Await,
        // Generator (3)
        Op::GeneratorCreate,
        Op::Yield,
        Op::YieldDelegate,
        // String operations (4)
        Op::StringConcat,
        Op::StringCompare,
        Op::StringLength,
        Op::StringCharAt,
        // Miscellaneous (7)
        Op::Nop,
        Op::Debugger,
        Op::ThisValue,
        Op::NewTarget,
        Op::ImportMeta,
        Op::SuperCall,
        Op::WithScope,
    ]
}

#[test]
fn op_variant_count_at_least_162() {
    let variants = all_op_variants();
    // Verify each variant has a unique discriminant.
    let discriminants: HashSet<_> = variants.iter().map(core::mem::discriminant).collect();
    assert_eq!(
        discriminants.len(),
        variants.len(),
        "duplicate discriminant in all_op_variants list"
    );
    assert!(
        variants.len() >= 162,
        "expected at least 162 Op variants, got {}",
        variants.len()
    );
}

#[test]
fn op_exact_variant_count() {
    // 7+26+20+9+18+5+1+7+6+5+19+8+12+3+8+4+4+6+4+3+4+7 = 187
    assert_eq!(all_op_variants().len(), 187);
}

// -- IrType size --------------------------------------------------------

#[test]
fn ir_type_size_is_reasonable() {
    let size = std::mem::size_of::<IrType>();
    // IrType should fit in 24 bytes or fewer (enum discriminant + largest
    // variant which is Array(Box<IrType>, u32) = 8 + 4 = 12 payload).
    assert!(size <= 24, "IrType is unexpectedly large: {size} bytes");
}

// -- is_terminator ------------------------------------------------------

#[test]
fn is_terminator_positive() {
    let terminators = [
        Op::Br,
        Op::BrIf,
        Op::Switch,
        Op::Ret,
        Op::Unreachable,
        Op::Throw,
        Op::Rethrow,
    ];
    for op in &terminators {
        assert!(op.is_terminator(), "{op:?} should be a terminator");
    }
}

#[test]
fn is_terminator_negative() {
    let non_terminators = [
        Op::Nop,
        Op::AddI32,
        Op::Call,
        Op::Phi,
        Op::ConstI32(0),
        Op::AllocZone,
        Op::GetProp,
    ];
    for op in &non_terminators {
        assert!(!op.is_terminator(), "{op:?} should NOT be a terminator");
    }
}

// -- is_call ------------------------------------------------------------

#[test]
fn is_call_positive() {
    let calls = [
        Op::Call,
        Op::CallMethod,
        Op::CallNew,
        Op::CallEval,
        Op::CallVarargs,
        Op::CallRuntime,
        Op::TailCall,
        Op::Invoke,
    ];
    for op in &calls {
        assert!(op.is_call(), "{op:?} should be a call");
    }
}

#[test]
fn is_call_negative() {
    let non_calls = [Op::Nop, Op::Br, Op::AddI32, Op::AllocZone];
    for op in &non_calls {
        assert!(!op.is_call(), "{op:?} should NOT be a call");
    }
}

// -- is_memory ----------------------------------------------------------

#[test]
fn is_memory_positive() {
    let memory_ops = [
        Op::AllocZone,
        Op::AllocHeap,
        Op::AllocStack,
        Op::AllocArray,
        Op::FreeZone,
        Op::IncRef,
        Op::DecRef,
        Op::RcIncStrong,
        Op::RcDecStrong,
        Op::RcIncWeak,
        Op::RcDecWeak,
    ];
    for op in &memory_ops {
        assert!(op.is_memory(), "{op:?} should be a memory op");
    }
}

#[test]
fn is_memory_negative() {
    let non_memory = [Op::Nop, Op::Call, Op::Br, Op::GetProp];
    for op in &non_memory {
        assert!(!op.is_memory(), "{op:?} should NOT be a memory op");
    }
}

// -- category -----------------------------------------------------------

#[test]
fn category_constants() {
    assert_eq!(Op::ConstI32(42).category(), "constants");
    assert_eq!(Op::ConstNull.category(), "constants");
    assert_eq!(Op::ConstString(0).category(), "constants");
}

#[test]
fn category_arithmetic() {
    assert_eq!(Op::AddI32.category(), "arithmetic");
    assert_eq!(Op::ExpJS.category(), "arithmetic");
    assert_eq!(Op::ShiftRightUnsigned.category(), "arithmetic");
}

#[test]
fn category_comparison() {
    assert_eq!(Op::EqStrict.category(), "comparison");
    assert_eq!(Op::LtJS.category(), "comparison");
    assert_eq!(Op::GeF64.category(), "comparison");
}

#[test]
fn category_control_flow() {
    assert_eq!(Op::Br.category(), "control_flow");
    assert_eq!(Op::BrIf.category(), "control_flow");
    assert_eq!(Op::Ret.category(), "control_flow");
}

#[test]
fn category_calls() {
    assert_eq!(Op::Call.category(), "calls");
    assert_eq!(Op::Invoke.category(), "calls");
    assert_eq!(Op::TailCall.category(), "calls");
}

#[test]
fn category_property_access() {
    assert_eq!(Op::GetProp.category(), "property_access");
    assert_eq!(Op::SetPrivate.category(), "property_access");
}

#[test]
fn category_exception_handling() {
    assert_eq!(Op::TryBegin.category(), "exception_handling");
    assert_eq!(Op::Catch.category(), "exception_handling");
    assert_eq!(Op::GetException.category(), "exception_handling");
}

#[test]
fn category_promise_async() {
    assert_eq!(Op::PromiseCreate.category(), "promise_async");
    assert_eq!(Op::Await.category(), "promise_async");
}

#[test]
fn category_generator() {
    assert_eq!(Op::GeneratorCreate.category(), "generator");
    assert_eq!(Op::Yield.category(), "generator");
}

#[test]
fn category_miscellaneous() {
    assert_eq!(Op::Nop.category(), "miscellaneous");
    assert_eq!(Op::Debugger.category(), "miscellaneous");
    assert_eq!(Op::WithScope.category(), "miscellaneous");
}

// -- has_side_effects ---------------------------------------------------

#[test]
fn has_side_effects_positive() {
    let side_effectful = [
        Op::Call,
        Op::StoreField,
        Op::SetProp,
        Op::Throw,
        Op::AllocHeap,
        Op::Debugger,
        Op::PromiseResolve,
    ];
    for op in &side_effectful {
        assert!(op.has_side_effects(), "{op:?} should have side effects");
    }
}

#[test]
fn has_side_effects_negative() {
    let pure_ops = [
        Op::ConstI32(0),
        Op::AddI32,
        Op::EqStrict,
        Op::BoxI32,
        Op::LoadField,
        Op::GetProp,
        Op::Phi,
    ];
    for op in &pure_ops {
        assert!(
            !op.has_side_effects(),
            "{op:?} should NOT have side effects"
        );
    }
}

// -- TypedInstruction creation ------------------------------------------

#[test]
fn typed_instruction_creation() {
    let inst = TypedInstruction {
        id: ValueId(0),
        op: Op::ConstI32(42),
        ty: IrType::I32,
        operands: vec![],
        block_targets: vec![],
        span: SourceSpan::DUMMY,
    };
    assert_eq!(inst.id, ValueId(0));
    assert_eq!(inst.ty, IrType::I32);
    assert_eq!(inst.op, Op::ConstI32(42));
    assert!(inst.operands.is_empty());
    assert!(inst.block_targets.is_empty());
}

#[test]
fn typed_instruction_with_operands() {
    let inst = TypedInstruction {
        id: ValueId(2),
        op: Op::AddI32,
        ty: IrType::I32,
        operands: vec![ValueId(0), ValueId(1)],
        block_targets: vec![],
        span: SourceSpan::DUMMY,
    };
    assert_eq!(inst.operands.len(), 2);
    assert_eq!(inst.operands[0], ValueId(0));
    assert_eq!(inst.operands[1], ValueId(1));
}

#[test]
fn typed_instruction_branch_with_targets() {
    let inst = TypedInstruction {
        id: ValueId(5),
        op: Op::BrIf,
        ty: IrType::Void,
        operands: vec![ValueId(4)],
        block_targets: vec![BlockId(1), BlockId(2)],
        span: SourceSpan::DUMMY,
    };
    assert!(inst.op.is_terminator());
    assert_eq!(inst.block_targets.len(), 2);
}

// -- IrType equality and hashing ----------------------------------------

#[test]
fn ir_type_equality() {
    assert_eq!(IrType::I32, IrType::I32);
    assert_ne!(IrType::I32, IrType::I64);
    assert_eq!(IrType::JSValue, IrType::JSValue);
    assert_ne!(IrType::ZonePtr, IrType::HeapPtr);
}

#[test]
fn ir_type_struct_equality() {
    assert_eq!(
        IrType::Struct(StructTypeId(1)),
        IrType::Struct(StructTypeId(1))
    );
    assert_ne!(
        IrType::Struct(StructTypeId(1)),
        IrType::Struct(StructTypeId(2))
    );
}

#[test]
fn ir_type_array_equality() {
    assert_eq!(
        IrType::Array(Box::new(IrType::I32), 4),
        IrType::Array(Box::new(IrType::I32), 4)
    );
    assert_ne!(
        IrType::Array(Box::new(IrType::I32), 4),
        IrType::Array(Box::new(IrType::F64), 4)
    );
    assert_ne!(
        IrType::Array(Box::new(IrType::I32), 4),
        IrType::Array(Box::new(IrType::I32), 8)
    );
}

#[test]
fn ir_type_hashing() {
    let mut set = HashSet::new();
    set.insert(IrType::I32);
    set.insert(IrType::F64);
    set.insert(IrType::JSValue);
    set.insert(IrType::I32); // duplicate
    assert_eq!(set.len(), 3);
}

// -- Op equality --------------------------------------------------------

#[test]
fn op_equality() {
    assert_eq!(Op::ConstI32(42), Op::ConstI32(42));
    assert_ne!(Op::ConstI32(42), Op::ConstI32(43));
    assert_eq!(Op::Nop, Op::Nop);
    assert_ne!(Op::Nop, Op::Debugger);
}

#[test]
fn op_f64_nan_equality() {
    // NaN should equal NaN via bitwise comparison.
    assert_eq!(Op::ConstF64(f64::NAN), Op::ConstF64(f64::NAN));
}

// -- Exhaustive category coverage ---------------------------------------

#[test]
fn all_ops_have_a_category() {
    for op in all_op_variants() {
        let cat = op.category();
        assert!(!cat.is_empty(), "{op:?} returned empty category");
    }
}

// ===========================================================================
// Printer tests (from printer.rs)
// ===========================================================================

// -- Legacy printer tests (keep existing) --

#[test]
fn print_simple_function() {
    let mut b = IrBuilder::new("identity", vec![Type::Any], Type::Any);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let p = b.push(Type::Any, Instruction::Param(0));
    b.push(Type::Void, Instruction::Return(Some(p)));
    let func = b.finish();

    let text = print_function(&func);
    assert!(text.contains("fn @identity(any) -> any"));
    assert!(text.contains("param 0"));
    assert!(text.contains("ret v0"));
}

// -- Typed printer tests --

// 1. Print constant instructions

#[test]
fn print_typed_const_i32() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let c = b.const_i32(42);
    b.ret(Some(c));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("const_i32 42"), "output: {output}");
    assert!(
        output.contains("%v0: i32 = const_i32 42"),
        "output: {output}"
    );
}

#[test]
fn print_typed_const_f64() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::F64);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let c = b.const_f64(2.5);
    b.ret(Some(c));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("const_f64 2.5"), "output: {output}");
}

#[test]
fn print_typed_const_bool() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Bool);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let c = b.const_bool(true);
    b.ret(Some(c));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("const_bool true"), "output: {output}");
}

// 2. Print arithmetic instructions

#[test]
fn print_typed_arithmetic() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let a = b.const_i32(10);
    let c = b.const_i32(20);
    let sum = b.add_i32(a, c);
    b.ret(Some(sum));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(
        output.contains("%v2: i32 = add_i32 %v0, %v1"),
        "output: {output}"
    );
}

// 3. Print control flow (br, br_if, ret)

#[test]
fn print_typed_control_flow() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
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
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("br_if %v0, bb1, bb2"), "output: {output}");
    assert!(output.contains("ret"), "output: {output}");
}

#[test]
fn print_typed_br() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let entry = b.create_block();
    let target = b.create_block();

    b.switch_to_block(entry);
    b.br(target);

    b.switch_to_block(target);
    b.ret(None);

    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("br bb1"), "output: {output}");
}

// 4. Print function header with params

#[test]
fn print_typed_function_header() {
    let mut b = TypedIrBuilder::new();
    b.begin_function(
        "add",
        vec![("a", IrType::I32), ("b", IrType::I32)],
        IrType::I32,
    );
    let bb = b.create_block();
    b.switch_to_block(bb);
    let c = b.const_i32(0);
    b.ret(Some(c));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(
        output.contains("fn @add(%a: i32, %b: i32) -> i32"),
        "output: {output}"
    );
}

// 5. Print module with struct types

#[test]
fn print_typed_module_with_structs() {
    let mut b = TypedIrBuilder::new();
    b.add_struct_type("Point", vec![("x", IrType::F64), ("y", IrType::F64)]);

    b.begin_function("main", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();
    b.set_entry(0);

    let module = b.finish();
    let output = print_typed_module(&module);
    assert!(
        output.contains("struct Point { x: f64, y: f64 }"),
        "output: {output}"
    );
}

// 6. Print entry point

#[test]
fn print_typed_module_entry() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("main", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();
    b.set_entry(0);

    let module = b.finish();
    let output = print_typed_module(&module);
    assert!(output.contains("entry: @main"), "output: {output}");
}

// 7. Fibonacci function print

#[test]
fn print_typed_fibonacci() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("fib", vec![("n", IrType::I32)], IrType::I32);

    let entry = b.create_block();
    let base_case = b.create_block();
    let recurse = b.create_block();

    b.switch_to_block(entry);
    let n = b.const_i32(0);
    let two = b.const_i32(2);
    let cmp = b.lt_i32(n, two);
    b.br_if(cmp, base_case, recurse);

    b.switch_to_block(base_case);
    b.ret(Some(n));

    b.switch_to_block(recurse);
    let one = b.const_i32(1);
    let n_minus_1 = b.sub_i32(n, one);
    let n_minus_2 = b.sub_i32(n, two);
    let result = b.add_i32(n_minus_1, n_minus_2);
    b.ret(Some(result));

    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    // Check key elements
    assert!(
        output.contains("fn @fib(%n: i32) -> i32"),
        "output: {output}"
    );
    assert!(output.contains("const_i32 2"), "output: {output}");
    assert!(output.contains("lt_i32"), "output: {output}");
    assert!(output.contains("br_if"), "output: {output}");
    assert!(output.contains("sub_i32"), "output: {output}");
    assert!(output.contains("add_i32"), "output: {output}");
    assert!(output.contains("ret"), "output: {output}");
}

// 8. Simple arithmetic function print

#[test]
fn print_typed_simple_arithmetic() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("calc", vec![("x", IrType::F64)], IrType::F64);

    let bb = b.create_block();
    b.switch_to_block(bb);
    let x = b.const_f64(2.0);
    let y = b.const_f64(3.0);
    let sum = b.add_f64(x, y);
    b.ret(Some(sum));

    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(
        output.contains("fn @calc(%x: f64) -> f64"),
        "output: {output}"
    );
    assert!(output.contains("add_f64 %v0, %v1"), "output: {output}");
}

// 9. All Op variants have a name in format_op_name (exhaustive match)

#[test]
fn all_ops_have_name() {
    // Construct one of each variant and verify non-empty name
    let ops = vec![
        Op::ConstI32(0),
        Op::ConstI64(0),
        Op::ConstF64(0.0),
        Op::ConstBool(false),
        Op::ConstNull,
        Op::ConstUndefined,
        Op::ConstString(0),
        Op::AddI32,
        Op::SubI32,
        Op::MulI32,
        Op::DivI32,
        Op::ModI32,
        Op::NegI32,
        Op::AddF64,
        Op::SubF64,
        Op::MulF64,
        Op::DivF64,
        Op::ModF64,
        Op::NegF64,
        Op::AddJS,
        Op::SubJS,
        Op::MulJS,
        Op::DivJS,
        Op::ModJS,
        Op::NegJS,
        Op::ExpJS,
        Op::BitwiseAnd,
        Op::BitwiseOr,
        Op::BitwiseXor,
        Op::BitwiseNot,
        Op::ShiftLeft,
        Op::ShiftRight,
        Op::ShiftRightUnsigned,
        Op::EqI32,
        Op::EqF64,
        Op::EqStrict,
        Op::EqAbstract,
        Op::NeI32,
        Op::NeF64,
        Op::NeStrict,
        Op::NeAbstract,
        Op::LtI32,
        Op::LtF64,
        Op::LtJS,
        Op::LeI32,
        Op::LeF64,
        Op::LeJS,
        Op::GtI32,
        Op::GtF64,
        Op::GtJS,
        Op::GeI32,
        Op::GeF64,
        Op::GeJS,
        Op::ToNumber,
        Op::ToNumeric,
        Op::ToString,
        Op::ToBoolean,
        Op::ToObject,
        Op::ToPrimitive,
        Op::ToPropertyKey,
        Op::ToInt32,
        Op::ToUint32,
        Op::BoxI32,
        Op::BoxUnsignedI32,
        Op::BoxF64,
        Op::BoxBool,
        Op::BoxNull,
        Op::BoxUndefined,
        Op::BoxString,
        Op::BoxObject,
        Op::BoxSymbol,
        Op::UnboxI32,
        Op::UnboxF64,
        Op::UnboxBool,
        Op::UnboxString,
        Op::UnboxObject,
        Op::UnboxSymbol,
        Op::TypeofBoxed,
        Op::IsNullish,
        Op::IsFalsy,
        Op::Br,
        Op::BrIf,
        Op::Switch,
        Op::Ret,
        Op::Unreachable,
        Op::Phi,
        Op::AllocZone,
        Op::AllocHeap,
        Op::AllocStack,
        Op::AllocArray,
        Op::FreeZone,
        Op::IncRef,
        Op::DecRef,
        Op::LoadField,
        Op::StoreField,
        Op::LoadElement,
        Op::StoreElement,
        Op::LoadLocal,
        Op::StoreLocal,
        Op::LoadParam(0),
        Op::RcIncStrong,
        Op::RcDecStrong,
        Op::RcIncWeak,
        Op::RcDecWeak,
        Op::RcIsUnique,
        Op::GetProp,
        Op::SetProp,
        Op::SetPropStrict,
        Op::DeleteProp,
        Op::HasProp,
        Op::GetElem,
        Op::SetElem,
        Op::DeleteElem,
        Op::GetPropDynamic,
        Op::SetPropDynamic,
        Op::SetPropDynamicStrict,
        Op::GetSuper,
        Op::SetSuper,
        Op::GetPrivate,
        Op::SetPrivate,
        Op::PrivateFieldGet,
        Op::PrivateFieldSet,
        Op::PrivateFieldHas,
        Op::InstallPrivateField,
        Op::Call,
        Op::CallMethod,
        Op::CallNew,
        Op::CallEval,
        Op::CallVarargs,
        Op::CallRuntime,
        Op::TailCall,
        Op::Invoke,
        Op::CreateObject,
        Op::CreateObjectLiteral,
        Op::CreateArray,
        Op::CreateRegExp,
        Op::CreateClosure,
        Op::CreateArguments,
        Op::ObjectDefineProperty,
        Op::ObjectGetPrototype,
        Op::ObjectSetPrototype,
        Op::ShapeCheck,
        Op::ShapeTransition,
        Op::InstanceOf,
        Op::GuardType,
        Op::GuardShape,
        Op::GuardTruthiness,
        Op::TryBegin,
        Op::TryEnd,
        Op::Throw,
        Op::Catch,
        Op::Finally,
        Op::Rethrow,
        Op::IsException,
        Op::GetException,
        Op::TdzCheck,
        Op::TdzInit,
        Op::DropFlagSet,
        Op::DropFlagCheck,
        Op::EnvCreate,
        Op::EnvLoad,
        Op::EnvStore,
        Op::EnvExtend,
        Op::IterInit,
        Op::IterInitAsync,
        Op::IterNext,
        Op::IterDone,
        Op::IterValue,
        Op::IterClose,
        Op::PromiseCreate,
        Op::PromiseResolve,
        Op::PromiseReject,
        Op::Await,
        Op::GeneratorCreate,
        Op::Yield,
        Op::YieldDelegate,
        Op::StringConcat,
        Op::StringCompare,
        Op::StringLength,
        Op::StringCharAt,
        Op::Nop,
        Op::Debugger,
        Op::ThisValue,
        Op::NewTarget,
        Op::ImportMeta,
        Op::SuperCall,
        Op::WithScope,
    ];
    assert_eq!(ops.len(), 187);
    for op in &ops {
        let name = format_op_name(op);
        assert!(!name.is_empty(), "Op variant {:?} has empty name", op);
    }
}

// 10. Void instructions don't have result prefix

#[test]
fn void_instruction_no_result_prefix() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    // "ret" should appear without "%vN: type = " prefix
    let ret_line = output
        .lines()
        .find(|l| l.trim().starts_with("ret"))
        .unwrap();
    assert!(
        !ret_line.contains("="),
        "void instruction should not have '=': {ret_line}"
    );
}

// 11. Print nop instruction

#[test]
fn print_typed_nop() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.nop();
    b.ret(None);
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("nop"), "output: {output}");
}

// 12. Print module without entry

#[test]
fn print_typed_module_no_entry() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("helper", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();

    let module = b.finish();
    let output = print_typed_module(&module);
    assert!(!output.contains("entry:"), "output: {output}");
}

// 13. format_ir_type covers all basic types

#[test]
fn format_ir_type_all_basic() {
    let types = vec![
        (IrType::Void, "void"),
        (IrType::Bool, "bool"),
        (IrType::I32, "i32"),
        (IrType::I64, "i64"),
        (IrType::F64, "f64"),
        (IrType::Ptr, "ptr"),
        (IrType::ZonePtr, "zone_ptr"),
        (IrType::HeapPtr, "heap_ptr"),
        (IrType::JSValue, "jsvalue"),
        (IrType::JSString, "jsstring"),
        (IrType::JSObject, "jsobject"),
        (IrType::JSArray, "jsarray"),
        (IrType::JSFunction, "jsfunction"),
        (IrType::JSSymbol, "jssymbol"),
        (IrType::CompletionRecord, "completion_record"),
        (IrType::IteratorRecord, "iterator_record"),
    ];
    for (ty, expected) in &types {
        assert_eq!(format_ir_type(ty), *expected);
    }
}

// 14. format_ir_type_full handles struct and array

#[test]
fn format_ir_type_full_dynamic() {
    assert_eq!(
        format_ir_type_full(&IrType::Struct(StructTypeId(3))),
        "struct#3"
    );
    assert_eq!(
        format_ir_type_full(&IrType::Array(Box::new(IrType::I32), 10)),
        "[i32; 10]"
    );
}

// 15. Print const_string with string table index

#[test]
fn print_typed_const_string() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSString);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let s = b.const_string(7);
    b.ret(Some(s));
    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("const_string @7"), "output: {output}");
}

// 16. Print with phi node

#[test]
fn print_typed_phi() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::I32);

    let entry = b.create_block();
    let then_bb = b.create_block();
    let else_bb = b.create_block();
    let merge = b.create_block();

    b.switch_to_block(entry);
    let cond = b.const_bool(true);
    b.br_if(cond, then_bb, else_bb);
    b.seal_block(entry);

    b.switch_to_block(then_bb);
    b.add_predecessor(then_bb, entry);
    let val_then = b.const_i32(1);
    b.write_variable(0, val_then);
    b.br(merge);
    b.seal_block(then_bb);

    b.switch_to_block(else_bb);
    b.add_predecessor(else_bb, entry);
    let val_else = b.const_i32(2);
    b.write_variable(0, val_else);
    b.br(merge);
    b.seal_block(else_bb);

    b.switch_to_block(merge);
    b.add_predecessor(merge, then_bb);
    b.add_predecessor(merge, else_bb);
    b.seal_block(merge);

    let result = b.read_variable(0, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    let func = &b.finish().functions[0];

    let output = print_typed_function(func);
    assert!(output.contains("phi"), "output: {output}");
}

// -- Insta snapshot tests --

#[test]
fn snapshot_arithmetic() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("arithmetic", vec![], IrType::I32);
    let bb = b.create_block();
    b.switch_to_block(bb);

    // let x = 2 + 3 * 4
    let two = b.const_i32(2);
    let three = b.const_i32(3);
    let four = b.const_i32(4);
    let mul = b.mul_i32(three, four);
    let add = b.add_i32(two, mul);
    b.ret(Some(add));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_fibonacci() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("fib", vec![("n", IrType::I32)], IrType::I32);

    let entry = b.create_block();
    let base_case = b.create_block();
    let recurse = b.create_block();
    let merge = b.create_block();

    // entry: if n < 2 goto base else recurse
    b.switch_to_block(entry);
    b.seal_block(entry);
    let n = b.const_i32(0); // placeholder param
    let two = b.const_i32(2);
    let cmp = b.lt_i32(n, two);
    b.br_if(cmp, base_case, recurse);

    // base_case: result = n
    b.switch_to_block(base_case);
    b.add_predecessor(base_case, entry);
    b.seal_block(base_case);
    b.write_variable(0, n);
    b.br(merge);

    // recurse: result = (n-1) + (n-2) (simplified)
    b.switch_to_block(recurse);
    b.add_predecessor(recurse, entry);
    b.seal_block(recurse);
    let one = b.const_i32(1);
    let n_minus_1 = b.sub_i32(n, one);
    let n_minus_2 = b.sub_i32(n, two);
    let sum = b.add_i32(n_minus_1, n_minus_2);
    b.write_variable(0, sum);
    b.br(merge);

    // merge: phi + return
    b.switch_to_block(merge);
    b.add_predecessor(merge, base_case);
    b.add_predecessor(merge, recurse);
    b.seal_block(merge);
    let result = b.read_variable(0, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_closure() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("make_counter", vec![], IrType::JSFunction);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let env = b.env_create(1);
    let zero = b.const_i32(0);
    b.env_store(env, 0, zero);
    let loaded = b.env_load(env, 0);
    let func_ref = b.const_i32(1); // placeholder function ref
    let flags = b.const_i32(0);
    let closure = b.create_closure(func_ref, env, flags);
    let _ = loaded;
    b.ret(Some(closure));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_try_catch() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("try_catch", vec![], IrType::JSValue);

    let try_bb = b.create_block();
    let catch_bb = b.create_block();
    let after = b.create_block();

    b.switch_to_block(try_bb);
    b.try_begin(catch_bb);
    let err = b.const_string(0); // "error"
    b.throw_(err);

    b.switch_to_block(catch_bb);
    let exc = b.catch_();
    let exc_val = b.get_exception(exc);
    b.br(after);

    b.switch_to_block(after);
    b.ret(Some(exc_val));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_property_access() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("props", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let obj = b.create_object();
    let key = b.const_string(0);
    let val = b.const_i32(42);
    let boxed = b.box_i32(val);
    b.set_prop(obj, key, boxed);
    let loaded = b.get_prop(obj, key);
    let _has = b.has_prop(obj, key);
    b.ret(Some(loaded));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

#[test]
fn snapshot_for_loop() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("loop_sum", vec![], IrType::I32);

    let entry = b.create_block();
    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    // entry: i = 0, sum = 0
    b.switch_to_block(entry);
    let zero = b.const_i32(0);
    b.write_variable(0, zero); // i
    b.write_variable(1, zero); // sum
    b.br(header);
    b.seal_block(entry);

    // header: if i < 10 goto body else exit
    b.switch_to_block(header);
    b.add_predecessor(header, entry);
    b.add_predecessor(header, body);

    let i = b.read_variable(0, IrType::I32);
    let ten = b.const_i32(10);
    let cmp = b.lt_i32(i, ten);
    b.br_if(cmp, body, exit);

    // body: sum += i; i += 1
    b.switch_to_block(body);
    b.add_predecessor(body, header);
    b.seal_block(body);
    let sum_val = b.read_variable(1, IrType::I32);
    let i_val = b.read_variable(0, IrType::I32);
    let new_sum = b.add_i32(sum_val, i_val);
    b.write_variable(1, new_sum);
    let one = b.const_i32(1);
    let new_i = b.add_i32(i_val, one);
    b.write_variable(0, new_i);
    b.br(header);

    // seal header after body is done
    b.seal_block(header);

    // exit: return sum
    b.switch_to_block(exit);
    b.add_predecessor(exit, header);
    b.seal_block(exit);
    let result = b.read_variable(1, IrType::I32);
    b.ret(Some(result));

    b.end_function();
    let func = &b.finish().functions[0];
    let output = print_typed_function(func);
    insta::assert_snapshot!(output);
}

// ===========================================================================
// Lib tests (from lib.rs)
// ===========================================================================

#[test]
fn build_simple_add_function() {
    // fn add(i32, i32) -> i32 { return param0 + param1 }
    let mut b = IrBuilder::new("add", vec![Type::Int32, Type::Int32], Type::Int32);
    let entry = b.create_block();
    b.switch_to_block(entry);

    let p0 = b.push(Type::Int32, Instruction::Param(0));
    let p1 = b.push(Type::Int32, Instruction::Param(1));
    let sum = b.push(Type::Int32, Instruction::Add(p0, p1));
    b.push(Type::Void, Instruction::Return(Some(sum)));

    let func = b.finish();
    assert_eq!(func.name, "add");
    assert_eq!(func.blocks.len(), 1);
    assert_eq!(func.blocks[0].instructions.len(), 4);

    let ir_text = print_function(&func);
    assert!(ir_text.contains("add"));
    assert!(ir_text.contains("param"));
}

#[test]
fn value_id_display() {
    assert_eq!(format!("{}", ValueId(0)), "v0");
    assert_eq!(format!("{}", ValueId(42)), "v42");
}

#[test]
fn block_id_display() {
    assert_eq!(format!("{}", BlockId(0)), "bb0");
}

#[test]
fn type_display() {
    assert_eq!(format!("{}", Type::Int32), "i32");
    assert_eq!(
        format!("{}", Type::ZonePtr(Box::new(Type::Object))),
        "zone_ptr<object>"
    );
}

// ===========================================================================
// Error path / edge case tests
// ===========================================================================

// -- Builder edge cases: empty function ------------------------------------

#[test]
fn test_builder_empty_function_no_blocks() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("empty", vec![], IrType::Void);
    // End function with no blocks at all
    b.end_function();
    let m = b.finish();
    assert_eq!(m.functions.len(), 1);
    assert!(m.functions[0].blocks.is_empty());
}

// -- Builder edge cases: multiple functions --------------------------------

#[test]
fn test_builder_multiple_functions() {
    let mut b = TypedIrBuilder::new();

    b.begin_function("f1", vec![], IrType::Void);
    let bb1 = b.create_block();
    b.switch_to_block(bb1);
    b.ret(None);
    b.end_function();

    b.begin_function("f2", vec![], IrType::I32);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    let c = b.const_i32(42);
    b.ret(Some(c));
    b.end_function();

    let m = b.finish();
    assert_eq!(m.functions.len(), 2);
    assert_eq!(m.functions[0].name, "f1");
    assert_eq!(m.functions[1].name, "f2");
}

// -- Builder edge cases: no entry set --------------------------------------

#[test]
fn test_builder_no_entry_set() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.ret(None);
    b.end_function();
    let m = b.finish();
    assert!(m.entry.is_none());
}

// -- Builder edge cases: panics -------------------------------------------

#[test]
#[should_panic(expected = "begin_function: already inside a function")]
fn test_builder_begin_function_twice_panics() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f1", vec![], IrType::Void);
    b.begin_function("f2", vec![], IrType::Void);
}

#[test]
#[should_panic(expected = "end_function: not inside a function")]
fn test_builder_end_function_without_begin_panics() {
    let mut b = TypedIrBuilder::new();
    b.end_function();
}

#[test]
#[should_panic(expected = "finish: still inside a function")]
fn test_builder_finish_while_in_function_panics() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f", vec![], IrType::Void);
    let _ = b.finish();
}

#[test]
#[should_panic(expected = "no current block")]
fn test_builder_emit_without_block_panics() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f", vec![], IrType::Void);
    // No block created or switched to
    b.const_i32(42);
}

#[test]
#[should_panic(expected = "switch_to_block: block not found")]
fn test_builder_switch_to_nonexistent_block_panics() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f", vec![], IrType::Void);
    b.switch_to_block(BlockId(999));
}

// -- Verifier edge cases: empty function ----------------------------------

#[test]
fn test_verify_empty_function_reports_error() {
    let func = TypedFunction {
        name: "empty".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
}

// -- Verifier edge cases: empty block -------------------------------------

#[test]
fn test_verify_empty_block_reports_error() {
    let func = TypedFunction {
        name: "f".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 0,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
}

// -- Verifier edge cases: missing terminator ------------------------------

#[test]
fn test_verify_missing_terminator() {
    let func = TypedFunction {
        name: "f".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::Nop,
                ty: IrType::Void,
                operands: vec![],
                block_targets: vec![],
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::InvalidTerminator)
    );
}

// -- Verifier edge cases: undefined value ---------------------------------

#[test]
fn test_verify_undefined_value_in_operand() {
    let func = TypedFunction {
        name: "f".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![
                TypedInstruction {
                    id: ValueId(0),
                    op: Op::BoxI32,
                    ty: IrType::JSValue,
                    operands: vec![ValueId(999)], // undefined operand
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(1),
                    op: Op::Ret,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
            ],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 2,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::UndefinedValue)
    );
}

// -- Verifier edge cases: invalid block target ----------------------------

#[test]
fn test_verify_invalid_block_target() {
    let func = TypedFunction {
        name: "f".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![TypedInstruction {
                id: ValueId(0),
                op: Op::Br,
                ty: IrType::Void,
                operands: vec![],
                block_targets: vec![BlockId(999)], // non-existent block
                span: SourceSpan::DUMMY,
            }],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 1,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::StructuralError)
    );
}

// -- Verifier edge cases: module with out-of-bounds entry -----------------

#[test]
fn test_verify_module_invalid_entry() {
    let module = TypedModule {
        functions: vec![],
        struct_types: vec![],
        entry: Some(0), // out of bounds
    };
    let result = verify_typed_module(&module);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(
            |e| e.kind == VerifyErrorKind::StructuralError && e.message.contains("entry index")
        )
    );
}

// -- Verifier edge cases: phi after non-phi instruction -------------------

#[test]
fn test_verify_phi_after_non_phi() {
    let func = TypedFunction {
        name: "f".to_string(),
        params: vec![],
        return_type: IrType::Void,
        blocks: vec![TypedBasicBlock {
            id: BlockId(0),
            instructions: vec![
                TypedInstruction {
                    id: ValueId(0),
                    op: Op::Nop,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(1),
                    op: Op::Phi,
                    ty: IrType::I32,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
                TypedInstruction {
                    id: ValueId(2),
                    op: Op::Ret,
                    ty: IrType::Void,
                    operands: vec![],
                    block_targets: vec![],
                    span: SourceSpan::DUMMY,
                },
            ],
            sealed: true,
            predecessors: vec![],
        }],
        next_value: 3,
        next_block: 1,
        is_generator: false,
        is_async: false,
    };
    let result = verify_typed_function(&func);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(errors.iter().any(|e| e.kind == VerifyErrorKind::InvalidPhi));
}

// -- Op utility edge cases ------------------------------------------------

#[test]
fn test_op_is_terminator_completeness() {
    // Verify some non-terminators
    assert!(!Op::Nop.is_terminator());
    assert!(!Op::ConstI32(0).is_terminator());
    assert!(!Op::AddI32.is_terminator());
    assert!(!Op::Call.is_terminator());
    // Verify all terminators
    assert!(Op::Br.is_terminator());
    assert!(Op::BrIf.is_terminator());
    assert!(Op::Switch.is_terminator());
    assert!(Op::Ret.is_terminator());
    assert!(Op::Unreachable.is_terminator());
    assert!(Op::Throw.is_terminator());
    assert!(Op::Rethrow.is_terminator());
}

#[test]
fn test_op_is_call_completeness() {
    assert!(Op::Call.is_call());
    assert!(Op::CallMethod.is_call());
    assert!(Op::CallNew.is_call());
    assert!(Op::CallEval.is_call());
    assert!(Op::CallVarargs.is_call());
    assert!(Op::CallRuntime.is_call());
    assert!(Op::TailCall.is_call());
    assert!(Op::Invoke.is_call());
    assert!(!Op::Nop.is_call());
    assert!(!Op::Br.is_call());
}

#[test]
fn test_op_is_memory() {
    assert!(Op::AllocZone.is_memory());
    assert!(Op::AllocHeap.is_memory());
    assert!(Op::FreeZone.is_memory());
    assert!(Op::IncRef.is_memory());
    assert!(Op::DecRef.is_memory());
    assert!(Op::RcIncStrong.is_memory());
    assert!(!Op::Nop.is_memory());
    assert!(!Op::ConstI32(0).is_memory());
}

#[test]
fn test_op_has_side_effects() {
    // Side-effectful ops
    assert!(Op::StoreField.has_side_effects());
    assert!(Op::SetProp.has_side_effects());
    assert!(Op::Throw.has_side_effects());
    assert!(Op::Call.has_side_effects());
    assert!(Op::AllocZone.has_side_effects());
    // Side-effect-free ops
    assert!(!Op::ConstI32(0).has_side_effects());
    assert!(!Op::AddI32.has_side_effects());
    assert!(!Op::EqI32.has_side_effects());
    assert!(!Op::LoadField.has_side_effects());
    assert!(!Op::Phi.has_side_effects());
}

#[test]
fn test_op_category() {
    assert_eq!(Op::ConstI32(0).category(), "constants");
    assert_eq!(Op::AddI32.category(), "arithmetic");
    assert_eq!(Op::EqStrict.category(), "comparison");
    assert_eq!(Op::ToNumber.category(), "type_conversion");
    assert_eq!(Op::BoxI32.category(), "nan_boxing");
    assert_eq!(Op::Br.category(), "control_flow");
    assert_eq!(Op::Phi.category(), "ssa");
    assert_eq!(Op::AllocZone.category(), "memory_allocation");
    assert_eq!(Op::LoadField.category(), "field_element_access");
    assert_eq!(Op::RcIncStrong.category(), "rc_operations");
    assert_eq!(Op::GetProp.category(), "property_access");
    assert_eq!(Op::Call.category(), "calls");
    assert_eq!(Op::CreateObject.category(), "object_shape");
    assert_eq!(Op::GuardType.category(), "type_guards");
    assert_eq!(Op::TryBegin.category(), "exception_handling");
    assert_eq!(Op::TdzCheck.category(), "tdz_drop_flags");
    assert_eq!(Op::EnvCreate.category(), "closure_environment");
    assert_eq!(Op::IterInit.category(), "iterator_protocol");
    assert_eq!(Op::PromiseCreate.category(), "promise_async");
    assert_eq!(Op::GeneratorCreate.category(), "generator");
    assert_eq!(Op::StringConcat.category(), "string_operations");
    assert_eq!(Op::Nop.category(), "miscellaneous");
}

// -- Op equality edge cases -----------------------------------------------

#[test]
fn test_op_eq_const_f64_nan() {
    // NaN == NaN should be true for Op::ConstF64 (bitwise comparison)
    let a = Op::ConstF64(f64::NAN);
    let b = Op::ConstF64(f64::NAN);
    assert_eq!(a, b);
}

#[test]
fn test_op_eq_different_const_values() {
    assert_ne!(Op::ConstI32(1), Op::ConstI32(2));
    assert_ne!(Op::ConstF64(1.0), Op::ConstF64(2.0));
    assert_ne!(Op::ConstBool(true), Op::ConstBool(false));
    assert_ne!(Op::ConstString(0), Op::ConstString(1));
}

#[test]
fn test_op_eq_different_variants() {
    assert_ne!(Op::ConstI32(0), Op::ConstI64(0));
    assert_ne!(Op::AddI32, Op::AddF64);
    assert_ne!(Op::Br, Op::Ret);
}

#[test]
fn test_op_hash_consistency() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(Op::ConstI32(42));
    set.insert(Op::ConstI32(42));
    set.insert(Op::ConstI32(43));
    assert_eq!(set.len(), 2);
}

// -- IrType edge cases ----------------------------------------------------

#[test]
fn test_ir_type_nested_array() {
    let ty = IrType::Array(Box::new(IrType::Array(Box::new(IrType::I32), 3)), 5);
    let formatted = format_ir_type_full(&ty);
    assert_eq!(formatted, "[[i32; 3]; 5]");
}

#[test]
fn test_ir_type_struct_formatting() {
    let ty = IrType::Struct(StructTypeId(0));
    assert_eq!(format_ir_type(&ty), "struct");
    assert_eq!(format_ir_type_full(&ty), "struct#0");
}

// -- ID Display edge cases ------------------------------------------------

#[test]
fn test_value_id_max() {
    assert_eq!(format!("{}", ValueId(u32::MAX)), format!("v{}", u32::MAX));
}

#[test]
fn test_block_id_max() {
    assert_eq!(format!("{}", BlockId(u32::MAX)), format!("bb{}", u32::MAX));
}

#[test]
fn test_function_id_display() {
    assert_eq!(format!("{}", FunctionId(0)), "fn0");
    assert_eq!(format!("{}", FunctionId(42)), "fn42");
}

// -- ConstValue Display ---------------------------------------------------

#[test]
fn test_const_value_display_all_variants() {
    assert_eq!(format!("{}", ConstValue::Undefined), "undefined");
    assert_eq!(format!("{}", ConstValue::Null), "null");
    assert_eq!(format!("{}", ConstValue::Boolean(true)), "true");
    assert_eq!(format!("{}", ConstValue::Boolean(false)), "false");
    assert_eq!(format!("{}", ConstValue::Int32(42)), "42i");
    assert_eq!(format!("{}", ConstValue::Int32(-1)), "-1i");
    assert_eq!(format!("{}", ConstValue::Float64(2.5)), "2.5f");
    assert_eq!(
        format!("{}", ConstValue::String("hello".into())),
        "\"hello\""
    );
}

// -- Legacy Type Display --------------------------------------------------

#[test]
fn test_type_display_all_variants() {
    assert_eq!(format!("{}", Type::Void), "void");
    assert_eq!(format!("{}", Type::Boolean), "bool");
    assert_eq!(format!("{}", Type::Int32), "i32");
    assert_eq!(format!("{}", Type::Float64), "f64");
    assert_eq!(format!("{}", Type::String), "string");
    assert_eq!(format!("{}", Type::Object), "object");
    assert_eq!(format!("{}", Type::Any), "any");
    assert_eq!(
        format!("{}", Type::HeapPtr(Box::new(Type::String))),
        "heap_ptr<string>"
    );
    assert_eq!(
        format!(
            "{}",
            Type::Function(Box::new(crate::FunctionType {
                params: vec![Type::Int32, Type::Float64],
                ret: Type::Boolean,
            }))
        ),
        "fn(i32, f64) -> bool"
    );
}

// -- Module edge cases ----------------------------------------------------

#[test]
fn test_module_new_is_empty() {
    let module = Module::new();
    assert!(module.functions.is_empty());
    assert!(module.entry.is_none());
}

#[test]
fn test_module_default_is_new() {
    let module = Module::default();
    assert!(module.functions.is_empty());
    assert!(module.entry.is_none());
}

// -- Function edge cases --------------------------------------------------

#[test]
fn test_function_entry_block_empty() {
    let func = Function {
        id: FunctionId(0),
        name: "f".to_string(),
        params: vec![],
        return_type: Type::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        local_count: 0,
    };
    assert_eq!(func.entry_block(), None);
}

// -- VerifyError formatting -----------------------------------------------

#[test]
fn test_verify_error_display() {
    let err = VerifyError {
        kind: VerifyErrorKind::UndefinedValue,
        message: "value v99 not defined".to_string(),
    };
    let display = format!("{err}");
    assert!(display.contains("UndefinedValue"));
    assert!(display.contains("value v99 not defined"));
}

// -- Verify valid function passes -----------------------------------------

#[test]
fn test_verify_valid_function_passes() {
    let m = build_simple_module();
    let result = verify_typed_function(&m.functions[0]);
    assert!(result.is_ok());
}

#[test]
fn test_verify_valid_module_passes() {
    let m = build_simple_module();
    let result = verify_typed_module(&m);
    assert!(result.is_ok());
}

// -- Legacy verifier always passes (TODO stub) ----------------------------

#[test]
fn test_verify_legacy_function_always_ok() {
    let func = Function {
        id: FunctionId(0),
        name: "f".to_string(),
        params: vec![],
        return_type: Type::Void,
        blocks: vec![],
        next_value: 0,
        next_block: 0,
        local_count: 0,
    };
    assert!(verify_function(&func).is_ok());
}

// -- Builder: suspend/resume edge cases -----------------------------------

#[test]
fn test_builder_suspend_resume_round_trip() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("outer", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.const_i32(1);

    let saved = b.suspend_function();

    // Build inner function
    b.begin_function("inner", vec![], IrType::Void);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    b.ret(None);
    b.end_function();

    // Resume outer
    b.resume_function(saved);
    b.ret(None);
    b.end_function();

    let m = b.finish();
    assert_eq!(m.functions.len(), 2);
    assert_eq!(m.functions[0].name, "outer");
    assert_eq!(m.functions[1].name, "inner");
}

#[test]
#[should_panic(expected = "resume_function: already inside a function")]
fn test_builder_resume_while_in_function_panics() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("f1", vec![], IrType::Void);
    let saved = b.suspend_function();
    b.begin_function("f2", vec![], IrType::Void);
    b.resume_function(saved);
}

// -- Builder default impl -------------------------------------------------

#[test]
fn test_builder_default() {
    let b = TypedIrBuilder::default();
    let m = b.finish();
    assert!(m.functions.is_empty());
    assert!(m.entry.is_none());
}

// ===========================================================================
// IC opcode tests
// ===========================================================================

#[test]
fn test_ic_get_prop_builder() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_ic", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let obj = b.const_undefined();
    let key = b.const_string(0);
    let ic_id = b.const_i32(0);
    let result = b.ic_get_prop(obj, key, ic_id);

    b.ret(Some(result));
    b.end_function();
    let module = b.finish();

    let func = &module.functions[0];
    let ic_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.op == Op::ICGetProp);
    assert!(ic_inst.is_some(), "ICGetProp should be emitted");
    let inst = ic_inst.unwrap();
    assert_eq!(inst.operands.len(), 3, "ICGetProp should have 3 operands");
    assert_eq!(inst.ty, IrType::JSValue, "ICGetProp should return JSValue");
}

#[test]
fn test_ic_set_prop_builder() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_ic_set", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);

    let obj = b.const_undefined();
    let key = b.const_string(0);
    let val = b.const_i32(42);
    let ic_id = b.const_i32(0);
    b.ic_set_prop(obj, key, val, ic_id);

    b.ret(None);
    b.end_function();
    let module = b.finish();

    let func = &module.functions[0];
    let ic_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.op == Op::ICSetProp);
    assert!(ic_inst.is_some(), "ICSetProp should be emitted");
    let inst = ic_inst.unwrap();
    assert_eq!(inst.operands.len(), 4, "ICSetProp should have 4 operands");
    assert_eq!(inst.ty, IrType::Void, "ICSetProp should return Void");
}

#[test]
fn test_ic_get_prop_verifier_accepts_3_operands() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSValue);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let obj = b.const_undefined();
    let key = b.const_string(0);
    let ic_id = b.const_i32(0);
    let result = b.ic_get_prop(obj, key, ic_id);
    b.ret(Some(result));
    b.end_function();
    let module = b.finish();
    let verify_result = verify_typed_module(&module);
    assert!(
        verify_result.is_ok(),
        "ICGetProp with 3 operands should verify: {verify_result:?}"
    );
}

#[test]
fn test_ic_set_prop_verifier_accepts_4_operands() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    let obj = b.const_undefined();
    let key = b.const_string(0);
    let val = b.const_i32(42);
    let ic_id = b.const_i32(0);
    b.ic_set_prop(obj, key, val, ic_id);
    b.ret(None);
    b.end_function();
    let module = b.finish();
    let verify_result = verify_typed_module(&module);
    assert!(
        verify_result.is_ok(),
        "ICSetProp with 4 operands should verify: {verify_result:?}"
    );
}

#[test]
fn test_ic_get_prop_category() {
    assert_eq!(
        Op::ICGetProp.category(),
        "property_access",
        "ICGetProp should be in property_access category"
    );
}

#[test]
fn test_ic_set_prop_has_side_effects() {
    assert!(
        Op::ICSetProp.has_side_effects(),
        "ICSetProp should have side effects"
    );
}

#[test]
fn test_ic_get_prop_printer() {
    assert_eq!(
        format_op_name(&Op::ICGetProp),
        "ic_get_prop",
        "ICGetProp should print as ic_get_prop"
    );
    assert_eq!(
        format_op_name(&Op::ICSetProp),
        "ic_set_prop",
        "ICSetProp should print as ic_set_prop"
    );
}

// -- CreateObjectLiteral tests ------------------------------------------------

#[test]
fn test_create_object_literal_builder() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test", vec![], IrType::JSObject);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);

    let k0 = b.const_string(0);
    let v0 = b.const_i32(1);
    let k1 = b.const_string(1);
    let v1 = b.const_i32(2);
    let obj = b.create_object_literal(vec![k0, v0, k1, v1]);
    b.ret(Some(obj));
    b.end_function();
    let module = b.finish();

    let func = &module.functions[0];
    let create_inst = func.blocks[0]
        .instructions
        .iter()
        .find(|i| i.op == Op::CreateObjectLiteral);
    assert!(
        create_inst.is_some(),
        "should have CreateObjectLiteral instruction"
    );
    let inst = create_inst.unwrap();
    assert_eq!(inst.ty, IrType::JSObject);
    assert_eq!(inst.operands.len(), 4);
}

#[test]
fn test_create_object_literal_printer() {
    assert_eq!(
        format_op_name(&Op::CreateObjectLiteral),
        "create_object_literal"
    );
}

#[test]
fn test_create_object_literal_category() {
    assert_eq!(Op::CreateObjectLiteral.category(), "object_shape");
}

#[test]
fn test_create_object_literal_has_side_effects() {
    assert!(
        Op::CreateObjectLiteral.has_side_effects(),
        "CreateObjectLiteral should have side effects (allocates)"
    );
}

// =========================================================================
// NewTarget opcode tests
// =========================================================================

#[test]
fn test_new_target_opcode_category() {
    assert_eq!(Op::NewTarget.category(), "miscellaneous");
}

#[test]
fn test_new_target_opcode_is_not_terminator() {
    assert!(!Op::NewTarget.is_terminator());
}

#[test]
fn test_new_target_opcode_is_not_call() {
    assert!(!Op::NewTarget.is_call());
}

#[test]
fn test_new_target_builder_emits_correct_op() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_nt", vec![], IrType::JSValue);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let nt = b.new_target();
    b.ret(Some(nt));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let func = &module.functions[0];

    // Find the NewTarget instruction
    let found = func.blocks[0]
        .instructions
        .iter()
        .any(|inst| inst.op == Op::NewTarget && inst.ty == IrType::JSValue);
    assert!(found, "NewTarget instruction should be emitted by builder");
}

#[test]
fn test_import_meta_builder_emits_correct_op() {
    let mut b = TypedIrBuilder::new();
    b.begin_function("test_im", vec![], IrType::JSValue);
    let entry = b.create_block();
    b.switch_to_block(entry);
    let im = b.import_meta();
    b.ret(Some(im));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let func = &module.functions[0];

    let found = func.blocks[0]
        .instructions
        .iter()
        .any(|inst| inst.op == Op::ImportMeta && inst.ty == IrType::JSValue);
    assert!(found, "ImportMeta instruction should be emitted by builder");
}
