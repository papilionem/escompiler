#![cfg(test)]

use crate::NodeId;
use crate::collector::CycleCollector;
use crate::suspect::SuspectList;
use crate::trace::Trace;

/// Test helper: a simple object that holds edges to other nodes.
#[derive(Debug, Clone)]
struct TestObj {
    edges: Vec<NodeId>,
}

impl TestObj {
    fn new(edges: Vec<NodeId>) -> Self {
        Self { edges }
    }

    fn empty() -> Self {
        Self { edges: Vec::new() }
    }
}

impl Trace for TestObj {
    fn trace(&self, tracer: &mut dyn FnMut(NodeId)) {
        for &edge in &self.edges {
            tracer(edge);
        }
    }
}

// ---------------------------------------------------------------------------
// SuspectList tests
// ---------------------------------------------------------------------------

#[test]
fn test_suspect_list_new_is_empty() {
    let list = SuspectList::new();
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
}

#[test]
fn test_suspect_list_add_and_drain() {
    let mut list = SuspectList::new();
    list.add_suspect(NodeId(1));
    list.add_suspect(NodeId(2));
    assert_eq!(list.len(), 2);
    assert!(!list.is_empty());

    let drained = list.drain();
    assert_eq!(drained, vec![NodeId(1), NodeId(2)]);
    assert!(list.is_empty());
}

#[test]
fn test_suspect_list_drain_clears() {
    let mut list = SuspectList::new();
    list.add_suspect(NodeId(42));
    let _ = list.drain();
    assert!(list.is_empty());
    let again = list.drain();
    assert!(again.is_empty());
}

// ---------------------------------------------------------------------------
// CycleCollector — basic lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_register_unregister() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    cc.register(a, 1, TestObj::empty());
    assert!(cc.contains(a));
    assert_eq!(cc.node_count(), 1);

    cc.unregister(a);
    assert!(!cc.contains(a));
    assert_eq!(cc.node_count(), 0);
}

#[test]
fn test_empty_collect() {
    let mut cc: CycleCollector<TestObj> = CycleCollector::new();
    let garbage = cc.collect().unwrap();
    assert!(garbage.is_empty());
}

#[test]
fn test_increment_decrement() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    cc.register(a, 1, TestObj::empty());
    cc.increment(a);
    // RC is now 2. Decrement to 1 — should not be zero.
    let zero = cc.decrement(a);
    assert!(!zero);
    // Decrement to 0 — should signal release.
    let zero = cc.decrement(a);
    assert!(zero);
}

// ---------------------------------------------------------------------------
// CycleCollector — no cycles
// ---------------------------------------------------------------------------

#[test]
fn test_no_cycles_not_collected() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    let b = NodeId(2);
    // A→B (no cycle). Both have RC=1 from an external root.
    cc.register(a, 1, TestObj::new(vec![b]));
    cc.register(b, 1, TestObj::empty());

    // Nothing to suspect — collect should find nothing.
    let garbage = cc.collect().unwrap();
    assert!(garbage.is_empty());
}

#[test]
fn test_no_false_positives_with_external_refs() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    let b = NodeId(2);
    // A→B→A cycle, but both also have an external reference (RC=2).
    cc.register(a, 2, TestObj::new(vec![b]));
    cc.register(b, 2, TestObj::new(vec![a]));

    // Simulate decrementing one external ref on each (RC goes to 2→1 internally
    // but the collector sees rc=2, we decrement to make suspects).
    cc.add_suspect(a);
    cc.add_suspect(b);

    let garbage = cc.collect().unwrap();
    // They both still have external references (trial RC > 0), so NOT collected.
    assert!(garbage.is_empty());
}

// ---------------------------------------------------------------------------
// CycleCollector — simple cycle detection
// ---------------------------------------------------------------------------

#[test]
fn test_simple_cycle_a_b() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    let b = NodeId(2);
    // A→B→A, each with RC=1 (only internal refs).
    cc.register(a, 1, TestObj::new(vec![b]));
    cc.register(b, 1, TestObj::new(vec![a]));

    cc.add_suspect(a);
    cc.add_suspect(b);

    let mut garbage = cc.collect().unwrap();
    garbage.sort();
    assert_eq!(garbage, vec![NodeId(1), NodeId(2)]);
}

#[test]
fn test_self_reference() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    // A→A self-cycle, RC=1.
    cc.register(a, 1, TestObj::new(vec![a]));
    cc.add_suspect(a);

    let garbage = cc.collect().unwrap();
    assert_eq!(garbage, vec![NodeId(1)]);
}

#[test]
fn test_three_node_cycle() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    let b = NodeId(2);
    let c = NodeId(3);
    // A→B→C→A, all RC=1.
    cc.register(a, 1, TestObj::new(vec![b]));
    cc.register(b, 1, TestObj::new(vec![c]));
    cc.register(c, 1, TestObj::new(vec![a]));

    cc.add_suspect(a);

    let mut garbage = cc.collect().unwrap();
    garbage.sort();
    assert_eq!(garbage, vec![NodeId(1), NodeId(2), NodeId(3)]);
}

