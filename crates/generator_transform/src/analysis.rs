//! Suspension point discovery, live variable analysis, and slot assignment.
//!
//! This module provides the analysis phase of the generator/async transform:
//!
//! 1. **Suspension point discovery** — scan all blocks for `Yield`/`Await`/`YieldDelegate`
//! 2. **Live variable analysis** — backward dataflow to find values live across yields
//! 3. **Slot assignment** — map each cross-yield live variable to a state struct slot

use std::collections::{HashMap, HashSet};

use ir::builder::TypedFunction;
use ir::{BlockId, Op, ValueId};

// ---------------------------------------------------------------------------
// Suspension point
// ---------------------------------------------------------------------------

/// A suspension point (yield or await) discovered in a generator/async function.
///
/// Each suspension point corresponds to a `Yield`, `Await`, or `YieldDelegate`
/// instruction. The transform will split the function at these points into
/// segments that execute atomically.
#[derive(Debug, Clone)]
pub struct SuspensionPoint {
    /// Sequential index (0, 1, 2, ...) assigned during discovery.
    pub index: u32,
    /// The block containing this suspension instruction.
    pub block_id: BlockId,
    /// Position of the instruction within the block's instruction list.
    pub instruction_index: usize,
    /// The opcode (`Yield`, `Await`, or `YieldDelegate`).
    pub op: Op,
    /// The value being yielded/awaited (first operand), if any.
    pub yield_value: Option<ValueId>,
}

// ---------------------------------------------------------------------------
// Liveness result
// ---------------------------------------------------------------------------

