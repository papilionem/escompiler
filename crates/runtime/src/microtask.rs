//! Internal microtask queue used by the `Runtime` struct.
//!
//! This is the queue owned by the `Runtime`. For the promise-related
//! thread-local microtask queue, see `crate::promise`.

use std::collections::VecDeque;

/// A FIFO queue of microtasks (boxed closures) for the runtime.
pub struct MicrotaskQueue {
    tasks: VecDeque<Box<dyn FnOnce()>>,
}

impl MicrotaskQueue {
    /// Creates a new empty microtask queue.
    pub fn new() -> Self {
        Self {
            tasks: VecDeque::new(),
        }
    }

    /// Enqueues a microtask to be run on the next drain cycle.
    pub fn enqueue(&mut self, task: Box<dyn FnOnce()>) {
        self.tasks.push_back(task);
    }

    /// Run all enqueued microtasks in FIFO order until the queue is empty.
    pub fn drain(&mut self) {
        while let Some(task) = self.tasks.pop_front() {
            task();
        }
    }
}

impl Default for MicrotaskQueue {
    fn default() -> Self {
        Self::new()
    }
}
