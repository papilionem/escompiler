//! Inline cache infrastructure for fast property access.
//!
//! Provides monomorphic and polymorphic inline caches that speed up repeated
//! property accesses on objects with the same shape. When a shape matches a
//! cached entry, the slot offset is used directly instead of performing a full
//! shape-table lookup. Falls back to the slow path (`__esc_rt_get_prop` /
//! `__esc_rt_set_prop`) on a cache miss.
//!
//! The IC table is thread-local and safe for single-threaded JS execution.

use std::cell::{Cell, RefCell};

use crate::rt_api::{__esc_rt_get_prop, __esc_rt_set_prop, INTERNER, SHAPES};
use crate::tagged_obj::{ObjTag, deref_tagged, read_obj_tag};

// =========================================================================
// IC entry types
// =========================================================================

/// A monomorphic inline cache entry for a single shape.
#[derive(Debug)]
struct MonoIC {
    /// The shape ID this cache entry targets.
    shape_id: u32,
    /// The slot offset within the object's inline storage.
    slot_offset: u32,
    /// The prototype epoch at the time this cache was filled.
    epoch: u32,
    /// Whether this cache entry has been filled with valid data.
    initialized: bool,
}

/// A polymorphic inline cache entry supporting up to 4 shapes.
#[derive(Debug)]
struct PolyIC {
    /// Cached (shape_id, slot_offset) pairs. Up to 4 entries.
    entries: [(u32, u32); 4],
    /// Number of valid entries in `entries`.
    count: u8,
    /// The prototype epoch at the time this cache was last updated.
    epoch: u32,
    /// When true, the cache has seen too many shapes and always takes the slow path.
    megamorphic: bool,
}

/// An inline cache site, either monomorphic or polymorphic.
#[derive(Debug)]
enum ICSite {
    /// A monomorphic cache targeting a single shape.
    Mono(MonoIC),
    /// A polymorphic cache targeting up to 4 shapes.
    Poly(PolyIC),
}

// =========================================================================
// Thread-local IC table and prototype epoch
// =========================================================================

thread_local! {
    /// The per-thread IC site table. Each compiled property access site
    /// has a unique index into this table.
    static IC_TABLE: RefCell<Vec<ICSite>> = const { RefCell::new(Vec::new()) };

    /// The global prototype epoch counter. Incremented whenever any object's
    /// prototype is modified, invalidating all IC entries that cache
    /// prototype-chain lookups.
    static PROTOTYPE_EPOCH: Cell<u32> = const { Cell::new(0) };
}

// =========================================================================
// IC registration
// =========================================================================

/// Register a new IC site in the table and return its index.
///
/// Called during module initialization to allocate IC slots for each
/// property access site in the compiled code.
pub fn register_ic_site() -> u32 {
    IC_TABLE.with(|table| {
        let mut table = table.borrow_mut();
        let id = table.len() as u32;
        table.push(ICSite::Mono(MonoIC {
            shape_id: 0,
            slot_offset: 0,
            epoch: 0,
            initialized: false,
        }));
        id
    })
}

// =========================================================================
// Prototype epoch management
// =========================================================================

/// Bump the global prototype epoch counter.
///
/// Called whenever any object's prototype is modified (e.g. via
/// `Object.setPrototypeOf`), which invalidates cached prototype-chain
/// lookups in all IC entries.
pub fn bump_prototype_epoch() {
    PROTOTYPE_EPOCH.with(|epoch| {
        epoch.set(epoch.get().wrapping_add(1));
    });
}

/// Returns the current prototype epoch value.
pub fn current_prototype_epoch() -> u32 {
    PROTOTYPE_EPOCH.with(|epoch| epoch.get())
}

// =========================================================================
// IC get/set — extern "C" entry points
// =========================================================================

/// Inline-cached property get.
///
/// Fast path: if the IC is initialized and the object's shape matches the
/// cached shape, reads the value directly from the object's slot storage.
/// Slow path: delegates to `__esc_rt_get_prop` and caches the shape + offset
/// for future hits.
///
/// # Safety
///
/// The `ic_id` must be a valid IC site index allocated by `register_ic_site`
/// or a compile-time constant that will be lazily registered.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_ic_get_prop(obj: u64, key: u64, ic_id: u32) -> u64 {
    let current_epoch = current_prototype_epoch();

    // Try fast path via IC table
    let fast_result = IC_TABLE.with(|table| {
        let table = table.borrow();
        // Ensure the IC site exists
        let site = table.get(ic_id as usize)?;

        match site {
            ICSite::Mono(mono) => {
                if !mono.initialized || mono.epoch != current_epoch {
                    return None;
                }
                try_fast_get(obj, mono.shape_id, mono.slot_offset)
            }
            ICSite::Poly(poly) => {
                if poly.megamorphic || poly.epoch != current_epoch {
                    return None;
                }
                // Linear scan of poly entries
                let obj_shape = get_object_shape_id(obj)?;
                for i in 0..poly.count as usize {
                    let (cached_shape, cached_offset) = poly.entries[i];
                    if cached_shape == obj_shape {
                        return try_fast_get_with_offset(obj, cached_offset);
                    }
                }
                None
            }
        }
    });

    if let Some(result) = fast_result {
        return result;
    }

    // Slow path: call the full property access
    let result = __esc_rt_get_prop(obj, key);

    // Cache the shape + offset for next time
    cache_get_miss(obj, key, ic_id, current_epoch);

    result
}

