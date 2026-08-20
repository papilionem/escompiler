use crate::NodeId;

/// Trait for objects that participate in cycle collection.
///
/// Implementors must enumerate all outgoing references (edges in the
/// object graph) so the collector can perform trial deletion.
pub trait Trace {
    /// Visit all outgoing references from this object.
    ///
    /// The `tracer` callback must be invoked once for each [`NodeId`]
    /// that this object holds a strong reference to.
    fn trace(&self, tracer: &mut dyn FnMut(NodeId));
}
