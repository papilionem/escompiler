//! Exception handling support for compiled JavaScript.
//!
//! Provides the completion record types used to track try/catch/finally control
//! flow and error propagation in compiled JS code.
//!
//! Key types:
//! - [`CompletionRecord`] — the result of evaluating a statement (normal, throw, return, etc.)
//! - [`CompletionType`] — discriminant for the completion kind
//! - [`ErrorObject`] — a structured JavaScript error with kind, message, and stack trace

use nanbox::JsValue;

// Exception handling strategy: LLVM invoke inside try blocks, call elsewhere.
// See design doc 04 for full details.

/// The kind of completion produced by evaluating a statement or expression.
///
/// Maps to the ECMAScript completion record specification types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionType {
    /// The statement completed normally without any control transfer.
    Normal,
    /// An exception was thrown.
    Throw,
    /// A `return` statement was encountered.
    Return,
    /// A `break` statement was encountered.
    Break,
    /// A `continue` statement was encountered.
    Continue,
}

/// The kind of JavaScript error, corresponding to the built-in error constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Generic `Error`.
    Error,
    /// `TypeError` — a value is not the expected type.
    TypeError,
    /// `ReferenceError` — an invalid reference was detected.
    ReferenceError,
    /// `SyntaxError` — a parsing error in `eval` or `new Function`.
    SyntaxError,
    /// `RangeError` — a numeric value is out of its valid range.
    RangeError,
    /// `URIError` — an invalid URI was passed to `encodeURI`/`decodeURI`.
    URIError,
    /// `EvalError` — legacy error related to `eval()`.
    EvalError,
    /// Internal compiler error, not part of the ECMAScript spec.
    InternalError,
}

/// A single frame in a JavaScript error's stack trace.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// The name of the function at this stack frame (or `"<anonymous>"`).
    pub function_name: String,
    /// The source file path or URL.
    pub file: String,
    /// One-based line number within the source file.
    pub line: u32,
    /// Zero-based column offset within the line.
    pub column: u32,
}

/// A structured JavaScript error with kind, message, stack trace, and optional cause.
#[derive(Debug, Clone)]
pub struct ErrorObject {
    /// Which built-in error constructor this error corresponds to.
    pub kind: ErrorKind,
    /// The human-readable error message string.
    pub message: String,
    /// The captured stack trace frames, from innermost to outermost.
    pub stack: Vec<StackFrame>,
    /// An optional chained cause (from `new Error("msg", { cause })` syntax).
    pub cause: Option<Box<ErrorObject>>,
}

/// The value carried by a throw: either a structured error object or an
/// arbitrary NaN-boxed JavaScript value (e.g., `throw 42`).
#[derive(Debug, Clone)]
pub enum ThrowValue {
    /// A structured error object (from `throw new Error(...)` etc.).
    Error(ErrorObject),
    /// Raw NaN-boxed bits for non-Error throw values (e.g., `throw "oops"`).
    Value(u64),
}

/// The result of evaluating a statement: a type tag plus an optional value.
///
/// Normal completions carry the statement's value; abrupt completions
/// (throw, return, break, continue) carry the thrown/returned value.
#[derive(Debug, Clone)]
pub struct CompletionRecord {
    /// The kind of completion (normal, throw, return, break, or continue).
    pub ty: CompletionType,
    /// The associated value, if any.
    pub value: Option<JsValue>,
}

impl CompletionRecord {
    /// Creates a Normal completion carrying the given value.
    pub fn normal(val: JsValue) -> Self {
        Self {
            ty: CompletionType::Normal,
            value: Some(val),
        }
    }

    /// Creates a Throw completion carrying the given value.
    pub fn throw(val: JsValue) -> Self {
        Self {
            ty: CompletionType::Throw,
            value: Some(val),
        }
    }

    /// Creates an empty Normal completion with no value.
    pub fn empty() -> Self {
        Self {
            ty: CompletionType::Normal,
            value: None,
        }
    }

    /// Returns true if this completion is abrupt (not Normal).
    pub fn is_abrupt(&self) -> bool {
        self.ty != CompletionType::Normal
    }

    /// Returns true if this is a Throw completion.
    pub fn is_throw(&self) -> bool {
        self.ty == CompletionType::Throw
    }

    /// Returns true if this is a Return completion.
    pub fn is_return(&self) -> bool {
        self.ty == CompletionType::Return
    }

