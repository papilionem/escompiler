//! NaN-boxing encode/decode as Cranelift IR instruction sequences.
//!
//! Emits inline bit-manipulation sequences to box and unbox JavaScript values
//! using the NaN-boxing layout defined in `nanbox`. Each box/unbox operation
//! is a short sequence of Cranelift instructions (iconst, bor, band, etc.).

use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::ir::types;
use cranelift_frontend::FunctionBuilder;

/// Quiet NaN base — all tagged values have these bits set.
const QNAN: u64 = 0x7FF8_0000_0000_0000;
/// Tag for i32 integer values.
const TAG_INT: u64 = 0x0001;
/// Tag for boolean values.
const TAG_BOOL: u64 = 0x0002;
/// Tag for null.
const TAG_NULL: u64 = 0x0003;
/// Tag for undefined.
const TAG_UNDEFINED: u64 = 0x0004;
/// Tag for object pointers.
const TAG_OBJECT: u64 = 0x0005;
/// Tag for string pointers.
const TAG_STRING: u64 = 0x0006;
/// Tag for symbol values.
const TAG_SYMBOL: u64 = 0x0007;
/// Bit shift to position the 3-bit tag above the 48-bit payload.
const TAG_SHIFT: u64 = 48;
/// Mask for extracting the lower 48-bit payload.
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Emit a `BoxI32` sequence: `QNAN | (TAG_INT << TAG_SHIFT) | (val as u32 as u64)`.
///
/// Takes an i32 Cranelift value and returns a NaN-boxed i64.
pub fn emit_box_i32(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let tag_bits = builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_INT << TAG_SHIFT)) as i64);
    let extended = builder.ins().uextend(types::I64, val);
    builder.ins().bor(tag_bits, extended)
}

/// Emit a `BoxF64` sequence: reinterpret the f64 bits as i64.
///
/// Since NaN-boxed doubles are stored as their raw IEEE 754 bit pattern,
/// boxing an f64 is simply a bitcast.
pub fn emit_box_f64(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    builder
        .ins()
        .bitcast(types::I64, cranelift_codegen::ir::MemFlags::new(), val)
}

/// Emit a `BoxBool` sequence: `QNAN | (TAG_BOOL << TAG_SHIFT) | (val as u64)`.
///
/// Handles both i8 (raw bool) and i64 (e.g., from runtime calls that return
/// a boolean as i64). For i64 inputs, masks to lowest bit.
pub fn emit_box_bool(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let tag_bits = builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_BOOL << TAG_SHIFT)) as i64);
    let val_ty = builder.func.dfg.value_type(val);
    let extended = if val_ty == types::I64 {
        // Already i64 — mask to 0/1
        let one = builder.ins().iconst(types::I64, 1);
        builder.ins().band(val, one)
    } else {
        builder.ins().uextend(types::I64, val)
    };
    builder.ins().bor(tag_bits, extended)
}

/// Emit a `BoxNull` constant: `QNAN | (TAG_NULL << TAG_SHIFT)`.
pub fn emit_box_null(builder: &mut FunctionBuilder<'_>) -> cranelift_codegen::ir::Value {
    builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_NULL << TAG_SHIFT)) as i64)
}

/// Emit a `BoxUndefined` constant: `QNAN | (TAG_UNDEFINED << TAG_SHIFT)`.
pub fn emit_box_undefined(builder: &mut FunctionBuilder<'_>) -> cranelift_codegen::ir::Value {
    builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_UNDEFINED << TAG_SHIFT)) as i64)
}

/// Emit a `BoxString` sequence: `QNAN | (TAG_STRING << TAG_SHIFT) | (ptr & PAYLOAD_MASK)`.
pub fn emit_box_string(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let tag_bits = builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_STRING << TAG_SHIFT)) as i64);
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().bor(tag_bits, masked)
}

/// Emit a `BoxObject` sequence: `QNAN | (TAG_OBJECT << TAG_SHIFT) | (ptr & PAYLOAD_MASK)`.
pub fn emit_box_object(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let tag_bits = builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_OBJECT << TAG_SHIFT)) as i64);
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().bor(tag_bits, masked)
}

/// Emit a `BoxSymbol` sequence: `QNAN | (TAG_SYMBOL << TAG_SHIFT) | (val as u64)`.
pub fn emit_box_symbol(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let tag_bits = builder
        .ins()
        .iconst(types::I64, (QNAN | (TAG_SYMBOL << TAG_SHIFT)) as i64);
    let extended = builder.ins().uextend(types::I64, val);
    builder.ins().bor(tag_bits, extended)
}

/// Emit an `UnboxI32` sequence: `(val & PAYLOAD_MASK) as i32`.
pub fn emit_unbox_i32(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().ireduce(types::I32, masked)
}

/// Emit an `UnboxF64` sequence: bitcast i64 to f64.
///
/// The caller must ensure the value is actually a boxed f64 (i.e., a regular
/// double, not a tagged NaN-box value).
pub fn emit_unbox_f64(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    builder
        .ins()
        .bitcast(types::F64, cranelift_codegen::ir::MemFlags::new(), val)
}

/// Emit an `UnboxBool` sequence: `(val & PAYLOAD_MASK) as i8`.
pub fn emit_unbox_bool(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    let masked = builder.ins().band(val, mask);
    builder.ins().ireduce(types::I8, masked)
}

/// Emit an `UnboxString` sequence: extract the 48-bit payload as a pointer.
pub fn emit_unbox_string(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    builder.ins().band(val, mask)
}

/// Emit an `UnboxObject` sequence: extract the 48-bit payload as a pointer.
pub fn emit_unbox_object(
    builder: &mut FunctionBuilder<'_>,
    val: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let mask = builder.ins().iconst(types::I64, PAYLOAD_MASK as i64);
    builder.ins().band(val, mask)
}
