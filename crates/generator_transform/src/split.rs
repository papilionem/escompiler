//! Block splitting at yield points and segment identification.
//!
//! When a `Yield`/`Await`/`YieldDelegate` instruction appears in the middle of
//! a basic block (i.e., not as the last instruction before the terminator), the
//! block must be split into two: the original block ending at the yield, and a
//! new "resume" block containing everything after the yield.
//!
//! After splitting, blocks are partitioned into **segments** -- groups of blocks
//! that execute atomically between suspension points. The resume function's
//! dispatch switch jumps to the entry block of each segment.

use std::collections::{HashMap, HashSet};

use ir::BlockId;
use ir::builder::{TypedBasicBlock, TypedFunction};
use ir::types::TypedInstruction;

use crate::TransformError;
use crate::analysis::SuspensionPoint;

// ---------------------------------------------------------------------------
// Segment
// ---------------------------------------------------------------------------

/// A segment is a group of blocks that execute atomically (no suspension within).
///
/// The resume function dispatches to the entry block of the appropriate segment
/// based on the current state index.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Segment index: 0 = initial entry, 1 = after first yield, etc.
    pub index: u32,
    /// The block to jump to when resuming into this segment.
    pub entry_block: BlockId,
    /// Which suspension point ends this segment. `None` for the final segment
    /// (which runs from the last yield to the function end).
    pub suspension_point: Option<u32>,
}

// ---------------------------------------------------------------------------
// Split result
// ---------------------------------------------------------------------------

/// Result of block splitting for a single generator/async function.
///
/// Contains the segments and the modified blocks after splitting at yield points.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The segments into which the function was partitioned.
    pub segments: Vec<Segment>,
    /// The blocks after splitting. This replaces the function's original blocks.
    pub modified_blocks: Vec<TypedBasicBlock>,
}

// ---------------------------------------------------------------------------
// Block splitting
// ---------------------------------------------------------------------------

/// Find the index of a block with the given ID in a block list.
fn find_block_index(blocks: &[TypedBasicBlock], id: BlockId) -> Option<usize> {
    blocks.iter().position(|b| b.id == id)
}

/// Split blocks at yield points and identify segments.
///
/// For each suspension point where the yield is not the last non-terminator
/// instruction, splits the block into two: the original block ending with the
/// yield, and a new resume block containing everything after. Predecessor
/// and successor links are updated accordingly.
///
/// After splitting, identifies segments: contiguous groups of blocks between
/// suspension points.
///
/// # Errors
///
/// Returns [`TransformError`] if a suspension point references a non-existent
/// block or instruction index.
pub fn split_and_identify(
    func: &mut TypedFunction,
    suspension_points: &[SuspensionPoint],
) -> Result<SplitResult, TransformError> {
    let mut blocks = func.blocks.clone();
    let mut next_block_id = func.next_block;

    // Track which new blocks are "resume" blocks created by splitting,
    // keyed by the suspension point index.
    let mut resume_blocks: HashMap<u32, BlockId> = HashMap::new();

    // Process suspension points in reverse order so that instruction indices
    // remain valid as we split earlier blocks.
    let mut sorted_sps: Vec<&SuspensionPoint> = suspension_points.iter().collect();
    sorted_sps.sort_by(|a, b| {
        // Sort by block position first, then by instruction index, both descending
        let a_block_pos = find_block_index(&blocks, a.block_id);
        let b_block_pos = find_block_index(&blocks, b.block_id);
        b_block_pos
            .cmp(&a_block_pos)
            .then(b.instruction_index.cmp(&a.instruction_index))
    });

    for sp in &sorted_sps {
        let block_idx =
            find_block_index(&blocks, sp.block_id).ok_or(TransformError::InvalidBlock {
                index: sp.index,
                block_id: sp.block_id.0,
            })?;

        let block = &blocks[block_idx];
        if sp.instruction_index >= block.instructions.len() {
            return Err(TransformError::InvalidInstructionIndex {
                index: sp.index,
                instr_index: sp.instruction_index,
                block_len: block.instructions.len(),
            });
        }

        // Check if there are instructions after the yield that need to be
        // moved to a new resume block. The yield should ideally be the last
        // instruction before the terminator. If it's not, we need to split.
        let has_instructions_after = sp.instruction_index + 1 < block.instructions.len()
            && !block.instructions[sp.instruction_index + 1]
                .op
                .is_terminator();

        // Also check if the yield IS the last instruction (nothing to split)
        // or if there's just a terminator after it (still need a resume block
        // for the segment tracking).
        let needs_split = has_instructions_after
            || (sp.instruction_index + 1 < block.instructions.len()
                && block.instructions[sp.instruction_index + 1]
                    .op
                    .is_terminator());

        if needs_split && sp.instruction_index + 1 < blocks[block_idx].instructions.len() {
            // Create a new resume block with everything after the yield
            let resume_block_id = BlockId(next_block_id);
            next_block_id += 1;

            // Split: move instructions after the yield to the resume block
            let after_yield: Vec<TypedInstruction> = blocks[block_idx]
                .instructions
                .drain((sp.instruction_index + 1)..)
                .collect();

            let resume_block = TypedBasicBlock {
                id: resume_block_id,
                instructions: after_yield,
                sealed: true,
                predecessors: vec![sp.block_id],
            };

            // Update successor references: any block that was a successor of
            // the original block via the terminator that is now in the resume
            // block needs its predecessor updated.
            update_predecessor_refs(&mut blocks, sp.block_id, resume_block_id, &resume_block);

            // Insert the resume block right after the original block
            blocks.insert(block_idx + 1, resume_block);

            resume_blocks.insert(sp.index, resume_block_id);
        } else {
            // The yield is the last instruction (or only instruction). No split
            // needed, but we still track the resume point. If there's a
            // successor block, that becomes the resume entry point.
            // For segment identification, we'll handle this case below.
        }
    }

    // Update the function's next_block counter
    func.next_block = next_block_id;

    // Identify segments
    let segments = identify_segments_from_blocks(&blocks, suspension_points, &resume_blocks);

    Ok(SplitResult {
        segments,
        modified_blocks: blocks,
    })
}

