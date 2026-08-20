//! NaN-boxed value representation for the runtime.
//!
//! Encodes JavaScript values into a 64-bit representation using the NaN-boxing
//! technique. IEEE 754 doubles have a large space of NaN bit patterns that we
//! exploit to store tagged pointers, integers, booleans, null, and undefined.

use std::fmt;
use std::hash::{Hash, Hasher};

/// Quiet NaN bits — any f64 with these bits set (and additional payload) is NaN.
const QNAN: u64 = 0x7FF8_0000_0000_0000;

/// Tag values stored in bits 48..50 of the NaN payload.
const TAG_INT: u64 = 0x0001;
const TAG_BOOL: u64 = 0x0002;
const TAG_NULL: u64 = 0x0003;
const TAG_UNDEFINED: u64 = 0x0004;
const TAG_OBJECT: u64 = 0x0005;
const TAG_STRING: u64 = 0x0006;
const TAG_SYMBOL: u64 = 0x0007;

/// Mask to extract the 3-bit tag from the tagged region.
const TAG_MASK: u64 = 0x0007;

/// Mask to extract the lower 48-bit payload (pointer or integer data).
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Shift amount to position the tag above the 48-bit payload.
const TAG_SHIFT: u64 = 48;

/// A 64-bit NaN-boxed JavaScript value.
///
/// Layout:
/// - Regular f64: any bit pattern that is NOT a quiet NaN with our tag bits
/// - Tagged value: QNAN | (tag << 48) | payload
#[derive(Clone, Copy, PartialEq)]
pub struct JsValue {
    bits: u64,
}

impl JsValue {
    /// Creates a number value from an f64.
    pub fn number(n: f64) -> Self {
        Self { bits: n.to_bits() }
    }

    /// Creates an integer value from an i32.
    pub fn int(n: i32) -> Self {
        Self {
            bits: QNAN | (TAG_INT << TAG_SHIFT) | (n as u32 as u64),
        }
    }

    /// Creates a boolean value.
    pub fn bool(b: bool) -> Self {
        Self {
            bits: QNAN | (TAG_BOOL << TAG_SHIFT) | (b as u64),
        }
    }

    /// Creates the null value.
    pub fn null() -> Self {
        Self {
            bits: QNAN | (TAG_NULL << TAG_SHIFT),
        }
    }

    /// Creates the undefined value.
    pub fn undefined() -> Self {
        Self {
            bits: QNAN | (TAG_UNDEFINED << TAG_SHIFT),
        }
    }

    /// Creates an object value from a raw pointer.
    ///
    /// # Panics
    ///
    /// Panics if the pointer does not fit in the 48-bit address space.
    pub fn object(ptr: *const ()) -> Self {
        assert!(
            validate_pointer(ptr),
            "pointer exceeds 48-bit address space"
        );
        let addr = ptr as u64;
        Self {
            bits: QNAN | (TAG_OBJECT << TAG_SHIFT) | (addr & PAYLOAD_MASK),
        }
    }

    /// Creates a string value from a raw pointer.
    ///
    /// # Panics
    ///
    /// Panics if the pointer does not fit in the 48-bit address space.
    pub fn string(ptr: *const ()) -> Self {
        assert!(
            validate_pointer(ptr),
            "pointer exceeds 48-bit address space"
        );
        let addr = ptr as u64;
        Self {
            bits: QNAN | (TAG_STRING << TAG_SHIFT) | (addr & PAYLOAD_MASK),
        }
    }

    /// Creates a symbol value from a symbol ID.
    pub fn symbol(id: u32) -> Self {
        Self {
            bits: QNAN | (TAG_SYMBOL << TAG_SHIFT) | (id as u64),
        }
    }

    // --- Type checks ---

    fn tag(&self) -> Option<u64> {
        if self.bits & QNAN != QNAN {
            return None; // regular f64
        }
        let tag = (self.bits >> TAG_SHIFT) & TAG_MASK;
        if tag == 0 {
            None // plain NaN, no tag
        } else {
            Some(tag)
        }
    }

    /// Returns true if this value is a number (f64).
    pub fn is_number(&self) -> bool {
        self.tag().is_none()
    }

    /// Returns true if this value is a tagged i32 integer.
    pub fn is_int(&self) -> bool {
        self.tag() == Some(TAG_INT)
    }

    /// Returns true if this value is a boolean.
    pub fn is_bool(&self) -> bool {
        self.tag() == Some(TAG_BOOL)
    }

