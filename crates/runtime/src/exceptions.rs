//! Exception handling state for the runtime.
//!
//! Uses thread-local storage to track the current exception value and
//! a stack of catch frames for try/catch/finally support.
//!
//! ## Spec References
//!
//! - Throw — §6.2.4.4 (Completion Record with type throw)
//! - Error constructor — <https://tc39.es/ecma262/#sec-error-constructor> (§20.5.1)
//! - NativeError constructors — <https://tc39.es/ecma262/#sec-nativeerror-constructors> (§20.5.5)
//! - NativeError types — §20.5.5.1 (TypeError), §20.5.5.2 (RangeError),
//!   §20.5.5.3 (ReferenceError), §20.5.5.4 (SyntaxError), §20.5.5.5 (URIError),
//!   §20.5.5.6 (EvalError)

use std::cell::RefCell;

use nanbox::JsValue;

/// Thread-local exception state.
struct ExceptionState {
    /// The current pending exception value, if any.
    current: Option<u64>,
    /// Stack of catch frames. Each entry is a generation counter that
    /// lets `catch_begin` detect whether an exception was thrown during
    /// the current try block.
    catch_stack: Vec<bool>,
}

impl ExceptionState {
    fn new() -> Self {
        Self {
            current: None,
            catch_stack: Vec::new(),
        }
    }
}

thread_local! {
    static EXCEPTION_STATE: RefCell<ExceptionState> = RefCell::new(ExceptionState::new());
}

/// Implements the `throw` statement runtime behavior.
///
/// Sets the current exception value, equivalent to creating a Completion
/// Record of type `throw` with the given value.
///
/// [spec]: https://tc39.es/ecma262/#sec-throw-statement (§14.14)
///
/// # Spec Algorithm (ThrowStatement: `throw Expression ;`)
///
/// 1. Let exprRef be ? Evaluation of Expression.
/// 2. Let exprValue be ? GetValue(exprRef).
/// 3. Return ThrowCompletion(exprValue).
///
/// Note: Steps 1-2 (expression evaluation and GetValue) are handled by the
/// compiled code. This function implements step 3 by storing the throw
/// value in thread-local state and marking the innermost catch frame.
pub fn throw(val: u64) {
    EXCEPTION_STATE.with(|state| {
        let mut s = state.borrow_mut();
        // 3. Return ThrowCompletion(exprValue).
        // Store the exception value as the pending throw completion.
        s.current = Some(val);
        // Mark the innermost catch frame as having caught an exception
        if let Some(last) = s.catch_stack.last_mut() {
            *last = true;
        }
    });
}

/// Pushes a catch frame onto the exception state stack.
///
/// This is an internal runtime mechanism for try/catch support. It corresponds
/// to entering a `try` block (§14.15 Try Statement). Returns `true` if an
/// exception is already pending (i.e., re-entering a catch after a throw).
///
/// [spec]: https://tc39.es/ecma262/#sec-try-statement (§14.15)
pub fn catch_begin() -> bool {
    EXCEPTION_STATE.with(|state| {
        let mut s = state.borrow_mut();
        let pending = s.current.is_some();
        s.catch_stack.push(false);
        pending
    })
}

/// Pops the innermost catch frame from the exception state stack.
///
/// Called when exiting a try/catch block, corresponding to the completion
/// of the CatchClause or Finally block evaluation.
///
/// [spec]: https://tc39.es/ecma262/#sec-try-statement (§14.15)
pub fn catch_end() {
    EXCEPTION_STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.catch_stack.pop();
    });
}

/// Returns the current exception value, or `undefined` if no exception is pending.
///
/// Used to retrieve the caught value in a catch clause. The catch parameter
/// binding receives this value.
///
/// [spec]: https://tc39.es/ecma262/#sec-try-statement-runtime-semantics-catchclauseevaluation (§14.15.2)
pub fn get_exception() -> u64 {
    EXCEPTION_STATE.with(|state| {
        let s = state.borrow();
        s.current.unwrap_or(JsValue::undefined().raw_bits())
    })
}

