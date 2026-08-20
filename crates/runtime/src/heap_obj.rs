//! Heap-allocated objects with inline reference-counting headers.
//!
//! Provides [`HeapObj`], a `#[repr(C)]` wrapper that prepends a [`HeapHeader`]
//! (strong count, weak count, GC color, buffered flag) to any [`TaggedObj`].
//! Functions in this module allocate, deallocate, retain, release, and
//! enumerate children of heap objects, integrating with the cycle collector
//! via [`crate::cycle_integration`].
//!
//! # Layout
//!
//! ```text
//! HeapObj<T>
//! ┌──────────────────────────────────┐
//! │ HeapHeader (16 bytes)            │
//! │  strong_count: u32               │
//! │  weak_count:   u32               │
//! │  color:        u8                │
//! │  buffered:     u8                │
//! │  flags:        u16               │
//! │  _pad:         [u8; 4]           │
//! ├──────────────────────────────────┤
//! │ TaggedObj<T>                     │
//! │  tag: u8                         │
//! │  value: T                        │
//! └──────────────────────────────────┘
//! ```

use std::collections::VecDeque;

use nanbox::JsValue;

use crate::array::JsArray;
use crate::environment::Environment;
use crate::function::JsFunction;
use crate::internal_data::{ElementsStorage, InternalData, UnifiedObject};
use crate::iterator::{IteratorResult, JsIterator};
use crate::jsbox::JsBox;
use crate::object::JsObject;
use crate::promise::JsPromise;
use crate::proxy::ProxyObject;
use crate::regexp_bridge::JsRegExpData;
use crate::rt_api::{ClosureData, JsError, JsMap, JsSet, JsWeakRef, NativeFuncData};
use crate::tagged_obj::{ObjTag, TaggedObj, read_obj_tag};

/// High bit set on the tag byte to distinguish heap objects from zone objects.
pub const HEAP_BIT: u8 = 0x80;

/// Colors used by the inline GC header for Bacon-Rajan cycle collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HeapColor {
    /// In use or already scanned (default).
    Black = 0,
    /// Being traced during the mark phase.
    Gray = 1,
    /// Garbage candidate (trial RC reached zero).
    White = 2,
    /// Suspect — RC was decremented to non-zero.
    Purple = 3,
}

/// Inline reference-counting and GC header for heap-allocated objects.
///
/// Stored at the start of every [`HeapObj`] to enable retain/release
/// and cycle collection without external hash maps.
#[repr(C)]
#[derive(Debug)]
pub struct HeapHeader {
    /// Strong (owning) reference count.
    pub strong_count: u32,
    /// Weak reference count (reserved for weak refs).
    pub weak_count: u32,
    /// Current GC color (see [`HeapColor`]).
    pub color: u8,
    /// Whether this object is in the cycle collector's suspect buffer.
    pub buffered: u8,
    /// Reserved flags (e.g., frozen, sealed).
    pub flags: u16,
    /// Padding to align to 16 bytes.
    pub _pad: [u8; 4],
}

impl HeapHeader {
    /// Creates a new header with strong_count=1 and color=Black.
    fn new() -> Self {
        Self {
            strong_count: 1,
            weak_count: 0,
            color: HeapColor::Black as u8,
            buffered: 0,
            flags: 0,
            _pad: [0; 4],
        }
    }
}

/// A heap-allocated object with an inline reference-counting header.
///
/// The `#[repr(C)]` layout guarantees that `HeapHeader` comes first,
/// followed by the `TaggedObj<T>` payload. This allows `header_from_bits`
/// to locate the header from any NaN-boxed object pointer.
#[repr(C)]
pub struct HeapObj<T> {
    /// Inline GC / RC header.
    pub header: HeapHeader,
    /// The tagged object payload.
    pub tagged: TaggedObj<T>,
}

/// Allocate a heap object with the given tag and value, returning NaN-boxed bits.
///
/// The tag byte is OR'd with [`HEAP_BIT`] to mark this as a heap allocation.
/// The returned `u64` can be stored in a `JsValue::object()` slot.
pub fn alloc_heap_obj<T>(tag: ObjTag, value: T) -> u64 {
    let obj = Box::new(HeapObj {
        header: HeapHeader::new(),
        tagged: TaggedObj {
            tag: (tag as u8) | HEAP_BIT,
            value,
        },
    });
    // Compute pointer to the TaggedObj portion (which starts after HeapHeader).
    let raw = Box::into_raw(obj);
    // SAFETY: HeapObj is repr(C), so `tagged` is at a known offset from `raw`.
    let tagged_ptr = unsafe { &raw const (*raw).tagged as *const () };
    JsValue::object(tagged_ptr).raw_bits()
}

/// Deallocate a heap object previously allocated by [`alloc_heap_obj`].
///
/// # Safety
///
/// The caller must ensure that `bits` was returned by `alloc_heap_obj::<T>`
/// with the same type `T`, and that the object has not already been freed.
pub unsafe fn dealloc_heap_obj<T>(bits: u64) {
    let v = JsValue::from_raw_bits(bits);
    let Some(ptr) = v.as_object() else { return };
    if ptr.is_null() {
        return;
    }
    // The NaN-boxed pointer points to the TaggedObj inside the HeapObj.
    // Walk back to the HeapObj start.
    let tagged_ptr = ptr as *mut TaggedObj<T>;
    // SAFETY: The tagged_ptr is at offset `size_of::<HeapHeader>()` from the
    // HeapObj allocation. We recover the original Box pointer.
    let heap_obj_ptr = unsafe {
        (tagged_ptr as *mut u8).sub(std::mem::size_of::<HeapHeader>()) as *mut HeapObj<T>
    };
    unsafe {
        drop(Box::from_raw(heap_obj_ptr));
    }
}