    /// Returns true if this value is null.
    pub fn is_null(&self) -> bool {
        self.tag() == Some(TAG_NULL)
    }

    /// Returns true if this value is undefined.
    pub fn is_undefined(&self) -> bool {
        self.tag() == Some(TAG_UNDEFINED)
    }

    /// Returns true if this value is an object pointer.
    pub fn is_object(&self) -> bool {
        self.tag() == Some(TAG_OBJECT)
    }

    /// Returns true if this value is a string pointer.
    pub fn is_string(&self) -> bool {
        self.tag() == Some(TAG_STRING)
    }

    /// Returns true if this value is a symbol.
    pub fn is_symbol(&self) -> bool {
        self.tag() == Some(TAG_SYMBOL)
    }

    // --- Extractors ---

    /// Extracts the f64 number, if this is a number value.
    pub fn as_number(&self) -> Option<f64> {
        if self.is_number() {
            Some(f64::from_bits(self.bits))
        } else {
            None
        }
    }

    /// Extracts the i32 integer, if this is an int value.
    pub fn as_int(&self) -> Option<i32> {
        if self.is_int() {
            Some((self.bits & PAYLOAD_MASK) as u32 as i32)
        } else {
            None
        }
    }

    /// Extracts the boolean, if this is a bool value.
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.bits & PAYLOAD_MASK) != 0)
        } else {
            None
        }
    }

    /// Extracts the object pointer, if this is an object value.
    pub fn as_object(&self) -> Option<*const ()> {
        if self.is_object() {
            Some((self.bits & PAYLOAD_MASK) as *const ())
        } else {
            None
        }
    }

    /// Extracts the string pointer, if this is a string value.
    pub fn as_string(&self) -> Option<*const ()> {
        if self.is_string() {
            Some((self.bits & PAYLOAD_MASK) as *const ())
        } else {
            None
        }
    }

    /// Extracts the symbol ID, if this is a symbol value.
    pub fn as_symbol(&self) -> Option<u32> {
        if self.is_symbol() {
            Some((self.bits & PAYLOAD_MASK) as u32)
        } else {
            None
        }
    }

    // --- Convenience methods ---

    /// Returns true if this value is null or undefined.
    pub fn is_nullish(&self) -> bool {
        self.is_null() || self.is_undefined()
    }

    /// Returns true if this value is falsy in JavaScript.
    ///
    /// Falsy values detected here: `null`, `undefined`, `false`, integer `0`,
    /// number `0.0`, and number `NaN`.
    ///
    /// **Note:** Empty string (`""`) is also falsy per ECMAScript but cannot be
    /// detected at the nanbox level (requires runtime string dereference).
    /// Use `value_ops::to_boolean()` for full spec-compliant truthiness checks.
    pub fn is_falsy(&self) -> bool {
        if self.is_null() || self.is_undefined() {
            return true;
        }
        if let Some(b) = self.as_bool() {
            return !b;
        }
        if let Some(n) = self.as_int() {
            return n == 0;
        }
        if self.is_number() {
            let n = f64::from_bits(self.bits);
            return n == 0.0 || n.is_nan();
        }
        false
    }

    /// Returns the raw 64-bit representation of this value.
    pub fn raw_bits(&self) -> u64 {
        self.bits
    }

    /// Creates a value from a raw 64-bit representation.
    pub fn from_raw_bits(bits: u64) -> Self {
        Self { bits }
    }

    /// Returns `true` if two values have the same NaN-box type tag.
    ///
    /// This uses O(1) integer comparison on the tag bits, rather than
    /// comparing type-name strings. Note that `int` and `number` are
    /// considered different tags (use `is_numeric()` or explicit checks
    /// for cross-int/number comparison).
    pub fn same_type_tag(&self, other: &JsValue) -> bool {
        self.tag() == other.tag()
    }

    /// Returns the type tag name as a static string.
    pub fn type_tag_name(&self) -> &'static str {
        match self.tag() {
            Some(TAG_INT) => "int",
            Some(TAG_BOOL) => "boolean",
            Some(TAG_NULL) => "null",
            Some(TAG_UNDEFINED) => "undefined",
            Some(TAG_OBJECT) => "object",
            Some(TAG_STRING) => "string",
            Some(TAG_SYMBOL) => "symbol",
            _ => "number",
        }
    }
}