/// Returns `true` if an exception is currently pending.
///
/// Used to check whether a throw completion occurred that hasn't been
/// handled by a catch clause yet.
pub fn is_exception() -> bool {
    EXCEPTION_STATE.with(|state| {
        let s = state.borrow();
        s.current.is_some()
    })
}

/// Clears the current exception (used after catching).
///
/// Called when a catch clause successfully handles the exception,
/// transitioning from a throw completion back to a normal completion.
///
/// [spec]: https://tc39.es/ecma262/#sec-try-statement-runtime-semantics-catchclauseevaluation (§14.15.2)
pub fn clear_exception() {
    EXCEPTION_STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.current = None;
    });
}

/// Error tag constants matching JavaScript NativeError types.
///
/// Each constant corresponds to a NativeError constructor defined in the spec:
///
/// - `Error` — §20.5.1 <https://tc39.es/ecma262/#sec-error-constructor>
/// - `TypeError` — §20.5.5.1 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-typeerror>
/// - `RangeError` — §20.5.5.2 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-rangeerror>
/// - `ReferenceError` — §20.5.5.3 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-referenceerror>
/// - `SyntaxError` — §20.5.5.4 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-syntaxerror>
/// - `URIError` — §20.5.5.5 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-urierror>
/// - `EvalError` — §20.5.5.6 <https://tc39.es/ecma262/#sec-native-error-types-used-in-this-standard-evalerror>
///
/// The NativeError constructors all follow the same algorithm pattern
/// (§20.5.6.1.1 `NativeError ( message [ , options ] )`):
///
/// 1. If NewTarget is undefined, let newTarget be the active function object;
///    else let newTarget be NewTarget.
/// 2. Let O be ? OrdinaryCreateFromConstructor(newTarget, "%NativeError.prototype%",
///    << [[ErrorData]] >>).
/// 3. If message is not undefined, then
///    a. Let msg be ? ToString(message).
///    b. Perform CreateNonEnumerableDataPropertyOrThrow(O, "message", msg).
/// 4. Perform ? InstallErrorCause(O, options).
/// 5. Return O.
///
/// Note: This AOT compiler represents error types as tagged values rather than
/// full objects. The tag discriminates error type at throw/catch boundaries.
pub mod error_tag {
    /// Generic `Error` (§20.5.1).
    pub const ERROR: u32 = 0;
    /// `TypeError` (§20.5.5.1) — type mismatch, non-callable, non-constructor, etc.
    pub const TYPE_ERROR: u32 = 1;
    /// `RangeError` (§20.5.5.2) — numeric value out of allowed range.
    pub const RANGE_ERROR: u32 = 2;
    /// `ReferenceError` (§20.5.5.3) — undeclared variable reference.
    pub const REFERENCE_ERROR: u32 = 3;
    /// `SyntaxError` (§20.5.5.4) — invalid syntax detected at runtime (e.g., eval).
    pub const SYNTAX_ERROR: u32 = 4;
    /// `URIError` (§20.5.5.5) — URI handling function misuse.
    pub const URI_ERROR: u32 = 5;
    /// `EvalError` (§20.5.5.6) — eval function misuse (largely legacy).
    pub const EVAL_ERROR: u32 = 6;
}

/// Returns the error type name string for a given NativeError tag.
///
/// Maps each tag constant to the constructor name used in JavaScript
/// (e.g., `"TypeError"`, `"RangeError"`). This corresponds to the
/// `name` property on each NativeError prototype:
///
/// [spec]: https://tc39.es/ecma262/#sec-nativeerror.prototype.name (§20.5.6.3.2)
///
/// Each `NativeError.prototype.name` is initialized to the String value
/// of the NativeError constructor's name (e.g., `"TypeError"`).
pub fn error_name(tag: u32) -> &'static str {
    match tag {
        error_tag::ERROR => "Error",
        error_tag::TYPE_ERROR => "TypeError",
        error_tag::RANGE_ERROR => "RangeError",
        error_tag::REFERENCE_ERROR => "ReferenceError",
        error_tag::SYNTAX_ERROR => "SyntaxError",
        error_tag::URI_ERROR => "URIError",
        error_tag::EVAL_ERROR => "EvalError",
        _ => "Error",
    }
}
