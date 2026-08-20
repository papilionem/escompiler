//! Cross-world write barrier for zone-to-zone and zone-to-heap references.

use arena::ZoneId;
use zone::ZoneRcTable;

/// Tracks cross-zone references so zones can be safely freed.
pub struct WriteBarrier {
    /// Cross-zone reference count table.
    pub rc_table: ZoneRcTable,
}

impl WriteBarrier {
    /// Creates a new write barrier with an empty reference table.
    pub fn new() -> Self {
        Self {
            rc_table: ZoneRcTable::new(),
        }
    }

    /// Called when storing a reference from one zone to another.
    ///
    /// If `source_zone` and `target_zone` differ, records a cross-zone
    /// reference in the RC table.
    pub fn on_store(&mut self, source_zone: ZoneId, target_zone: ZoneId) {
        if source_zone != target_zone {
            self.rc_table.add_ref(source_zone, target_zone);
        }
    }

    /// Called when a cross-zone reference is removed.
    ///
    /// If `source_zone` and `target_zone` differ, releases the reference
    /// in the RC table.
    pub fn on_remove(&mut self, source_zone: ZoneId, target_zone: ZoneId) {
        if source_zone != target_zone {
            self.rc_table.release_ref(source_zone, target_zone);
        }
    }

    /// Returns `true` if the zone has no incoming cross-zone references
    /// and can safely be freed.
    pub fn can_free_zone(&self, zone: ZoneId) -> bool {
        self.rc_table.can_free(zone)
    }
}

impl Default for WriteBarrier {
    fn default() -> Self {
        Self::new()
    }
}
