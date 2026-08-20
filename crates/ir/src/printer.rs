//! IR printer — textual representation of SSA IR functions.
//!
//! Supports both the legacy `Function` type and the new `TypedFunction`/`TypedModule`.
//!
//! All public functions in this module return `String` and cannot fail, since
//! `fmt::Write` for `String` is infallible. Internal helpers use `fmt::Result`
//! with `?` to avoid `unwrap()` calls.

use std::fmt::{self, Write as _};

use crate::builder::{TypedFunction, TypedModule};
use crate::{BasicBlock, Function, Instruction, InstructionData, IrType, Op, TypedInstruction};

/// Infallible wrapper: `fmt::Write` for `String` never fails, so we can
/// safely discard the result. This helper keeps the public API returning
/// plain `String` rather than `Result`.
fn string_write(f: impl FnOnce(&mut String) -> fmt::Result) -> String {
    let mut out = String::new();
    // SAFETY rationale: `fmt::Write` for `String` is infallible — the only
    // possible `Err` source would be an allocator OOM, which Rust aborts on
    // rather than returning `Err`. We use `let _ =` to acknowledge this.
    let _ = f(&mut out);
    out
}

// ===========================================================================
// Legacy printer (unchanged)
// ===========================================================================

/// Render an IR function as a human-readable string.
pub fn print_function(func: &Function) -> String {
    string_write(|out| {
        // Function header
        out.push_str(&format!("fn @{}(", func.name));
        for (i, p) in func.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{p}")?;
        }
        writeln!(out, ") -> {} {{", func.return_type)?;

        // Blocks
        for block in &func.blocks {
            write_block(block, out)?;
        }

        out.push_str("}\n");
        Ok(())
    })
}

fn write_block(block: &BasicBlock, out: &mut String) -> fmt::Result {
    writeln!(out, "  {}:", block.id)?;
    for inst in &block.instructions {
        out.push_str("    ");
        write_instruction(inst, out)?;
        out.push('\n');
    }
    Ok(())
}

fn write_instruction(data: &InstructionData, out: &mut String) -> fmt::Result {
    let id = data.id;
    let ty = &data.ty;

    match &data.inst {
        Instruction::Const(val) => {
            write!(out, "{id}: {ty} = const {val}")?;
        }
        Instruction::Add(a, b) => {
            write!(out, "{id}: {ty} = add {a}, {b}")?;
        }
        Instruction::Sub(a, b) => {
            write!(out, "{id}: {ty} = sub {a}, {b}")?;
        }
        Instruction::Mul(a, b) => {
            write!(out, "{id}: {ty} = mul {a}, {b}")?;
        }
        Instruction::Div(a, b) => {
            write!(out, "{id}: {ty} = div {a}, {b}")?;
        }
        Instruction::Mod(a, b) => {
            write!(out, "{id}: {ty} = mod {a}, {b}")?;
        }
        Instruction::Neg(a) => {
            write!(out, "{id}: {ty} = neg {a}")?;
        }
        Instruction::Eq(a, b) => {
            write!(out, "{id}: {ty} = eq {a}, {b}")?;
        }
        Instruction::StrictEq(a, b) => {
            write!(out, "{id}: {ty} = strict_eq {a}, {b}")?;
        }
        Instruction::Lt(a, b) => {
            write!(out, "{id}: {ty} = lt {a}, {b}")?;
        }
        Instruction::Gt(a, b) => {
            write!(out, "{id}: {ty} = gt {a}, {b}")?;
        }
        Instruction::Not(a) => {
            write!(out, "{id}: {ty} = not {a}")?;
        }
        Instruction::Call(func_id, args) => {
            write!(out, "{id}: {ty} = call {func_id}(")?;
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{arg}")?;
            }
            out.push(')');
        }
        Instruction::Param(idx) => {
            write!(out, "{id}: {ty} = param {idx}")?;
        }
        Instruction::Return(val) => match val {
            Some(v) => write!(out, "ret {v}")?,
            None => out.push_str("ret void"),
        },
        Instruction::Branch(target) => {
            write!(out, "br {target}")?;
        }
        Instruction::BranchIf(cond, then_bb, else_bb) => {
            write!(out, "br_if {cond}, {then_bb}, {else_bb}")?;
        }
        Instruction::Phi(entries) => {
            write!(out, "{id}: {ty} = phi ")?;
            for (i, (block, val)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "[{block}: {val}]")?;
            }
        }
        Instruction::LoadLocal(slot) => {
            write!(out, "{id}: {ty} = load_local {slot}")?;
        }
        Instruction::StoreLocal(slot, val) => {
            write!(out, "store_local {slot}, {val}")?;
        }
        Instruction::Nop => {
            out.push_str("nop");
        }
    }
    Ok(())
}

