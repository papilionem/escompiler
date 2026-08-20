//! Phase timing for `--time-phases` diagnostic output.
//!
//! Records how long each compilation phase takes and optionally
//! reports the results.

use std::time::{Duration, Instant};

/// Recorded timings for all compilation phases.
#[derive(Debug, Clone)]
pub struct PhaseTimings {
    /// Each entry is `(phase_name, duration)`.
    pub phases: Vec<(String, Duration)>,
}

/// Accumulates phase start/end timestamps during compilation.
pub struct PhaseTimer {
    enabled: bool,
    phases: Vec<(String, Duration)>,
    current: Option<(String, Instant)>,
}

impl PhaseTimer {
    /// Create a new phase timer. If `enabled` is false, all operations are no-ops.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phases: Vec::new(),
            current: None,
        }
    }

    /// Start timing a named phase.
    ///
    /// # Panics
    ///
    /// Panics if a phase is already in progress (call `end` first).
    pub fn start(&mut self, name: &str) {
        if !self.enabled {
            return;
        }
        assert!(
            self.current.is_none(),
            "BUG: phase '{}' already in progress when starting '{name}'",
            self.current.as_ref().map_or("?", |(n, _)| n.as_str())
        );
        self.current = Some((name.to_string(), Instant::now()));
    }

    /// End the current phase and record its duration.
    ///
    /// # Panics
    ///
    /// Panics if no phase is currently in progress.
    pub fn end(&mut self, name: &str) {
        if !self.enabled {
            return;
        }
        let Some((current_name, start)) = self.current.take() else {
            panic!("BUG: end('{name}') called with no phase in progress");
        };
        assert_eq!(
            current_name, name,
            "BUG: ending phase '{name}' but '{current_name}' is in progress"
        );
        self.phases.push((current_name, start.elapsed()));
    }

    /// Finish timing and return the collected phase timings.
    ///
    /// Returns `None` if the timer was disabled.
    pub fn finish(self) -> Option<PhaseTimings> {
        if !self.enabled {
            return None;
        }
        Some(PhaseTimings {
            phases: self.phases,
        })
    }
}
