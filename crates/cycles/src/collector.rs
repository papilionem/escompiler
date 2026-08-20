use std::collections::HashMap;

use crate::trace::Trace;
use crate::{CycleError, NodeId};

/// Color used in the Bacon-Rajan trial deletion algorithm.
///
/// Each node is painted a color to track its state during collection:
/// - **Black**: In use or already scanned (default for live objects).
/// - **Purple**: A suspect — RC was decremented, may be part of a cycle.
/// - **Gray**: Being traced during the mark phase (trial RC decremented).
/// - **White**: Garbage — trial RC reached zero, will be collected.
/// - **Green**: Acyclic — known to not participate in cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// In use or already restored after trial deletion.
    Black,
    /// Suspect — reference count was decremented to non-zero.
    Purple,
    /// Currently being traced in the mark phase.
    Gray,
    /// Garbage — trial RC reached zero during scan.
    White,
    /// Known acyclic — excluded from cycle collection.
    Green,
}

/// Per-node metadata tracked by the cycle collector.
#[derive(Debug, Clone)]
struct NodeInfo {
    color: Color,
    /// The real reference count (mirrored from rc).
    rc: u32,
    /// Whether this node is in the suspect buffer.
    buffered: bool,
}

/// Bacon-Rajan cycle collector for heap-allocated objects.
///
/// Detects and collects cyclic reference graphs that cannot be
/// reclaimed by reference counting alone. The algorithm uses trial
/// deletion: it hypothetically removes all internal edges of a
/// suspect subgraph and checks whether any external references remain.
///
/// # Type parameter
///
/// `T` must implement [`Trace`] so the collector can enumerate edges.
/// A registry callback (`resolve`) maps [`NodeId`] to `&T` references.
pub struct CycleCollector<T: Trace> {
    nodes: HashMap<NodeId, NodeInfo>,
    suspects: Vec<NodeId>,
    /// Maps node IDs to their traceable objects.
    objects: HashMap<NodeId, T>,
}