/// Render an entire module as a string.
pub fn print_module(module: &crate::Module) -> String {
    string_write(|out| {
        for func in &module.functions {
            out.push_str(&print_function(func));
            out.push('\n');
        }
        Ok(())
    })
}

// ===========================================================================
// New typed printer
// ===========================================================================

/// Format an `IrType` as a human-readable string.
pub fn format_ir_type(ty: &IrType) -> &'static str {
    match ty {
        IrType::Void => "void",
        IrType::Bool => "bool",
        IrType::I32 => "i32",
        IrType::I64 => "i64",
        IrType::F64 => "f64",
        IrType::Ptr => "ptr",
        IrType::ZonePtr => "zone_ptr",
        IrType::HeapPtr => "heap_ptr",
        IrType::JSValue => "jsvalue",
        IrType::JSString => "jsstring",
        IrType::JSObject => "jsobject",
        IrType::JSArray => "jsarray",
        IrType::JSFunction => "jsfunction",
        IrType::JSSymbol => "jssymbol",
        IrType::CompletionRecord => "completion_record",
        IrType::IteratorRecord => "iterator_record",
        // Struct and Array need dynamic formatting — handled separately
        IrType::Struct(_) => "struct",
        IrType::Array(_, _) => "array",
    }
}

/// Format an `IrType` as a String, handling dynamic types.
pub(crate) fn format_ir_type_full(ty: &IrType) -> String {
    match ty {
        IrType::Struct(id) => format!("struct#{}", id.0),
        IrType::Array(inner, count) => {
            format!("[{}; {}]", format_ir_type_full(inner), count)
        }
        _ => format_ir_type(ty).to_string(),
    }
}

