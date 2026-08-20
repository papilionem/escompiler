//! Integration between the cycle collector (`cycles`) and the runtime object system.
//!
//! Provides a thread-local cycle collector that tracks NaN-boxed objects
//! and can detect/collect reference cycles among them.
//!
//! # Key types
//!
//! - [`TraceableObject`] — wraps a NaN-boxed object pointer and implements
//!   [`Trace`] by enumerating object-typed property values, array elements,
//!   closure environment slots, and other child references for all 19 `ObjTag`
//!   variants.
//!
//! # Public API
//!
//! - [`register_traceable`] — register an object with the cycle collector.
//! - [`on_decrement`] — notify that an object's RC decreased but did not reach zero.
//! - [`on_increment`] — notify that an object's RC increased.
//! - [`force_collect`] — run a collection cycle and return freed node IDs.

use std::cell::RefCell;

use cycles::{CycleCollector, NodeId, Trace};
use nanbox::JsValue;

use crate::array::JsArray;
use crate::environment::Environment;
use crate::function::JsFunction;
use crate::heap_obj::heap_obj_tag_kind;
use crate::internal_data::{ElementsStorage, InternalData, UnifiedObject};
use crate::iterator::{IteratorResult, JsIterator};
use crate::jsbox::JsBox;
use crate::object::JsObject;
use crate::promise::JsPromise;
use crate::proxy::ProxyObject;
use crate::rt_api::{ClosureData, JsError, JsMap, JsSet, NativeFuncData};
use crate::tagged_obj::{ObjTag, read_obj_tag};

/// A traceable runtime object that can enumerate its object-typed references.
///
/// Wraps the raw NaN-boxed bits of a heap-allocated object and implements
/// [`Trace`] so the cycle collector can discover outgoing references.
pub struct TraceableObject {
    /// The NaN-boxed bits of this object.
    bits: u64,
}

impl TraceableObject {
    /// Create a new traceable wrapper around a NaN-boxed object pointer.
    pub fn new(bits: u64) -> Self {
        Self { bits }
    }
}

impl Trace for TraceableObject {
    fn trace(&self, tracer: &mut dyn FnMut(NodeId)) {
        let val = JsValue::from_raw_bits(self.bits);
        if !val.is_object() {
            return;
        }

        let Some(raw_tag) = read_obj_tag(self.bits) else {
            return;
        };
        // Strip HEAP_BIT so the match works for both zone and heap objects.
        let tag = heap_obj_tag_kind(raw_tag);

        match tag {
            t if t == ObjTag::Plain as u8 => trace_object_properties(self.bits, tracer),
            t if t == ObjTag::Array as u8 => trace_array_elements(self.bits, tracer),
            t if t == ObjTag::Function as u8 => trace_function_env(self.bits, tracer),
            t if t == ObjTag::Iterator as u8 => trace_iterator(self.bits, tracer),
            t if t == ObjTag::Promise as u8 => trace_promise(self.bits, tracer),
            t if t == ObjTag::Error as u8 => trace_error(self.bits, tracer),
            t if t == ObjTag::IterResult as u8 => trace_iter_result(self.bits, tracer),
            t if t == ObjTag::Closure as u8 => trace_closure_env(self.bits, tracer),
            t if t == ObjTag::Map as u8 => trace_map(self.bits, tracer),
            t if t == ObjTag::Set as u8 => trace_set(self.bits, tracer),
            t if t == ObjTag::Proxy as u8 => trace_proxy(self.bits, tracer),
            t if t == ObjTag::NativeFunc as u8 => trace_native_func(self.bits, tracer),
            t if t == ObjTag::Generator as u8 => trace_generator(self.bits, tracer),
            t if t == ObjTag::JsBox as u8 => trace_jsbox(self.bits, tracer),
            t if t == ObjTag::Unified as u8 => trace_unified(self.bits, tracer),
            _ => {
                // Leaf/weak types: RegExp, Date, Symbol, WeakMap, WeakSet, WeakRef.
                // RegExp/Date/Symbol have no JS object references.
                // WeakMap/WeakSet/WeakRef hold weak references that must NOT be
                // traced strongly — the cycle collector should not consider them
                // as reachability edges.
            }
        }
    }
}

// =========================================================================
// Per-tag trace functions
// =========================================================================