impl<T: Trace> CycleCollector<T> {
    /// Create a new, empty cycle collector.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            suspects: Vec::new(),
            objects: HashMap::new(),
        }
    }

    /// Register a node with the collector.
    ///
    /// The initial reference count and traceable object must be provided.
    /// Nodes start with color [`Color::Black`].
    pub fn register(&mut self, id: NodeId, rc: u32, object: T) {
        self.nodes.insert(
            id,
            NodeInfo {
                color: Color::Black,
                rc,
                buffered: false,
            },
        );
        self.objects.insert(id, object);
    }

    /// Unregister a node, removing it from the collector entirely.
    pub fn unregister(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.objects.remove(&id);
    }

    /// Notify the collector that a node's reference count was incremented.
    ///
    /// The node is painted black (definitely in use).
    pub fn increment(&mut self, id: NodeId) {
        if let Some(info) = self.nodes.get_mut(&id) {
            info.rc = info.rc.saturating_add(1);
            info.color = Color::Black;
        }
    }

    /// Notify the collector that a node's reference count was decremented.
    ///
    /// If the RC reaches zero, the node is marked for release. Otherwise
    /// it becomes a suspect (purple) and is buffered for trial deletion.
    ///
    /// Returns `true` if the RC reached zero (caller should free the node).
    pub fn decrement(&mut self, id: NodeId) -> bool {
        let Some(info) = self.nodes.get_mut(&id) else {
            return false;
        };
        info.rc = info.rc.saturating_sub(1);
        if info.rc == 0 {
            // RC is zero — release immediately (not a cycle issue).
            true
        } else {
            self.add_suspect(id);
            false
        }
    }

    /// Add a node to the suspect buffer, marking it purple.
    pub fn add_suspect(&mut self, id: NodeId) {
        if let Some(info) = self.nodes.get_mut(&id) {
            if info.color == Color::Green {
                // Acyclic nodes are never suspects.
                return;
            }
            info.color = Color::Purple;
            if !info.buffered {
                info.buffered = true;
                self.suspects.push(id);
            }
        }
    }

    /// Run the full collection cycle: process all suspects through
    /// mark-gray, scan, and collect-white phases.
    ///
    /// Returns the set of [`NodeId`]s that were determined to be garbage.
    pub fn collect(&mut self) -> Result<Vec<NodeId>, CycleError> {
        let suspects: Vec<NodeId> = std::mem::take(&mut self.suspects);
        let mut garbage = Vec::new();

        for &id in &suspects {
            // Phase 1: mark gray
            if self.should_process(id) {
                self.mark_gray(id)?;
                // Phase 2: scan
                self.scan(id)?;
                // Phase 3: collect white
                self.collect_white(id, &mut garbage)?;
            }
            // Clear buffered flag
            if let Some(info) = self.nodes.get_mut(&id) {
                info.buffered = false;
            }
        }

        // Remove collected nodes from the registry.
        for &id in &garbage {
            self.nodes.remove(&id);
            self.objects.remove(&id);
        }

        Ok(garbage)
    }

    /// Returns `true` if this suspect should be processed (is still purple).
    fn should_process(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|info| info.color == Color::Purple)
    }

    /// Phase 1: Hypothetically remove internal edges by decrementing
    /// trial RCs and painting nodes gray.
    fn mark_gray(&mut self, id: NodeId) -> Result<(), CycleError> {
        let Some(info) = self.nodes.get(&id) else {
            return Ok(());
        };
        if info.color == Color::Gray {
            return Ok(());
        }

        // Paint gray.
        if let Some(info) = self.nodes.get_mut(&id) {
            info.color = Color::Gray;
        }

        // Get children by tracing.
        let children = self.get_children(id);

        for child in children {
            // Decrement child's trial RC.
            if let Some(child_info) = self.nodes.get_mut(&child) {
                child_info.rc = child_info.rc.saturating_sub(1);
            }
            self.mark_gray(child)?;
        }

        Ok(())
    }

    /// Phase 2: Determine which nodes are actually garbage.
    ///
    /// If a node's trial RC is > 0, it has external references — restore
    /// it (scan_black). If trial RC == 0, mark it white (garbage candidate).
    fn scan(&mut self, id: NodeId) -> Result<(), CycleError> {
        let Some(info) = self.nodes.get(&id) else {
            return Ok(());
        };
        if info.color != Color::Gray {
            return Ok(());
        }

        let rc = info.rc;
        if rc > 0 {
            // External references exist — restore the subgraph.
            self.scan_black(id)?;
        } else {
            // No external references — mark as garbage.
            if let Some(info) = self.nodes.get_mut(&id) {
                info.color = Color::White;
            }
            let children = self.get_children(id);
            for child in children {
                self.scan(child)?;
            }
        }

        Ok(())
    }

    /// Restore a node and its children to black (live).
    fn scan_black(&mut self, id: NodeId) -> Result<(), CycleError> {
        let Some(info) = self.nodes.get(&id) else {
            return Ok(());
        };
        if info.color == Color::Black {
            return Ok(());
        }

        if let Some(info) = self.nodes.get_mut(&id) {
            info.color = Color::Black;
        }

        let children = self.get_children(id);
        for child in children {
            // Restore trial RC.
            if let Some(child_info) = self.nodes.get_mut(&child) {
                child_info.rc = child_info.rc.saturating_add(1);
            }
            self.scan_black(child)?;
        }

        Ok(())
    }

    /// Phase 3: Collect all white (garbage) nodes reachable from `id`.
    fn collect_white(&mut self, id: NodeId, garbage: &mut Vec<NodeId>) -> Result<(), CycleError> {
        let Some(info) = self.nodes.get(&id) else {
            return Ok(());
        };
        if info.color != Color::White {
            return Ok(());
        }

        // Mark collected so we don't visit twice.
        if let Some(info) = self.nodes.get_mut(&id) {
            info.color = Color::Black;
        }

        let children = self.get_children(id);
        for child in children {
            self.collect_white(child, garbage)?;
        }

        garbage.push(id);
        Ok(())
    }

    /// Enumerate children of a node by invoking its [`Trace`] impl.
    fn get_children(&self, id: NodeId) -> Vec<NodeId> {
        let mut children = Vec::new();
        if let Some(obj) = self.objects.get(&id) {
            obj.trace(&mut |child| {
                children.push(child);
            });
        }
        children
    }

    /// Returns the number of registered nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if a node is currently registered.
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Mark a node as acyclic (green). It will be excluded from future
    /// cycle collection.
    pub fn mark_acyclic(&mut self, id: NodeId) {
        if let Some(info) = self.nodes.get_mut(&id) {
            info.color = Color::Green;
        }
    }
}

impl<T: Trace> Default for CycleCollector<T> {
    fn default() -> Self {
        Self::new()
    }
}
