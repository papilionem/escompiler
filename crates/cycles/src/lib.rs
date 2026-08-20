//! Bacon-Rajan cycle collector for heap-allocated objects.
//!
//! Implements trial deletion to detect and collect cyclic reference
//! graphs that cannot be reclaimed by reference counting alone.
//!
//! # Key types
//!
//! - [`CycleCollector`] — the main collector; tracks nodes, runs the 4-phase
//!   Bacon-Rajan algorithm (mark-gray, scan, scan-black, collect-white).
//! - [`Trace`] — trait that objects implement to enumerate outgoing references.
//! - [`SuspectList`] — buffer of nodes whose RC was decremented to non-zero.
//! - [`NodeId`] — opaque identifier for a heap-allocated object.
//!
//! # Usage
//!
//! ```rust
//! use cycles::{CycleCollector, Trace, NodeId};
//!
//! struct MyObj { edges: Vec<NodeId> }
//! impl Trace for MyObj {
//!     fn trace(&self, tracer: &mut dyn FnMut(NodeId)) {
//!         for &edge in &self.edges { tracer(edge); }
//!     }
//! }
//!
//! let mut cc = CycleCollector::new();
//! let a = NodeId(1);
//! let b = NodeId(2);
//! cc.register(a, 1, MyObj { edges: vec![b] });
//! cc.register(b, 1, MyObj { edges: vec![a] });
//! // After decrementing external references, run collection:
//! let garbage = cc.collect().unwrap();
//! ```

pub mod collector;
pub mod suspect;
pub mod trace;

#[cfg(test)]
mod tests;

pub use collector::{Color, CycleCollector};
pub use suspect::SuspectList;
pub use trace::Trace;

use thiserror::Error;

/// Opaque identifier for a heap-allocated object tracked by the cycle collector.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Errors that can occur during cycle collection.
#[derive(Debug, Error)]
pub enum CycleError {
    /// A node referenced during tracing was not registered with the collector.
    #[error("unregistered node: {0:?}")]
    UnregisteredNode(NodeId),
}
