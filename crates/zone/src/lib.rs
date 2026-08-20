//! Zone-level reference counting and cross-zone reference tracking.
//!
//! Tracks references between zones so that a zone can be safely freed only
//! when no other zone references objects within it.

use std::collections::HashMap;

use arena::ZoneId;

/// Represents a cross-zone reference from one zone to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ZoneRef {
    /// The zone that holds the reference.
    pub from_zone: ZoneId,
    /// The zone being referenced.
    pub to_zone: ZoneId,
}

/// Tracks per-zone incoming reference counts from other zones.
///
/// Each entry maps a (from_zone, to_zone) pair to a count of how many
/// cross-zone references exist. This allows determining whether a zone
/// can safely be freed.
pub struct ZoneRcTable {
    /// Maps (from_zone, to_zone) -> count of references.
    refs: HashMap<ZoneRef, u32>,
}

impl ZoneRcTable {
    /// Creates a new, empty cross-zone reference table.
    pub fn new() -> Self {
        Self {
            refs: HashMap::new(),
        }
    }

    /// Records a new cross-zone reference from `from` to `to`.
    pub fn add_ref(&mut self, from: ZoneId, to: ZoneId) {
        let key = ZoneRef {
            from_zone: from,
            to_zone: to,
        };
        *self.refs.entry(key).or_insert(0) += 1;
    }

    /// Releases a cross-zone reference from `from` to `to`.
    ///
    /// Removes the entry entirely when the count reaches zero.
    pub fn release_ref(&mut self, from: ZoneId, to: ZoneId) {
        let key = ZoneRef {
            from_zone: from,
            to_zone: to,
        };
        if let Some(count) = self.refs.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                self.refs.remove(&key);
            }
        }
    }

    /// Returns the total number of incoming cross-zone references to `zone`
    /// (from all other zones).
    pub fn ref_count(&self, zone: ZoneId) -> u32 {
        self.refs
            .iter()
            .filter(|(k, _)| k.to_zone == zone)
            .map(|(_, &count)| count)
            .sum()
    }

    /// Returns true if the zone has no incoming cross-zone references and
    /// can therefore be safely freed.
    pub fn can_free(&self, zone: ZoneId) -> bool {
        self.ref_count(zone) == 0
    }

    /// Returns all zones referenced BY the given zone, with their reference counts.
    pub fn refs_from(&self, zone: ZoneId) -> Vec<(ZoneId, u32)> {
        self.refs
            .iter()
            .filter(|(k, _)| k.from_zone == zone)
            .map(|(k, &count)| (k.to_zone, count))
            .collect()
    }

    /// Returns a read-only reference to the underlying reference map.
    pub fn all_refs(&self) -> &HashMap<ZoneRef, u32> {
        &self.refs
    }

    /// Removes all references from AND to the given zone.
    ///
    /// Used when a zone is destroyed to clean up all tracking state.
    pub fn clear_zone(&mut self, zone: ZoneId) {
        self.refs
            .retain(|k, _| k.from_zone != zone && k.to_zone != zone);
    }

    /// Returns true if no references are being tracked.
    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    /// Returns the sum of all reference counts across all zone pairs.
    pub fn total_ref_count(&self) -> u32 {
        self.refs.values().sum()
    }
}

impl Default for ZoneRcTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Receives notifications about zone lifecycle events.
pub trait ZoneListener {
    /// Called when a new zone is created.
    fn on_zone_created(&mut self, zone: ZoneId);

    /// Called when a zone is reset (all allocations freed, epoch incremented).
    fn on_zone_reset(&mut self, zone: ZoneId);

    /// Called when a zone is destroyed and removed from the arena.
    fn on_zone_destroyed(&mut self, zone: ZoneId);
}

/// Tracks zone epochs to detect stale pointers after a zone reset.
///
/// Records the epoch at which a pointer was obtained. If the zone's current
/// epoch differs from the recorded epoch, the pointer may be stale.
pub struct ZoneEpochTracker {
    epochs: HashMap<ZoneId, u64>,
}