/// Returns `true` if the NaN-boxed bits represent a heap-allocated object
/// (i.e., the tag byte has [`HEAP_BIT`] set).
pub fn is_heap_object(bits: u64) -> bool {
    let v = JsValue::from_raw_bits(bits);
    let Some(ptr) = v.as_object() else {
        return false;
    };
    if ptr.is_null() {
        return false;
    }
    // SAFETY: The pointer was created by TaggedObj::boxed or alloc_heap_obj,
    // so the first byte at this address is the tag byte.
    let tag = unsafe { *(ptr as *const u8) };
    tag & HEAP_BIT != 0
}

/// Strip the [`HEAP_BIT`] from a tag byte, returning the base `ObjTag` value.
pub fn heap_obj_tag_kind(tag: u8) -> u8 {
    tag & !HEAP_BIT
}

/// Get an immutable reference to the [`HeapHeader`] from NaN-boxed bits.
///
/// Returns `None` if `bits` is not a heap object.
pub fn header_from_bits(bits: u64) -> Option<&'static HeapHeader> {
    if !is_heap_object(bits) {
        return None;
    }
    let v = JsValue::from_raw_bits(bits);
    let ptr = v.as_object()?;
    if ptr.is_null() {
        return None;
    }
    // The NaN-boxed pointer targets the TaggedObj; the HeapHeader is just before it.
    // SAFETY: The allocation was made by alloc_heap_obj, which places HeapHeader
    // immediately before TaggedObj in a repr(C) struct.
    let header_ptr =
        unsafe { (ptr as *const u8).sub(std::mem::size_of::<HeapHeader>()) as *const HeapHeader };
    Some(unsafe { &*header_ptr })
}

/// Get a mutable reference to the [`HeapHeader`] from NaN-boxed bits.
///
/// Returns `None` if `bits` is not a heap object.
pub fn header_from_bits_mut(bits: u64) -> Option<&'static mut HeapHeader> {
    if !is_heap_object(bits) {
        return None;
    }
    let v = JsValue::from_raw_bits(bits);
    let ptr = v.as_object()?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Same as header_from_bits, but returns &mut. The caller must
    // ensure exclusive access.
    let header_ptr =
        unsafe { (ptr as *mut u8).sub(std::mem::size_of::<HeapHeader>()) as *mut HeapHeader };
    Some(unsafe { &mut *header_ptr })
}

/// Read the strong reference count of a heap object.
///
/// Returns `None` if `bits` is not a heap object.
pub fn strong_count(bits: u64) -> Option<u32> {
    header_from_bits(bits).map(|h| h.strong_count)
}

/// Increment the strong reference count of a heap object.
///
/// No-op if `bits` is not a heap object. Also notifies the cycle collector
/// via [`crate::cycle_integration::on_increment`].
pub fn retain(bits: u64) {
    if let Some(header) = header_from_bits_mut(bits) {
        header.strong_count = header.strong_count.saturating_add(1);
        crate::cycle_integration::on_increment(bits);
    }
}

/// Decrement the strong reference count of a heap object.
///
/// - If the count reaches zero, calls [`release_children`] to iteratively
///   decrement all child references, then returns `true` (caller should dealloc).
/// - If the count is still nonzero, notifies the cycle collector via
///   [`crate::cycle_integration::on_decrement`] and returns `false`.
/// - Returns `false` for non-heap objects (no-op).
pub fn release(bits: u64) -> bool {
    let Some(header) = header_from_bits_mut(bits) else {
        return false;
    };
    header.strong_count = header.strong_count.saturating_sub(1);
    if header.strong_count == 0 {
        // SAFETY: The object is dead (RC=0). We iteratively release children
        // and collect objects to deallocate.
        unsafe {
            release_children(bits);
        }
        true
    } else {
        crate::cycle_integration::on_decrement(bits);
        false
    }
}

/// Release all children of a heap object iteratively (non-recursive).
///
/// Uses a [`VecDeque`] work queue to avoid stack overflow on deep object chains.
/// For each child whose strong count reaches zero, its children are also
/// enqueued. Dead children (RC=0) are deallocated via [`dealloc_by_tag`].
///
/// # Safety
///
/// The caller must ensure that `bits` points to a valid heap object whose
/// strong count has already reached zero.
pub unsafe fn release_children(bits: u64) {
    let mut work = VecDeque::new();
    // SAFETY: bits is a valid heap object per caller contract.
    unsafe {
        enumerate_children(bits, &mut work);
    }
    let mut to_dealloc: Vec<u64> = Vec::new();

    while let Some(child_bits) = work.pop_front() {
        if !is_heap_object(child_bits) {
            continue;
        }
        let Some(header) = header_from_bits_mut(child_bits) else {
            continue;
        };
        header.strong_count = header.strong_count.saturating_sub(1);
        if header.strong_count == 0 {
            // SAFETY: child_bits is a valid heap object (checked above).
            unsafe {
                enumerate_children(child_bits, &mut work);
            }
            to_dealloc.push(child_bits);
        }
    }

    // Dealloc all dead children.
    for &dead in &to_dealloc {
        // SAFETY: Each dead object had RC decremented to 0 above and was
        // allocated by alloc_heap_obj.
        unsafe {
            dealloc_by_tag(dead);
        }
    }
}

