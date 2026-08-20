//! Heap-allocated variable cell for closure capture-by-reference.
//!
//! A [`JsBox`] wraps a single NaN-boxed value and is allocated on the heap
//! via [`TaggedObj`](crate::tagged_obj::TaggedObj). Multiple closures can share
//! the same `JsBox`, enabling mutation visibility across closures.
//!
//! ## Usage
//!
//! When a variable is captured AND mutated by a closure, the compiler boxes it
//! into a `JsBox`. The box pointer is stored in the environment slot so all
//! closures sharing the capture see the same mutable cell.
//!
//! ## ABI Functions
//!
//! - [`__esc_rt_alloc_box`] — allocate a new JsBox initialized with a value
//! - [`__esc_rt_box_load`] — load the current value from a JsBox
//! - [`__esc_rt_box_store`] — store a new value into a JsBox

use nanbox::JsValue;

use crate::tagged_obj::{ObjTag, TaggedObj, deref_tagged, deref_tagged_mut, free_tagged};

/// A heap-allocated cell holding a single NaN-boxed value.
///
/// Used for closure capture-by-reference: when a variable is captured
/// AND mutated, it is boxed into a `JsBox` so all closures sharing the
/// capture see the same mutable cell.
#[repr(C)]
pub struct JsBox {
    /// The current value stored in this cell (NaN-boxed).
    pub value: u64,
}

/// Allocate a new `JsBox` on the heap, initialized with `init_val`.
///
/// Returns NaN-boxed bits suitable for storage in a JsValue slot.
/// The returned pointer is tagged with [`ObjTag::JsBox`].
///
/// # Safety
///
/// Called by compiled code. `init_val` must be a valid NaN-boxed value.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_alloc_box(init_val: u64) -> u64 {
    TaggedObj::boxed(ObjTag::JsBox, JsBox { value: init_val })
}

/// Load the current value from a `JsBox`.
///
/// Returns `undefined` if `box_bits` is not a valid JsBox pointer.
///
/// # Safety
///
/// Called by compiled code. `box_bits` must be a valid NaN-boxed pointer
/// to a `JsBox` allocated by [`__esc_rt_alloc_box`].
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_box_load(box_bits: u64) -> u64 {
    // SAFETY: The caller guarantees box_bits was allocated by __esc_rt_alloc_box
    // which uses TaggedObj::boxed with ObjTag::JsBox, so the pointer type matches.
    let Some(jsbox) = (unsafe { deref_tagged::<JsBox>(box_bits) }) else {
        return JsValue::undefined().raw_bits();
    };
    jsbox.value
}

/// Store a new value into a `JsBox`.
///
/// No-op if `box_bits` is not a valid JsBox pointer.
///
/// # Safety
///
/// Called by compiled code. `box_bits` must be a valid NaN-boxed pointer
/// to a `JsBox` allocated by [`__esc_rt_alloc_box`].
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_box_store(box_bits: u64, new_val: u64) {
    // SAFETY: The caller guarantees box_bits was allocated by __esc_rt_alloc_box
    // which uses TaggedObj::boxed with ObjTag::JsBox, so the pointer type matches.
    // The caller also guarantees no other mutable references exist.
    let Some(jsbox) = (unsafe { deref_tagged_mut::<JsBox>(box_bits) }) else {
        return;
    };
    jsbox.value = new_val;
}