/// Map an `Op` variant to a lowercase snake_case name string.
pub fn format_op_name(op: &Op) -> &'static str {
    match op {
        // Constants (7)
        Op::ConstI32(_) => "const_i32",
        Op::ConstI64(_) => "const_i64",
        Op::ConstF64(_) => "const_f64",
        Op::ConstBool(_) => "const_bool",
        Op::ConstNull => "const_null",
        Op::ConstUndefined => "const_undefined",
        Op::ConstString(_) => "const_string",
        Op::LoadGlobal(_) => "load_global",

        // Arithmetic (26)
        Op::AddI32 => "add_i32",
        Op::SubI32 => "sub_i32",
        Op::MulI32 => "mul_i32",
        Op::DivI32 => "div_i32",
        Op::ModI32 => "mod_i32",
        Op::NegI32 => "neg_i32",
        Op::AddF64 => "add_f64",
        Op::SubF64 => "sub_f64",
        Op::MulF64 => "mul_f64",
        Op::DivF64 => "div_f64",
        Op::ModF64 => "mod_f64",
        Op::NegF64 => "neg_f64",
        Op::AddJS => "add_js",
        Op::SubJS => "sub_js",
        Op::MulJS => "mul_js",
        Op::DivJS => "div_js",
        Op::ModJS => "mod_js",
        Op::NegJS => "neg_js",
        Op::ExpJS => "exp_js",
        Op::BitwiseAnd => "bitwise_and",
        Op::BitwiseOr => "bitwise_or",
        Op::BitwiseXor => "bitwise_xor",
        Op::BitwiseNot => "bitwise_not",
        Op::ShiftLeft => "shift_left",
        Op::ShiftRight => "shift_right",
        Op::ShiftRightUnsigned => "shift_right_unsigned",

        // Comparison (20)
        Op::EqI32 => "eq_i32",
        Op::EqF64 => "eq_f64",
        Op::EqStrict => "eq_strict",
        Op::EqAbstract => "eq_abstract",
        Op::NeI32 => "ne_i32",
        Op::NeF64 => "ne_f64",
        Op::NeStrict => "ne_strict",
        Op::NeAbstract => "ne_abstract",
        Op::LtI32 => "lt_i32",
        Op::LtF64 => "lt_f64",
        Op::LtJS => "lt_js",
        Op::LeI32 => "le_i32",
        Op::LeF64 => "le_f64",
        Op::LeJS => "le_js",
        Op::GtI32 => "gt_i32",
        Op::GtF64 => "gt_f64",
        Op::GtJS => "gt_js",
        Op::GeI32 => "ge_i32",
        Op::GeF64 => "ge_f64",
        Op::GeJS => "ge_js",

        // Type conversion (8)
        Op::ToNumber => "to_number",
        Op::ToNumeric => "to_numeric",
        Op::ToString => "to_string",
        Op::ToBoolean => "to_boolean",
        Op::ToObject => "to_object",
        Op::ToPrimitive => "to_primitive",
        Op::ToPropertyKey => "to_property_key",
        Op::ToInt32 => "to_int32",
        Op::ToUint32 => "to_uint32",

        // NaN-boxing (18)
        Op::BoxI32 => "box_i32",
        Op::BoxUnsignedI32 => "box_unsigned_i32",
        Op::BoxF64 => "box_f64",
        Op::BoxBool => "box_bool",
        Op::BoxNull => "box_null",
        Op::BoxUndefined => "box_undefined",
        Op::BoxString => "box_string",
        Op::BoxObject => "box_object",
        Op::BoxSymbol => "box_symbol",
        Op::UnboxI32 => "unbox_i32",
        Op::UnboxF64 => "unbox_f64",
        Op::UnboxBool => "unbox_bool",
        Op::UnboxString => "unbox_string",
        Op::UnboxObject => "unbox_object",
        Op::UnboxSymbol => "unbox_symbol",
        Op::TypeofBoxed => "typeof_boxed",
        Op::IsNullish => "is_nullish",
        Op::IsFalsy => "is_falsy",

        // Control flow (5)
        Op::Br => "br",
        Op::BrIf => "br_if",
        Op::Switch => "switch",
        Op::Ret => "ret",
        Op::Unreachable => "unreachable",

        // SSA (1)
        Op::Phi => "phi",

        // Memory allocation (7)
        Op::AllocZone => "alloc_zone",
        Op::AllocHeap => "alloc_heap",
        Op::AllocStack => "alloc_stack",
        Op::AllocArray => "alloc_array",
        Op::FreeZone => "free_zone",
        Op::IncRef => "inc_ref",
        Op::DecRef => "dec_ref",

        // Field/element access (6)
        Op::LoadField => "load_field",
        Op::StoreField => "store_field",
        Op::LoadElement => "load_element",
        Op::StoreElement => "store_element",
        Op::LoadLocal => "load_local",
        Op::StoreLocal => "store_local",
        Op::LoadParam(_) => "load_param",

        // RC operations (5)
        Op::RcIncStrong => "rc_inc_strong",
        Op::RcDecStrong => "rc_dec_strong",
        Op::RcIncWeak => "rc_inc_weak",
        Op::RcDecWeak => "rc_dec_weak",
        Op::RcIsUnique => "rc_is_unique",

        // Property access (15)
        Op::GetProp => "get_prop",
        Op::SetProp => "set_prop",
        Op::SetPropStrict => "set_prop_strict",
        Op::DeleteProp => "delete_prop",
        Op::HasProp => "has_prop",
        Op::GetElem => "get_elem",
        Op::SetElem => "set_elem",
        Op::DeleteElem => "delete_elem",
        Op::GetPropDynamic => "get_prop_dynamic",
        Op::SetPropDynamic => "set_prop_dynamic",
        Op::SetPropDynamicStrict => "set_prop_dynamic_strict",
        Op::GetSuper => "get_super",
        Op::SetSuper => "set_super",
        Op::GetPrivate => "get_private",
        Op::SetPrivate => "set_private",
        Op::PrivateFieldGet => "private_field_get",
        Op::PrivateFieldSet => "private_field_set",
        Op::PrivateFieldHas => "private_field_has",
        Op::InstallPrivateField => "install_private_field",
        Op::ICGetProp => "ic_get_prop",
        Op::ICSetProp => "ic_set_prop",

        // Calls (8)
        Op::Call => "call",
        Op::CallMethod => "call_method",
        Op::CallNew => "call_new",
        Op::CallEval => "call_eval",
        Op::CallEvalDirect => "call_eval_direct",
        Op::CallVarargs => "call_varargs",
        Op::CallRuntime => "call_runtime",
        Op::TailCall => "tail_call",
        Op::Invoke => "invoke",

        // Object/Shape (12)
        Op::CreateObject => "create_object",
        Op::CreateObjectLiteral => "create_object_literal",
        Op::CreateArray => "create_array",
        Op::CreateRegExp => "create_regexp",
        Op::CreateClosure => "create_closure",
        Op::CreateArguments => "create_arguments",
        Op::ObjectDefineProperty => "object_define_property",
        Op::ObjectGetPrototype => "object_get_prototype",
        Op::ObjectSetPrototype => "object_set_prototype",
        Op::ShapeCheck => "shape_check",
        Op::ShapeTransition => "shape_transition",
        Op::InstanceOf => "instance_of",

        // Type guards (3)
        Op::GuardType => "guard_type",
        Op::GuardShape => "guard_shape",
        Op::GuardTruthiness => "guard_truthiness",

        // Exception handling (8)
        Op::TryBegin => "try_begin",
        Op::TryEnd => "try_end",
        Op::Throw => "throw",
        Op::Catch => "catch",
        Op::Finally => "finally",
        Op::Rethrow => "rethrow",
        Op::IsException => "is_exception",
        Op::GetException => "get_exception",

        // TDZ / Drop flags (4)
        Op::TdzCheck => "tdz_check",
        Op::TdzInit => "tdz_init",
        Op::DropFlagSet => "drop_flag_set",
        Op::DropFlagCheck => "drop_flag_check",

        // Closure environment (6)
        Op::EnvCreate => "env_create",
        Op::EnvLoad => "env_load",
        Op::EnvStore => "env_store",
        Op::EnvExtend => "env_extend",
        Op::EnvLookup => "env_lookup",
        Op::EnvLookupStore => "env_lookup_store",

        // JsBox (3)
        Op::AllocBox => "alloc_box",
        Op::BoxLoad => "box_load",
        Op::BoxStore => "box_store",

        // Iterator protocol (7)
        Op::IterInit => "iter_init",
        Op::ForInInit => "for_in_init",
        Op::IterInitAsync => "iter_init_async",
        Op::IterNext => "iter_next",
        Op::IterDone => "iter_done",
        Op::IterValue => "iter_value",
        Op::IterClose => "iter_close",

        // Promise/Async (4)
        Op::PromiseCreate => "promise_create",
        Op::PromiseResolve => "promise_resolve",
        Op::PromiseReject => "promise_reject",
        Op::Await => "await",

        // Generator (3)
        Op::GeneratorCreate => "generator_create",
        Op::Yield => "yield",
        Op::YieldDelegate => "yield_delegate",

        // String operations (4)
        Op::StringConcat => "string_concat",
        Op::StringCompare => "string_compare",
        Op::StringLength => "string_length",
        Op::StringCharAt => "string_char_at",

        // Miscellaneous (7)
        Op::Nop => "nop",
        Op::Debugger => "debugger",
        Op::ThisValue => "this_value",
        Op::NewTarget => "new_target",
        Op::ImportMeta => "import_meta",
        Op::SuperCall => "super_call",
        Op::WithScope => "with_scope",
    }
}

