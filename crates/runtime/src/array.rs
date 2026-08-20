//! JsArray: dense elements + length tracking.

use nanbox::JsValue;

/// A dense JavaScript array backed by a `Vec<JsValue>`.
#[derive(Debug)]
pub struct JsArray {
    /// Dense element storage.
    pub elements: Vec<JsValue>,
    /// The ECMAScript `.length` property, which may exceed `elements.len()`
    /// if holes exist at the tail.
    pub length: u32,
}

impl JsArray {
    /// Creates a new empty array.
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            length: 0,
        }
    }

    /// Creates a new array with pre-allocated capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            elements: Vec::with_capacity(cap),
            length: 0,
        }
    }

    /// Pushes a value onto the end of the array.
    pub fn push(&mut self, val: JsValue) {
        self.elements.push(val);
        self.length = self.elements.len() as u32;
    }

    /// Pops the last element from the array.
    pub fn pop(&mut self) -> Option<JsValue> {
        let val = self.elements.pop();
        self.length = self.elements.len() as u32;
        val
    }

    /// Gets the element at the given index, or `None` if out of bounds.
    pub fn get(&self, index: u32) -> Option<JsValue> {
        self.elements.get(index as usize).copied()
    }

    /// Sets the element at the given index. If the index is beyond current
    /// length, the array is extended with `undefined` to fill holes.
    pub fn set(&mut self, index: u32, val: JsValue) {
        let idx = index as usize;
        if idx >= self.elements.len() {
            self.elements.resize(idx + 1, JsValue::undefined());
        }
        self.elements[idx] = val;
        self.length = self.elements.len() as u32;
    }

    /// Returns the current length of the array.
    pub fn len(&self) -> u32 {
        self.length
    }

    /// Returns `true` if the array has no elements.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Sets the length of the array.
    ///
    /// If `new_len` is less than the current element count the elements
    /// vector is truncated. If greater, the length field is updated but
    /// no new elements are inserted (sparse tail).
    pub fn set_length(&mut self, new_len: u32) {
        let new_len_usize = new_len as usize;
        if new_len_usize < self.elements.len() {
            self.elements.truncate(new_len_usize);
        }
        self.length = new_len;
    }
}

impl Default for JsArray {
    fn default() -> Self {
        Self::new()
    }
}