/// Update predecessor references when a block is split.
///
/// When block A is split into A (before yield) and A_resume (after yield),
/// any block that had A as a predecessor via a branch in the "after" portion
/// needs to be updated to reference A_resume instead.
fn update_predecessor_refs(
    blocks: &mut [TypedBasicBlock],
    original_block: BlockId,
    resume_block: BlockId,
    resume: &TypedBasicBlock,
) {
    // Collect successor block IDs from the resume block's terminator
    let mut successor_ids: HashSet<BlockId> = HashSet::new();
    for inst in &resume.instructions {
        if inst.op.is_terminator() {
            successor_ids.extend(inst.block_targets.iter().copied());
        }
    }

    // Update predecessors of successor blocks
    for block in blocks.iter_mut() {
        if successor_ids.contains(&block.id) {
            // Replace original_block with resume_block in predecessors
            for pred in &mut block.predecessors {
                if *pred == original_block {
                    *pred = resume_block;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Segment identification
// ---------------------------------------------------------------------------

/// Identify segments from the split blocks and suspension points.
///
/// Segment 0: from the entry block through the first yield.
/// Segment N: from the resume point after yield N-1 through yield N (or function end).
///
/// Each segment has an entry_block where the dispatch switch jumps to on resume.
fn identify_segments_from_blocks(
    blocks: &[TypedBasicBlock],
    suspension_points: &[SuspensionPoint],
    resume_blocks: &HashMap<u32, BlockId>,
) -> Vec<Segment> {
    let mut segments = Vec::new();

    if blocks.is_empty() {
        return segments;
    }

    // Segment 0: entry block is the function's first block
    let entry_block = blocks[0].id;
    let first_sp = if suspension_points.is_empty() {
        None
    } else {
        Some(0u32)
    };

    segments.push(Segment {
        index: 0,
        entry_block,
        suspension_point: first_sp,
    });

    // For each suspension point, create a segment starting at the resume block
    for sp in suspension_points {
        let resume_entry = if let Some(&resume_id) = resume_blocks.get(&sp.index) {
            resume_id
        } else {
            // No resume block was created (yield was at end of block).
            // The resume entry is the successor of the yield's block.
            find_successor_block(blocks, sp.block_id).unwrap_or(sp.block_id)
        };

        let next_sp = if (sp.index + 1) < suspension_points.len() as u32 {
            Some(sp.index + 1)
        } else {
            None // Last segment
        };

        segments.push(Segment {
            index: sp.index + 1,
            entry_block: resume_entry,
            suspension_point: next_sp,
        });
    }

    segments
}

/// Find the first successor block of a given block by examining its terminator.
fn find_successor_block(blocks: &[TypedBasicBlock], block_id: BlockId) -> Option<BlockId> {
    let block = blocks.iter().find(|b| b.id == block_id)?;
    for inst in &block.instructions {
        if inst.op.is_terminator() && !inst.block_targets.is_empty() {
            return Some(inst.block_targets[0]);
        }
    }
    None
}

/// Identify segments from a function and its suspension points.
///
/// This is a convenience function that combines block splitting and segment
/// identification. For functions where blocks have already been split, use
/// [`identify_segments_from_blocks`] directly.
pub fn identify_segments(
    func: &TypedFunction,
    suspension_points: &[SuspensionPoint],
) -> Vec<Segment> {
    // When called on an already-split function, there are no resume blocks
    // to track (they've already been created).
    identify_segments_from_blocks(&func.blocks, suspension_points, &HashMap::new())
}
