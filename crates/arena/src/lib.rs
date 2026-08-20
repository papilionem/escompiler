//! Zone allocator built on bumpalo. Provides bump-pointer allocation with bulk free.

use std::fmt;

use bumpalo::Bump;

/// Identifies a zone within the zone arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ZoneId(pub u32);

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "zone{}", self.0)
    }
}

/// A single zone backed by a bump allocator.
///
/// Objects allocated within a zone share its lifetime and are freed together
/// when the zone is reset or destroyed.
pub struct Zone {
    bump: Bump,
    id: ZoneId,
    object_count: usize,
    epoch: u64,
    bytes_allocated: usize,
}

impl Zone {
    /// Creates a new zone with the given id.
    pub fn new(id: ZoneId) -> Self {
        Self {
            bump: Bump::new(),
            id,
            object_count: 0,
            epoch: 0,
            bytes_allocated: 0,
        }
    }

    /// Creates a new zone with pre-allocated capacity in bytes.
    pub fn with_capacity(id: ZoneId, capacity: usize) -> Self {
        Self {
            bump: Bump::with_capacity(capacity),
            id,
            object_count: 0,
            epoch: 0,
            bytes_allocated: 0,
        }
    }

    /// Returns this zone's identifier.
    pub fn id(&self) -> ZoneId {
        self.id
    }

    /// Returns the number of objects allocated in this zone.
    pub fn object_count(&self) -> usize {
        self.object_count
    }

    /// Returns the current epoch. The epoch increments on each `reset()`.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Returns the approximate number of bytes allocated in this zone.
    pub fn bytes_allocated(&self) -> usize {
        self.bytes_allocated
    }

    /// Allocates a value within this zone, returning a reference tied to the
    /// zone's lifetime.
    pub fn alloc<T>(&mut self, val: T) -> &T {
        self.object_count += 1;
        self.bytes_allocated += std::mem::size_of::<T>();
        self.bump.alloc(val)
    }

    /// Allocates a copy of a slice within this zone.
    pub fn alloc_slice<T: Copy>(&mut self, slice: &[T]) -> &[T] {
        self.object_count += 1;
        self.bytes_allocated += std::mem::size_of_val(slice);
        self.bump.alloc_slice_copy(slice)
    }

    /// Allocates a copy of a string within this zone.
    pub fn alloc_str(&mut self, s: &str) -> &str {
        self.object_count += 1;
        self.bytes_allocated += s.len();
        self.bump.alloc_str(s)
    }

    /// Resets the zone, deallocating all objects. The underlying memory may be
    /// reused for future allocations. Increments the epoch.
    pub fn reset(&mut self) {
        self.bump.reset();
        self.object_count = 0;
        self.bytes_allocated = 0;
        self.epoch += 1;
    }
}

/// Manages multiple zones, providing create/get/destroy operations.
pub struct ZoneArena {
    zones: Vec<Option<Zone>>,
    next_id: u32,
}

impl ZoneArena {
    /// Creates a new, empty zone arena.
    pub fn new() -> Self {
        Self {
            zones: Vec::new(),
            next_id: 0,
        }
    }

    /// Creates a new zone and returns its identifier.
    pub fn create_zone(&mut self) -> ZoneId {
        let id = ZoneId(self.next_id);
        self.next_id += 1;
        let zone = Zone::new(id);
        self.zones.push(Some(zone));
        id
    }

    /// Returns a reference to the zone with the given id, if it exists and
    /// has not been destroyed.
    pub fn get(&self, id: ZoneId) -> Option<&Zone> {
        self.zones.get(id.0 as usize).and_then(|slot| slot.as_ref())
    }

    /// Returns a mutable reference to the zone with the given id, if it exists
    /// and has not been destroyed.
    pub fn get_mut(&mut self, id: ZoneId) -> Option<&mut Zone> {
        self.zones
            .get_mut(id.0 as usize)
            .and_then(|slot| slot.as_mut())
    }

    /// Destroys a zone by resetting its allocator and removing it from the
    /// arena. The zone id will not be reused.
    pub fn destroy_zone(&mut self, id: ZoneId) {
        if let Some(slot) = self.zones.get_mut(id.0 as usize) {
            if let Some(zone) = slot.as_mut() {
                zone.reset();
            }
            *slot = None;
        }
    }