/// Write inline constant data from Op variants.
fn write_op_data(op: &Op, out: &mut String) -> fmt::Result {
    match op {
        Op::ConstI32(v) => write!(out, " {v}")?,
        Op::ConstI64(v) => write!(out, " {v}")?,
        Op::ConstF64(v) => write!(out, " {v}")?,
        Op::ConstBool(v) => write!(out, " {v}")?,
        Op::ConstString(idx) => write!(out, " @{idx}")?,
        Op::LoadGlobal(idx) => write!(out, " @{idx}")?,
        Op::LoadParam(idx) => write!(out, " %param{idx}")?,
        _ => {}
    }
    Ok(())
}

/// Print a single typed instruction.
pub fn print_typed_instruction(inst: &TypedInstruction, out: &mut String) {
    // Writing to String is infallible, so we discard the Result.
    let _ = write_typed_instruction(inst, out);
}

/// Write a single typed instruction to the output.
fn write_typed_instruction(inst: &TypedInstruction, out: &mut String) -> fmt::Result {
    let has_result = inst.ty != IrType::Void;
    if has_result {
        write!(out, "%{}: {} = ", inst.id, format_ir_type_full(&inst.ty))?;
    }

    out.push_str(format_op_name(&inst.op));

    // Print inline data from Op variant (constant values)
    write_op_data(&inst.op, out)?;

    // Print operands
    for (i, operand) in inst.operands.iter().enumerate() {
        if i == 0 {
            out.push(' ');
        } else {
            out.push_str(", ");
        }
        write!(out, "%{operand}")?;
    }

    // Print block targets
    for (i, target) in inst.block_targets.iter().enumerate() {
        if !inst.operands.is_empty() || i > 0 {
            out.push_str(", ");
        } else {
            out.push(' ');
        }
        write!(out, "{target}")?;
    }
    Ok(())
}

