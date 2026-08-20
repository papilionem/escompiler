//! Intraprocedural escape analysis.
//!
//! Walks a `TypedFunction`'s IR instructions, tracking which allocations
//! remain local, which are zone candidates, and which escape the function.

use std::collections::{HashMap, HashSet};

use ir::Op;
use ir::builder::TypedFunction;

use crate::classifier::EscapeClassifier;

/// The escape state of an allocated value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscapeState {
    /// Only used in its defining block, never stored or returned.
    Local,
    /// Does not escape the function; candidate for zone allocation.
    ZoneCandidate,
    /// Must be heap-allocated (escapes the function boundary).
    Escapes,
}

/// Result of escape analysis: maps each allocation's `ValueId.0` to its state.
pub struct EscapeResult {
    pub states: HashMap<u32, EscapeState>,
}

/// Run intraprocedural escape analysis on a typed function.
///
/// Algorithm:
/// 1. Collect all allocation instructions.
/// 2. Build alias map through Phi nodes (value → set of allocations it may reference).
/// 3. Initialize all allocations as `Local`.
/// 4. Track value flow: identify escape points and store relationships.
/// 5. Propagate escapes transitively until fixed point.
/// 6. Promote `Local` → `ZoneCandidate` for values used across blocks.
pub fn analyze_escapes(func: &TypedFunction) -> EscapeResult {
    let mut analyzer = EscapeAnalyzer::new();
    analyzer.run(func);
    EscapeResult {
        states: analyzer.states,
    }
}

struct EscapeAnalyzer {
    /// Escape state for each allocation.
    states: HashMap<u32, EscapeState>,
    /// Set of all allocation ValueIds.
    allocations: HashSet<u32>,
    /// Maps any value → set of allocation ValueIds it may alias.
    /// Non-allocation values that flow from allocations (through Phi, etc.)
    /// alias those allocations.
    aliases: HashMap<u32, HashSet<u32>>,
    /// Maps value_id → set of value_ids that are stored INTO it.
    /// If container escapes, all stored values also escape.
    contained_by: HashMap<u32, HashSet<u32>>,
    /// Which block each value is defined in.
    def_block: HashMap<u32, u32>,
    /// Which blocks each value is used in (only tracked for allocations).
    use_blocks: HashMap<u32, HashSet<u32>>,
}

impl EscapeAnalyzer {
    fn new() -> Self {
        Self {
            states: HashMap::new(),
            allocations: HashSet::new(),
            aliases: HashMap::new(),
            contained_by: HashMap::new(),
            def_block: HashMap::new(),
            use_blocks: HashMap::new(),
        }
    }