/// Trace a single NaN-boxed u64 value: if it is an object, call the tracer.
fn trace_value_bits(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    let val = JsValue::from_raw_bits(bits);
    if val.is_object() {
        tracer(NodeId(bits));
    }
}

/// Trace all property values of a plain object that are themselves objects.
///
/// For each property value stored in the object's `PropertyStorage`, if the
/// value is an object pointer, call the tracer with its bits as a `NodeId`.
fn trace_object_properties(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Plain`, created via `TaggedObj::boxed`.
    let obj = unsafe { crate::tagged_obj::deref_tagged::<JsObject>(bits) };
    let Some(obj) = obj else { return };

    match &obj.storage {
        crate::object::PropertyStorage::Inline(slots) => {
            for slot in slots {
                if slot.is_object() {
                    tracer(NodeId(slot.raw_bits()));
                }
            }
        }
        crate::object::PropertyStorage::Dictionary(entries) => {
            for (_key, val) in entries {
                if val.is_object() {
                    tracer(NodeId(val.raw_bits()));
                }
            }
        }
    }
}

/// Trace all elements of a dense array that are themselves objects.
fn trace_array_elements(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Array`, created via `TaggedObj::boxed`.
    let arr = unsafe { crate::tagged_obj::deref_tagged::<JsArray>(bits) };
    let Some(arr) = arr else { return };

    for elem in &arr.elements {
        if elem.is_object() {
            tracer(NodeId(elem.raw_bits()));
        }
    }
}

/// Trace the closed-over environment of a `JsFunction`.
///
/// If the function has an `env: Some(vec)`, each object-typed slot is traced.
fn trace_function_env(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Function`, created via `TaggedObj::boxed`.
    let func = unsafe { crate::tagged_obj::deref_tagged::<JsFunction>(bits) };
    let Some(func) = func else { return };

    if let Some(ref env) = func.env {
        for slot in env {
            if slot.is_object() {
                tracer(NodeId(slot.raw_bits()));
            }
        }
    }
}

/// Trace the target object of a `JsIterator`.
fn trace_iterator(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Iterator`, created via `TaggedObj::boxed`.
    let iter = unsafe { crate::tagged_obj::deref_tagged::<JsIterator>(bits) };
    let Some(iter) = iter else { return };

    trace_value_bits(iter.target, tracer);
}

/// Trace the value and reaction handlers of a `JsPromise`.
///
/// The `result_promise` field in each `Reaction` is a raw `*mut JsPromise`,
/// not a NaN-boxed value. We trace `value`, `on_fulfill`, and `on_reject`.
///
/// TODO(v0.3): Migrate `result_promise` to NaN-boxed TaggedObj and trace it.
fn trace_promise(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Promise`, created via `TaggedObj::boxed`.
    let promise = unsafe { crate::tagged_obj::deref_tagged::<JsPromise>(bits) };
    let Some(promise) = promise else { return };

    trace_value_bits(promise.value, tracer);

    for reaction in &promise.reactions {
        trace_value_bits(reaction.on_fulfill, tracer);
        trace_value_bits(reaction.on_reject, tracer);
        // NOTE: reaction.result_promise is *mut JsPromise (raw pointer),
        // not a NaN-boxed u64. Cannot trace it as a NodeId yet.
    }
}

/// Trace the message, raw_message, and stack fields of a `JsError`.
fn trace_error(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Error`, created via `TaggedObj::boxed`.
    let error = unsafe { crate::tagged_obj::deref_tagged::<JsError>(bits) };
    let Some(error) = error else { return };

    trace_value_bits(error.message, tracer);
    trace_value_bits(error.raw_message, tracer);
    trace_value_bits(error.stack, tracer);
}

