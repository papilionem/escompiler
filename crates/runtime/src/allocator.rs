//! Allocation dispatch: zone vs heap mode.

use arena::{ZoneArena, ZoneId};

/// Manages allocation across zone and heap worlds.
pub struct Allocator {
    /// Zone arena for bump-pointer allocations.
    pub zones: ZoneArena,
    /// When true, all allocations go to heap (for differential testing).
    pub heap_only: bool,
    /// Running total of heap bytes allocated.
    total_heap_bytes: usize,
}

impl Allocator {
    /// Creates a new allocator.
    ///
    /// If `heap_only` is true, zone allocation is disabled and everything
    /// uses per-object reference counting (the `--heap-only` CLI flag).
    pub fn new(heap_only: bool) -> Self {
        Self {
            zones: ZoneArena::new(),
            heap_only,
            total_heap_bytes: 0,
        }
    }

    /// Creates a new zone for allocation.
    pub fn create_zone(&mut self) -> ZoneId {
        self.zones.create_zone()
    }

    /// Destroys a zone and frees all objects in it.
    pub fn destroy_zone(&mut self, zone: ZoneId) {
        self.zones.destroy_zone(zone);
    }

    /// Returns `true` if operating in heap-only mode.
    pub fn is_heap_only(&self) -> bool {
        self.heap_only
    }

    /// Returns the total heap bytes allocated so far.
    pub fn heap_bytes(&self) -> usize {
        self.total_heap_bytes
    }

    /// Records a heap allocation of the given size.
    pub fn record_heap_alloc(&mut self, bytes: usize) {
        self.total_heap_bytes += bytes;
    }
}