/// Print a typed function as human-readable text.
pub fn print_typed_function(func: &TypedFunction) -> String {
    string_write(|out| write_typed_function(func, out))
}

/// Write a typed function as human-readable text to the output.
fn write_typed_function(func: &TypedFunction, out: &mut String) -> fmt::Result {
    // Function header: fn @name(%0: type, %1: type) -> ret_type {
    out.push_str("fn @");
    out.push_str(&func.name);
    out.push('(');
    for (i, (name, ty)) in func.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "%{}: {}", name, format_ir_type_full(ty))?;
    }
    writeln!(out, ") -> {} {{", format_ir_type_full(&func.return_type))?;

    // Blocks
    for block in &func.blocks {
        writeln!(out, "  {}:", block.id)?;
        for inst in &block.instructions {
            out.push_str("    ");
            write_typed_instruction(inst, out)?;
            out.push('\n');
        }
    }

    out.push_str("}\n");
    Ok(())
}

/// Print a typed module as human-readable text.
pub fn print_typed_module(module: &TypedModule) -> String {
    string_write(|out| {
        // Struct types
        for (name, fields) in &module.struct_types {
            write!(out, "struct {name} {{ ")?;
            for (i, (fname, ftype)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "{}: {}", fname, format_ir_type_full(ftype))?;
            }
            out.push_str(" }\n");
        }

        if !module.struct_types.is_empty() {
            out.push('\n');
        }

        // Functions
        for func in &module.functions {
            write_typed_function(func, out)?;
            out.push('\n');
        }

        // Entry point
        if let Some(entry) = module.entry
            && entry < module.functions.len()
        {
            writeln!(out, "entry: @{}", module.functions[entry].name)?;
        }

        Ok(())
    })
}

// ===========================================================================
// Tests
// ===========================================================================
