//! Timer management for `setTimeout`/`setInterval`/`clearTimeout`.
//!
//! Uses a thread-local timer registry that stores callback function values
//! and their delay/repeat configuration. The event loop drains timers
//! when their deadlines expire.

use std::cell::RefCell;

/// A registered timer entry.
struct TimerEntry {
    /// Unique timer ID.
    id: u32,
    /// The callback function (NaN-boxed value).
    func: u64,
    /// Delay in milliseconds.
    _ms: u32,
    /// Whether this is a repeating interval.
    repeating: bool,
    /// Whether this timer has been cancelled.
    cancelled: bool,
}

/// Thread-local timer state.
struct TimerState {
    /// Registered timers.
    timers: Vec<TimerEntry>,
    /// Next timer ID to assign.
    next_id: u32,
}

impl TimerState {
    fn new() -> Self {
        Self {
            timers: Vec::new(),
            next_id: 1,
        }
    }
}

thread_local! {
    static TIMER_STATE: RefCell<TimerState> = RefCell::new(TimerState::new());
}

/// Registers a one-shot timer (setTimeout). Returns the timer ID.
pub fn set_timeout(func: u64, ms: u32) -> u32 {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        let id = s.next_id;
        s.next_id += 1;
        s.timers.push(TimerEntry {
            id,
            func,
            _ms: ms,
            repeating: false,
            cancelled: false,
        });
        id
    })
}

/// Registers a repeating timer (setInterval). Returns the timer ID.
pub fn set_interval(func: u64, ms: u32) -> u32 {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        let id = s.next_id;
        s.next_id += 1;
        s.timers.push(TimerEntry {
            id,
            func,
            _ms: ms,
            repeating: true,
            cancelled: false,
        });
        id
    })
}

/// Cancels a timer by ID (clearTimeout / clearInterval).
pub fn clear_timeout(id: u32) {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        if let Some(entry) = s.timers.iter_mut().find(|t| t.id == id) {
            entry.cancelled = true;
        }
    });
}

/// Returns the number of non-cancelled timers (for testing).
pub fn active_timer_count() -> usize {
    TIMER_STATE.with(|state| {
        let s = state.borrow();
        s.timers.iter().filter(|t| !t.cancelled).count()
    })
}

/// Returns whether the timer with the given ID is a repeating interval.
pub fn is_interval(id: u32) -> bool {
    TIMER_STATE.with(|state| {
        let s = state.borrow();
        s.timers
            .iter()
            .find(|t| t.id == id)
            .is_some_and(|t| t.repeating)
    })
}

/// Returns the callback function value for a timer (for testing).
pub fn get_timer_func(id: u32) -> Option<u64> {
    TIMER_STATE.with(|state| {
        let s = state.borrow();
        s.timers
            .iter()
            .find(|t| t.id == id && !t.cancelled)
            .map(|t| t.func)
    })
}

/// Resets all timer state (for testing).
pub fn reset() {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.timers.clear();
        s.next_id = 1;
    });
}