/// Returns true if the pointer fits in the 48-bit address space
/// used by the NaN-boxing payload. This is a PAC-aware validation
/// that checks the upper bits are clear.
pub fn validate_pointer(ptr: *const ()) -> bool {
    (ptr as u64) & !PAYLOAD_MASK == 0
}

impl Eq for JsValue {}

impl Hash for JsValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
    }
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_undefined() {
            write!(f, "undefined")
        } else if self.is_null() {
            write!(f, "null")
        } else if let Some(b) = self.as_bool() {
            write!(f, "{b}")
        } else if let Some(n) = self.as_int() {
            write!(f, "{n}i")
        } else if self.is_object() {
            write!(f, "Object({:#x})", self.bits & PAYLOAD_MASK)
        } else if self.is_string() {
            write!(f, "String({:#x})", self.bits & PAYLOAD_MASK)
        } else if let Some(id) = self.as_symbol() {
            write!(f, "Symbol({id})")
        } else if let Some(n) = self.as_number() {
            write!(f, "{n}")
        } else {
            write!(f, "JsValue({:#018x})", self.bits)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_number() {
        let vals = [0.0, 1.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, 2.56789];
        for v in vals {
            let js = JsValue::number(v);
            assert!(js.is_number());
            assert_eq!(js.as_number(), Some(v));
        }
    }

    #[test]
    fn round_trip_nan() {
        let js = JsValue::number(f64::NAN);
        assert!(js.is_number());
        assert!(js.as_number().unwrap().is_nan());
    }

    #[test]
    fn round_trip_int() {
        for v in [0i32, 1, -1, i32::MAX, i32::MIN] {
            let js = JsValue::int(v);
            assert!(js.is_int());
            assert!(!js.is_number());
            assert_eq!(js.as_int(), Some(v));
        }
    }

    #[test]
    fn round_trip_bool() {
        let t = JsValue::bool(true);
        let f = JsValue::bool(false);
        assert!(t.is_bool());
        assert!(f.is_bool());
        assert_eq!(t.as_bool(), Some(true));
        assert_eq!(f.as_bool(), Some(false));
    }

    #[test]
    fn round_trip_null() {
        let js = JsValue::null();
        assert!(js.is_null());
        assert!(!js.is_undefined());
        assert!(!js.is_number());
    }

    #[test]
    fn round_trip_undefined() {
        let js = JsValue::undefined();
        assert!(js.is_undefined());
        assert!(!js.is_null());
        assert!(!js.is_number());
    }

    #[test]
    fn round_trip_object() {
        let data: u64 = 42;
        let ptr: *const () = &data as *const u64 as *const ();
        let js = JsValue::object(ptr);
        assert!(js.is_object());
        assert_eq!(js.as_object(), Some(ptr));
    }

    #[test]
    fn round_trip_string() {
        let data: u64 = 99;
        let ptr: *const () = &data as *const u64 as *const ();
        let js = JsValue::string(ptr);
        assert!(js.is_string());
        assert_eq!(js.as_string(), Some(ptr));
    }

    #[test]
    fn types_are_exclusive() {
        let values = [
            JsValue::number(1.0),
            JsValue::int(1),
            JsValue::bool(true),
            JsValue::null(),
            JsValue::undefined(),
            JsValue::object(std::ptr::null()),
            JsValue::string(std::ptr::null()),
            JsValue::symbol(0),
        ];

        for (i, v) in values.iter().enumerate() {
            let checks = [
                v.is_number(),
                v.is_int(),
                v.is_bool(),
                v.is_null(),
                v.is_undefined(),
                v.is_object(),
                v.is_string(),
                v.is_symbol(),
            ];
            let true_count = checks.iter().filter(|&&c| c).count();
            assert_eq!(
                true_count, 1,
                "value at index {i} has {true_count} type checks true: {checks:?}"
            );
        }
    }

    #[test]
    fn debug_display() {
        assert_eq!(format!("{:?}", JsValue::undefined()), "undefined");
        assert_eq!(format!("{:?}", JsValue::null()), "null");
        assert_eq!(format!("{:?}", JsValue::bool(true)), "true");
        assert_eq!(format!("{:?}", JsValue::int(42)), "42i");
        assert_eq!(format!("{:?}", JsValue::number(2.5)), "2.5");
        assert_eq!(format!("{:?}", JsValue::symbol(7)), "Symbol(7)");
    }

    // --- New tests ---

    #[test]
    fn round_trip_symbol() {
        for id in [0u32, 1, 42, 1000, u32::MAX] {
            let js = JsValue::symbol(id);
            assert!(js.is_symbol());
            assert!(!js.is_number());
            assert!(!js.is_object());
            assert_eq!(js.as_symbol(), Some(id));
        }
    }

    #[test]
    fn is_nullish_positive() {
        assert!(JsValue::null().is_nullish());
        assert!(JsValue::undefined().is_nullish());
    }

    #[test]
    fn is_nullish_negative() {
        assert!(!JsValue::bool(false).is_nullish());
        assert!(!JsValue::int(0).is_nullish());
        assert!(!JsValue::number(0.0).is_nullish());
        assert!(!JsValue::object(std::ptr::null()).is_nullish());
        assert!(!JsValue::string(std::ptr::null()).is_nullish());
        assert!(!JsValue::symbol(0).is_nullish());
    }

    #[test]
    fn is_falsy_positive() {
        assert!(JsValue::null().is_falsy());
        assert!(JsValue::undefined().is_falsy());
        assert!(JsValue::bool(false).is_falsy());
        assert!(JsValue::int(0).is_falsy());
        assert!(JsValue::number(0.0).is_falsy());
        assert!(JsValue::number(-0.0).is_falsy());
        assert!(JsValue::number(f64::NAN).is_falsy());
    }

    #[test]
    fn is_falsy_negative() {
        assert!(!JsValue::bool(true).is_falsy());
        assert!(!JsValue::int(1).is_falsy());
        assert!(!JsValue::int(-1).is_falsy());
        assert!(!JsValue::number(1.0).is_falsy());
        assert!(!JsValue::number(-1.0).is_falsy());
        assert!(!JsValue::number(f64::INFINITY).is_falsy());
        assert!(!JsValue::object(std::ptr::null()).is_falsy());
        assert!(!JsValue::string(std::ptr::null()).is_falsy());
        assert!(!JsValue::symbol(0).is_falsy());
    }

    #[test]
    fn raw_bits_round_trip() {
        let values = [
            JsValue::number(2.5),
            JsValue::int(42),
            JsValue::bool(true),
            JsValue::null(),
            JsValue::undefined(),
            JsValue::symbol(99),
        ];
        for v in values {
            let bits = v.raw_bits();
            let reconstructed = JsValue::from_raw_bits(bits);
            assert_eq!(v, reconstructed);
        }
    }

    #[test]
    fn type_tag_name_all_types() {
        assert_eq!(JsValue::number(1.0).type_tag_name(), "number");
        assert_eq!(JsValue::number(f64::NAN).type_tag_name(), "number");
        assert_eq!(JsValue::int(1).type_tag_name(), "int");
        assert_eq!(JsValue::bool(true).type_tag_name(), "boolean");
        assert_eq!(JsValue::null().type_tag_name(), "null");
        assert_eq!(JsValue::undefined().type_tag_name(), "undefined");
        assert_eq!(JsValue::object(std::ptr::null()).type_tag_name(), "object");
        assert_eq!(JsValue::string(std::ptr::null()).type_tag_name(), "string");
        assert_eq!(JsValue::symbol(0).type_tag_name(), "symbol");
    }

    #[test]
    fn validate_pointer_valid() {
        let data: u64 = 42;
        let ptr: *const () = &data as *const u64 as *const ();
        assert!(validate_pointer(ptr));
        assert!(validate_pointer(std::ptr::null()));
    }

    #[test]
    fn validate_pointer_invalid() {
        // A pointer with bits set above the 48-bit range is invalid.
        let bad_ptr = 0xFFFF_0000_0000_0000u64 as *const ();
        assert!(!validate_pointer(bad_ptr));
    }

    #[test]
    fn eq_and_hash_consistent() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = JsValue::int(42);
        let b = JsValue::int(42);
        assert_eq!(a, b);

        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);
        assert_eq!(ha.finish(), hb.finish());
    }

    #[test]
    fn eq_different_types() {
        // Values of different types should not be equal even if payloads overlap.
        assert_ne!(JsValue::int(0), JsValue::bool(false));
        assert_ne!(JsValue::null(), JsValue::undefined());
        assert_ne!(JsValue::int(0), JsValue::number(0.0));
    }

    #[test]
    fn size_of_jsvalue() {
        assert_eq!(std::mem::size_of::<JsValue>(), 8);
    }

    #[test]
    fn hash_in_hashset() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(JsValue::int(1));
        set.insert(JsValue::int(1));
        set.insert(JsValue::int(2));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn symbol_debug_display() {
        assert_eq!(format!("{:?}", JsValue::symbol(0)), "Symbol(0)");
        assert_eq!(format!("{:?}", JsValue::symbol(123)), "Symbol(123)");
        assert_eq!(
            format!("{:?}", JsValue::symbol(u32::MAX)),
            format!("Symbol({})", u32::MAX)
        );
    }

    #[test]
    fn symbol_not_other_types() {
        let s = JsValue::symbol(42);
        assert!(!s.is_number());
        assert!(!s.is_int());
        assert!(!s.is_bool());
        assert!(!s.is_null());
        assert!(!s.is_undefined());
        assert!(!s.is_object());
        assert!(!s.is_string());
        assert!(s.is_symbol());
        assert_eq!(s.as_number(), None);
        assert_eq!(s.as_int(), None);
        assert_eq!(s.as_bool(), None);
        assert_eq!(s.as_object(), None);
        assert_eq!(s.as_string(), None);
    }

    // -- Edge cases: NaN vs -NaN -----------------------------------------------

    #[test]
    fn test_negative_nan() {
        // -NaN has the sign bit set
        let neg_nan = f64::from_bits(0xFFF8_0000_0000_0000);
        assert!(neg_nan.is_nan());
        let js = JsValue::number(neg_nan);
        // -NaN should be stored as a number (our tag system only uses quiet NaN payload)
        assert!(js.is_number());
        assert!(js.as_number().unwrap().is_nan());
    }

    #[test]
    fn test_signaling_nan() {
        // Signaling NaN (sNaN) has the quiet bit clear
        let snan = f64::from_bits(0x7FF0_0000_0000_0001);
        assert!(snan.is_nan());
        let js = JsValue::number(snan);
        // Should still be classified as a number
        assert!(js.is_number());
    }

    // -- Edge cases: +0 vs -0 ------------------------------------------------

    #[test]
    fn test_positive_zero_vs_negative_zero() {
        let pos = JsValue::number(0.0);
        let neg = JsValue::number(-0.0);
        // +0 and -0 have different bit patterns
        assert_ne!(pos.raw_bits(), neg.raw_bits());
        // Both should be numbers
        assert!(pos.is_number());
        assert!(neg.is_number());
        // Both should be falsy
        assert!(pos.is_falsy());
        assert!(neg.is_falsy());
    }

    // -- Edge cases: integer boundary values ---------------------------------

    #[test]
    fn test_int_max_min_boundary() {
        let max = JsValue::int(i32::MAX);
        let min = JsValue::int(i32::MIN);
        assert_eq!(max.as_int(), Some(i32::MAX));
        assert_eq!(min.as_int(), Some(i32::MIN));
        // Verify they round-trip correctly
        assert_eq!(
            JsValue::from_raw_bits(max.raw_bits()).as_int(),
            Some(i32::MAX)
        );
        assert_eq!(
            JsValue::from_raw_bits(min.raw_bits()).as_int(),
            Some(i32::MIN)
        );
    }

    // -- Edge cases: null pointer tagging ------------------------------------

    #[test]
    fn test_null_pointer_object() {
        let js = JsValue::object(std::ptr::null());
        assert!(js.is_object());
        assert_eq!(js.as_object(), Some(std::ptr::null()));
        // null pointer object is NOT falsy in the nanbox sense (objects are truthy)
        assert!(!js.is_falsy());
    }

    #[test]
    fn test_null_pointer_string() {
        let js = JsValue::string(std::ptr::null());
        assert!(js.is_string());
        assert_eq!(js.as_string(), Some(std::ptr::null()));
    }

    // -- Edge cases: wrong type extraction returns None ----------------------

    #[test]
    fn test_extract_wrong_type_returns_none() {
        let int = JsValue::int(42);
        assert_eq!(int.as_number(), None);
        assert_eq!(int.as_bool(), None);
        assert_eq!(int.as_object(), None);
        assert_eq!(int.as_string(), None);
        assert_eq!(int.as_symbol(), None);

        let num = JsValue::number(2.5);
        assert_eq!(num.as_int(), None);
        assert_eq!(num.as_bool(), None);
        assert_eq!(num.as_object(), None);
        assert_eq!(num.as_string(), None);
        assert_eq!(num.as_symbol(), None);

        let null = JsValue::null();
        assert_eq!(null.as_number(), None);
        assert_eq!(null.as_int(), None);
        assert_eq!(null.as_bool(), None);
        assert_eq!(null.as_object(), None);
        assert_eq!(null.as_string(), None);
        assert_eq!(null.as_symbol(), None);

        let undef = JsValue::undefined();
        assert_eq!(undef.as_number(), None);
        assert_eq!(undef.as_int(), None);
        assert_eq!(undef.as_bool(), None);
        assert_eq!(undef.as_object(), None);
        assert_eq!(undef.as_string(), None);
        assert_eq!(undef.as_symbol(), None);
    }

    // -- Edge cases: infinity ------------------------------------------------

    #[test]
    fn test_infinity_is_not_falsy() {
        assert!(!JsValue::number(f64::INFINITY).is_falsy());
        assert!(!JsValue::number(f64::NEG_INFINITY).is_falsy());
    }

    #[test]
    fn test_infinity_is_not_nullish() {
        assert!(!JsValue::number(f64::INFINITY).is_nullish());
        assert!(!JsValue::number(f64::NEG_INFINITY).is_nullish());
    }

    // -- Edge cases: special f64 values --------------------------------------

    #[test]
    fn test_number_subnormal() {
        let subnormal = f64::from_bits(1); // smallest subnormal
        let js = JsValue::number(subnormal);
        assert!(js.is_number());
        assert_eq!(js.as_number(), Some(subnormal));
        assert!(!js.is_falsy()); // subnormals are truthy (non-zero)
    }

    #[test]
    fn test_number_max_value() {
        let js = JsValue::number(f64::MAX);
        assert!(js.is_number());
        assert_eq!(js.as_number(), Some(f64::MAX));
    }

    #[test]
    fn test_number_min_value() {
        let js = JsValue::number(f64::MIN);
        assert!(js.is_number());
        assert_eq!(js.as_number(), Some(f64::MIN));
    }

    // -- Edge cases: symbol max id -------------------------------------------

    #[test]
    fn test_symbol_max_id() {
        let js = JsValue::symbol(u32::MAX);
        assert!(js.is_symbol());
        assert_eq!(js.as_symbol(), Some(u32::MAX));
    }

    // -- Edge cases: from_raw_bits with zero ---------------------------------

    #[test]
    fn test_from_raw_bits_zero_is_positive_zero() {
        let js = JsValue::from_raw_bits(0);
        assert!(js.is_number());
        assert_eq!(js.as_number(), Some(0.0));
    }

    // -- Edge cases: pointer validation --------------------------------------

    #[test]
    fn test_validate_pointer_just_below_48bit() {
        let ptr = 0x0000_FFFF_FFFF_FFFFu64 as *const ();
        assert!(validate_pointer(ptr));
    }

    #[test]
    fn test_validate_pointer_just_above_48bit() {
        let ptr = 0x0001_0000_0000_0000u64 as *const ();
        assert!(!validate_pointer(ptr));
    }

    #[test]
    #[should_panic(expected = "pointer exceeds 48-bit address space")]
    fn test_object_with_invalid_pointer_panics() {
        let bad_ptr = 0xFFFF_0000_0000_0000u64 as *const ();
        let _ = JsValue::object(bad_ptr);
    }

    #[test]
    #[should_panic(expected = "pointer exceeds 48-bit address space")]
    fn test_string_with_invalid_pointer_panics() {
        let bad_ptr = 0xFFFF_0000_0000_0000u64 as *const ();
        let _ = JsValue::string(bad_ptr);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn all_i32_round_trip(n: i32) {
            let js = JsValue::int(n);
            prop_assert!(js.is_int());
            prop_assert_eq!(js.as_int(), Some(n));
        }

        #[test]
        fn finite_f64_round_trip(n in proptest::num::f64::NORMAL | proptest::num::f64::SUBNORMAL | proptest::num::f64::ZERO) {
            // Filter to only finite, non-NaN values
            prop_assume!(n.is_finite());
            let js = JsValue::number(n);
            prop_assert!(js.is_number());
            prop_assert_eq!(js.as_number(), Some(n));
        }

        #[test]
        fn symbol_id_round_trip(id: u32) {
            let js = JsValue::symbol(id);
            prop_assert!(js.is_symbol());
            prop_assert_eq!(js.as_symbol(), Some(id));
        }
    }
}