    /// Returns the count of active (non-destroyed) zones.
    pub fn zone_count(&self) -> usize {
        self.zones.iter().filter(|slot| slot.is_some()).count()
    }

    /// Returns the sum of `bytes_allocated` across all active zones.
    pub fn total_bytes(&self) -> usize {
        self.zones
            .iter()
            .filter_map(|slot| slot.as_ref())
            .map(|zone| zone.bytes_allocated())
            .sum()
    }
}

impl Default for ZoneArena {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Index<ZoneId> for ZoneArena {
    type Output = Zone;

    fn index(&self, id: ZoneId) -> &Self::Output {
        self.get(id)
            .unwrap_or_else(|| panic!("no zone found for {id}"))
    }
}

impl std::ops::IndexMut<ZoneId> for ZoneArena {
    fn index_mut(&mut self, id: ZoneId) -> &mut Self::Output {
        self.get_mut(id)
            .unwrap_or_else(|| panic!("no zone found for {id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_creation_and_basic_alloc() {
        let mut zone = Zone::new(ZoneId(0));
        let val = zone.alloc(42u64);
        assert_eq!(*val, 42);
        assert_eq!(zone.object_count(), 1);
        assert_eq!(zone.id(), ZoneId(0));
    }

    #[test]
    fn alloc_slice_round_trip() {
        let mut zone = Zone::new(ZoneId(0));
        let data = [1u32, 2, 3, 4, 5];
        let slice = zone.alloc_slice(&data);
        assert_eq!(slice, &[1, 2, 3, 4, 5]);
        assert_eq!(zone.object_count(), 1);
    }

    #[test]
    fn alloc_str_round_trip() {
        let mut zone = Zone::new(ZoneId(0));
        let s = zone.alloc_str("hello world");
        assert_eq!(s, "hello world");
        assert_eq!(zone.object_count(), 1);
    }

    #[test]
    fn alloc_empty_slice() {
        let mut zone = Zone::new(ZoneId(0));
        let slice: &[u8] = zone.alloc_slice(&[]);
        assert!(slice.is_empty());
    }

    #[test]
    fn alloc_empty_str() {
        let mut zone = Zone::new(ZoneId(0));
        let s = zone.alloc_str("");
        assert_eq!(s, "");
    }

    #[test]
    fn with_capacity_creates_zone() {
        let mut zone = Zone::with_capacity(ZoneId(5), 4096);
        assert_eq!(zone.id(), ZoneId(5));
        assert_eq!(zone.epoch(), 0);
        let val = zone.alloc(99u32);
        assert_eq!(*val, 99);
    }

    #[test]
    fn epoch_starts_at_zero() {
        let zone = Zone::new(ZoneId(0));
        assert_eq!(zone.epoch(), 0);
    }

    #[test]
    fn epoch_increments_on_reset() {
        let mut zone = Zone::new(ZoneId(0));
        assert_eq!(zone.epoch(), 0);
        zone.reset();
        assert_eq!(zone.epoch(), 1);
        zone.reset();
        assert_eq!(zone.epoch(), 2);
    }

    #[test]
    fn bytes_allocated_tracking() {
        let mut zone = Zone::new(ZoneId(0));
        assert_eq!(zone.bytes_allocated(), 0);
        zone.alloc(42u64);
        assert_eq!(zone.bytes_allocated(), 8);
        zone.alloc(1u8);
        assert_eq!(zone.bytes_allocated(), 9);
    }

    #[test]
    fn bytes_allocated_resets() {
        let mut zone = Zone::new(ZoneId(0));
        zone.alloc(42u64);
        assert_eq!(zone.bytes_allocated(), 8);
        zone.reset();
        assert_eq!(zone.bytes_allocated(), 0);
    }

    #[test]
    fn bytes_allocated_slice() {
        let mut zone = Zone::new(ZoneId(0));
        zone.alloc_slice(&[1u32, 2, 3]);
        assert_eq!(zone.bytes_allocated(), 12); // 3 * 4 bytes
    }

    #[test]
    fn bytes_allocated_str() {
        let mut zone = Zone::new(ZoneId(0));
        zone.alloc_str("abc");
        assert_eq!(zone.bytes_allocated(), 3);
    }

    #[test]
    fn zone_arena_create_and_get() {
        let mut arena = ZoneArena::new();
        let id = arena.create_zone();
        assert!(arena.get(id).is_some());
        assert_eq!(arena.get(id).unwrap().id(), id);
    }

    #[test]
    fn zone_arena_zone_count() {
        let mut arena = ZoneArena::new();
        assert_eq!(arena.zone_count(), 0);
        let id0 = arena.create_zone();
        let _id1 = arena.create_zone();
        assert_eq!(arena.zone_count(), 2);
        arena.destroy_zone(id0);
        assert_eq!(arena.zone_count(), 1);
    }

    #[test]
    fn zone_arena_total_bytes() {
        let mut arena = ZoneArena::new();
        let id0 = arena.create_zone();
        let id1 = arena.create_zone();
        arena[id0].alloc(1u64);
        arena[id1].alloc(2u32);
        assert_eq!(arena.total_bytes(), 12); // 8 + 4
    }

    #[test]
    fn zone_arena_index_works() {
        let mut arena = ZoneArena::new();
        let id = arena.create_zone();
        arena[id].alloc(10u32);
        assert_eq!(arena[id].object_count(), 1);
    }

    #[test]
    #[should_panic(expected = "no zone found")]
    fn zone_arena_index_panics_on_invalid() {
        let arena = ZoneArena::new();
        let _ = &arena[ZoneId(999)];
    }

    #[test]
    #[should_panic(expected = "no zone found")]
    fn zone_arena_index_mut_panics_on_destroyed() {
        let mut arena = ZoneArena::new();
        let id = arena.create_zone();
        arena.destroy_zone(id);
        arena[id].alloc(1u32);
    }

    #[test]
    fn zone_id_display() {
        assert_eq!(format!("{}", ZoneId(0)), "zone0");
        assert_eq!(format!("{}", ZoneId(42)), "zone42");
    }

    #[test]
    fn zone_arena_destroy_and_get_returns_none() {
        let mut arena = ZoneArena::new();
        let id = arena.create_zone();
        arena.destroy_zone(id);
        assert!(arena.get(id).is_none());
    }

    // -- Edge cases: alloc after reset ----------------------------------------

    #[test]
    fn test_alloc_after_reset_works() {
        let mut zone = Zone::new(ZoneId(0));
        zone.alloc(1u32);
        zone.alloc(2u32);
        assert_eq!(zone.object_count(), 2);
        zone.reset();
        assert_eq!(zone.object_count(), 0);
        let val = zone.alloc(3u32);
        assert_eq!(*val, 3);
        assert_eq!(zone.object_count(), 1);
    }

    #[test]
    fn test_alloc_str_after_reset() {
        let mut zone = Zone::new(ZoneId(0));
        zone.alloc_str("first");
        zone.reset();
        let s = zone.alloc_str("second");
        assert_eq!(s, "second");
    }

    // -- Edge cases: zero-size types -----------------------------------------

    #[test]
    fn test_alloc_zero_size_type() {
        let mut zone = Zone::new(ZoneId(0));
        let val = zone.alloc(());
        assert_eq!(*val, ());
        assert_eq!(zone.object_count(), 1);
        // ZST has size 0
        assert_eq!(zone.bytes_allocated(), 0);
    }

    // -- Edge cases: alignment -----------------------------------------------

    #[test]
    fn test_alloc_different_alignments() {
        let mut zone = Zone::new(ZoneId(0));
        let _a = zone.alloc(1u8);
        let _b = zone.alloc(2u64);
        let _c = zone.alloc(3u16);
        assert_eq!(zone.object_count(), 3);
        // 1 + 8 + 2 = 11 bytes tracked
        assert_eq!(zone.bytes_allocated(), 11);
    }

    // -- Edge cases: large allocations ---------------------------------------

    #[test]
    fn test_alloc_large_slice() {
        let mut zone = Zone::new(ZoneId(0));
        let data: Vec<u64> = (0..1000).collect();
        let slice = zone.alloc_slice(&data);
        assert_eq!(slice.len(), 1000);
        assert_eq!(slice[0], 0);
        assert_eq!(slice[999], 999);
        assert_eq!(zone.bytes_allocated(), 8000);
    }

    #[test]
    fn test_alloc_large_str() {
        let mut zone = Zone::new(ZoneId(0));
        let s = "x".repeat(10_000);
        let result = zone.alloc_str(&s);
        assert_eq!(result.len(), 10_000);
        assert_eq!(zone.bytes_allocated(), 10_000);
    }

    // -- Edge cases: multiple resets -----------------------------------------

    #[test]
    fn test_multiple_resets_epoch_tracking() {
        let mut zone = Zone::new(ZoneId(0));
        for i in 0..100 {
            assert_eq!(zone.epoch(), i);
            zone.alloc(42u32);
            zone.reset();
        }
        assert_eq!(zone.epoch(), 100);
    }

    // -- Edge cases: ZoneArena -----------------------------------------------

    #[test]
    fn test_zone_arena_get_nonexistent_returns_none() {
        let arena = ZoneArena::new();
        assert!(arena.get(ZoneId(0)).is_none());
        assert!(arena.get(ZoneId(100)).is_none());
    }

    #[test]
    fn test_zone_arena_get_mut_nonexistent_returns_none() {
        let mut arena = ZoneArena::new();
        assert!(arena.get_mut(ZoneId(0)).is_none());
    }

    #[test]
    fn test_zone_arena_destroy_nonexistent_is_noop() {
        let mut arena = ZoneArena::new();
        // Destroying a zone that was never created should not panic.
        arena.destroy_zone(ZoneId(0));
        arena.destroy_zone(ZoneId(999));
        assert_eq!(arena.zone_count(), 0);
    }

    #[test]
    fn test_zone_arena_destroy_already_destroyed() {
        let mut arena = ZoneArena::new();
        let id = arena.create_zone();
        arena.destroy_zone(id);
        // Destroying again should be a no-op.
        arena.destroy_zone(id);
        assert_eq!(arena.zone_count(), 0);
    }

    #[test]
    fn test_zone_arena_total_bytes_after_destroy() {
        let mut arena = ZoneArena::new();
        let id0 = arena.create_zone();
        let id1 = arena.create_zone();
        arena[id0].alloc(1u64);
        arena[id1].alloc(2u64);
        assert_eq!(arena.total_bytes(), 16);
        arena.destroy_zone(id0);
        assert_eq!(arena.total_bytes(), 8);
    }

    #[test]
    fn test_zone_arena_ids_are_sequential() {
        let mut arena = ZoneArena::new();
        let id0 = arena.create_zone();
        let id1 = arena.create_zone();
        let id2 = arena.create_zone();
        assert_eq!(id0, ZoneId(0));
        assert_eq!(id1, ZoneId(1));
        assert_eq!(id2, ZoneId(2));
    }

    #[test]
    fn test_zone_arena_ids_not_reused_after_destroy() {
        let mut arena = ZoneArena::new();
        let id0 = arena.create_zone();
        arena.destroy_zone(id0);
        let id1 = arena.create_zone();
        // id1 should be ZoneId(1), not ZoneId(0) (ids are never reused)
        assert_eq!(id1, ZoneId(1));
        assert_ne!(id0, id1);
    }

    #[test]
    fn test_zone_arena_default() {
        let arena = ZoneArena::default();
        assert_eq!(arena.zone_count(), 0);
    }

    #[test]
    fn test_zone_with_capacity_zero() {
        let mut zone = Zone::with_capacity(ZoneId(0), 0);
        // Should still work even with zero initial capacity
        let val = zone.alloc(42u32);
        assert_eq!(*val, 42);
    }

    // -- Edge cases: ZoneId --------------------------------------------------

    #[test]
    fn test_zone_id_hash_consistent() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ZoneId(0));
        set.insert(ZoneId(0));
        set.insert(ZoneId(1));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_zone_id_max_value() {
        let id = ZoneId(u32::MAX);
        assert_eq!(format!("{}", id), format!("zone{}", u32::MAX));
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn proptest_random_alloc_reset_no_crash(ops in proptest::collection::vec(0u8..3, 1..50)) {
            let mut zone = Zone::new(ZoneId(0));
            for op in ops {
                match op {
                    0 => { zone.alloc(42u64); }
                    1 => { zone.alloc_str("test"); }
                    _ => { zone.reset(); }
                }
            }
        }
    }
}