#[test]
fn test_large_cycle_100_nodes() {
    let mut cc = CycleCollector::new();
    let n = 100u64;
    // Create a ring: 0→1→2→...→99→0
    for i in 0..n {
        let next = (i + 1) % n;
        cc.register(NodeId(i), 1, TestObj::new(vec![NodeId(next)]));
    }
    cc.add_suspect(NodeId(0));

    let garbage = cc.collect().unwrap();
    assert_eq!(garbage.len(), 100);
}

// ---------------------------------------------------------------------------
// CycleCollector — mixed graphs
// ---------------------------------------------------------------------------

#[test]
fn test_mixed_graph_cyclic_and_acyclic() {
    let mut cc = CycleCollector::new();
    // Cyclic: 1→2→1 (RC=1 each)
    // Acyclic: 3→4 (RC=2 for 3, RC=1 for 4, but 3 has an external ref)
    cc.register(NodeId(1), 1, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(1)]));
    cc.register(NodeId(3), 2, TestObj::new(vec![NodeId(4)]));
    cc.register(NodeId(4), 1, TestObj::empty());

    cc.add_suspect(NodeId(1));
    cc.add_suspect(NodeId(2));
    cc.add_suspect(NodeId(3));

    let mut garbage = cc.collect().unwrap();
    garbage.sort();
    // Only the cycle (1,2) should be collected. 3 has external RC, 4 is reachable from 3.
    assert_eq!(garbage, vec![NodeId(1), NodeId(2)]);
}

#[test]
fn test_diamond_graph_not_collected() {
    let mut cc = CycleCollector::new();
    // A→B, A→C, B→D, C→D — no cycle, A has external ref (RC=2).
    let a = NodeId(1);
    let b = NodeId(2);
    let c = NodeId(3);
    let d = NodeId(4);
    cc.register(a, 2, TestObj::new(vec![b, c]));
    cc.register(b, 1, TestObj::new(vec![d]));
    cc.register(c, 1, TestObj::new(vec![d]));
    cc.register(d, 2, TestObj::empty()); // RC=2 because B and C both point to D.

    cc.add_suspect(a);
    let garbage = cc.collect().unwrap();
    assert!(garbage.is_empty());
}

#[test]
fn test_diamond_with_back_edge_is_cycle() {
    let mut cc = CycleCollector::new();
    // A→B, A→C, B→D, C→D, D→A — cycle through all 4.
    let a = NodeId(1);
    let b = NodeId(2);
    let c = NodeId(3);
    let d = NodeId(4);
    // RC counts: A has refs from D (1), B from A (1), C from A (1), D from B+C (2)
    // But all are only internal — no external roots.
    cc.register(a, 1, TestObj::new(vec![b, c]));
    cc.register(b, 1, TestObj::new(vec![d]));
    cc.register(c, 1, TestObj::new(vec![d]));
    cc.register(d, 2, TestObj::new(vec![a]));

    cc.add_suspect(a);

    let mut garbage = cc.collect().unwrap();
    garbage.sort();
    assert_eq!(garbage, vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)]);
}

// ---------------------------------------------------------------------------
// CycleCollector — incremental / multiple collections
// ---------------------------------------------------------------------------

#[test]
fn test_incremental_collection_batches() {
    let mut cc = CycleCollector::new();
    // Batch 1: cycle 1→2→1
    cc.register(NodeId(1), 1, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(1)]));
    cc.add_suspect(NodeId(1));

    let garbage1 = cc.collect().unwrap();
    assert_eq!(garbage1.len(), 2);

    // Batch 2: cycle 3→4→3
    cc.register(NodeId(3), 1, TestObj::new(vec![NodeId(4)]));
    cc.register(NodeId(4), 1, TestObj::new(vec![NodeId(3)]));
    cc.add_suspect(NodeId(3));

    let garbage2 = cc.collect().unwrap();
    assert_eq!(garbage2.len(), 2);
}

#[test]
fn test_multiple_collections_idempotent() {
    let mut cc = CycleCollector::new();
    cc.register(NodeId(1), 1, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(1)]));
    cc.add_suspect(NodeId(1));
    cc.add_suspect(NodeId(2));

    let garbage1 = cc.collect().unwrap();
    assert_eq!(garbage1.len(), 2);

    // Second collect with no new suspects should find nothing.
    let garbage2 = cc.collect().unwrap();
    assert!(garbage2.is_empty());
}

#[test]
fn test_collect_after_unregister() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    cc.register(a, 1, TestObj::empty());
    cc.add_suspect(a);
    cc.unregister(a);

    // Suspect references a now-unregistered node — should be harmless.
    let garbage = cc.collect().unwrap();
    assert!(garbage.is_empty());
}

// ---------------------------------------------------------------------------
// CycleCollector — acyclic (green) nodes
// ---------------------------------------------------------------------------

