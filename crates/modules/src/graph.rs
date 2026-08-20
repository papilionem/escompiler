//! Dependency graph with topological sort and cycle detection.
//!
//! Implements Kahn's algorithm for topological ordering and Tarjan's
//! algorithm for finding strongly connected components (cycles).

use std::collections::{HashMap, HashSet, VecDeque};

use thiserror::Error;

use crate::ModuleId;

/// A directed dependency graph of modules.
///
/// Edges go from a module to the modules it imports (forward edges).
/// Supports topological sorting (Kahn's algorithm) and cycle detection
/// (Tarjan's algorithm for strongly connected components).
pub struct DependencyGraph {
    /// Forward edges: module → set of modules it depends on.
    edges: HashMap<u32, HashSet<u32>>,
    /// Reverse edges: module → set of modules that depend on it.
    reverse_edges: HashMap<u32, HashSet<u32>>,
    /// All known node ids.
    nodes: HashSet<u32>,
}

/// Error indicating a cycle was detected in the dependency graph.
#[derive(Debug, Error)]
#[error("dependency cycle: {}", format_cycle(.cycle))]
pub struct CycleError {
    /// The module ids forming the cycle.
    pub cycle: Vec<ModuleId>,
}

/// Format a cycle as a human-readable string of module ids joined by ` -> `.
fn format_cycle(cycle: &[ModuleId]) -> String {
    cycle
        .iter()
        .map(|id| id.0.to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Internal state for Tarjan's SCC algorithm.
#[derive(Default)]
struct TarjanState {
    index_counter: u32,
    stack: Vec<u32>,
    on_stack: HashSet<u32>,
    index: HashMap<u32, u32>,
    lowlink: HashMap<u32, u32>,
    result: Vec<Vec<ModuleId>>,
}

impl DependencyGraph {
    /// Create an empty dependency graph.
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            nodes: HashSet::new(),
        }
    }

    /// Register a node in the graph.
    pub fn add_node(&mut self, id: ModuleId) {
        self.nodes.insert(id.0);
        self.edges.entry(id.0).or_default();
        self.reverse_edges.entry(id.0).or_default();
    }

    /// Add a directed edge: `from` depends on `to`.
    pub fn add_edge(&mut self, from: ModuleId, to: ModuleId) {
        self.nodes.insert(from.0);
        self.nodes.insert(to.0);
        self.edges.entry(from.0).or_default().insert(to.0);
        self.reverse_edges.entry(to.0).or_default().insert(from.0);
        // Ensure both ends exist in both maps
        self.edges.entry(to.0).or_default();
        self.reverse_edges.entry(from.0).or_default();
    }

    /// Check whether an edge exists from `from` to `to`.
    pub fn has_edge(&self, from: ModuleId, to: ModuleId) -> bool {
        self.edges
            .get(&from.0)
            .is_some_and(|deps| deps.contains(&to.0))
    }

    /// Topological sort using Kahn's algorithm.
    ///
    /// Returns modules in compilation order (dependencies first).
    /// Returns `CycleError` if the graph contains cycles.
    ///
    /// Edge semantics: `from → to` means `from` depends on `to`,
    /// so `to` must be compiled before `from`. In-degree here counts
    /// how many dependencies a node has (how many modules it imports).
    pub fn topological_sort(&self) -> Result<Vec<ModuleId>, CycleError> {
        // In-degree = number of dependencies (forward edges from this node)
        let mut in_degree: HashMap<u32, usize> = HashMap::new();
        for &node in &self.nodes {
            let dep_count = self.edges.get(&node).map(|s| s.len()).unwrap_or(0);
            in_degree.insert(node, dep_count);
        }

        // Start with nodes that have no dependencies (in-degree 0)
        let mut queue: VecDeque<u32> = VecDeque::new();
        let mut zero_deg: Vec<u32> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&node, _)| node)
            .collect();
        zero_deg.sort_unstable();
        queue.extend(zero_deg);

        let mut result = Vec::new();

        while let Some(node) = queue.pop_front() {
            result.push(ModuleId(node));

            // For each module that depends on `node`, reduce its in-degree
            if let Some(dependents) = self.reverse_edges.get(&node) {
                let mut sorted_deps: Vec<u32> = dependents.iter().copied().collect();
                sorted_deps.sort_unstable();
                for dep in sorted_deps {
                    if let Some(deg) = in_degree.get_mut(&dep) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            // Cycle detected — find it using Tarjan's
            let cycles = self.find_cycles();
            if let Some(cycle) = cycles.into_iter().find(|c| c.len() > 1) {
                return Err(CycleError { cycle });
            }
            // Single-node cycles (self-loops)
            let remaining: Vec<ModuleId> = self
                .nodes
                .iter()
                .filter(|n| !result.iter().any(|r| r.0 == **n))
                .map(|&n| ModuleId(n))
                .collect();
            return Err(CycleError { cycle: remaining });
        }

        Ok(result)
    }

    /// Find strongly connected components using Tarjan's algorithm.
    ///
    /// Returns only components with more than one node (actual cycles).
    pub fn find_cycles(&self) -> Vec<Vec<ModuleId>> {
        let mut state = TarjanState::default();

        // Sort nodes for deterministic output
        let mut sorted_nodes: Vec<u32> = self.nodes.iter().copied().collect();
        sorted_nodes.sort_unstable();

        for node in &sorted_nodes {
            if !state.index.contains_key(node) {
                self.strongconnect(*node, &mut state);
            }
        }

        // Only return components with more than 1 node (actual cycles)
        state.result.retain(|c| c.len() > 1);
        state.result
    }

    /// Tarjan's strongconnect helper.
    fn strongconnect(&self, v: u32, state: &mut TarjanState) {
        state.index.insert(v, state.index_counter);
        state.lowlink.insert(v, state.index_counter);
        state.index_counter += 1;
        state.stack.push(v);
        state.on_stack.insert(v);

        // Consider successors (modules that v depends on)
        if let Some(deps) = self.edges.get(&v) {
            let mut sorted_deps: Vec<u32> = deps.iter().copied().collect();
            sorted_deps.sort_unstable();
            for w in sorted_deps {
                if !state.index.contains_key(&w) {
                    self.strongconnect(w, state);
                    let new_low = state.lowlink[&v].min(state.lowlink[&w]);
                    state.lowlink.insert(v, new_low);
                } else if state.on_stack.contains(&w) {
                    let new_low = state.lowlink[&v].min(state.index[&w]);
                    state.lowlink.insert(v, new_low);
                }
            }
        }

        // If v is a root node, pop the SCC
        if state.lowlink[&v] == state.index[&v] {
            let mut component = Vec::new();
            loop {
                // Tarjan's invariant: v is on the stack, so pop cannot fail.
                let Some(w) = state.stack.pop() else {
                    unreachable!("BUG: Tarjan stack empty while popping SCC containing node {v}");
                };
                state.on_stack.remove(&w);
                component.push(ModuleId(w));
                if w == v {
                    break;
                }
            }
            component.reverse();
            state.result.push(component);
        }
    }

    /// Get direct dependencies of a module (modules it imports).
    pub fn dependencies(&self, id: ModuleId) -> Vec<ModuleId> {
        self.edges
            .get(&id.0)
            .map(|deps| {
                let mut v: Vec<ModuleId> = deps.iter().map(|&d| ModuleId(d)).collect();
                v.sort_by_key(|m| m.0);
                v
            })
            .unwrap_or_default()
    }

    /// Get direct dependents of a module (modules that import it).
    pub fn dependents(&self, id: ModuleId) -> Vec<ModuleId> {
        self.reverse_edges
            .get(&id.0)
            .map(|deps| {
                let mut v: Vec<ModuleId> = deps.iter().map(|&d| ModuleId(d)).collect();
                v.sort_by_key(|m| m.0);
                v
            })
            .unwrap_or_default()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}