/// Enumerate all object-typed child references of a heap object.
///
/// For each child that is itself an object pointer, its raw bits are pushed
/// into `out`. Currently handles Plain (properties), Array (elements), and
/// Closure (environment slots). Other tag types will gain child enumeration
/// in step 0.3.0c.
///
/// # Safety
///
/// The caller must ensure that `bits` points to a valid tagged/heap object
/// whose tag matches the actual allocated type.
unsafe fn enumerate_children(bits: u64, out: &mut VecDeque<u64>) {
    let Some(raw_tag) = read_obj_tag(bits) else {
        return;
    };
    let tag = heap_obj_tag_kind(raw_tag);

    match tag {
        t if t == ObjTag::Plain as u8 => {
            // SAFETY: Caller guarantees bits was allocated with ObjTag::Plain.
            let obj = unsafe { crate::tagged_obj::deref_tagged::<JsObject>(bits) };
            let Some(obj) = obj else { return };
            match &obj.storage {
                crate::object::PropertyStorage::Inline(slots) => {
                    for slot in slots {
                        if slot.is_object() {
                            out.push_back(slot.raw_bits());
                        }
                    }
                }
                crate::object::PropertyStorage::Dictionary(entries) => {
                    for (_key, val) in entries {
                        if val.is_object() {
                            out.push_back(val.raw_bits());
                        }
                    }
                }
            }
        }
        t if t == ObjTag::Array as u8 => {
            // SAFETY: Caller guarantees bits was allocated with ObjTag::Array.
            let arr = unsafe { crate::tagged_obj::deref_tagged::<JsArray>(bits) };
            let Some(arr) = arr else { return };
            for elem in &arr.elements {
                if elem.is_object() {
                    out.push_back(elem.raw_bits());
                }
            }
        }
        t if t == ObjTag::Closure as u8 => {
            // SAFETY: Caller guarantees bits was allocated with ObjTag::Closure.
            let closure = unsafe { crate::tagged_obj::deref_tagged::<ClosureData>(bits) };
            let Some(closure) = closure else { return };
            let env_val = JsValue::from_raw_bits(closure.env);
            if !env_val.is_object() {
                return;
            }
            let Some(ptr) = env_val.as_object() else {
                return;
            };
            if ptr.is_null() {
                return;
            }
            // SAFETY: The closure's env field was created by __esc_rt_env_create
            // which allocates an Environment via Box::into_raw.
            let env = unsafe { &*(ptr as *const Environment) };
            for &slot_bits in &env.slots {
                let slot_val = JsValue::from_raw_bits(slot_bits);
                if slot_val.is_object() {
                    out.push_back(slot_bits);
                }
            }
        }
        t if t == ObjTag::JsBox as u8 => {
            // SAFETY: Caller guarantees bits was allocated with ObjTag::JsBox.
            let jsbox = unsafe { crate::tagged_obj::deref_tagged::<JsBox>(bits) };
            let Some(jsbox) = jsbox else { return };
            let val = JsValue::from_raw_bits(jsbox.value);
            if val.is_object() {
                out.push_back(jsbox.value);
            }
        }
        t if t == ObjTag::Unified as u8 => {
            // SAFETY: Caller guarantees bits was allocated with ObjTag::Unified.
            let uobj = unsafe { crate::tagged_obj::deref_tagged::<UnifiedObject>(bits) };
            let Some(uobj) = uobj else { return };
            // Trace named property slots.
            for slot in &uobj.slots {
                if slot.is_object() {
                    out.push_back(slot.raw_bits());
                }
            }
            // Trace indexed elements.
            match &uobj.elements {
                ElementsStorage::Dense(elems) => {
                    for elem in elems {
                        if elem.is_object() {
                            out.push_back(elem.raw_bits());
                        }
                    }
                }
                ElementsStorage::Holey(elems) => {
                    for val in elems.iter().flatten() {
                        if val.is_object() {
                            out.push_back(val.raw_bits());
                        }
                    }
                }
                ElementsStorage::Dictionary(map) => {
                    for val in map.values() {
                        if val.is_object() {
                            out.push_back(val.raw_bits());
                        }
                    }
                }
                ElementsStorage::None => {}
            }
            // Trace internal data children.
            if let Some(ref data) = uobj.internal {
                enumerate_internal_data_children(data, out);
            }
        }
        _ => {
            // Other types: child enumeration will be added in step 0.3.0c.
        }
    }
}

