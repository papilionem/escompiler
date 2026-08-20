use crate::NodeId;

/// A buffer of nodes whose reference count was decremented to a non-zero value.
///
/// These nodes are *suspects* — they might be part of a garbage cycle.
/// The cycle collector drains this list periodically to run trial deletion.
#[derive(Debug, Default)]
pub struct SuspectList {
    suspects: Vec<NodeId>,
}

impl SuspectList {
    /// Create an empty suspect list.
    pub fn new() -> Self {
        Self {
            suspects: Vec::new(),
        }
    }

    /// Add a node to the suspect buffer.
    ///
    /// Called by the reference-counting system when a decrement does
    /// not reach zero — the node *might* be part of a cycle.
    pub fn add_suspect(&mut self, node: NodeId) {
        self.suspects.push(node);
    }

    /// Drain all suspects and return them, clearing the internal buffer.
    pub fn drain(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.suspects)
    }

    /// Returns `true` if there are no suspects.
    pub fn is_empty(&self) -> bool {
        self.suspects.is_empty()
    }

    /// Returns the number of suspects currently buffered.
    pub fn len(&self) -> usize {
        self.suspects.len()
    }
}