/// Inline-cached property set.
///
/// Fast path: if the IC is initialized and the object's shape matches the
/// cached shape, writes the value directly to the object's slot storage.
/// Slow path: delegates to `__esc_rt_set_prop` and caches the shape + offset
/// for future hits.
///
/// # Safety
///
/// The `ic_id` must be a valid IC site index allocated by `register_ic_site`
/// or a compile-time constant that will be lazily registered.
#[unsafe(no_mangle)]
pub extern "C" fn __esc_rt_ic_set_prop(obj: u64, key: u64, val: u64, ic_id: u32) {
    // Always take the slow path for set — caching set is more complex
    // because shape transitions can occur. We still cache the shape
    // for the get path.
    __esc_rt_set_prop(obj, key, val);

    // Update cache metadata
    cache_set_miss(obj, key, ic_id);
}

// =========================================================================
// Fast-path helpers
// =========================================================================

/// Get the shape ID from a NaN-boxed object pointer.
///
/// Returns `None` if the value is not a unified object.
fn get_object_shape_id(obj: u64) -> Option<u32> {
    let tag = read_obj_tag(obj);
    if tag != Some(ObjTag::Unified as u8) {
        return None;
    }
    let uni = unsafe {
        // SAFETY: tag check confirms this is a unified object.
        deref_tagged::<crate::internal_data::UnifiedObject>(obj)
    };
    uni.map(|u| u.shape_id.0)
}

/// Try the fast path: check if the object's shape matches and read the slot directly.
fn try_fast_get(obj: u64, cached_shape_id: u32, cached_offset: u32) -> Option<u64> {
    let obj_shape = get_object_shape_id(obj)?;
    if obj_shape != cached_shape_id {
        return None;
    }
    try_fast_get_with_offset(obj, cached_offset)
}

/// Read a slot value by offset from a unified object.
fn try_fast_get_with_offset(obj: u64, offset: u32) -> Option<u64> {
    let uni = unsafe {
        // SAFETY: we only call this after verifying the tag is ObjTag::Unified.
        deref_tagged::<crate::internal_data::UnifiedObject>(obj)
    };
    let u = uni?;
    u.slots.get(offset as usize).map(|v| v.raw_bits())
}

// =========================================================================
// Cache miss handlers
// =========================================================================

/// Handle a get-prop cache miss: look up the shape and offset, then update the IC.
fn cache_get_miss(obj: u64, key: u64, ic_id: u32, epoch: u32) {
    let Some(obj_shape_id) = get_object_shape_id(obj) else {
        return;
    };

    // Look up the slot offset via the shape table
    let offset = lookup_slot_offset(obj_shape_id, key);
    let Some(slot_offset) = offset else {
        return;
    };

    IC_TABLE.with(|table| {
        let mut table = table.borrow_mut();
        // Ensure the IC site exists, growing the table if needed
        while table.len() <= ic_id as usize {
            table.push(ICSite::Mono(MonoIC {
                shape_id: 0,
                slot_offset: 0,
                epoch: 0,
                initialized: false,
            }));
        }

        let site = &mut table[ic_id as usize];
        match site {
            ICSite::Mono(mono) => {
                if !mono.initialized {
                    // First miss: fill the mono cache
                    mono.shape_id = obj_shape_id;
                    mono.slot_offset = slot_offset;
                    mono.epoch = epoch;
                    mono.initialized = true;
                } else if mono.shape_id != obj_shape_id {
                    // Second shape seen: promote to poly
                    let old_shape = mono.shape_id;
                    let old_offset = mono.slot_offset;
                    let mut entries = [(0u32, 0u32); 4];
                    entries[0] = (old_shape, old_offset);
                    entries[1] = (obj_shape_id, slot_offset);
                    *site = ICSite::Poly(PolyIC {
                        entries,
                        count: 2,
                        epoch,
                        megamorphic: false,
                    });
                }
                // Same shape hit: no update needed
            }
            ICSite::Poly(poly) => {
                if poly.megamorphic {
                    return;
                }
                // Check if this shape is already cached
                for i in 0..poly.count as usize {
                    if poly.entries[i].0 == obj_shape_id {
                        return;
                    }
                }
                if poly.count < 4 {
                    // Add new entry
                    poly.entries[poly.count as usize] = (obj_shape_id, slot_offset);
                    poly.count += 1;
                    poly.epoch = epoch;
                } else {
                    // Too many shapes: go megamorphic
                    poly.megamorphic = true;
                }
            }
        }
    });
}

/// Handle a set-prop cache miss: update cache metadata.
fn cache_set_miss(obj: u64, key: u64, ic_id: u32) {
    // For set, we just update the cache for future get hits.
    // The shape might have changed during set (new property added),
    // so we re-read the shape after the set.
    let epoch = current_prototype_epoch();
    cache_get_miss(obj, key, ic_id, epoch);
}

/// Look up the slot offset for a property on a given shape.
fn lookup_slot_offset(shape_id: u32, key: u64) -> Option<u32> {
    let key_str = crate::rt_api::key_to_string(key);

    SHAPES.with(|shapes| {
        INTERNER.with(|interner| {
            let shapes = shapes.borrow();
            let interner = interner.borrow();
            let atom = interner.intern(&key_str);
            let desc = shapes.lookup(shapes::ShapeId(shape_id), atom)?;
            Some(desc.offset)
        })
    })
}