/// Enumerate object-typed children from [`InternalData`].
fn enumerate_internal_data_children(data: &InternalData, out: &mut VecDeque<u64>) {
    match data {
        InternalData::Function { env, name, .. } => {
            let env_val = JsValue::from_raw_bits(*env);
            if env_val.is_object() {
                out.push_back(*env);
            }
            let name_val = JsValue::from_raw_bits(*name);
            if name_val.is_object() {
                out.push_back(*name);
            }
        }
        InternalData::Error {
            message,
            raw_message,
            stack,
            ..
        } => {
            for &bits in &[*message, *raw_message, *stack] {
                let val = JsValue::from_raw_bits(bits);
                if val.is_object() {
                    out.push_back(bits);
                }
            }
        }
        InternalData::Proxy {
            target, handler, ..
        } => {
            for &bits in &[*target, *handler] {
                let val = JsValue::from_raw_bits(bits);
                if val.is_object() {
                    out.push_back(bits);
                }
            }
        }
        InternalData::Promise { inner } => {
            let val = JsValue::from_raw_bits(inner.value);
            if val.is_object() {
                out.push_back(inner.value);
            }
            // Trace reaction handlers
            for reaction in &inner.reactions {
                if reaction.on_fulfill != 0 {
                    let v = JsValue::from_raw_bits(reaction.on_fulfill);
                    if v.is_object() {
                        out.push_back(reaction.on_fulfill);
                    }
                }
                if reaction.on_reject != 0 {
                    let v = JsValue::from_raw_bits(reaction.on_reject);
                    if v.is_object() {
                        out.push_back(reaction.on_reject);
                    }
                }
            }
        }
        InternalData::IteratorState { inner } => {
            let val = JsValue::from_raw_bits(inner.target);
            if val.is_object() {
                out.push_back(inner.target);
            }
        }
        InternalData::IterResult { value, done } => {
            for &bits in &[*value, *done] {
                let val = JsValue::from_raw_bits(bits);
                if val.is_object() {
                    out.push_back(bits);
                }
            }
        }
        InternalData::Generator { state_obj, .. } => {
            let val = JsValue::from_raw_bits(*state_obj);
            if val.is_object() {
                out.push_back(*state_obj);
            }
        }
        InternalData::Map { entries } => {
            for (key, val) in entries {
                if key.is_object() {
                    out.push_back(key.raw_bits());
                }
                if val.is_object() {
                    out.push_back(val.raw_bits());
                }
            }
        }
        InternalData::Set { values } => {
            for val in values {
                if val.is_object() {
                    out.push_back(val.raw_bits());
                }
            }
        }
        InternalData::NativeFunc { context, .. } => {
            let val = JsValue::from_raw_bits(*context);
            if val.is_object() {
                out.push_back(*context);
            }
        }
        InternalData::WeakRef { target } => {
            let val = JsValue::from_raw_bits(*target);
            if val.is_object() {
                out.push_back(*target);
            }
        }
        InternalData::AsyncGenerator {
            generator, queue, ..
        } => {
            let val = JsValue::from_raw_bits(*generator);
            if val.is_object() {
                out.push_back(*generator);
            }
            // Trace promise objects in queued requests
            for req in queue {
                let pv = JsValue::from_raw_bits(req.promise_bits);
                if pv.is_object() {
                    out.push_back(req.promise_bits);
                }
                let rv = JsValue::from_raw_bits(req.value);
                if rv.is_object() {
                    out.push_back(req.value);
                }
            }
        }
        InternalData::AsyncIterator { inner } => {
            let src = JsValue::from_raw_bits(inner.source);
            if src.is_object() {
                out.push_back(inner.source);
            }
            if inner.callback != 0 {
                let cb = JsValue::from_raw_bits(inner.callback);
                if cb.is_object() {
                    out.push_back(inner.callback);
                }
            }
            if inner.inner_source != 0 {
                let is_val = JsValue::from_raw_bits(inner.inner_source);
                if is_val.is_object() {
                    out.push_back(inner.inner_source);
                }
            }
        }
        InternalData::None
        | InternalData::Array { .. }
        | InternalData::RegExp { .. }
        | InternalData::Symbol { .. }
        | InternalData::Date { .. }
        | InternalData::BooleanWrapper { .. }
        | InternalData::NumberWrapper { .. }
        | InternalData::StringWrapper { .. } => {}
    }
}

/// Deallocate a heap object by reading its tag and dispatching to the
/// correct typed deallocation.
///
/// # Safety
///
/// The caller must ensure that `bits` was returned by [`alloc_heap_obj`]
/// and the object has not already been freed. The tag byte must accurately
/// reflect the allocated type.
pub unsafe fn dealloc_by_tag(bits: u64) {
    let Some(raw_tag) = read_obj_tag(bits) else {
        return;
    };
    let tag = heap_obj_tag_kind(raw_tag);

    // SAFETY: The caller guarantees bits was allocated by alloc_heap_obj
    // with the type matching the stored tag.
    unsafe {
        match tag {
            t if t == ObjTag::Plain as u8 => dealloc_heap_obj::<JsObject>(bits),
            t if t == ObjTag::Array as u8 => dealloc_heap_obj::<JsArray>(bits),
            t if t == ObjTag::Closure as u8 => dealloc_heap_obj::<ClosureData>(bits),
            t if t == ObjTag::Function as u8 => dealloc_heap_obj::<JsFunction>(bits),
            t if t == ObjTag::Iterator as u8 => dealloc_heap_obj::<JsIterator>(bits),
            t if t == ObjTag::Promise as u8 => dealloc_heap_obj::<JsPromise>(bits),
            t if t == ObjTag::Error as u8 => dealloc_heap_obj::<JsError>(bits),
            t if t == ObjTag::IterResult as u8 => dealloc_heap_obj::<IteratorResult>(bits),
            t if t == ObjTag::Map as u8 => dealloc_heap_obj::<JsMap>(bits),
            t if t == ObjTag::Set as u8 => dealloc_heap_obj::<JsSet>(bits),
            t if t == ObjTag::Proxy as u8 => dealloc_heap_obj::<ProxyObject>(bits),
            t if t == ObjTag::WeakRef as u8 => dealloc_heap_obj::<JsWeakRef>(bits),
            t if t == ObjTag::NativeFunc as u8 => dealloc_heap_obj::<NativeFuncData>(bits),
            // Legacy tag — generators now use ObjTag::Unified.
            t if t == ObjTag::Generator as u8 => dealloc_heap_obj::<u64>(bits),
            t if t == ObjTag::RegExp as u8 => dealloc_heap_obj::<JsRegExpData>(bits),
            t if t == ObjTag::JsBox as u8 => dealloc_heap_obj::<JsBox>(bits),
            t if t == ObjTag::Unified as u8 => dealloc_heap_obj::<UnifiedObject>(bits),
            // Date, WeakMap, WeakSet, Symbol — use u64 placeholder for now.
            _ => dealloc_heap_obj::<u64>(bits),
        }
    }
}

