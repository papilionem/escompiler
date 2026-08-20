//! Generator object support for ES2015 `function*`/`yield`.
//!
//! Implements the generator state machine protocol per the ECMAScript specification.
//!
//! ## Spec References
//!
//! - `GeneratorStart` — [ES2024 §27.5.3.2](https://tc39.es/ecma262/#sec-generatorstart)
//! - `GeneratorResume` — [ES2024 §27.5.3.3](https://tc39.es/ecma262/#sec-generatorresume)
//! - `GeneratorResumeAbrupt` — [ES2024 §27.5.3.4](https://tc39.es/ecma262/#sec-generatorresumeabrupt)
//! - `GeneratorYield` — [ES2024 §27.5.3.5](https://tc39.es/ecma262/#sec-generatoryield)
//!
//! ## Protocol
//!
//! When a generator function is called, a ramp function allocates a state object
//! and returns a generator object. Each `.next()` / `.throw()` / `.return()` call
//! invokes the resume function (the compiled state machine) which dispatches to the
//! correct segment based on the current `state_index`.
//!
//! ## State Object Layout
//!
//! The state object is a plain JS object with the following properties:
//! - `state_index`: current state (-1 = not started, -2 = done, -3 = executing)
//! - `resume_mode`: 0 = next, 1 = throw, 2 = return
//! - `sent_value`: value passed to `.next(val)` / `.throw(err)` / `.return(val)`
//! - `param_0`, `param_1`, ...: saved function parameters
//! - `slot_0`, `slot_1`, ...: saved live variables across yield points
//!
//! ## Resume Modes (GeneratorResume / GeneratorResumeAbrupt)
//!
//! - `RESUME_NEXT` (0): Normal `.next(value)` — `GeneratorResume(generator, value)`.
//! - `RESUME_THROW` (1): `.throw(error)` — `GeneratorResumeAbrupt(generator, ThrowCompletion(error))`.
//! - `RESUME_RETURN` (2): `.return(value)` — `GeneratorResumeAbrupt(generator, ReturnCompletion(value))`.
//!
//! ## Generator States (§27.5.3)
//!
//! The spec defines four generator states:
//! - **suspendedStart**: Created but `.next()` not yet called (`STATE_NOT_STARTED`).
//! - **suspendedYield**: Suspended at a `yield` expression (state_index >= 0).
//! - **executing**: Currently running (`STATE_EXECUTING`).
//! - **completed**: Returned or threw (`STATE_DONE`).

/// Resume mode: normal `.next()` invocation.
///
/// Corresponds to `GeneratorResume(generator, value)`.
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorresume
pub const RESUME_NEXT: i32 = 0;

/// Resume mode: `.throw()` invocation.
///
/// Corresponds to `GeneratorResumeAbrupt(generator, ThrowCompletion(error))`.
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorresumeabrupt
pub const RESUME_THROW: i32 = 1;

/// Resume mode: `.return()` invocation.
///
/// Corresponds to `GeneratorResumeAbrupt(generator, ReturnCompletion(value))`.
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorresumeabrupt
pub const RESUME_RETURN: i32 = 2;

/// State index: generator has not started yet (suspendedStart).
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorstart
pub const STATE_NOT_STARTED: i32 = -1;

/// State index: generator has completed (done).
///
/// Corresponds to the "completed" generator state in the spec.
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorresume (step 5)
pub const STATE_DONE: i32 = -2;

/// State index: generator is currently executing (re-entrancy guard).
///
/// Corresponds to the "executing" generator state. If `.next()` is called
/// while the generator is in this state, a TypeError is thrown per
/// `GeneratorResume` step 4.
///
/// [spec]: https://tc39.es/ecma262/#sec-generatorresume (step 4)
pub const STATE_EXECUTING: i32 = -3;