    /// Extracts the value from a Normal completion, returning None for abrupt completions.
    pub fn unwrap_value(self) -> Option<JsValue> {
        if self.ty == CompletionType::Normal {
            self.value
        } else {
            None
        }
    }

    /// Creates a Throw completion with a TypeError.
    pub fn type_error(message: impl Into<String>) -> Self {
        let _ = ErrorObject {
            kind: ErrorKind::TypeError,
            message: message.into(),
            stack: Vec::new(),
            cause: None,
        };
        Self {
            ty: CompletionType::Throw,
            value: None,
        }
    }

    /// Creates a Throw completion with a ReferenceError.
    pub fn reference_error(message: impl Into<String>) -> Self {
        let _ = ErrorObject {
            kind: ErrorKind::ReferenceError,
            message: message.into(),
            stack: Vec::new(),
            cause: None,
        };
        Self {
            ty: CompletionType::Throw,
            value: None,
        }
    }

    /// Creates a Throw completion with a SyntaxError.
    pub fn syntax_error(message: impl Into<String>) -> Self {
        let _ = ErrorObject {
            kind: ErrorKind::SyntaxError,
            message: message.into(),
            stack: Vec::new(),
            cause: None,
        };
        Self {
            ty: CompletionType::Throw,
            value: None,
        }
    }

    /// Creates a Throw completion with a RangeError.
    pub fn range_error(message: impl Into<String>) -> Self {
        let _ = ErrorObject {
            kind: ErrorKind::RangeError,
            message: message.into(),
            stack: Vec::new(),
            cause: None,
        };
        Self {
            ty: CompletionType::Throw,
            value: None,
        }
    }
}

/// Maps to IR basic blocks for structured exception handling.
///
/// `try_block` is the entry block ID; `catch_block` and `finally_block` are
/// optional landing pads. The LLVM backend emits `invoke` for calls inside
/// try blocks and plain `call` elsewhere.
#[derive(Debug, Clone)]
pub struct TryCatchBlock {
    /// Block ID of the try body entry.
    pub try_block: u32,
    /// Block ID of the catch handler, if present.
    pub catch_block: Option<u32>,
    /// Block ID of the finally block, if present.
    pub finally_block: Option<u32>,
}

/// A single entry in the exception table mapping a try-block range to its handler.
#[derive(Debug, Clone)]
pub struct ExceptionEntry {
    /// First block ID in the protected range (inclusive).
    pub try_start: u32,
    /// Block ID past the end of the protected range (exclusive).
    pub try_end: u32,
    /// Block ID of the catch handler.
    pub handler: u32,
    /// Block ID of the finally block, if present.
    pub finally: Option<u32>,
}

/// Lookup table mapping block ranges to exception handlers.
///
/// Used by the backend to determine which catch/finally block handles
/// an exception thrown from a given block.
#[derive(Debug, Clone, Default)]
pub struct ExceptionTable {
    entries: Vec<ExceptionEntry>,
}

impl ExceptionTable {
    /// Create an empty exception table.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a new exception handler entry.
    pub fn add_entry(&mut self, entry: ExceptionEntry) {
        self.entries.push(entry);
    }

    /// Finds a handler for the given block ID.
    /// Returns the first entry whose range [try_start, try_end) contains the block.
    pub fn find_handler(&self, block: u32) -> Option<&ExceptionEntry> {
        self.entries
            .iter()
            .find(|e| block >= e.try_start && block < e.try_end)
    }