/// Free a `JsBox` allocated by [`__esc_rt_alloc_box`].
///
/// # Safety
///
/// The pointer must have been allocated by [`__esc_rt_alloc_box`] and not yet freed.
pub unsafe fn free_jsbox(bits: u64) {
    // SAFETY: The caller guarantees bits was allocated by __esc_rt_alloc_box
    // which uses TaggedObj::boxed with JsBox, so the type matches.
    unsafe {
        free_tagged::<JsBox>(bits);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tagged_obj::read_obj_tag;
    use nanbox::JsValue;

    // -----------------------------------------------------------------------
    // 1. Basic allocation
    // -----------------------------------------------------------------------

    #[test]
    fn test_alloc_box_returns_object_pointer() {
        let bits = __esc_rt_alloc_box(JsValue::number(42.0).raw_bits());
        let val = JsValue::from_raw_bits(bits);
        assert!(val.is_object(), "alloc_box should return an object pointer");
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_alloc_box_has_jsbox_tag() {
        let bits = __esc_rt_alloc_box(JsValue::int(0).raw_bits());
        let tag = read_obj_tag(bits);
        assert_eq!(tag, Some(ObjTag::JsBox as u8));
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 2. Load returns init value
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_load_returns_init_value_number() {
        let init = JsValue::number(42.0).raw_bits();
        let bits = __esc_rt_alloc_box(init);
        let loaded = __esc_rt_box_load(bits);
        assert_eq!(loaded, init, "load should return the init value");
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_box_load_returns_init_value_int() {
        let init = JsValue::int(123).raw_bits();
        let bits = __esc_rt_alloc_box(init);
        let loaded = __esc_rt_box_load(bits);
        assert_eq!(loaded, init);
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 3. Store updates value
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_store_updates_value() {
        let bits = __esc_rt_alloc_box(JsValue::int(1).raw_bits());
        let new_val = JsValue::int(2).raw_bits();
        __esc_rt_box_store(bits, new_val);
        let loaded = __esc_rt_box_load(bits);
        assert_eq!(loaded, new_val, "store should update the value");
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 4. Load from invalid bits returns undefined
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_load_undefined_on_null() {
        let null_bits = JsValue::null().raw_bits();
        let loaded = __esc_rt_box_load(null_bits);
        assert_eq!(
            loaded,
            JsValue::undefined().raw_bits(),
            "load from null should return undefined"
        );
    }

    #[test]
    fn test_box_load_undefined_on_primitive() {
        let int_bits = JsValue::int(42).raw_bits();
        let loaded = __esc_rt_box_load(int_bits);
        assert_eq!(
            loaded,
            JsValue::undefined().raw_bits(),
            "load from non-object should return undefined"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Store to invalid bits is a no-op
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_store_noop_on_null() {
        // Should not crash
        let null_bits = JsValue::null().raw_bits();
        __esc_rt_box_store(null_bits, JsValue::int(99).raw_bits());
    }

    #[test]
    fn test_box_store_noop_on_primitive() {
        // Should not crash
        let int_bits = JsValue::int(42).raw_bits();
        __esc_rt_box_store(int_bits, JsValue::int(99).raw_bits());
    }

    // -----------------------------------------------------------------------
    // 6. Multiple stores
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_stores() {
        let bits = __esc_rt_alloc_box(JsValue::int(1).raw_bits());
        __esc_rt_box_store(bits, JsValue::int(2).raw_bits());
        __esc_rt_box_store(bits, JsValue::int(3).raw_bits());
        let loaded = __esc_rt_box_load(bits);
        assert_eq!(
            loaded,
            JsValue::int(3).raw_bits(),
            "should hold the last stored value"
        );
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 7. Box with various value types
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_with_undefined() {
        let init = JsValue::undefined().raw_bits();
        let bits = __esc_rt_alloc_box(init);
        assert_eq!(__esc_rt_box_load(bits), init);
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_box_with_boolean_true() {
        let init = JsValue::bool(true).raw_bits();
        let bits = __esc_rt_alloc_box(init);
        assert_eq!(__esc_rt_box_load(bits), init);
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_box_with_boolean_false() {
        let init = JsValue::bool(false).raw_bits();
        let bits = __esc_rt_alloc_box(init);
        assert_eq!(__esc_rt_box_load(bits), init);
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_box_with_null() {
        let init = JsValue::null().raw_bits();
        let bits = __esc_rt_alloc_box(init);
        assert_eq!(__esc_rt_box_load(bits), init);
        unsafe { free_jsbox(bits) };
    }

    #[test]
    fn test_box_with_object_value() {
        // Create a unified object, store it in a box, read it back.
        let obj = crate::internal_data::UnifiedObject::ordinary(shapes::ShapeTable::EMPTY_SHAPE);
        let obj_bits = crate::tagged_obj::TaggedObj::boxed(ObjTag::Unified, obj);
        let bits = __esc_rt_alloc_box(obj_bits);
        let loaded = __esc_rt_box_load(bits);
        assert_eq!(loaded, obj_bits, "should round-trip object values");
        unsafe {
            free_jsbox(bits);
            free_tagged::<crate::internal_data::UnifiedObject>(obj_bits);
        }
    }

    // -----------------------------------------------------------------------
    // 8. Free doesn't crash
    // -----------------------------------------------------------------------

    #[test]
    fn test_free_jsbox() {
        let bits = __esc_rt_alloc_box(JsValue::int(42).raw_bits());
        // Should not crash
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 9. Two boxes are independent
    // -----------------------------------------------------------------------

    #[test]
    fn test_two_boxes_independent() {
        let box_a = __esc_rt_alloc_box(JsValue::int(10).raw_bits());
        let box_b = __esc_rt_alloc_box(JsValue::int(20).raw_bits());

        // Modify box_a, box_b should be unaffected.
        __esc_rt_box_store(box_a, JsValue::int(99).raw_bits());

        assert_eq!(__esc_rt_box_load(box_a), JsValue::int(99).raw_bits());
        assert_eq!(
            __esc_rt_box_load(box_b),
            JsValue::int(20).raw_bits(),
            "box_b should be unaffected by store to box_a"
        );

        unsafe {
            free_jsbox(box_a);
            free_jsbox(box_b);
        }
    }

    // -----------------------------------------------------------------------
    // 10. Sharing simulation (two "closures" reading the same box)
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_sharing_simulation() {
        // Simulate: outer function creates box, two closures share it.
        let shared_box = __esc_rt_alloc_box(JsValue::int(0).raw_bits());

        // "Closure A" reads initial value.
        assert_eq!(__esc_rt_box_load(shared_box), JsValue::int(0).raw_bits());

        // "Closure B" writes a new value.
        __esc_rt_box_store(shared_box, JsValue::int(42).raw_bits());

        // "Closure A" reads the updated value.
        assert_eq!(
            __esc_rt_box_load(shared_box),
            JsValue::int(42).raw_bits(),
            "both closures should see the same mutated value"
        );

        unsafe { free_jsbox(shared_box) };
    }

    // -----------------------------------------------------------------------
    // 11. Store then load with type change
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_store_changes_type() {
        let bits = __esc_rt_alloc_box(JsValue::int(1).raw_bits());
        // Store a number (different type) into the same box.
        __esc_rt_box_store(bits, JsValue::number(2.5).raw_bits());
        let loaded = JsValue::from_raw_bits(__esc_rt_box_load(bits));
        assert!(loaded.is_number());
        assert_eq!(loaded.as_number(), Some(2.5));
        unsafe { free_jsbox(bits) };
    }

    // -----------------------------------------------------------------------
    // 12. Round-trip through raw bits
    // -----------------------------------------------------------------------

    #[test]
    fn test_box_raw_bits_round_trip() {
        let init = JsValue::number(2.75).raw_bits();
        let bits = __esc_rt_alloc_box(init);
        // Convert to JsValue and back to raw bits.
        let val = JsValue::from_raw_bits(bits);
        assert!(val.is_object());
        let bits2 = val.raw_bits();
        assert_eq!(__esc_rt_box_load(bits2), init);
        unsafe { free_jsbox(bits) };
    }
}
