//! Tagged object wrappers for NaN-boxed object pointers.
//!
//! Provides a `TaggedObj<T>` type that stores a tag byte alongside a value `T`,
//! heap-allocated via `Box`. The resulting pointer is stored in a `JsValue::object()`
//! NaN-boxed representation. The `ObjTag` enum identifies the runtime type of the
//! wrapped value (plain object, array, closure, etc.).

use nanbox::JsValue;

/// Identifies the runtime type of a tagged object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjTag {
    /// A plain JS object (`JsObject`).
    Plain = 0,
    /// A dense JS array (`JsArray`).
    Array = 1,
    /// A JS function (`JsFunction`).
    Function = 2,
    /// A JS iterator.
    Iterator = 3,
    /// A JS Promise.
    Promise = 4,
    /// A JS Error object.
    Error = 5,
    /// An iterator result `{value, done}`.
    IterResult = 6,
    /// A compiled closure with captured environment.
    Closure = 7,
    /// A JS RegExp (reserved for Phase E).
    RegExp = 8,
    /// A JS Date (reserved).
    Date = 9,
    /// A JS Map (reserved).
    Map = 10,
    /// A JS Set (reserved).
    Set = 11,
    /// A JS Proxy wrapping a target with handler traps.
    Proxy = 12,
    /// A JS WeakMap.
    WeakMap = 13,
    /// A JS WeakSet.
    WeakSet = 14,
    /// A JS WeakRef.
    WeakRef = 15,
    /// A JS Symbol.
    Symbol = 16,
    /// A native function (e.g., Proxy revoke).
    NativeFunc = 17,
    /// A JS generator object (created by calling a generator function).
    Generator = 18,
    /// A heap-allocated variable cell for closure capture-by-reference.
    JsBox = 19,
    /// A unified JavaScript object ([`crate::internal_data::UnifiedObject`]).
    ///
    /// During migration, this tag identifies objects using the new unified
    /// representation. After migration completes, all objects (except JsBox)
    /// will use this tag and the `InternalKind` field provides the type
    /// discriminant.
    Unified = 20,
}

/// A heap-allocated tagged wrapper around a value of type `T`.
///
/// The tag byte precedes the payload so that `read_obj_tag` can determine the
/// runtime type without knowing `T`. The `boxed` constructor allocates via `Box`
/// and returns the raw `u64` bits suitable for NaN-boxing as an object pointer.
#[repr(C)]
pub struct TaggedObj<T> {
    /// The object tag identifying the runtime type.
    pub tag: u8,
    /// The wrapped value.
    pub value: T,
}

impl<T> TaggedObj<T> {
    /// Heap-allocates a `TaggedObj<T>` and returns the NaN-boxed `u64` bits.
    ///
    /// The returned value can be stored in a `JsValue::object()` slot and later
    /// recovered via `deref_tagged` or `deref_tagged_mut`.
    pub fn boxed(tag: ObjTag, value: T) -> u64 {
        let obj = Box::new(TaggedObj {
            tag: tag as u8,
            value,
        });
        let ptr = Box::into_raw(obj) as *const ();
        JsValue::object(ptr).raw_bits()
    }
}

/// Read the `ObjTag` byte from a NaN-boxed object pointer.
///
/// Returns `None` if the value is not an object pointer or the pointer is null.
pub fn read_obj_tag(bits: u64) -> Option<u8> {
    let v = JsValue::from_raw_bits(bits);
    let ptr = v.as_object()?;
    if ptr.is_null() {
        return None;
    }
    // Read just the tag byte (first byte of TaggedObj<T>).
    let tag = unsafe {
        // SAFETY: The pointer was created by TaggedObj::boxed via Box::into_raw,
        // guaranteeing the first byte is the tag.
        *(ptr as *const u8)
    };
    Some(tag)
}

/// Dereference a NaN-boxed object pointer to an immutable `TaggedObj<T>` reference.
///
/// Returns `None` if the value is not an object pointer or the pointer is null.
///
/// # Safety
///
/// The caller must ensure that the pointer was created by `TaggedObj::<T>::boxed`
/// with the same type `T`. The caller should verify the tag before calling this.
pub unsafe fn deref_tagged<T>(bits: u64) -> Option<&'static T> {
    let v = JsValue::from_raw_bits(bits);
    let ptr = v.as_object()?;
    if ptr.is_null() {
        return None;
    }
    let tagged = unsafe {
        // SAFETY: Caller guarantees the pointer was created by TaggedObj::<T>::boxed.
        &*(ptr as *const TaggedObj<T>)
    };
    Some(&tagged.value)
}

/// Dereference a NaN-boxed object pointer to a mutable `TaggedObj<T>` reference.
///
/// Returns `None` if the value is not an object pointer or the pointer is null.
///
/// # Safety
///
/// The caller must ensure that the pointer was created by `TaggedObj::<T>::boxed`
/// with the same type `T`. The caller should verify the tag before calling this.
/// The caller must also ensure no other references to this object exist.
pub unsafe fn deref_tagged_mut<T>(bits: u64) -> Option<&'static mut T> {
    let v = JsValue::from_raw_bits(bits);
    let ptr = v.as_object()?;
    if ptr.is_null() {
        return None;
    }
    let tagged = unsafe {
        // SAFETY: Caller guarantees the pointer was created by TaggedObj::<T>::boxed
        // and no aliasing references exist.
        &mut *(ptr as *mut TaggedObj<T>)
    };
    Some(&mut tagged.value)
}

/// Free a tagged object allocated by `TaggedObj::<T>::boxed`.
///
/// # Safety
///
/// The caller must ensure that the pointer was created by `TaggedObj::<T>::boxed`
/// with the same type `T`, and that the object has not already been freed.
pub unsafe fn free_tagged<T>(bits: u64) {
    let v = JsValue::from_raw_bits(bits);
    if let Some(ptr) = v.as_object()
        && !ptr.is_null()
    {
        unsafe {
            // SAFETY: Caller guarantees the pointer was created by TaggedObj::<T>::boxed.
            drop(Box::from_raw(ptr as *mut TaggedObj<T>));
        }
    }
}