impl ZoneEpochTracker {
    /// Creates a new, empty epoch tracker.
    pub fn new() -> Self {
        Self {
            epochs: HashMap::new(),
        }
    }

    /// Records the current epoch for a zone.
    pub fn record(&mut self, zone: ZoneId, epoch: u64) {
        self.epochs.insert(zone, epoch);
    }

    /// Returns true if the recorded epoch for the zone differs from
    /// `current_epoch`, indicating a potential stale pointer.
    ///
    /// Also returns true if the zone has no recorded epoch.
    pub fn is_stale(&self, zone: ZoneId, current_epoch: u64) -> bool {
        match self.epochs.get(&zone) {
            Some(&recorded) => recorded != current_epoch,
            None => true,
        }
    }

    /// Removes tracking for a destroyed zone.
    pub fn remove(&mut self, zone: ZoneId) {
        self.epochs.remove(&zone);
    }
}

impl Default for ZoneEpochTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ZoneListener for ZoneEpochTracker {
    fn on_zone_created(&mut self, zone: ZoneId) {
        self.record(zone, 0);
    }

    fn on_zone_reset(&mut self, zone: ZoneId) {
        if let Some(epoch) = self.epochs.get_mut(&zone) {
            *epoch += 1;
        }
    }

    fn on_zone_destroyed(&mut self, zone: ZoneId) {
        self.remove(zone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Original 5 tests ---

    #[test]
    fn empty_table_can_free() {
        let table = ZoneRcTable::new();
        assert!(table.can_free(ZoneId(0)));
    }

    #[test]
    fn add_ref_prevents_free() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        assert!(!table.can_free(ZoneId(1)));
        assert!(table.can_free(ZoneId(0)));
    }

    #[test]
    fn release_ref_allows_free() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        table.release_ref(ZoneId(0), ZoneId(1));
        assert!(table.can_free(ZoneId(1)));
    }