/// Trace the value and done fields of an `IteratorResult`.
fn trace_iter_result(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::IterResult`, created via `TaggedObj::boxed`.
    let result = unsafe { crate::tagged_obj::deref_tagged::<IteratorResult>(bits) };
    let Some(result) = result else { return };

    trace_value_bits(result.value, tracer);
    trace_value_bits(result.done, tracer);
}

/// Trace the captured environment of a closure for object-typed slots.
fn trace_closure_env(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Closure`, created via `TaggedObj::boxed`.
    let closure = unsafe { crate::tagged_obj::deref_tagged::<ClosureData>(bits) };
    let Some(closure) = closure else { return };

    let env_val = JsValue::from_raw_bits(closure.env);
    if !env_val.is_object() {
        return;
    }

    // The environment is a raw pointer to an `Environment` struct allocated
    // via `Box::into_raw`. Walk its slots for object references.
    let env_ptr = env_val.as_object();
    let Some(ptr) = env_ptr else { return };
    if ptr.is_null() {
        return;
    }

    // SAFETY: The closure's env field was created by `__esc_rt_env_create`
    // which allocates an `Environment` via `Box::into_raw`.
    let env = unsafe { &*(ptr as *const Environment) };
    for &slot_bits in &env.slots {
        let slot_val = JsValue::from_raw_bits(slot_bits);
        if slot_val.is_object() {
            tracer(NodeId(slot_bits));
        }
    }
}

/// Trace all keys and values of a `JsMap`.
fn trace_map(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Map`, created via `TaggedObj::boxed`.
    let map = unsafe { crate::tagged_obj::deref_tagged::<JsMap>(bits) };
    let Some(map) = map else { return };

    for (key, val) in &map.entries {
        if key.is_object() {
            tracer(NodeId(key.raw_bits()));
        }
        if val.is_object() {
            tracer(NodeId(val.raw_bits()));
        }
    }
}

/// Trace all values of a `JsSet`.
fn trace_set(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Set`, created via `TaggedObj::boxed`.
    let set = unsafe { crate::tagged_obj::deref_tagged::<JsSet>(bits) };
    let Some(set) = set else { return };

    for val in &set.values {
        if val.is_object() {
            tracer(NodeId(val.raw_bits()));
        }
    }
}

/// Trace the target and handler of a `ProxyObject`.
fn trace_proxy(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Proxy`, created via `TaggedObj::boxed`.
    let proxy = unsafe { crate::tagged_obj::deref_tagged::<ProxyObject>(bits) };
    let Some(proxy) = proxy else { return };

    trace_value_bits(proxy.target, tracer);
    trace_value_bits(proxy.handler, tracer);
}

/// Trace the context value of a `NativeFuncData`.
fn trace_native_func(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::NativeFunc`, created via `TaggedObj::boxed`.
    let nf = unsafe { crate::tagged_obj::deref_tagged::<NativeFuncData>(bits) };
    let Some(nf) = nf else { return };

    trace_value_bits(nf.context, tracer);
}

/// Trace the state object reference of a generator.
///
/// In the state machine protocol, generator data is held in a state object.
/// This function is kept for backward compatibility but does nothing since
/// generators are now traced via the `UnifiedObject` path.
fn trace_generator(_bits: u64, _tracer: &mut dyn FnMut(NodeId)) {
    // Generator objects are now always UnifiedObject with InternalKind::Generator.
    // Their state_obj reference is traced via trace_unified -> InternalData::Generator.
    // This function is a no-op but retained so callers that reference it still compile.
}