/// Result of live variable analysis for a single generator/async function.
///
/// Contains the discovered suspension points, the set of variables live across
/// each suspension point, and the mapping from variables to state struct slots.
#[derive(Debug, Clone)]
pub struct LivenessResult {
    /// All suspension points in the function, sorted by block order.
    pub suspension_points: Vec<SuspensionPoint>,
    /// For each suspension point index: the set of values that are live across it
    /// (defined before and used after).
    pub live_across: HashMap<u32, HashSet<ValueId>>,
    /// Mapping from each cross-yield live variable to its slot index in the
    /// state struct. Populated by [`assign_slots`].
    pub slot_assignment: HashMap<ValueId, u32>,
    /// Total number of slots needed in the state struct.
    pub num_slots: u32,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Returns `true` if the opcode is a suspension point (`Yield`, `Await`, or `YieldDelegate`).
fn is_suspension_op(op: &Op) -> bool {
    matches!(op, Op::Yield | Op::Await | Op::YieldDelegate)
}

/// Scan all blocks in a function to discover suspension points.
///
/// Returns a list of [`SuspensionPoint`]s sorted by block order (the order
/// blocks appear in the function), with sequential indices starting from 0.
pub fn discover_suspension_points(func: &TypedFunction) -> Vec<SuspensionPoint> {
    let mut points = Vec::new();
    let mut index = 0u32;

    for block in &func.blocks {
        for (instr_idx, instr) in block.instructions.iter().enumerate() {
            if is_suspension_op(&instr.op) {
                let yield_value = instr.operands.first().copied();
                points.push(SuspensionPoint {
                    index,
                    block_id: block.id,
                    instruction_index: instr_idx,
                    op: instr.op.clone(),
                    yield_value,
                });
                index += 1;
            }
        }
    }

    points
}

// ---------------------------------------------------------------------------
// Live variable analysis
// ---------------------------------------------------------------------------

/// Build a successor map from the function's blocks.
///
/// For each block, collects the set of successor blocks by examining
/// branch targets in terminator instructions.
fn build_successor_map(func: &TypedFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut successors: HashMap<BlockId, Vec<BlockId>> = HashMap::new();

    for block in &func.blocks {
        let mut succs = Vec::new();
        for inst in &block.instructions {
            if inst.op.is_terminator() {
                succs.extend_from_slice(&inst.block_targets);
            }
        }
        successors.insert(block.id, succs);
    }

    successors
}

/// Build a predecessor map from a successor map.
fn build_predecessor_map(
    func: &TypedFunction,
    successors: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut predecessors: HashMap<BlockId, Vec<BlockId>> = HashMap::new();

    // Initialize all blocks
    for block in &func.blocks {
        predecessors.entry(block.id).or_default();
    }

    for (&block_id, succs) in successors {
        for &succ in succs {
            predecessors.entry(succ).or_default().push(block_id);
        }
    }

    predecessors
}

/// Perform backward dataflow liveness analysis on a function.
///
/// For each suspension point S, computes `live_across(S)`: the set of values
/// that are defined before S and used after S. These values must be saved to
/// the state struct before yielding and loaded after resuming.
///
/// Uses standard iterative backward dataflow:
/// - `use[B]` = values used in block B (before any local def)
/// - `def[B]` = values defined in block B
/// - `live_in[B]` = use[B] U (live_out[B] - def[B])
/// - `live_out[B]` = U { live_in[S] : S in successors(B) }
pub fn analyze_liveness(
    func: &TypedFunction,
    suspension_points: &[SuspensionPoint],
) -> LivenessResult {
    let successors = build_successor_map(func);
    let _predecessors = build_predecessor_map(func, &successors);

    // Compute use and def sets for each block
    let mut block_use: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut block_def: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();

    for block in &func.blocks {
        // Compute use-before-def for this block
        let mut uses = HashSet::new();
        let mut defs = HashSet::new();
        for inst in &block.instructions {
            // Uses: operands that are NOT defined earlier in this block
            for &operand in &inst.operands {
                if !defs.contains(&operand) {
                    uses.insert(operand);
                }
            }
            // Def: the value produced by this instruction
            defs.insert(inst.id);
        }
        block_use.insert(block.id, uses);
        block_def.insert(block.id, defs);
    }

    // Iterative backward dataflow: compute live_in and live_out for each block
    let mut live_in: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();

    for block in &func.blocks {
        live_in.insert(block.id, HashSet::new());
        live_out.insert(block.id, HashSet::new());
    }

    let mut changed = true;
    while changed {
        changed = false;

        // Process blocks in reverse order for faster convergence
        for block in func.blocks.iter().rev() {
            let bid = block.id;

            // live_out[B] = U { live_in[S] : S in successors(B) }
            let mut new_live_out = HashSet::new();
            if let Some(succs) = successors.get(&bid) {
                for &succ in succs {
                    if let Some(succ_live_in) = live_in.get(&succ) {
                        new_live_out.extend(succ_live_in.iter().copied());
                    }
                }
            }

            // live_in[B] = use[B] U (live_out[B] - def[B])
            let uses = block_use.get(&bid).cloned().unwrap_or_default();
            let defs = block_def.get(&bid).cloned().unwrap_or_default();
            let mut new_live_in: HashSet<ValueId> =
                new_live_out.difference(&defs).copied().collect();
            new_live_in.extend(uses.iter().copied());

            if new_live_in != *live_in.get(&bid).unwrap_or(&HashSet::new()) {
                live_in.insert(bid, new_live_in);
                changed = true;
            }
            if new_live_out != *live_out.get(&bid).unwrap_or(&HashSet::new()) {
                live_out.insert(bid, new_live_out);
                changed = true;
            }
        }
    }

    // For each suspension point, compute live_across:
    // A value is live across S if:
    //   1. It is in live_out of S's block (live after the block) AND defined
    //      before or at S in the block (or in a dominating block), OR
    //   2. It is defined before S in the same block AND used after S in the
    //      same block or in live_out.
    //
    // More precisely: split the block at the suspension point into "before"
    // and "after" portions, then compute what crosses.
    let mut live_across: HashMap<u32, HashSet<ValueId>> = HashMap::new();

    for sp in suspension_points {
        let block = match func.blocks.iter().find(|b| b.id == sp.block_id) {
            Some(b) => b,
            None => continue,
        };

        // Values defined BEFORE the suspension point (including in other blocks)
        let mut defined_before: HashSet<ValueId> = HashSet::new();

        // All values defined in blocks other than this one are "before" if they
        // dominate this point. In SSA, if a value is used here, it was defined
        // before. So we use a simpler approach: values used after S that are
        // NOT defined after S must have been defined before S.

        // Values defined after the suspension point in this block
        let mut defined_after: HashSet<ValueId> = HashSet::new();
        for (idx, inst) in block.instructions.iter().enumerate() {
            if idx > sp.instruction_index {
                defined_after.insert(inst.id);
            } else {
                defined_before.insert(inst.id);
            }
        }

        // Values used after S in this block
        let mut used_after: HashSet<ValueId> = HashSet::new();
        for (idx, inst) in block.instructions.iter().enumerate() {
            if idx > sp.instruction_index {
                for &operand in &inst.operands {
                    used_after.insert(operand);
                }
            }
        }

        // Values live after S = used_after_in_block + live_out[block]
        let block_live_out = live_out.get(&sp.block_id).cloned().unwrap_or_default();
        let mut live_after_s: HashSet<ValueId> = used_after;
        live_after_s.extend(block_live_out.iter().copied());

        // A value is live across S if:
        // - It is in live_after_s AND
        // - It is NOT defined after S in the same block (i.e., it was defined before S)
        let crossing: HashSet<ValueId> = live_after_s
            .into_iter()
            .filter(|v| !defined_after.contains(v))
            // Also filter out the suspension instruction's own result
            // (the yield result is not yet available; it comes from sent_value on resume)
            .filter(|v| {
                // Don't include the suspension point's own produced value
                block.instructions[sp.instruction_index].id != *v
            })
            .collect();

        live_across.insert(sp.index, crossing);
    }

    LivenessResult {
        suspension_points: suspension_points.to_vec(),
        live_across,
        slot_assignment: HashMap::new(),
        num_slots: 0,
    }
}

// ---------------------------------------------------------------------------
// Slot assignment
// ---------------------------------------------------------------------------

/// Assign state struct slots to cross-yield live variables.
///
/// Simple v1 strategy: each unique live variable that appears in any
/// `live_across` set gets its own dedicated slot. Variables with
/// non-overlapping live ranges could share slots (graph coloring),
/// but that optimization is deferred.
pub fn assign_slots(liveness: &mut LivenessResult) {
    let mut all_live_vars: HashSet<ValueId> = HashSet::new();

    for vars in liveness.live_across.values() {
        all_live_vars.extend(vars.iter().copied());
    }

    // Sort for deterministic slot assignment
    let mut sorted_vars: Vec<ValueId> = all_live_vars.into_iter().collect();
    sorted_vars.sort_by_key(|v| v.0);

    let mut assignment = HashMap::new();
    for (slot, &var) in sorted_vars.iter().enumerate() {
        assignment.insert(var, slot as u32);
    }

    liveness.num_slots = sorted_vars.len() as u32;
    liveness.slot_assignment = assignment;
}