    fn run(&mut self, func: &TypedFunction) {
        // Phase 1: Collect allocations, record def blocks, initialize as Local.
        for block in &func.blocks {
            for inst in &block.instructions {
                self.def_block.insert(inst.id.0, block.id.0);
                if EscapeClassifier::is_allocation(&inst.op) {
                    self.allocations.insert(inst.id.0);
                    self.states.insert(inst.id.0, EscapeState::Local);
                    // An allocation aliases itself.
                    self.aliases.entry(inst.id.0).or_default().insert(inst.id.0);
                }
            }
        }

        // Phase 2: Build alias map through Phi nodes.
        // A Phi's result aliases all allocations that any of its operands alias.
        // Iterate until fixed point since Phis can reference other Phis.
        let mut changed = true;
        while changed {
            changed = false;
            for block in &func.blocks {
                for inst in &block.instructions {
                    if inst.op == Op::Phi {
                        let phi_id = inst.id.0;
                        let mut new_aliases = HashSet::new();
                        for &operand in &inst.operands {
                            if let Some(op_aliases) = self.aliases.get(&operand.0) {
                                new_aliases.extend(op_aliases.iter());
                            }
                            // If the operand is itself an allocation, include it.
                            if self.allocations.contains(&operand.0) {
                                new_aliases.insert(operand.0);
                            }
                        }
                        if !new_aliases.is_empty() {
                            let existing = self.aliases.entry(phi_id).or_default();
                            for a in &new_aliases {
                                if existing.insert(*a) {
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Phase 3: Walk instructions, tracking escape points, stores, and use-blocks.
        for block in &func.blocks {
            for inst in &block.instructions {
                // Record use-blocks for operands that alias allocations.
                for &operand in &inst.operands {
                    let alloc_ids = self.resolve_allocations(operand.0);
                    for alloc_id in alloc_ids {
                        self.use_blocks
                            .entry(alloc_id)
                            .or_default()
                            .insert(block.id.0);
                    }
                }

                // Check for escape points.
                if EscapeClassifier::is_escape_point(&inst.op) {
                    self.mark_escape_point_operands(inst);
                }

                // Check for store relationships.
                if EscapeClassifier::is_store(&inst.op) {
                    self.track_store(inst);
                }
            }
        }

        // Phase 4: Transitive propagation until fixed point.
        self.propagate();

        // Phase 5: Promote Local → ZoneCandidate for cross-block usage.
        self.promote_cross_block();
    }

    /// Resolve a value to the set of allocation IDs it may alias.
    fn resolve_allocations(&self, value_id: u32) -> Vec<u32> {
        if let Some(aliases) = self.aliases.get(&value_id) {
            aliases.iter().copied().collect()
        } else if self.allocations.contains(&value_id) {
            vec![value_id]
        } else {
            vec![]
        }
    }

    /// Mark operands of escape-point instructions as Escapes.
    fn mark_escape_point_operands(&mut self, inst: &ir::TypedInstruction) {
        // For Ret and all call-like escape points: mark all operands
        // (and their aliased allocations) as escaped.
        for &operand in &inst.operands {
            self.mark_escaped_transitive(operand.0);
        }
    }

    /// Track containment relationships from store instructions.
    fn track_store(&mut self, inst: &ir::TypedInstruction) {
        match &inst.op {
            Op::StoreField | Op::StoreElement if inst.operands.len() >= 3 => {
                // operands = [obj, field_idx, val]
                let container = inst.operands[0].0;
                let value = inst.operands[2].0;
                self.add_containment(container, value);
            }
            Op::SetProp
            | Op::SetPropStrict
            | Op::SetElem
            | Op::SetPropDynamic
            | Op::SetPropDynamicStrict
            | Op::SetSuper
            | Op::SetPrivate
            | Op::PrivateFieldSet
            | Op::InstallPrivateField
            | Op::ICSetProp
                if inst.operands.len() >= 3 =>
            {
                // operands = [obj, key, val]
                let container = inst.operands[0].0;
                let value = inst.operands[2].0;
                self.add_containment(container, value);
            }
            Op::EnvStore if inst.operands.len() >= 3 => {
                // operands = [env, slot_idx, val]
                let container = inst.operands[0].0;
                let value = inst.operands[2].0;
                self.add_containment(container, value);
            }
            _ => {}
        }
    }

    /// Add containment relationship: value is stored in container.
    /// Resolves aliases for both container and value.
    fn add_containment(&mut self, container: u32, value: u32) {
        let value_allocs = self.resolve_allocations(value);
        let container_allocs = self.resolve_allocations(container);

        if value_allocs.is_empty() {
            return;
        }

        if container_allocs.is_empty() {
            // Container is not an allocation — track raw id for non-allocation containers.
            for val_alloc in &value_allocs {
                self.contained_by
                    .entry(container)
                    .or_default()
                    .insert(*val_alloc);
            }
        } else {
            // Container is (aliased to) allocation(s).
            for cont_alloc in &container_allocs {
                for val_alloc in &value_allocs {
                    self.contained_by
                        .entry(*cont_alloc)
                        .or_default()
                        .insert(*val_alloc);
                }
            }
        }
    }

    /// Propagate escape state transitively through containment relationships.
    fn propagate(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;

            let all_containers: Vec<u32> = self.contained_by.keys().copied().collect();
            for container in all_containers {
                let container_escaped = self
                    .states
                    .get(&container)
                    .is_some_and(|s| *s == EscapeState::Escapes);

                if container_escaped
                    && let Some(values) = self.contained_by.get(&container).cloned()
                {
                    for value in values {
                        if let Some(state) = self.states.get(&value)
                            && *state != EscapeState::Escapes
                        {
                            self.states.insert(value, EscapeState::Escapes);
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    /// Promote `Local` allocations that are used in multiple blocks to
    /// `ZoneCandidate`.
    fn promote_cross_block(&mut self) {
        for &alloc_id in &self.allocations {
            if self.states.get(&alloc_id) == Some(&EscapeState::Local) {
                let def_block = self.def_block.get(&alloc_id);
                if let Some(use_blocks) = self.use_blocks.get(&alloc_id) {
                    let cross_block = use_blocks.iter().any(|b| Some(b) != def_block);
                    if cross_block {
                        self.states.insert(alloc_id, EscapeState::ZoneCandidate);
                    }
                }
            }
        }
    }

    /// Mark a value and all allocations it aliases as escaped.
    fn mark_escaped_transitive(&mut self, value_id: u32) {
        let alloc_ids = self.resolve_allocations(value_id);
        for alloc_id in alloc_ids {
            self.states.insert(alloc_id, EscapeState::Escapes);
        }
    }
}