/// Trace the contained value of a `JsBox` if it is an object.
fn trace_jsbox(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::JsBox`, created via `TaggedObj::boxed`.
    let jsbox = unsafe { crate::tagged_obj::deref_tagged::<JsBox>(bits) };
    let Some(jsbox) = jsbox else { return };

    trace_value_bits(jsbox.value, tracer);
}

/// Trace all references inside a [`UnifiedObject`].
///
/// Walks named property slots, indexed elements, and internal data for
/// object-typed references.
fn trace_unified(bits: u64, tracer: &mut dyn FnMut(NodeId)) {
    // SAFETY: The caller has verified that `bits` is a valid NaN-boxed object
    // pointer with tag `ObjTag::Unified`, created via `TaggedObj::boxed`.
    let uobj = unsafe { crate::tagged_obj::deref_tagged::<UnifiedObject>(bits) };
    let Some(uobj) = uobj else { return };

    // Trace named property slots.
    for slot in &uobj.slots {
        if slot.is_object() {
            tracer(NodeId(slot.raw_bits()));
        }
    }
    // Trace indexed elements.
    match &uobj.elements {
        ElementsStorage::Dense(elems) => {
            for elem in elems {
                if elem.is_object() {
                    tracer(NodeId(elem.raw_bits()));
                }
            }
        }
        ElementsStorage::Holey(elems) => {
            for val in elems.iter().flatten() {
                if val.is_object() {
                    tracer(NodeId(val.raw_bits()));
                }
            }
        }
        ElementsStorage::Dictionary(map) => {
            for val in map.values() {
                if val.is_object() {
                    tracer(NodeId(val.raw_bits()));
                }
            }
        }
        ElementsStorage::None => {}
    }
    // Trace internal data.
    if let Some(ref data) = uobj.internal {
        trace_internal_data(data, tracer);
    }
}

/// Trace all object-typed references inside [`InternalData`].
fn trace_internal_data(data: &InternalData, tracer: &mut dyn FnMut(NodeId)) {
    match data {
        InternalData::Function { env, name, .. } => {
            trace_value_bits(*env, tracer);
            trace_value_bits(*name, tracer);
        }
        InternalData::Error {
            message,
            raw_message,
            stack,
            ..
        } => {
            trace_value_bits(*message, tracer);
            trace_value_bits(*raw_message, tracer);
            trace_value_bits(*stack, tracer);
        }
        InternalData::Proxy {
            target, handler, ..
        } => {
            trace_value_bits(*target, tracer);
            trace_value_bits(*handler, tracer);
        }
        InternalData::Promise { inner } => {
            trace_value_bits(inner.value, tracer);
            for reaction in &inner.reactions {
                if reaction.on_fulfill != 0 {
                    trace_value_bits(reaction.on_fulfill, tracer);
                }
                if reaction.on_reject != 0 {
                    trace_value_bits(reaction.on_reject, tracer);
                }
            }
        }
        InternalData::IteratorState { inner } => {
            trace_value_bits(inner.target, tracer);
        }
        InternalData::IterResult { value, done } => {
            trace_value_bits(*value, tracer);
            trace_value_bits(*done, tracer);
        }
        InternalData::Generator { state_obj, .. } => {
            trace_value_bits(*state_obj, tracer);
        }
        InternalData::Map { entries } => {
            for (key, val) in entries {
                if key.is_object() {
                    tracer(NodeId(key.raw_bits()));
                }
                if val.is_object() {
                    tracer(NodeId(val.raw_bits()));
                }
            }
        }
        InternalData::Set { values } => {
            for val in values {
                if val.is_object() {
                    tracer(NodeId(val.raw_bits()));
                }
            }
        }
        InternalData::NativeFunc { context, .. } => {
            trace_value_bits(*context, tracer);
        }
        InternalData::WeakRef { target } => {
            // WeakRef targets are traced as strong refs for now;
            // will become weak when GC matures.
            trace_value_bits(*target, tracer);
        }
        InternalData::AsyncGenerator {
            generator, queue, ..
        } => {
            trace_value_bits(*generator, tracer);
            for req in queue {
                trace_value_bits(req.promise_bits, tracer);
                trace_value_bits(req.value, tracer);
            }
        }
        InternalData::AsyncIterator { inner } => {
            trace_value_bits(inner.source, tracer);
            if inner.callback != 0 {
                trace_value_bits(inner.callback, tracer);
            }
            if inner.inner_source != 0 {
                trace_value_bits(inner.inner_source, tracer);
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

// =========================================================================
// Thread-local cycle collector
// =========================================================================

thread_local! {
    static CYCLE_COLLECTOR: RefCell<CycleCollector<TraceableObject>> =
        RefCell::new(CycleCollector::new());
}

/// Counter for periodic automatic collection.
static DECREMENT_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How often to trigger automatic collection (every N decrements).
const AUTO_COLLECT_INTERVAL: u64 = 256;

/// Register an object with the cycle collector.
///
/// The object is identified by its NaN-boxed `bits` and its current
/// reference count `rc`. Only objects (not primitives) should be registered.
pub fn register_traceable(bits: u64, rc: u32) {
    let id = NodeId(bits);
    let obj = TraceableObject::new(bits);
    CYCLE_COLLECTOR.with(|cc| {
        if let Ok(mut cc) = cc.try_borrow_mut() {
            cc.register(id, rc, obj);
        }
    });
}

/// Called when an object's reference count decreases but does not reach zero.
///
/// Adds the object to the suspect list and periodically triggers automatic
/// collection every 256 decrements.
pub fn on_decrement(bits: u64) {
    let id = NodeId(bits);
    CYCLE_COLLECTOR.with(|cc| {
        if let Ok(mut cc) = cc.try_borrow_mut() {
            cc.add_suspect(id);
        }
    });

    // Periodic auto-collection
    let count = DECREMENT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if count.is_multiple_of(AUTO_COLLECT_INTERVAL) {
        let _ = force_collect();
    }
}

/// Called when an object's reference count increases.
///
/// Informs the collector that the object is definitely still alive,
/// painting it black.
pub fn on_increment(bits: u64) {
    let id = NodeId(bits);
    CYCLE_COLLECTOR.with(|cc| {
        if let Ok(mut cc) = cc.try_borrow_mut() {
            cc.increment(id);
        }
    });
}

/// Force a collection cycle. Returns the list of freed [`NodeId`]s.
///
/// After the collector identifies garbage nodes, each one is deallocated.
/// Heap objects (with [`crate::heap_obj::HEAP_BIT`]) are freed via
/// [`crate::heap_obj::dealloc_by_tag`]; zone/legacy objects are freed via
/// [`free_tagged_by_tag`]. Returns an empty vector if the collector is
/// currently borrowed (reentrant call) or if no cycles are detected.
pub fn force_collect() -> Vec<NodeId> {
    CYCLE_COLLECTOR.with(|cc| match cc.try_borrow_mut() {
        Ok(mut cc) => {
            let garbage = cc.collect().unwrap_or_default();
            // Dealloc all cycle-collected objects. The cycle collector already
            // traced the full graph, so we skip release_children here.
            for &id in &garbage {
                let bits = id.0;
                if crate::heap_obj::is_heap_object(bits) {
                    // SAFETY: The cycle collector determined these objects are
                    // unreachable garbage. The bits were allocated by alloc_heap_obj.
                    unsafe {
                        crate::heap_obj::dealloc_by_tag(bits);
                    }
                } else {
                    // Legacy/zone objects allocated via TaggedObj::boxed.
                    // SAFETY: The cycle collector determined these are garbage.
                    unsafe {
                        free_tagged_by_tag(bits);
                    }
                }
            }
            garbage
        }
        Err(_) => Vec::new(), // Reentrant call, skip
    })
}

/// Free a non-heap tagged object by reading its tag and dispatching to
/// the correct typed [`crate::tagged_obj::free_tagged`].
///
/// # Safety
///
/// The caller must ensure that `bits` was returned by [`crate::tagged_obj::TaggedObj::boxed`]
/// and the object has not already been freed.
unsafe fn free_tagged_by_tag(bits: u64) {
    use crate::jsbox::JsBox;
    use crate::regexp_bridge::JsRegExpData;
    use crate::rt_api::{JsWeakRef, NativeFuncData as NfData};
    use crate::tagged_obj::free_tagged;

    let Some(tag) = read_obj_tag(bits) else {
        return;
    };
    // SAFETY: The caller guarantees bits was allocated by TaggedObj::boxed
    // with the type matching the stored tag.
    unsafe {
        match tag {
            t if t == ObjTag::Plain as u8 => free_tagged::<JsObject>(bits),
            t if t == ObjTag::Array as u8 => free_tagged::<JsArray>(bits),
            t if t == ObjTag::Closure as u8 => free_tagged::<ClosureData>(bits),
            t if t == ObjTag::Function as u8 => free_tagged::<JsFunction>(bits),
            t if t == ObjTag::Iterator as u8 => free_tagged::<JsIterator>(bits),
            t if t == ObjTag::Promise as u8 => free_tagged::<JsPromise>(bits),
            t if t == ObjTag::Error as u8 => free_tagged::<JsError>(bits),
            t if t == ObjTag::IterResult as u8 => free_tagged::<IteratorResult>(bits),
            t if t == ObjTag::Map as u8 => free_tagged::<JsMap>(bits),
            t if t == ObjTag::Set as u8 => free_tagged::<JsSet>(bits),
            t if t == ObjTag::Proxy as u8 => free_tagged::<ProxyObject>(bits),
            t if t == ObjTag::WeakRef as u8 => free_tagged::<JsWeakRef>(bits),
            t if t == ObjTag::NativeFunc as u8 => free_tagged::<NfData>(bits),
            t if t == ObjTag::Generator as u8 => {
                // Legacy tag — generators now use ObjTag::Unified.
                // Free as raw bytes.
                free_tagged::<u64>(bits);
            }
            t if t == ObjTag::RegExp as u8 => free_tagged::<JsRegExpData>(bits),
            t if t == ObjTag::JsBox as u8 => free_tagged::<JsBox>(bits),
            t if t == ObjTag::Unified as u8 => free_tagged::<UnifiedObject>(bits),
            _ => free_tagged::<u64>(bits),
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::JsArray;
    use crate::function::{FunctionKind, JsFunction};
    use crate::heap_obj::{HEAP_BIT, heap_obj_tag_kind};
    use crate::iterator::{IteratorResult, JsIterator};
    use crate::object::{JsObject, ObjectHeader, PropertyStorage};
    use crate::promise::{JsPromise, PromiseState, Reaction};
    use crate::proxy::ProxyObject;
    use crate::regexp_bridge::JsRegExpData;
    use crate::rt_api::{ClosureData, JsError, JsMap, JsSet, JsWeakRef, NativeFuncData};
    use crate::tagged_obj::{ObjTag, TaggedObj};
    use cycles::NodeId;
    use nanbox::JsValue;
    use shapes::ShapeTable;

    /// Helper: create a TraceableObject and collect all traced children.
    fn collect_trace(bits: u64) -> Vec<NodeId> {
        let obj = TraceableObject::new(bits);
        let mut children = Vec::new();
        obj.trace(&mut |id| children.push(id));
        children
    }

    /// Helper: create a fake NaN-boxed object pointer from a raw address.
    /// This creates a tiny heap allocation with the given tag so
    /// `JsValue::is_object()` returns true.
    fn make_fake_obj(tag: ObjTag) -> u64 {
        TaggedObj::boxed(tag, 0u64)
    }

    // -----------------------------------------------------------------
    // 1. Plain object with inline storage containing object refs
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_plain_with_object_properties() {
        let child1 = make_fake_obj(ObjTag::Plain);
        let child2 = make_fake_obj(ObjTag::Array);

        let obj = JsObject {
            header: ObjectHeader {
                flags: 0,
                alloc_class: 3,
            },
            shape_id: ShapeTable::EMPTY_SHAPE,
            storage: PropertyStorage::Inline(vec![
                JsValue::int(42),
                JsValue::from_raw_bits(child1),
                JsValue::from_raw_bits(child2),
            ]),
            prototype: None,
        };
        let bits = TaggedObj::boxed(ObjTag::Plain, obj);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&NodeId(child1)));
        assert!(children.contains(&NodeId(child2)));
    }

    // -----------------------------------------------------------------
    // 2. Plain object with dictionary storage
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_plain_with_dictionary_storage() {
        let child = make_fake_obj(ObjTag::Plain);

        let obj = JsObject {
            header: ObjectHeader {
                flags: 0,
                alloc_class: 3,
            },
            shape_id: ShapeTable::EMPTY_SHAPE,
            storage: PropertyStorage::Dictionary(vec![
                ("x".to_string(), JsValue::int(1)),
                ("y".to_string(), JsValue::from_raw_bits(child)),
            ]),
            prototype: None,
        };
        let bits = TaggedObj::boxed(ObjTag::Plain, obj);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(child));
    }

    // -----------------------------------------------------------------
    // 3. Plain object with only primitive properties
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_plain_no_object_properties() {
        let obj = JsObject {
            header: ObjectHeader {
                flags: 0,
                alloc_class: 3,
            },
            shape_id: ShapeTable::EMPTY_SHAPE,
            storage: PropertyStorage::Inline(vec![
                JsValue::int(1),
                JsValue::bool(true),
                JsValue::undefined(),
            ]),
            prototype: None,
        };
        let bits = TaggedObj::boxed(ObjTag::Plain, obj);

        let children = collect_trace(bits);
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 4. Array with object elements
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_array_with_object_elements() {
        let child = make_fake_obj(ObjTag::Plain);

        let arr = JsArray {
            elements: vec![
                JsValue::int(10),
                JsValue::from_raw_bits(child),
                JsValue::null(),
            ],
            length: 3,
        };
        let bits = TaggedObj::boxed(ObjTag::Array, arr);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(child));
    }

    // -----------------------------------------------------------------
    // 5. Empty array
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_array_empty() {
        let arr = JsArray {
            elements: vec![],
            length: 0,
        };
        let bits = TaggedObj::boxed(ObjTag::Array, arr);

        let children = collect_trace(bits);
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 6. Function with env containing objects
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_function_with_env() {
        let child = make_fake_obj(ObjTag::Plain);

        let func = JsFunction::new("test".to_string(), FunctionKind::Normal, 0)
            .with_env(vec![JsValue::int(1), JsValue::from_raw_bits(child)]);
        let bits = TaggedObj::boxed(ObjTag::Function, func);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(child));
    }

    // -----------------------------------------------------------------
    // 7. Function with no env
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_function_no_env() {
        let func = JsFunction::new("test".to_string(), FunctionKind::Arrow, 0);
        let bits = TaggedObj::boxed(ObjTag::Function, func);

        let children = collect_trace(bits);
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 8. Iterator with object target
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_iterator_target() {
        let target = make_fake_obj(ObjTag::Array);

        let iter = JsIterator::new_array(target);
        let bits = TaggedObj::boxed(ObjTag::Iterator, iter);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(target));
    }

    // -----------------------------------------------------------------
    // 9. Promise with object value and reactions
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_promise_value_and_reactions() {
        let handler = make_fake_obj(ObjTag::Function);

        let mut promise = JsPromise::new();
        promise.state = PromiseState::Fulfilled;
        promise.value = make_fake_obj(ObjTag::Plain);
        // Manually add a reaction without triggering microtask scheduling
        promise.reactions.push(Reaction {
            on_fulfill: handler,
            on_reject: JsValue::undefined().raw_bits(),
            result_promise: std::ptr::null_mut(),
        });
        let bits = TaggedObj::boxed(ObjTag::Promise, promise);

        let children = collect_trace(bits);
        // value (object) + on_fulfill (object) = 2
        assert_eq!(children.len(), 2);
    }

    // -----------------------------------------------------------------
    // 10. Error with object message/stack
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_error_fields() {
        let msg = make_fake_obj(ObjTag::Plain);
        let raw_msg = make_fake_obj(ObjTag::Plain);
        let stack = make_fake_obj(ObjTag::Plain);

        let error = JsError {
            error_tag: 1,
            message: msg,
            raw_message: raw_msg,
            stack,
        };
        let bits = TaggedObj::boxed(ObjTag::Error, error);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 3);
        assert!(children.contains(&NodeId(msg)));
        assert!(children.contains(&NodeId(raw_msg)));
        assert!(children.contains(&NodeId(stack)));
    }

    // -----------------------------------------------------------------
    // 11. IteratorResult with object value
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_iter_result_value() {
        let value = make_fake_obj(ObjTag::Plain);

        let result = IteratorResult {
            value,
            done: JsValue::bool(false).raw_bits(),
        };
        let bits = TaggedObj::boxed(ObjTag::IterResult, result);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(value));
    }

    // -----------------------------------------------------------------
    // 12. Map with object keys and values
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_map_entries() {
        let key_obj = make_fake_obj(ObjTag::Plain);
        let val_obj = make_fake_obj(ObjTag::Plain);

        let map = JsMap {
            entries: vec![
                (JsValue::int(1), JsValue::int(2)),
                (
                    JsValue::from_raw_bits(key_obj),
                    JsValue::from_raw_bits(val_obj),
                ),
            ],
        };
        let bits = TaggedObj::boxed(ObjTag::Map, map);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&NodeId(key_obj)));
        assert!(children.contains(&NodeId(val_obj)));
    }

    // -----------------------------------------------------------------
    // 13. Set with object values
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_set_values() {
        let obj1 = make_fake_obj(ObjTag::Plain);
        let obj2 = make_fake_obj(ObjTag::Plain);

        let set = JsSet {
            values: vec![
                JsValue::int(42),
                JsValue::from_raw_bits(obj1),
                JsValue::from_raw_bits(obj2),
            ],
        };
        let bits = TaggedObj::boxed(ObjTag::Set, set);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&NodeId(obj1)));
        assert!(children.contains(&NodeId(obj2)));
    }

    // -----------------------------------------------------------------
    // 14. Proxy with object target and handler
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_proxy_target_handler() {
        let target = make_fake_obj(ObjTag::Plain);
        let handler = make_fake_obj(ObjTag::Plain);

        let proxy = ProxyObject::new(target, handler);
        let bits = TaggedObj::boxed(ObjTag::Proxy, proxy);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&NodeId(target)));
        assert!(children.contains(&NodeId(handler)));
    }

    // -----------------------------------------------------------------
    // 15. NativeFunc with object context
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_native_func_context() {
        let ctx = make_fake_obj(ObjTag::Plain);

        fn dummy_native(_ctx: u64) -> u64 {
            0
        }

        let nf = NativeFuncData {
            func: dummy_native,
            context: ctx,
        };
        let bits = TaggedObj::boxed(ObjTag::NativeFunc, nf);

        let children = collect_trace(bits);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], NodeId(ctx));
    }

    // -----------------------------------------------------------------
    // 16. Generator with object yields
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_generator_state_obj() {
        let state_obj = make_fake_obj(ObjTag::Unified);

        let uni =
            crate::internal_data::UnifiedObject::generator(ShapeTable::EMPTY_SHAPE, state_obj, 0);
        let bits = TaggedObj::boxed(ObjTag::Unified, uni);

        let children = collect_trace(bits);
        assert!(
            children.contains(&NodeId(state_obj)),
            "generator should trace its state object"
        );
    }

    // -----------------------------------------------------------------
    // 17. RegExp is a leaf (no children)
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_regexp_is_leaf() {
        let data = JsRegExpData::new("abc", "g");
        let Ok(data) = data else { return };
        let uni = crate::internal_data::UnifiedObject::regexp(
            shapes::ShapeTable::EMPTY_SHAPE,
            Box::new(data),
        );
        let bits = TaggedObj::boxed(ObjTag::Unified, uni);

        let children = collect_trace(bits);
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 18. WeakRef target is NOT traced
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_weakref_no_trace() {
        let target = make_fake_obj(ObjTag::Plain);

        let wr = JsWeakRef { target };
        let bits = TaggedObj::boxed(ObjTag::WeakRef, wr);

        let children = collect_trace(bits);
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 19. Primitives produce no children
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_primitives_noop() {
        let children = collect_trace(JsValue::number(2.5).raw_bits());
        assert!(children.is_empty());

        let children = collect_trace(JsValue::undefined().raw_bits());
        assert!(children.is_empty());

        let children = collect_trace(JsValue::null().raw_bits());
        assert!(children.is_empty());

        let children = collect_trace(JsValue::bool(true).raw_bits());
        assert!(children.is_empty());

        let children = collect_trace(JsValue::int(0).raw_bits());
        assert!(children.is_empty());
    }

    // -----------------------------------------------------------------
    // 20. HEAP_BIT stripping
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_heap_bit_stripping() {
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Plain as u8 | HEAP_BIT),
            ObjTag::Plain as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Array as u8 | HEAP_BIT),
            ObjTag::Array as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Function as u8 | HEAP_BIT),
            ObjTag::Function as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Closure as u8 | HEAP_BIT),
            ObjTag::Closure as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Map as u8 | HEAP_BIT),
            ObjTag::Map as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::Generator as u8 | HEAP_BIT),
            ObjTag::Generator as u8
        );
        assert_eq!(
            heap_obj_tag_kind(ObjTag::WeakRef as u8 | HEAP_BIT),
            ObjTag::WeakRef as u8
        );

        // Non-heap tags pass through unchanged
        assert_eq!(heap_obj_tag_kind(ObjTag::Plain as u8), ObjTag::Plain as u8);
        assert_eq!(heap_obj_tag_kind(ObjTag::Array as u8), ObjTag::Array as u8);
    }

    // -----------------------------------------------------------------
    // 21. Closure with empty env (regression for original 3-tag logic)
    // -----------------------------------------------------------------
    #[test]
    fn test_trace_closure_empty_env() {
        let closure = ClosureData {
            func_idx: 0,
            env: 0,
        };
        let bits = TaggedObj::boxed(ObjTag::Closure, closure);
        let children = collect_trace(bits);
        assert!(children.is_empty());
    }
}
