//! NaN-boxing encode/decode as LLVM IR instruction sequences.
//!
//! Emits inline bit-manipulation sequences to box and unbox JavaScript values
//! using the NaN-boxing layout defined in `nanbox`. Each box/unbox operation
//! is a short sequence of LLVM instructions (const, or, and, bitcast, etc.).

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::values::IntValue;

/// Quiet NaN base -- all tagged values have these bits set.
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
/// Bit shift to position the 3-bit tag above the 48-bit payload.
const TAG_SHIFT: u64 = 48;
/// Mask for extracting the lower 48-bit payload.
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Emit a `BoxI32` sequence: `QNAN | (TAG_INT << TAG_SHIFT) | (val as u32 as u64)`.
///
/// Takes an i32 LLVM value and returns a NaN-boxed i64.
pub fn emit_box_i32<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let tag_bits = ctx
        .i64_type()
        .const_int(QNAN | (TAG_INT << TAG_SHIFT), false);
    let extended = builder
        .build_int_z_extend(val, ctx.i64_type(), "box_i32_ext")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_or(tag_bits, extended, "box_i32")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}

/// Emit a `BoxF64` sequence: reinterpret the f64 bits as i64.
///
/// Since NaN-boxed doubles are stored as their raw IEEE 754 bit pattern,
/// boxing an f64 is simply a bitcast.
pub fn emit_box_f64<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: inkwell::values::FloatValue<'ctx>,
) -> IntValue<'ctx> {
    builder
        .build_bit_cast(val, ctx.i64_type(), "box_f64")
        .unwrap_or_else(|_| ctx.i64_type().const_zero().into())
        .into_int_value()
}

/// Emit a `BoxBool` sequence: `QNAN | (TAG_BOOL << TAG_SHIFT) | (val as u64)`.
pub fn emit_box_bool<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let tag_bits = ctx
        .i64_type()
        .const_int(QNAN | (TAG_BOOL << TAG_SHIFT), false);
    let extended = builder
        .build_int_z_extend(val, ctx.i64_type(), "box_bool_ext")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_or(tag_bits, extended, "box_bool")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}

/// Emit a `BoxNull` constant: `QNAN | (TAG_NULL << TAG_SHIFT)`.
pub fn emit_box_null<'ctx>(ctx: &'ctx Context) -> IntValue<'ctx> {
    ctx.i64_type()
        .const_int(QNAN | (TAG_NULL << TAG_SHIFT), false)
}

/// Emit a `BoxUndefined` constant: `QNAN | (TAG_UNDEFINED << TAG_SHIFT)`.
pub fn emit_box_undefined<'ctx>(ctx: &'ctx Context) -> IntValue<'ctx> {
    ctx.i64_type()
        .const_int(QNAN | (TAG_UNDEFINED << TAG_SHIFT), false)
}

/// Emit a `BoxString` sequence: `QNAN | (TAG_STRING << TAG_SHIFT) | (ptr & PAYLOAD_MASK)`.
pub fn emit_box_string<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let tag_bits = ctx
        .i64_type()
        .const_int(QNAN | (TAG_STRING << TAG_SHIFT), false);
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    let masked = builder
        .build_and(val, mask, "box_str_mask")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_or(tag_bits, masked, "box_string")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}

/// Emit a `BoxObject` sequence: `QNAN | (TAG_OBJECT << TAG_SHIFT) | (ptr & PAYLOAD_MASK)`.
pub fn emit_box_object<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let tag_bits = ctx
        .i64_type()
        .const_int(QNAN | (TAG_OBJECT << TAG_SHIFT), false);
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    let masked = builder
        .build_and(val, mask, "box_obj_mask")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_or(tag_bits, masked, "box_object")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}

/// Emit an `UnboxI32` sequence: `(val & PAYLOAD_MASK) as i32`.
pub fn emit_unbox_i32<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    let masked = builder
        .build_and(val, mask, "unbox_i32_mask")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_int_truncate(masked, ctx.i32_type(), "unbox_i32")
        .unwrap_or_else(|_| ctx.i32_type().const_zero())
}

/// Emit an `UnboxF64` sequence: bitcast i64 to f64.
///
/// The caller must ensure the value is actually a boxed f64 (i.e., a regular
/// double, not a tagged NaN-box value).
pub fn emit_unbox_f64<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> inkwell::values::FloatValue<'ctx> {
    builder
        .build_bit_cast(val, ctx.f64_type(), "unbox_f64")
        .unwrap_or_else(|_| ctx.f64_type().const_zero().into())
        .into_float_value()
}

/// Emit an `UnboxBool` sequence: `(val & PAYLOAD_MASK) as i1`.
pub fn emit_unbox_bool<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    let masked = builder
        .build_and(val, mask, "unbox_bool_mask")
        .unwrap_or_else(|_| ctx.i64_type().const_zero());
    builder
        .build_int_truncate(masked, ctx.bool_type(), "unbox_bool")
        .unwrap_or_else(|_| ctx.bool_type().const_zero())
}

/// Emit an `UnboxString` sequence: extract the 48-bit payload as a pointer (i64).
pub fn emit_unbox_string<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    builder
        .build_and(val, mask, "unbox_string")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}

/// Emit an `UnboxObject` sequence: extract the 48-bit payload as a pointer (i64).
pub fn emit_unbox_object<'ctx>(
    builder: &Builder<'ctx>,
    ctx: &'ctx Context,
    val: IntValue<'ctx>,
) -> IntValue<'ctx> {
    let mask = ctx.i64_type().const_int(PAYLOAD_MASK, false);
    builder
        .build_and(val, mask, "unbox_object")
        .unwrap_or_else(|_| ctx.i64_type().const_zero())
}