    #[test]
    fn multiple_refs_from_different_zones() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(2));
        table.add_ref(ZoneId(1), ZoneId(2));
        assert_eq!(table.ref_count(ZoneId(2)), 2);
        assert!(!table.can_free(ZoneId(2)));

        table.release_ref(ZoneId(0), ZoneId(2));
        assert_eq!(table.ref_count(ZoneId(2)), 1);
        assert!(!table.can_free(ZoneId(2)));

        table.release_ref(ZoneId(1), ZoneId(2));
        assert!(table.can_free(ZoneId(2)));
    }

    #[test]
    fn multiple_refs_same_pair() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(0), ZoneId(1));
        assert_eq!(table.ref_count(ZoneId(1)), 2);

        table.release_ref(ZoneId(0), ZoneId(1));
        assert_eq!(table.ref_count(ZoneId(1)), 1);
    }

    // --- New tests ---

    #[test]
    fn refs_from_returns_correct_zones_and_counts() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(0), ZoneId(2));

        let mut from_0 = table.refs_from(ZoneId(0));
        from_0.sort_by_key(|(id, _)| id.0);
        assert_eq!(from_0, vec![(ZoneId(1), 2), (ZoneId(2), 1)]);
    }

    #[test]
    fn refs_from_empty_for_unknown_zone() {
        let table = ZoneRcTable::new();
        assert!(table.refs_from(ZoneId(99)).is_empty());
    }

    #[test]
    fn all_refs_exposes_map() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        let map = table.all_refs();
        assert_eq!(map.len(), 1);
        let key = ZoneRef {
            from_zone: ZoneId(0),
            to_zone: ZoneId(1),
        };
        assert_eq!(map[&key], 1);
    }

    #[test]
    fn clear_zone_removes_all_related_refs() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(1), ZoneId(2));
        table.add_ref(ZoneId(2), ZoneId(0));

        table.clear_zone(ZoneId(1));

        // ref from 0->1 gone, ref from 1->2 gone, ref from 2->0 remains
        assert!(table.can_free(ZoneId(1)));
        assert!(table.can_free(ZoneId(2)));
        assert!(!table.can_free(ZoneId(0)));
        assert_eq!(table.all_refs().len(), 1);
    }

    #[test]
    fn is_empty_on_new_table() {
        let table = ZoneRcTable::new();
        assert!(table.is_empty());
    }

    #[test]
    fn is_empty_after_adding_refs() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        assert!(!table.is_empty());
    }

    #[test]
    fn total_ref_count_sums_all() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(0), ZoneId(1));
        table.add_ref(ZoneId(2), ZoneId(3));
        assert_eq!(table.total_ref_count(), 3);
    }

    #[test]
    fn total_ref_count_zero_when_empty() {
        let table = ZoneRcTable::new();
        assert_eq!(table.total_ref_count(), 0);
    }

    #[test]
    fn epoch_tracker_basic_usage() {
        let mut tracker = ZoneEpochTracker::new();
        tracker.record(ZoneId(0), 0);
        assert!(!tracker.is_stale(ZoneId(0), 0));
        assert!(tracker.is_stale(ZoneId(0), 1));
    }

    #[test]
    fn epoch_tracker_stale_after_zone_reset() {
        let mut tracker = ZoneEpochTracker::new();
        tracker.record(ZoneId(0), 0);
        // Simulate zone reset: epoch goes from 0 to 1
        assert!(!tracker.is_stale(ZoneId(0), 0));
        assert!(tracker.is_stale(ZoneId(0), 1));
    }

    #[test]
    fn epoch_tracker_unknown_zone_is_stale() {
        let tracker = ZoneEpochTracker::new();
        assert!(tracker.is_stale(ZoneId(99), 0));
    }

    #[test]
    fn epoch_tracker_remove() {
        let mut tracker = ZoneEpochTracker::new();
        tracker.record(ZoneId(0), 5);
        tracker.remove(ZoneId(0));
        assert!(tracker.is_stale(ZoneId(0), 5));
    }

    #[test]
    fn zone_listener_impl_for_epoch_tracker() {
        let mut tracker = ZoneEpochTracker::new();

        // on_zone_created records epoch 0
        tracker.on_zone_created(ZoneId(0));
        assert!(!tracker.is_stale(ZoneId(0), 0));

        // on_zone_reset increments epoch
        tracker.on_zone_reset(ZoneId(0));
        assert!(tracker.is_stale(ZoneId(0), 0));
        assert!(!tracker.is_stale(ZoneId(0), 1));

        // on_zone_destroyed removes tracking
        tracker.on_zone_destroyed(ZoneId(0));
        assert!(tracker.is_stale(ZoneId(0), 1));
    }

    #[test]
    fn zone_listener_multiple_resets() {
        let mut tracker = ZoneEpochTracker::new();
        tracker.on_zone_created(ZoneId(0));
        tracker.on_zone_reset(ZoneId(0));
        tracker.on_zone_reset(ZoneId(0));
        tracker.on_zone_reset(ZoneId(0));
        assert!(!tracker.is_stale(ZoneId(0), 3));
        assert!(tracker.is_stale(ZoneId(0), 2));
    }

    // -- Edge cases: ZoneRcTable release_ref --------------------------------

    #[test]
    fn test_release_ref_nonexistent_is_noop() {
        let mut table = ZoneRcTable::new();
        // Releasing a ref that was never added should not panic.
        table.release_ref(ZoneId(0), ZoneId(1));
        assert!(table.is_empty());
    }

    #[test]
    fn test_release_ref_removes_entry_at_zero() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(1));
        assert_eq!(table.all_refs().len(), 1);
        table.release_ref(ZoneId(0), ZoneId(1));
        // Entry should be fully removed when count reaches 0
        assert_eq!(table.all_refs().len(), 0);
        assert!(table.is_empty());
    }

    // -- Edge cases: self-referencing zone -----------------------------------

    #[test]
    fn test_self_referencing_zone() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(0));
        assert!(!table.can_free(ZoneId(0)));
        assert_eq!(table.ref_count(ZoneId(0)), 1);
        table.release_ref(ZoneId(0), ZoneId(0));
        assert!(table.can_free(ZoneId(0)));
    }

    // -- Edge cases: clear_zone with self-refs ------------------------------

    #[test]
    fn test_clear_zone_removes_self_refs() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(0));
        table.add_ref(ZoneId(0), ZoneId(1));
        table.clear_zone(ZoneId(0));
        assert!(table.is_empty());
    }

    // -- Edge cases: ref_count for zone with no refs -------------------------

    #[test]
    fn test_ref_count_for_unknown_zone() {
        let table = ZoneRcTable::new();
        assert_eq!(table.ref_count(ZoneId(999)), 0);
    }

    // -- Edge cases: multiple refs from same zone ---------------------------

    #[test]
    fn test_many_refs_same_direction() {
        let mut table = ZoneRcTable::new();
        for _ in 0..100 {
            table.add_ref(ZoneId(0), ZoneId(1));
        }
        assert_eq!(table.ref_count(ZoneId(1)), 100);
        assert_eq!(table.total_ref_count(), 100);
    }

    // -- Edge cases: clear_zone on empty table ------------------------------

    #[test]
    fn test_clear_zone_on_empty_table() {
        let mut table = ZoneRcTable::new();
        table.clear_zone(ZoneId(0)); // Should not panic
        assert!(table.is_empty());
    }

    // -- Edge cases: ZoneEpochTracker --------------------------------------

    #[test]
    fn test_epoch_tracker_record_overwrites() {
        let mut tracker = ZoneEpochTracker::new();
        tracker.record(ZoneId(0), 5);
        assert!(!tracker.is_stale(ZoneId(0), 5));
        tracker.record(ZoneId(0), 10);
        assert!(tracker.is_stale(ZoneId(0), 5));
        assert!(!tracker.is_stale(ZoneId(0), 10));
    }

    #[test]
    fn test_epoch_tracker_remove_nonexistent() {
        let mut tracker = ZoneEpochTracker::new();
        // Removing a zone that was never recorded should not panic.
        tracker.remove(ZoneId(999));
    }

    #[test]
    fn test_epoch_tracker_default() {
        let tracker = ZoneEpochTracker::default();
        assert!(tracker.is_stale(ZoneId(0), 0));
    }

    #[test]
    fn test_zone_rc_table_default() {
        let table = ZoneRcTable::default();
        assert!(table.is_empty());
        assert_eq!(table.total_ref_count(), 0);
    }

    // -- Edge cases: ZoneListener reset on uncreated zone -------------------

    #[test]
    fn test_zone_listener_reset_uncreated_zone() {
        let mut tracker = ZoneEpochTracker::new();
        // Reset a zone that was never created — should not panic.
        tracker.on_zone_reset(ZoneId(99));
        // Since it was never created, it's still stale.
        assert!(tracker.is_stale(ZoneId(99), 0));
    }

    #[test]
    fn test_zone_listener_destroy_uncreated_zone() {
        let mut tracker = ZoneEpochTracker::new();
        // Destroy a zone that was never created — should not panic.
        tracker.on_zone_destroyed(ZoneId(99));
    }

    // -- Edge cases: refs_from with self-refs --------------------------------

    #[test]
    fn test_refs_from_includes_self_refs() {
        let mut table = ZoneRcTable::new();
        table.add_ref(ZoneId(0), ZoneId(0));
        table.add_ref(ZoneId(0), ZoneId(1));
        let from_0 = table.refs_from(ZoneId(0));
        assert_eq!(from_0.len(), 2);
    }

    // -- Edge cases: ZoneRef equality and hash ------------------------------

    #[test]
    fn test_zone_ref_equality() {
        let a = ZoneRef {
            from_zone: ZoneId(0),
            to_zone: ZoneId(1),
        };
        let b = ZoneRef {
            from_zone: ZoneId(0),
            to_zone: ZoneId(1),
        };
        let c = ZoneRef {
            from_zone: ZoneId(1),
            to_zone: ZoneId(0),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_zone_ref_hash_in_set() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ZoneRef {
            from_zone: ZoneId(0),
            to_zone: ZoneId(1),
        });
        set.insert(ZoneRef {
            from_zone: ZoneId(0),
            to_zone: ZoneId(1),
        });
        assert_eq!(set.len(), 1);
    }
}
