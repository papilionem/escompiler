//! Escape analysis + EDZA zone assignment.
//!
//! Provides intraprocedural escape analysis that determines whether values
//! allocated within a function can escape to the caller or to closures,
//! and assigns zone ids for zone-allocatable values.

pub mod analysis;
pub mod classifier;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

pub use analysis::{EscapeResult, EscapeState, analyze_escapes};
pub use classifier::EscapeClassifier;

/// Zone identifier (lightweight alias to avoid pulling in arena).
pub type ZoneId = u32;

/// Maps each allocatable ValueId to a zone identifier for zone allocation.
pub struct ZoneAssignment {
    pub assignments: HashMap<u32, ZoneId>,
}

/// Assign values to zones based on escape analysis results.
///
/// All `ZoneCandidate` values are assigned to zone 0 (single zone per function
/// for now). Future EDZA+S will split into multiple zones based on lifetime
/// analysis.
pub fn assign_zones(escapes: &EscapeResult) -> ZoneAssignment {
    let mut assignments = HashMap::new();
    let mut next_zone: ZoneId = 0;

    for (&value_id, state) in &escapes.states {
        if *state == EscapeState::ZoneCandidate {
            // For now, assign all zone candidates to a single zone per function.
            // Future: EDZA+S splitting will create multiple zones.
            if next_zone == 0 {
                next_zone = 1;
            }
            assignments.insert(value_id, 0);
        }
    }

    ZoneAssignment { assignments }
}