/// Extern "C" wrapper for [`retain`], callable from compiled code.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_retain(bits: u64) {
    retain(bits);
}

/// Extern "C" wrapper for [`release`], callable from compiled code.
///
/// Returns `1` if the object was freed (RC reached zero), `0` otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_release(bits: u64) -> u8 {
    if release(bits) {
        // SAFETY: release() already released children; now dealloc the root.
        unsafe {
            dealloc_by_tag(bits);
        }
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ObjectFlags, ObjectHeader, PropertyStorage};
    use shapes::ShapeTable;

    // -----------------------------------------------------------------------
    // Helper: create a heap-allocated plain object with inline properties.
    // -----------------------------------------------------------------------
    fn make_heap_plain(props: Vec<JsValue>) -> u64 {
        let obj = JsObject {
            header: ObjectHeader {
                flags: ObjectFlags::NONE,
                alloc_class: 3,
            },
            shape_id: ShapeTable::EMPTY_SHAPE,
            storage: PropertyStorage::Inline(props),
            prototype: None,
        };
        alloc_heap_obj(ObjTag::Plain, obj)
    }

    /// Create a heap-allocated plain object with dictionary storage.
    fn make_heap_plain_dict(entries: Vec<(String, JsValue)>) -> u64 {
        let obj = JsObject {
            header: ObjectHeader {
                flags: ObjectFlags::NONE,
                alloc_class: 3,
            },
            shape_id: ShapeTable::EMPTY_SHAPE,
            storage: PropertyStorage::Dictionary(entries),
            prototype: None,
        };
        alloc_heap_obj(ObjTag::Plain, obj)
    }

    /// Create a heap-allocated array with the given elements.
    fn make_heap_array(elements: Vec<JsValue>) -> u64 {
        let len = elements.len() as u32;
        let arr = JsArray {
            elements,
            length: len,
        };
        alloc_heap_obj(ObjTag::Array, arr)
    }

    /// Create a heap-allocated closure with the given env bits.
    fn make_heap_closure(func_idx: u32, env: u64) -> u64 {
        alloc_heap_obj(ObjTag::Closure, ClosureData { func_idx, env })
    }

    // -----------------------------------------------------------------------
    // 1. Basic HeapObj allocation and deallocation
    // -----------------------------------------------------------------------

    #[test]
    fn test_alloc_heap_obj_is_heap() {
        let bits = make_heap_plain(vec![]);
        assert!(
            is_heap_object(bits),
            "alloc_heap_obj should produce a heap object"
        );
        // Cleanup
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_alloc_heap_obj_tag_readable() {
        let bits = make_heap_array(vec![]);
        let raw_tag = read_obj_tag(bits);
        assert!(raw_tag.is_some());
        let tag = raw_tag.unwrap();
        assert_eq!(tag & HEAP_BIT, HEAP_BIT, "tag should have HEAP_BIT set");
        assert_eq!(heap_obj_tag_kind(tag), ObjTag::Array as u8);
        unsafe { dealloc_heap_obj::<JsArray>(bits) };
    }

    #[test]
    fn test_header_from_bits_initial_state() {
        let bits = make_heap_plain(vec![]);
        let header = header_from_bits(bits);
        assert!(header.is_some());
        let h = header.unwrap();
        assert_eq!(h.strong_count, 1, "initial strong_count should be 1");
        assert_eq!(h.weak_count, 0);
        assert_eq!(h.color, HeapColor::Black as u8);
        assert_eq!(h.buffered, 0);
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_header_from_bits_mut_modifiable() {
        let bits = make_heap_plain(vec![]);
        {
            let h = header_from_bits_mut(bits).unwrap();
            h.strong_count = 42;
            h.color = HeapColor::Purple as u8;
        }
        let h = header_from_bits(bits).unwrap();
        assert_eq!(h.strong_count, 42);
        assert_eq!(h.color, HeapColor::Purple as u8);
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_is_heap_object_false_for_non_heap() {
        // A regular TaggedObj (no HeapHeader) should NOT have HEAP_BIT.
        let bits = crate::tagged_obj::TaggedObj::boxed(
            ObjTag::Plain,
            JsObject::new(ShapeTable::EMPTY_SHAPE, 3),
        );
        assert!(
            !is_heap_object(bits),
            "non-heap object should not be detected as heap"
        );
        unsafe { crate::tagged_obj::free_tagged::<JsObject>(bits) };
    }

    #[test]
    fn test_is_heap_object_false_for_primitives() {
        assert!(!is_heap_object(JsValue::int(42).raw_bits()));
        assert!(!is_heap_object(JsValue::number(2.5).raw_bits()));
        assert!(!is_heap_object(JsValue::bool(true).raw_bits()));
        assert!(!is_heap_object(JsValue::undefined().raw_bits()));
        assert!(!is_heap_object(JsValue::null().raw_bits()));
    }

    #[test]
    fn test_header_from_bits_none_for_non_heap() {
        assert!(header_from_bits(JsValue::int(10).raw_bits()).is_none());
        let zone_bits = crate::tagged_obj::TaggedObj::boxed(ObjTag::Array, JsArray::new());
        assert!(header_from_bits(zone_bits).is_none());
        unsafe { crate::tagged_obj::free_tagged::<JsArray>(zone_bits) };
    }

    #[test]
    fn test_heap_obj_tag_kind_strips_heap_bit() {
        assert_eq!(heap_obj_tag_kind(0x80), 0);
        assert_eq!(heap_obj_tag_kind(0x87), ObjTag::Closure as u8);
        assert_eq!(heap_obj_tag_kind(0x81), ObjTag::Array as u8);
        assert_eq!(heap_obj_tag_kind(0x00), 0); // no HEAP_BIT
    }

    // -----------------------------------------------------------------------
    // 2. strong_count
    // -----------------------------------------------------------------------

    #[test]
    fn test_strong_count_initial() {
        let bits = make_heap_plain(vec![]);
        assert_eq!(strong_count(bits), Some(1));
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_strong_count_none_for_primitive() {
        assert_eq!(strong_count(JsValue::int(5).raw_bits()), None);
    }

    // -----------------------------------------------------------------------
    // 3. retain / release basics
    // -----------------------------------------------------------------------

    #[test]
    fn test_retain_increments_count() {
        let bits = make_heap_plain(vec![]);
        assert_eq!(strong_count(bits), Some(1));
        retain(bits);
        assert_eq!(strong_count(bits), Some(2));
        retain(bits);
        assert_eq!(strong_count(bits), Some(3));
        // Manually reset to avoid triggering release_children on dealloc
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_retain_noop_for_non_heap() {
        // Should not crash on primitives
        retain(JsValue::int(42).raw_bits());
        retain(JsValue::undefined().raw_bits());
    }

    #[test]
    fn test_release_to_zero_returns_true() {
        let bits = make_heap_plain(vec![]);
        assert_eq!(strong_count(bits), Some(1));
        // release returns true when RC hits 0
        let freed = release(bits);
        assert!(freed, "release should return true when RC reaches zero");
        // Object is now dead — do NOT access it. Dealloc the root.
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_release_nonzero_returns_false() {
        let bits = make_heap_plain(vec![]);
        retain(bits); // RC=2
        let freed = release(bits); // RC=1
        assert!(!freed, "release should return false when RC > 0");
        assert_eq!(strong_count(bits), Some(1));
        // Clean up — decrement to zero manually
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_release_noop_for_non_heap() {
        assert!(!release(JsValue::int(42).raw_bits()));
        assert!(!release(JsValue::undefined().raw_bits()));
    }

    // -----------------------------------------------------------------------
    // 4. enumerate_children
    // -----------------------------------------------------------------------

    #[test]
    fn test_enumerate_children_plain_inline() {
        let child1 = make_heap_plain(vec![]);
        let child2 = make_heap_array(vec![]);
        let parent = make_heap_plain(vec![
            JsValue::from_raw_bits(child1),
            JsValue::int(99), // primitive, should be skipped
            JsValue::from_raw_bits(child2),
        ]);
        let mut out = VecDeque::new();
        unsafe { enumerate_children(parent, &mut out) };
        assert_eq!(out.len(), 2);
        assert!(out.contains(&child1));
        assert!(out.contains(&child2));
        // Cleanup (manually, no release_children)
        header_from_bits_mut(child1).unwrap().strong_count = 0;
        header_from_bits_mut(child2).unwrap().strong_count = 0;
        unsafe {
            dealloc_heap_obj::<JsObject>(parent);
            dealloc_heap_obj::<JsObject>(child1);
            dealloc_heap_obj::<JsArray>(child2);
        }
    }

    #[test]
    fn test_enumerate_children_plain_dictionary() {
        let child = make_heap_plain(vec![]);
        let parent = make_heap_plain_dict(vec![
            ("a".to_string(), JsValue::int(1)),
            ("b".to_string(), JsValue::from_raw_bits(child)),
        ]);
        let mut out = VecDeque::new();
        unsafe { enumerate_children(parent, &mut out) };
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], child);
        header_from_bits_mut(child).unwrap().strong_count = 0;
        unsafe {
            dealloc_heap_obj::<JsObject>(parent);
            dealloc_heap_obj::<JsObject>(child);
        }
    }

    #[test]
    fn test_enumerate_children_array() {
        let elem = make_heap_plain(vec![]);
        let arr = make_heap_array(vec![
            JsValue::int(1),
            JsValue::from_raw_bits(elem),
            JsValue::number(2.5),
        ]);
        let mut out = VecDeque::new();
        unsafe { enumerate_children(arr, &mut out) };
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], elem);
        header_from_bits_mut(elem).unwrap().strong_count = 0;
        unsafe {
            dealloc_heap_obj::<JsArray>(arr);
            dealloc_heap_obj::<JsObject>(elem);
        }
    }

    #[test]
    fn test_enumerate_children_closure() {
        // Create an environment with object-typed slots.
        let child = make_heap_plain(vec![]);
        let env = Box::new(Environment {
            slots: vec![child, JsValue::int(10).raw_bits()],
            parent: std::ptr::null_mut(),
        });
        let env_ptr = Box::into_raw(env) as *const ();
        let env_bits = JsValue::object(env_ptr).raw_bits();
        let closure = make_heap_closure(0, env_bits);

        let mut out = VecDeque::new();
        unsafe { enumerate_children(closure, &mut out) };
        assert_eq!(
            out.len(),
            1,
            "closure should enumerate one object child from env"
        );
        assert_eq!(out[0], child);

        // Cleanup
        header_from_bits_mut(child).unwrap().strong_count = 0;
        unsafe {
            dealloc_heap_obj::<ClosureData>(closure);
            dealloc_heap_obj::<JsObject>(child);
            // Reconstruct and drop the Environment Box
            drop(Box::from_raw(env_ptr as *mut Environment));
        }
    }

    #[test]
    fn test_enumerate_children_unknown_tag() {
        // A heap-allocated error object — enumerate_children returns nothing
        // (not yet implemented for Error).
        let err = alloc_heap_obj(
            ObjTag::Error,
            JsError {
                error_tag: 0,
                message: 0,
                raw_message: 0,
                stack: 0,
            },
        );
        let mut out = VecDeque::new();
        unsafe { enumerate_children(err, &mut out) };
        assert!(out.is_empty(), "unknown tags should produce no children");
        unsafe { dealloc_heap_obj::<JsError>(err) };
    }

    #[test]
    fn test_enumerate_children_primitive_children() {
        // An object whose properties are all primitives — no children.
        let parent = make_heap_plain(vec![
            JsValue::int(1),
            JsValue::number(2.0),
            JsValue::bool(false),
            JsValue::undefined(),
        ]);
        let mut out = VecDeque::new();
        unsafe { enumerate_children(parent, &mut out) };
        assert!(out.is_empty());
        unsafe { dealloc_heap_obj::<JsObject>(parent) };
    }

    // -----------------------------------------------------------------------
    // 5. release_children
    // -----------------------------------------------------------------------

    #[test]
    fn test_release_children_single_child() {
        let child = make_heap_plain(vec![]);
        // Retain child to RC=2 (one ref from parent, one "external")
        retain(child);
        let parent = make_heap_plain(vec![JsValue::from_raw_bits(child)]);

        // Release children of parent (simulating parent reaching RC=0)
        unsafe { release_children(parent) };

        // Child's RC should have been decremented from 2 to 1.
        assert_eq!(strong_count(child), Some(1));

        // Cleanup
        header_from_bits_mut(child).unwrap().strong_count = 0;
        unsafe {
            dealloc_heap_obj::<JsObject>(parent);
            dealloc_heap_obj::<JsObject>(child);
        }
    }

    #[test]
    fn test_release_children_deep_chain() {
        // Build a chain: obj1 -> obj2 -> obj3 -> obj4 -> obj5
        // Each child has RC=1 (only held by parent).
        let obj5 = make_heap_plain(vec![]);
        let obj4 = make_heap_plain(vec![JsValue::from_raw_bits(obj5)]);
        let obj3 = make_heap_plain(vec![JsValue::from_raw_bits(obj4)]);
        let obj2 = make_heap_plain(vec![JsValue::from_raw_bits(obj3)]);
        let obj1 = make_heap_plain(vec![JsValue::from_raw_bits(obj2)]);

        // release_children on obj1 should cascade: obj2->0, obj3->0, obj4->0, obj5->0
        unsafe { release_children(obj1) };

        // All children should have been deallocated. We can only verify indirectly
        // by checking the chain did not stack overflow (iterative approach works).
        // obj1 itself is NOT deallocated by release_children — caller does that.
        unsafe { dealloc_heap_obj::<JsObject>(obj1) };
    }

    #[test]
    fn test_release_children_wide() {
        // Parent with 4 children, each at RC=1
        let c1 = make_heap_plain(vec![]);
        let c2 = make_heap_plain(vec![]);
        let c3 = make_heap_plain(vec![]);
        let c4 = make_heap_plain(vec![]);
        let parent = make_heap_plain(vec![
            JsValue::from_raw_bits(c1),
            JsValue::from_raw_bits(c2),
            JsValue::from_raw_bits(c3),
            JsValue::from_raw_bits(c4),
        ]);

        // All children should be freed (RC=1 -> 0)
        unsafe { release_children(parent) };

        // Parent is not freed by release_children
        unsafe { dealloc_heap_obj::<JsObject>(parent) };
    }

    #[test]
    fn test_release_children_shared_child() {
        // Two parents point to the same child (child RC=2)
        let shared_child = make_heap_plain(vec![]);
        retain(shared_child); // RC=2

        let parent_a = make_heap_plain(vec![JsValue::from_raw_bits(shared_child)]);
        let parent_b = make_heap_plain(vec![JsValue::from_raw_bits(shared_child)]);

        // Release parent_a's children — shared_child RC: 2 -> 1
        unsafe { release_children(parent_a) };
        assert_eq!(strong_count(shared_child), Some(1));

        // Release parent_b's children — shared_child RC: 1 -> 0, gets deallocated
        unsafe { release_children(parent_b) };

        // Cleanup parents (children already freed)
        unsafe {
            dealloc_heap_obj::<JsObject>(parent_a);
            dealloc_heap_obj::<JsObject>(parent_b);
        }
    }

    #[test]
    fn test_release_children_mixed_zone_heap() {
        // Create a zone (non-heap) object and a heap object as children
        let zone_child = crate::tagged_obj::TaggedObj::boxed(
            ObjTag::Plain,
            JsObject::new(ShapeTable::EMPTY_SHAPE, 1),
        );
        let heap_child = make_heap_plain(vec![]);
        let parent = make_heap_plain(vec![
            JsValue::from_raw_bits(zone_child),
            JsValue::from_raw_bits(heap_child),
        ]);

        // release_children should only decrement heap_child, skip zone_child
        unsafe { release_children(parent) };

        // zone_child is still alive (no RC decrement for zone objects)
        // heap_child was freed (RC=1 -> 0)
        unsafe {
            dealloc_heap_obj::<JsObject>(parent);
            crate::tagged_obj::free_tagged::<JsObject>(zone_child);
        }
    }

    // -----------------------------------------------------------------------
    // 6. dealloc_by_tag
    // -----------------------------------------------------------------------

    #[test]
    fn test_dealloc_by_tag_plain() {
        let bits = make_heap_plain(vec![]);
        // Just verify it doesn't crash
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_dealloc_by_tag_array() {
        let bits = make_heap_array(vec![JsValue::int(1), JsValue::int(2)]);
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_dealloc_by_tag_closure() {
        let bits = make_heap_closure(0, 0);
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_dealloc_by_tag_error() {
        let bits = alloc_heap_obj(
            ObjTag::Error,
            JsError {
                error_tag: 1,
                message: 0,
                raw_message: 0,
                stack: 0,
            },
        );
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_dealloc_by_tag_all_known_variants() {
        use crate::function::{FunctionKind, JsFunction};
        use crate::iterator::IteratorKind;

        // Plain
        let plain = make_heap_plain(vec![]);
        unsafe { dealloc_by_tag(plain) };

        // Array
        let arr = make_heap_array(vec![]);
        unsafe { dealloc_by_tag(arr) };

        // Closure
        let closure = make_heap_closure(0, 0);
        unsafe { dealloc_by_tag(closure) };

        // Function
        let func = alloc_heap_obj(
            ObjTag::Function,
            JsFunction::new("f".to_string(), FunctionKind::Normal, 0),
        );
        unsafe { dealloc_by_tag(func) };

        // Iterator
        let iter = alloc_heap_obj(
            ObjTag::Iterator,
            JsIterator {
                kind: IteratorKind::Array,
                target: 0,
                index: 0,
                done: false,
                keys: Vec::new(),
                helper: None,
            },
        );
        unsafe { dealloc_by_tag(iter) };

        // IterResult
        let iter_result = alloc_heap_obj(
            ObjTag::IterResult,
            IteratorResult {
                value: JsValue::undefined().raw_bits(),
                done: JsValue::bool(true).raw_bits(),
            },
        );
        unsafe { dealloc_by_tag(iter_result) };

        // Error
        let err = alloc_heap_obj(
            ObjTag::Error,
            JsError {
                error_tag: 0,
                message: 0,
                raw_message: 0,
                stack: 0,
            },
        );
        unsafe { dealloc_by_tag(err) };

        // Map
        let map = alloc_heap_obj(ObjTag::Map, JsMap::default());
        unsafe { dealloc_by_tag(map) };

        // Set
        let set = alloc_heap_obj(ObjTag::Set, JsSet::default());
        unsafe { dealloc_by_tag(set) };

        // WeakRef
        let wr = alloc_heap_obj(ObjTag::WeakRef, JsWeakRef { target: 0 });
        unsafe { dealloc_by_tag(wr) };

        // NativeFunc
        let nf = alloc_heap_obj(
            ObjTag::NativeFunc,
            NativeFuncData {
                func: |x| x,
                context: 0,
            },
        );
        unsafe { dealloc_by_tag(nf) };
    }

    // -----------------------------------------------------------------------
    // 7. __esc_rt_retain / __esc_rt_release extern "C" wrappers
    // -----------------------------------------------------------------------

    #[test]
    fn test_cs_rt_retain_release_roundtrip() {
        let bits = make_heap_plain(vec![]);
        __esc_rt_retain(bits);
        assert_eq!(strong_count(bits), Some(2));
        let freed = __esc_rt_release(bits);
        assert_eq!(freed, 0, "should not be freed at RC=1");
        assert_eq!(strong_count(bits), Some(1));
        let freed = __esc_rt_release(bits);
        assert_eq!(freed, 1, "should be freed at RC=0");
        // Object is now deallocated by __esc_rt_release
    }

    // -----------------------------------------------------------------------
    // 8. Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_release_saturating_sub_at_zero() {
        // If somehow RC is already 0, release should not underflow
        let bits = make_heap_plain(vec![]);
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        // This should be a no-op (saturating sub), returns true
        let freed = release(bits);
        assert!(freed);
        unsafe { dealloc_by_tag(bits) };
    }

    #[test]
    fn test_retain_saturating_add_near_max() {
        let bits = make_heap_plain(vec![]);
        header_from_bits_mut(bits).unwrap().strong_count = u32::MAX;
        retain(bits);
        assert_eq!(
            strong_count(bits),
            Some(u32::MAX),
            "should saturate at u32::MAX"
        );
        header_from_bits_mut(bits).unwrap().strong_count = 0;
        unsafe { dealloc_heap_obj::<JsObject>(bits) };
    }

    #[test]
    fn test_heap_color_values() {
        assert_eq!(HeapColor::Black as u8, 0);
        assert_eq!(HeapColor::Gray as u8, 1);
        assert_eq!(HeapColor::White as u8, 2);
        assert_eq!(HeapColor::Purple as u8, 3);
    }
}