    /// Returns the number of entries in the exception table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the exception table contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_record_empty_is_normal() {
        let cr = CompletionRecord::empty();
        assert_eq!(cr.ty, CompletionType::Normal);
        assert!(cr.value.is_none());
        assert!(!cr.is_abrupt());
    }

    #[test]
    fn completion_record_normal_with_value() {
        let cr = CompletionRecord::normal(JsValue::int(42));
        assert_eq!(cr.ty, CompletionType::Normal);
        assert!(!cr.is_abrupt());
        assert!(!cr.is_throw());
        assert!(!cr.is_return());
    }

    #[test]
    fn is_abrupt_for_throw() {
        let cr = CompletionRecord::throw(JsValue::undefined());
        assert!(cr.is_abrupt());
        assert!(cr.is_throw());
        assert!(!cr.is_return());
    }

    #[test]
    fn is_abrupt_for_return_break_continue() {
        let ret = CompletionRecord {
            ty: CompletionType::Return,
            value: Some(JsValue::int(1)),
        };
        assert!(ret.is_abrupt());
        assert!(ret.is_return());
        assert!(!ret.is_throw());

        let brk = CompletionRecord {
            ty: CompletionType::Break,
            value: None,
        };
        assert!(brk.is_abrupt());

        let cont = CompletionRecord {
            ty: CompletionType::Continue,
            value: None,
        };
        assert!(cont.is_abrupt());
    }

    #[test]
    fn type_error_creates_throw() {
        let cr = CompletionRecord::type_error("not a function");
        assert_eq!(cr.ty, CompletionType::Throw);
        assert!(cr.is_abrupt());
        assert!(cr.is_throw());
    }

    #[test]
    fn reference_error_creates_throw() {
        let cr = CompletionRecord::reference_error("x is not defined");
        assert_eq!(cr.ty, CompletionType::Throw);
        assert!(cr.is_throw());
    }

    #[test]
    fn syntax_error_creates_throw() {
        let cr = CompletionRecord::syntax_error("unexpected token");
        assert_eq!(cr.ty, CompletionType::Throw);
        assert!(cr.is_throw());
    }

    #[test]
    fn range_error_creates_throw() {
        let cr = CompletionRecord::range_error("invalid array length");
        assert_eq!(cr.ty, CompletionType::Throw);
        assert!(cr.is_throw());
    }

    #[test]
    fn unwrap_value_on_normal() {
        let cr = CompletionRecord::normal(JsValue::int(7));
        let val = cr.unwrap_value();
        assert!(val.is_some());
        assert_eq!(val.unwrap().as_int(), Some(7));
    }

    #[test]
    fn unwrap_value_on_throw_returns_none() {
        let cr = CompletionRecord::throw(JsValue::int(7));
        assert!(cr.unwrap_value().is_none());
    }

    #[test]
    fn error_object_creation() {
        let err = ErrorObject {
            kind: ErrorKind::TypeError,
            message: "x is not a function".to_string(),
            stack: vec![StackFrame {
                function_name: "main".to_string(),
                file: "test.js".to_string(),
                line: 10,
                column: 5,
            }],
            cause: None,
        };
        assert_eq!(err.kind, ErrorKind::TypeError);
        assert_eq!(err.message, "x is not a function");
        assert_eq!(err.stack.len(), 1);
        assert_eq!(err.stack[0].function_name, "main");
        assert_eq!(err.stack[0].line, 10);
    }

    #[test]
    fn error_object_with_cause() {
        let cause = ErrorObject {
            kind: ErrorKind::Error,
            message: "original error".to_string(),
            stack: Vec::new(),
            cause: None,
        };
        let err = ErrorObject {
            kind: ErrorKind::TypeError,
            message: "wrapper".to_string(),
            stack: Vec::new(),
            cause: Some(Box::new(cause)),
        };
        assert!(err.cause.is_some());
        assert_eq!(err.cause.as_ref().unwrap().kind, ErrorKind::Error);
    }

    #[test]
    fn throw_value_error_variant() {
        let err = ErrorObject {
            kind: ErrorKind::RangeError,
            message: "out of range".to_string(),
            stack: Vec::new(),
            cause: None,
        };
        let tv = ThrowValue::Error(err);
        match &tv {
            ThrowValue::Error(e) => assert_eq!(e.kind, ErrorKind::RangeError),
            ThrowValue::Value(_) => panic!("expected Error variant"),
        }
    }

    #[test]
    fn throw_value_raw_bits() {
        let val = JsValue::int(42);
        let tv = ThrowValue::Value(val.raw_bits());
        match tv {
            ThrowValue::Value(bits) => {
                let recovered = JsValue::from_raw_bits(bits);
                assert_eq!(recovered.as_int(), Some(42));
            }
            ThrowValue::Error(_) => panic!("expected Value variant"),
        }
    }

    #[test]
    fn exception_table_add_and_find() {
        let mut table = ExceptionTable::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);

        table.add_entry(ExceptionEntry {
            try_start: 5,
            try_end: 10,
            handler: 11,
            finally: Some(15),
        });

        assert!(!table.is_empty());
        assert_eq!(table.len(), 1);

        // Block inside the try range
        let entry = table.find_handler(7).unwrap();
        assert_eq!(entry.handler, 11);
        assert_eq!(entry.finally, Some(15));
    }

    #[test]
    fn exception_table_find_returns_none_for_unhandled() {
        let mut table = ExceptionTable::new();
        table.add_entry(ExceptionEntry {
            try_start: 5,
            try_end: 10,
            handler: 11,
            finally: None,
        });

        // Block outside the range
        assert!(table.find_handler(0).is_none());
        assert!(table.find_handler(4).is_none());
        assert!(table.find_handler(10).is_none()); // end is exclusive
        assert!(table.find_handler(20).is_none());
    }

    #[test]
    fn stack_frame_fields() {
        let frame = StackFrame {
            function_name: "doSomething".to_string(),
            file: "app.js".to_string(),
            line: 42,
            column: 12,
        };
        assert_eq!(frame.function_name, "doSomething");
        assert_eq!(frame.file, "app.js");
        assert_eq!(frame.line, 42);
        assert_eq!(frame.column, 12);
    }

    #[test]
    fn error_kind_all_variants() {
        let kinds = [
            ErrorKind::Error,
            ErrorKind::TypeError,
            ErrorKind::ReferenceError,
            ErrorKind::SyntaxError,
            ErrorKind::RangeError,
            ErrorKind::URIError,
            ErrorKind::EvalError,
            ErrorKind::InternalError,
        ];
        // All variants should be distinct
        for i in 0..kinds.len() {
            for j in (i + 1)..kinds.len() {
                assert_ne!(kinds[i], kinds[j]);
            }
        }
    }

    // --- ExceptionTable nested/overlapping tests ---

    #[test]
    fn test_exception_table_nested_try_blocks() {
        let mut table = ExceptionTable::new();
        // Outer try: blocks 0..20, handler at 20
        table.add_entry(ExceptionEntry {
            try_start: 0,
            try_end: 20,
            handler: 20,
            finally: Some(25),
        });
        // Inner try: blocks 5..10, handler at 10
        table.add_entry(ExceptionEntry {
            try_start: 5,
            try_end: 10,
            handler: 10,
            finally: None,
        });
        assert_eq!(table.len(), 2);

        // Block 7 is inside both ranges; find_handler returns the first match (outer).
        let entry = table.find_handler(7).unwrap();
        assert_eq!(entry.handler, 20);

        // Block 15 is only in the outer range.
        let entry = table.find_handler(15).unwrap();
        assert_eq!(entry.handler, 20);
    }

    #[test]
    fn test_exception_table_adjacent_ranges() {
        let mut table = ExceptionTable::new();
        // Range [0, 5) handler 5
        table.add_entry(ExceptionEntry {
            try_start: 0,
            try_end: 5,
            handler: 5,
            finally: None,
        });
        // Range [5, 10) handler 10
        table.add_entry(ExceptionEntry {
            try_start: 5,
            try_end: 10,
            handler: 10,
            finally: None,
        });

        // Block 4 is in first range
        assert_eq!(table.find_handler(4).unwrap().handler, 5);
        // Block 5 is at boundary — in second range (exclusive end on first)
        assert_eq!(table.find_handler(5).unwrap().handler, 10);
        // Block 9 is in second range
        assert_eq!(table.find_handler(9).unwrap().handler, 10);
        // Block 10 is outside both
        assert!(table.find_handler(10).is_none());
    }

    #[test]
    fn test_exception_table_find_handler_at_try_start() {
        let mut table = ExceptionTable::new();
        table.add_entry(ExceptionEntry {
            try_start: 3,
            try_end: 8,
            handler: 8,
            finally: None,
        });
        // Exact start boundary is inclusive.
        let entry = table.find_handler(3).unwrap();
        assert_eq!(entry.handler, 8);
    }

    #[test]
    fn test_exception_table_find_handler_at_try_end_minus_one() {
        let mut table = ExceptionTable::new();
        table.add_entry(ExceptionEntry {
            try_start: 3,
            try_end: 8,
            handler: 8,
            finally: None,
        });
        // Last valid block in range
        let entry = table.find_handler(7).unwrap();
        assert_eq!(entry.handler, 8);
    }

    #[test]
    fn test_exception_table_empty_find_handler() {
        let table = ExceptionTable::new();
        assert!(table.find_handler(0).is_none());
        assert!(table.find_handler(u32::MAX).is_none());
    }

    #[test]
    fn test_exception_table_default_is_empty() {
        let table = ExceptionTable::default();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_exception_entry_without_finally() {
        let entry = ExceptionEntry {
            try_start: 0,
            try_end: 5,
            handler: 5,
            finally: None,
        };
        assert!(entry.finally.is_none());
        assert_eq!(entry.handler, 5);
    }

    #[test]
    fn test_exception_entry_with_finally() {
        let entry = ExceptionEntry {
            try_start: 0,
            try_end: 5,
            handler: 5,
            finally: Some(10),
        };
        assert_eq!(entry.finally, Some(10));
    }

    // --- ErrorObject chained cause tests ---

    #[test]
    fn test_error_object_deeply_chained_cause() {
        let inner = ErrorObject {
            kind: ErrorKind::Error,
            message: "root cause".to_string(),
            stack: Vec::new(),
            cause: None,
        };
        let middle = ErrorObject {
            kind: ErrorKind::TypeError,
            message: "wrapper 1".to_string(),
            stack: Vec::new(),
            cause: Some(Box::new(inner)),
        };
        let outer = ErrorObject {
            kind: ErrorKind::ReferenceError,
            message: "wrapper 2".to_string(),
            stack: Vec::new(),
            cause: Some(Box::new(middle)),
        };
        assert_eq!(outer.kind, ErrorKind::ReferenceError);
        let mid = outer.cause.as_ref().unwrap();
        assert_eq!(mid.kind, ErrorKind::TypeError);
        let root = mid.cause.as_ref().unwrap();
        assert_eq!(root.kind, ErrorKind::Error);
        assert_eq!(root.message, "root cause");
        assert!(root.cause.is_none());
    }

    #[test]
    fn test_error_object_multiple_stack_frames() {
        let err = ErrorObject {
            kind: ErrorKind::Error,
            message: "oops".to_string(),
            stack: vec![
                StackFrame {
                    function_name: "inner".to_string(),
                    file: "a.js".to_string(),
                    line: 5,
                    column: 1,
                },
                StackFrame {
                    function_name: "outer".to_string(),
                    file: "a.js".to_string(),
                    line: 10,
                    column: 3,
                },
                StackFrame {
                    function_name: "<anonymous>".to_string(),
                    file: "a.js".to_string(),
                    line: 15,
                    column: 0,
                },
            ],
            cause: None,
        };
        assert_eq!(err.stack.len(), 3);
        assert_eq!(err.stack[0].function_name, "inner");
        assert_eq!(err.stack[2].function_name, "<anonymous>");
    }

    // --- TryCatchBlock tests ---

    #[test]
    fn test_try_catch_block_with_catch_only() {
        let tcb = TryCatchBlock {
            try_block: 0,
            catch_block: Some(5),
            finally_block: None,
        };
        assert_eq!(tcb.try_block, 0);
        assert_eq!(tcb.catch_block, Some(5));
        assert!(tcb.finally_block.is_none());
    }

    #[test]
    fn test_try_catch_block_with_finally_only() {
        let tcb = TryCatchBlock {
            try_block: 0,
            catch_block: None,
            finally_block: Some(10),
        };
        assert!(tcb.catch_block.is_none());
        assert_eq!(tcb.finally_block, Some(10));
    }

    #[test]
    fn test_try_catch_block_with_both() {
        let tcb = TryCatchBlock {
            try_block: 0,
            catch_block: Some(5),
            finally_block: Some(10),
        };
        assert_eq!(tcb.catch_block, Some(5));
        assert_eq!(tcb.finally_block, Some(10));
    }

    // --- CompletionRecord edge case tests ---

    #[test]
    fn test_completion_record_unwrap_value_on_break() {
        let cr = CompletionRecord {
            ty: CompletionType::Break,
            value: Some(JsValue::int(99)),
        };
        // Break is abrupt, so unwrap_value returns None even though a value exists.
        assert!(cr.unwrap_value().is_none());
    }

    #[test]
    fn test_completion_record_unwrap_value_on_continue() {
        let cr = CompletionRecord {
            ty: CompletionType::Continue,
            value: Some(JsValue::int(99)),
        };
        assert!(cr.unwrap_value().is_none());
    }

    #[test]
    fn test_completion_record_empty_is_not_throw() {
        let cr = CompletionRecord::empty();
        assert!(!cr.is_throw());
        assert!(!cr.is_return());
        assert!(!cr.is_abrupt());
    }
}
