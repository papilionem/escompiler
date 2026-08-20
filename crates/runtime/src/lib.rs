//! # runtime — JavaScript Runtime Object Model
//!
//! This crate implements the core runtime for compiled JavaScript programs.
//! It provides the object model, property access, prototype chains, and
//! memory management needed to execute JS semantics at native speed.
//!
//! ## Key Modules
//!
//! - [`object`] — `JsObject` (header + property storage + shape + prototype)
//! - [`property`] — property get/set/has/delete with descriptor support
//! - [`prototype`] — prototype chain walking and inheritance
//! - `array` — `JsArray` with dense element storage
//! - [`function`] — `JsFunction` (closures, native functions, bound functions)
//! - [`allocator`] — dual-world allocation dispatch (zone vs heap)
//! - [`write_barrier`] — cross-zone reference count barriers
//! - [`environment`] — closure environment (scope chain)
//! - [`esc_environment`] — dynamic environment for `with`/`eval` poisoned functions
//! - [`generator`] — ES2015 generator objects (`function*`/`yield`)
//! - [`promise`] — Promise state machine and resolution
//! - [`async_generator`] — async generator protocol (`async function*`/`yield`)
//! - [`async_wrap`] — async function wrapper (drives generator via Promise)
//! - [`internal_data`] — unified object internal data types (InternalKind, InternalData, UnifiedObject)
//! - [`iterator`] — ES2015 iterator protocol
//! - [`iterator_helpers`] — ES2025 Iterator Helpers (map, filter, take, drop, flatMap, reduce, etc.)
//! - [`async_iterator_helpers`] — Async Iterator Helpers (async map, filter, take, drop, flatMap, forEach, etc.)
//! - [`ic`] — inline cache infrastructure for fast property access
//! - [`jsbox`] — heap-allocated variable cell for closure capture-by-reference
//! - [`proxy`] — ES2015 Proxy with all 13 handler traps
//! - [`exceptions`] — error object creation (TypeError, RangeError, etc.)
//! - [`rt_api`] — `#[no_mangle] extern "C"` entry points called by compiled code
//! - [`value_ops`] — JS value operations (typeof, coercions)
//! - [`string_ops`] — string concatenation, comparison, conversion
//! - [`symbol`] — JavaScript Symbol primitive type (unique IDs, registry, descriptions)
//! - [`display`] — value formatting for console output
//! - [`builtin_builder`] — fluent API for registering builtin constructor methods
//! - [`builtins`] — built-in constructor and method registration
//! - [`tagged_obj`] — object tagging for runtime type dispatch
//! - [`regexp_bridge`] — bridge to `regexp` for RegExp operations
//! - [`cycle_integration`] — bridge to `cycles` for cycle collection
//! - [`microtask`] — microtask queue (Promise reactions, queueMicrotask)
//! - [`timer`] — timer-based scheduling
//!
//! ## Top-Level Type
//!
//! [`Runtime`] owns the allocator and microtask queue. Create one at
//! program startup and use it throughout execution.

pub mod allocator;
pub mod array;
pub mod async_generator;
pub mod async_iterator_helpers;
pub mod async_wrap;
pub mod builtin_builder;
pub mod builtins;
pub mod cycle_integration;
pub mod display;
pub mod environment;
pub mod esc_environment;
pub mod exceptions;
pub mod function;
pub mod generator;
pub mod heap_obj;
pub mod ic;
pub mod internal_data;
pub mod iterator;
pub mod iterator_helpers;
pub mod jsbox;
pub mod microtask;
pub mod object;
pub mod promise;
pub mod property;
pub mod prototype;
pub mod proxy;
pub mod regexp_bridge;
pub mod rt_api;
pub mod string_ops;
pub mod symbol;
pub mod tagged_obj;
pub mod timer;
pub mod value_ops;
pub mod write_barrier;

#[cfg(test)]
mod tests;

use allocator::Allocator;
use microtask::MicrotaskQueue;

/// The top-level runtime, owning the allocator and microtask queue.
pub struct Runtime {
    pub allocator: Allocator,
    microtasks: MicrotaskQueue,
}

impl Runtime {
    /// Creates a new runtime.
    ///
    /// If `heap_only` is true, zone allocation is disabled (useful for
    /// differential testing against a pure-RC baseline).
    pub fn new(heap_only: bool) -> Self {
        Self {
            allocator: Allocator::new(heap_only),
            microtasks: MicrotaskQueue::new(),
        }
    }

    /// Drains the microtask queue, running all enqueued tasks in FIFO order.
    pub fn run_microtasks(&mut self) {
        self.microtasks.drain();
    }

    /// Enqueues a microtask to be run on the next `run_microtasks` call.
    pub fn enqueue_microtask(&mut self, task: Box<dyn FnOnce()>) {
        self.microtasks.enqueue(task);
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(false)
    }
}