#[test]
fn test_acyclic_node_skipped() {
    let mut cc = CycleCollector::new();
    let a = NodeId(1);
    cc.register(a, 1, TestObj::new(vec![a]));
    cc.mark_acyclic(a);
    cc.add_suspect(a); // Should be ignored because green.

    let garbage = cc.collect().unwrap();
    assert!(garbage.is_empty());
}

// ---------------------------------------------------------------------------
// CycleCollector — edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_decrement_unregistered_node() {
    let mut cc: CycleCollector<TestObj> = CycleCollector::new();
    // Should not panic.
    let zero = cc.decrement(NodeId(999));
    assert!(!zero);
}

#[test]
fn test_increment_unregistered_node() {
    let mut cc: CycleCollector<TestObj> = CycleCollector::new();
    // Should not panic.
    cc.increment(NodeId(999));
}

#[test]
fn test_cycle_with_tail() {
    let mut cc = CycleCollector::new();
    // 1→2→3→2 (cycle is 2↔3, node 1 is a tail into the cycle)
    // All internal refs only.
    cc.register(NodeId(1), 0, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(3)])); // ref from 1
    cc.register(NodeId(3), 1, TestObj::new(vec![NodeId(2)]));

    // Node 1 has RC=0 (would be freed by RC alone), but let's
    // test that the cycle 2↔3 is detected.
    cc.add_suspect(NodeId(2));
    cc.add_suspect(NodeId(3));

    let mut garbage = cc.collect().unwrap();
    garbage.sort();
    assert_eq!(garbage, vec![NodeId(2), NodeId(3)]);
}

#[test]
fn test_cycle_preserved_by_external_ref() {
    let mut cc = CycleCollector::new();
    // A→B→A cycle, but A has RC=2 (one from B, one external).
    cc.register(NodeId(1), 2, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(1)]));

    cc.add_suspect(NodeId(1));
    cc.add_suspect(NodeId(2));

    let garbage = cc.collect().unwrap();
    // A has an external ref so the cycle is preserved.
    assert!(garbage.is_empty());
}

#[test]
fn test_node_count_after_collection() {
    let mut cc = CycleCollector::new();
    cc.register(NodeId(1), 1, TestObj::new(vec![NodeId(2)]));
    cc.register(NodeId(2), 1, TestObj::new(vec![NodeId(1)]));
    assert_eq!(cc.node_count(), 2);

    cc.add_suspect(NodeId(1));
    let _ = cc.collect().unwrap();
    assert_eq!(cc.node_count(), 0);
}

#[test]
fn test_default_impl() {
    let cc: CycleCollector<TestObj> = CycleCollector::default();
    assert_eq!(cc.node_count(), 0);
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Generate a random graph with `n` nodes and random edges, then verify
    /// that nodes with external references are never collected.
    fn arb_graph(max_nodes: u64) -> impl Strategy<Value = (Vec<(u64, Vec<u64>)>, Vec<u64>)> {
        (1..=max_nodes).prop_flat_map(move |n| {
            let edges = proptest::collection::vec(
                (0..n, proptest::collection::vec(0..n, 0..3)),
                n as usize,
            );
            let external = proptest::collection::vec(0..n, 0..n as usize);
            (edges, external)
        })
    }

    proptest! {
        #[test]
        fn random_graph_no_false_positives(
            (edges, external_roots) in arb_graph(20)
        ) {
            let mut cc = CycleCollector::new();

            // Build edge map.
            let mut edge_map: std::collections::HashMap<u64, Vec<NodeId>> =
                std::collections::HashMap::new();
            for (node, targets) in &edges {
                let t: Vec<NodeId> = targets.iter().map(|&t| NodeId(t)).collect();
                edge_map.insert(*node, t);
            }

            // Compute RCs: count of incoming edges + external root count.
            let mut rc_map: std::collections::HashMap<u64, u32> =
                std::collections::HashMap::new();
            for (node, _) in &edges {
                rc_map.entry(*node).or_insert(0);
            }
            for targets in edge_map.values() {
                for t in targets {
                    *rc_map.entry(t.0).or_insert(0) += 1;
                }
            }
            for &ext in &external_roots {
                *rc_map.entry(ext).or_insert(0) += 1;
            }

            // Register nodes.
            for (node, _) in &edges {
                let obj = TestObj::new(
                    edge_map.get(node).cloned().unwrap_or_default(),
                );
                cc.register(
                    NodeId(*node),
                    *rc_map.get(node).unwrap_or(&0),
                    obj,
                );
            }

            // All nodes are suspects.
            for (node, _) in &edges {
                cc.add_suspect(NodeId(*node));
            }

            let garbage = cc.collect().unwrap();
            let garbage_set: std::collections::HashSet<NodeId> =
                garbage.into_iter().collect();

            // Verify: no node reachable from an external root is in garbage.
            // (Simplified: external root nodes themselves must not be collected,
            // since they have at least one external RC.)
            for &ext in &external_roots {
                prop_assert!(
                    !garbage_set.contains(&NodeId(ext)),
                    "External root node {} was incorrectly collected",
                    ext
                );
            }
        }
    }
}
