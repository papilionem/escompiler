//! Comprehensive IR type system and opcode definitions.
//!
//! This module defines the full `IrType` enum (18 variants), the `Op` enum
//! (171+ opcodes organised by category), and the `TypedInstruction` struct
//! that pairs an opcode with its type, operands, and source span.

use std::hash::{Hash, Hasher};

use common::{SourceSpan, StructTypeId};

use crate::{BlockId, ValueId};

// ---------------------------------------------------------------------------
// IrType — full type system
// ---------------------------------------------------------------------------

/// Comprehensive IR type covering primitive, pointer, JS-level, and composite
/// types used throughout the compiler pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IrType {
    /// No value (used for statements and side-effect-only instructions).
    Void,
    /// Boolean value.
    Bool,
    /// 32-bit signed integer.
    I32,
    /// 64-bit signed integer.
    I64,
    /// 64-bit IEEE 754 floating-point number.
    F64,
    /// Generic pointer.
    Ptr,
    /// Pointer to zone-allocated object.
    ZonePtr,
    /// Pointer to RC-managed object.
    HeapPtr,
    /// NaN-boxed JS value.
    JSValue,
    /// JS string (dual Latin1/UTF-16).
    JSString,
    /// JS object.
    JSObject,
    /// JS array.
    JSArray,
    /// JS function.
    JSFunction,
    /// JS symbol.
    JSSymbol,
    /// Named struct type.
    Struct(StructTypeId),
    /// Fixed-size array `[type; count]`.
    Array(Box<IrType>, u32),
    /// ECMAScript completion record.
    CompletionRecord,
    /// ECMAScript iterator record.
    IteratorRecord,
}

// ---------------------------------------------------------------------------
// Op — all 171+ opcodes
// ---------------------------------------------------------------------------

/// All opcodes for the IR, organised by category.
///
/// Each variant represents a single operation. Operands are stored externally
/// in `TypedInstruction::operands`; only immediate data that is part of the
/// opcode itself lives here (e.g. constant values, string-table indices).
#[derive(Debug, Clone)]
pub enum Op {
    // === Constants (7) ===
    /// 32-bit integer constant.
    ConstI32(i32),
    /// 64-bit integer constant.
    ConstI64(i64),
    /// 64-bit float constant.
    ConstF64(f64),
    /// Boolean constant.
    ConstBool(bool),
    /// JS `null` constant.
    ConstNull,
    /// JS `undefined` constant.
    ConstUndefined,
    /// Index into the module string table.
    ConstString(u32),

    /// Load a built-in global object by name (string table index).
    ///
    /// Semantically equivalent to calling `__esc_rt_get_global(name)` at
    /// runtime. Produces a NaN-boxed JS value representing the global object
    /// (e.g., `Array`, `Object`, `Math`).
    LoadGlobal(u32),

    // === Arithmetic (26) ===
    /// Integer addition.
    AddI32,
    /// Integer subtraction.
    SubI32,
    /// Integer multiplication.
    MulI32,
    /// Integer division.
    DivI32,
    /// Integer modulo.
    ModI32,
    /// Integer negation.
    NegI32,
    /// Float addition.
    AddF64,
    /// Float subtraction.
    SubF64,
    /// Float multiplication.
    MulF64,
    /// Float division.
    DivF64,
    /// Float modulo.
    ModF64,
    /// Float negation.
    NegF64,
    /// JS `+` with `ToNumber` coercion.
    AddJS,
    /// JS `-` with `ToNumber` coercion.
    SubJS,
    /// JS `*` with `ToNumber` coercion.
    MulJS,
    /// JS `/` with `ToNumber` coercion.
    DivJS,
    /// JS `%` with `ToNumber` coercion.
    ModJS,
    /// JS unary `-` with `ToNumber` coercion.
    NegJS,
    /// JS `**` exponentiation.
    ExpJS,
    /// Bitwise AND.
    BitwiseAnd,
    /// Bitwise OR.
    BitwiseOr,
    /// Bitwise XOR.
    BitwiseXor,
    /// Bitwise NOT.
    BitwiseNot,
    /// Left shift.
    ShiftLeft,
    /// Arithmetic right shift (sign-extending).
    ShiftRight,
    /// Logical right shift (zero-filling).
    ShiftRightUnsigned,

    // === Comparison (20) ===
    /// Integer equality.
    EqI32,
    /// Float equality.
    EqF64,
    /// JS `===`.
    EqStrict,
    /// JS `==`.
    EqAbstract,
    /// Integer inequality.
    NeI32,
    /// Float inequality.
    NeF64,
    /// JS `!==`.
    NeStrict,
    /// JS `!=`.
    NeAbstract,
    /// Integer less-than.
    LtI32,
    /// Float less-than.
    LtF64,
    /// JS `<` with coercion.
    LtJS,
    /// Integer less-than-or-equal.
    LeI32,
    /// Float less-than-or-equal.
    LeF64,
    /// JS `<=` with coercion.
    LeJS,
    /// Integer greater-than.
    GtI32,
    /// Float greater-than.
    GtF64,
    /// JS `>` with coercion.
    GtJS,
    /// Integer greater-than-or-equal.
    GeI32,
    /// Float greater-than-or-equal.
    GeF64,
    /// JS `>=`.
    GeJS,

    // === Type conversion (8) ===
    /// Abstract `ToNumber` conversion.
    ToNumber,
    /// Abstract `ToNumeric` conversion.
    ToNumeric,
    /// Abstract `ToString` conversion.
    #[allow(clippy::doc_markdown)]
    ToString,
    /// Abstract `ToBoolean` conversion.
    ToBoolean,
    /// Abstract `ToObject` conversion.
    ToObject,
    /// Abstract `ToPrimitive` conversion.
    ToPrimitive,
    /// Abstract `ToPropertyKey` conversion.
    ToPropertyKey,
    /// Abstract `ToInt32` conversion.
    ToInt32,
    /// Abstract `ToUint32` conversion.
    ToUint32,

    // === NaN-boxing (18) ===
    /// Box an i32 into a NaN-boxed JS value.
    BoxI32,
    /// Box an i32 into a NaN-boxed JS value treating it as unsigned.
    ///
    /// For values where the sign bit is set (i.e. >= 2^31), the result
    /// is boxed as an f64 to preserve the full unsigned range.
    BoxUnsignedI32,
    /// Box an f64 into a NaN-boxed JS value.
    BoxF64,
    /// Box a bool into a NaN-boxed JS value.
    BoxBool,
    /// Box null into a NaN-boxed JS value.
    BoxNull,
    /// Box undefined into a NaN-boxed JS value.
    BoxUndefined,
    /// Box a string pointer into a NaN-boxed JS value.
    BoxString,
    /// Box an object pointer into a NaN-boxed JS value.
    BoxObject,
    /// Box a symbol into a NaN-boxed JS value.
    BoxSymbol,
    /// Extract an i32 from a NaN-boxed JS value.
    UnboxI32,
    /// Extract an f64 from a NaN-boxed JS value.
    UnboxF64,
    /// Extract a bool from a NaN-boxed JS value.
    UnboxBool,
    /// Extract a string pointer from a NaN-boxed JS value.
    UnboxString,
    /// Extract an object pointer from a NaN-boxed JS value.
    UnboxObject,
    /// Extract a symbol from a NaN-boxed JS value.
    UnboxSymbol,
    /// Returns the NaN-box type tag.
    TypeofBoxed,
    /// Checks for `null` or `undefined`.
    IsNullish,
    /// Checks JS falsiness.
    IsFalsy,

    // === Control flow (5) ===
    /// Unconditional branch (target in `block_targets`).
    Br,
    /// Conditional branch: `(cond, then_block, else_block)`.
    BrIf,
    /// Multi-way branch.
    Switch,
    /// Return from function.
    Ret,
    /// Marks unreachable code.
    Unreachable,

    // === SSA (1) ===
    /// Phi function — merges values from predecessor blocks.
    Phi,

    // === Memory allocation (7) ===
    /// Allocate in zone.
    AllocZone,
    /// Allocate on heap (RC).
    AllocHeap,
    /// Stack allocation.
    AllocStack,
    /// Allocate array.
    AllocArray,
    /// Free zone object.
    FreeZone,
    /// Increment reference count.
    IncRef,
    /// Decrement reference count (may free).
    DecRef,

    // === Field / element access (6) ===
    /// Load struct field by index.
    LoadField,
    /// Store struct field by index.
    StoreField,
    /// Load array element by index.
    LoadElement,
    /// Store array element by index.
    StoreElement,
    /// Load SSA local variable.
    LoadLocal,
    /// Store SSA local variable.
    StoreLocal,
    /// Load a function parameter by index.
    LoadParam(u32),

    // === RC operations (5) ===
    /// Increment strong reference count.
    RcIncStrong,
    /// Decrement strong reference count (may free).
    RcDecStrong,
    /// Increment weak reference count.
    RcIncWeak,
    /// Decrement weak reference count.
    RcDecWeak,
    /// Check if `strong_count == 1`.
    RcIsUnique,

    // === Property access (15) ===
    /// `obj.prop` (by atom).
    GetProp,
    /// `obj.prop = val`.
    SetProp,
    /// `obj.prop = val` in strict mode (throws TypeError on frozen/sealed/non-extensible).
    SetPropStrict,
    /// `delete obj.prop`.
    DeleteProp,
    /// `prop in obj`.
    HasProp,
    /// `obj[key]`.
    GetElem,
    /// `obj[key] = val`.
    SetElem,
    /// `delete obj[key]`.
    DeleteElem,
    /// Dynamic property access (megamorphic).
    GetPropDynamic,
    /// Dynamic property set (megamorphic, sloppy mode).
    SetPropDynamic,
    /// Dynamic property set (megamorphic, strict mode — throws TypeError).
    SetPropDynamicStrict,
    /// `super.prop`.
    GetSuper,
    /// `super.prop = val`.
    SetSuper,
    /// `#field` access (legacy — uses string key).
    GetPrivate,
    /// `#field = val` (legacy — uses string key).
    SetPrivate,
    /// Get a private field by compile-time private name ID.
    /// Operands: `(obj, private_id_const)`. TypeError if brand check fails.
    PrivateFieldGet,
    /// Set a private field by compile-time private name ID.
    /// Operands: `(obj, private_id_const, value)`. TypeError if brand check fails.
    PrivateFieldSet,
    /// Check if an object has a private field (`#x in obj`).
    /// Operands: `(obj, private_id_const)`. Returns bool (no throw).
    PrivateFieldHas,
    /// Install a private field during construction.
    /// Operands: `(obj, private_id_const, value)`. Bypasses extensibility checks.
    InstallPrivateField,
    /// Inline-cached `obj.prop` (by atom, with IC site id).
    ICGetProp,
    /// Inline-cached `obj.prop = val` (by atom, with IC site id).
    ICSetProp,

    // === Calls (8) ===
    /// Direct call.
    Call,
    /// `obj.method()`.
    CallMethod,
    /// `new Ctor()`.
    CallNew,
    /// `eval()` special form.
    CallEval,
    /// Direct `eval()` with scope bridging in poisoned functions.
    ///
    /// Operands: `(code, lex_env, var_env, this_value)`.
    /// Carries the current lexical and variable environments so eval'd code
    /// can access variables from the enclosing scope.
    CallEvalDirect,
    /// Call with spread.
    CallVarargs,
    /// Call runtime helper.
    CallRuntime,
    /// Tail-position call.
    TailCall,
    /// Call that may throw (inside try).
    Invoke,

    // === Object / Shape (12) ===
    /// Create empty object.
    CreateObject,
    /// Create an object literal with statically-known data properties.
    ///
    /// Operands: `[key0, val0, key1, val1, ...]` — interleaved keys and values.
    /// Keys are `ConstString` values representing property names. Values are
    /// arbitrary expressions. The runtime builds the shape chain lazily and
    /// caches it for subsequent calls with the same key sequence.
    CreateObjectLiteral,
    /// Create array literal.
    CreateArray,
    /// Create regexp.
    CreateRegExp,
    /// Create closure.
    CreateClosure,
    /// Create arguments object.
    CreateArguments,
    /// `Object.defineProperty`.
    ObjectDefineProperty,
    /// `Object.getPrototypeOf`.
    ObjectGetPrototype,
    /// `Object.setPrototypeOf`.
    ObjectSetPrototype,
    /// Guard: object has expected shape.
    ShapeCheck,
    /// Transition to new shape.
    ShapeTransition,
    /// `instanceof` check.
    InstanceOf,

    // === Type guards (3) ===
    /// Deopt if type mismatch.
    GuardType,
    /// Deopt if shape mismatch.
    GuardShape,
    /// Deopt if falsy.
    GuardTruthiness,

    // === Exception handling (8) ===
    /// Start try block.
    TryBegin,
    /// End try block.
    TryEnd,
    /// Throw value.
    Throw,
    /// Catch handler (binds exception).
    Catch,
    /// Finally handler.
    Finally,
    /// Rethrow caught exception.
    Rethrow,
    /// Check if completion is exception.
    IsException,
    /// Extract exception value.
    GetException,

    // === TDZ / Drop flags (4) ===
    /// Check temporal dead zone.
    TdzCheck,
    /// Mark slot as initialised.
    TdzInit,
    /// Set drop flag.
    DropFlagSet,
    /// Check drop flag.
    DropFlagCheck,

    // === Closure environment (6) ===
    /// Create closure environment.
    EnvCreate,
    /// Load from env slot.
    EnvLoad,
    /// Store to env slot.
    EnvStore,
    /// Extend env chain.
    EnvExtend,
    /// Dynamic name-based lookup through an `EscEnvironment` chain.
    ///
    /// Operands: `(env, name_string)`. Returns the value found, or `undefined`
    /// if the name is not bound in any scope in the chain. Used inside `with`
    /// statement bodies for identifiers that are not lexically declared.
    EnvLookup,
    /// Dynamic name-based store through an `EscEnvironment` chain.
    ///
    /// Operands: `(env, name_string, value)`. Returns `true` (NaN-boxed bool)
    /// if the store succeeded, `false` if the name was not found. Used inside
    /// `with` statement bodies for assignments to non-lexical identifiers.
    EnvLookupStore,

    // === JsBox (heap-allocated variable cell) (3) ===
    /// Allocate a JsBox on the heap, initialized with a value.
    /// Operand: initial value. Result: NaN-boxed pointer to JsBox.
    AllocBox,
    /// Load the current value from a JsBox.
    /// Operand: JsBox pointer. Result: the stored JsValue.
    BoxLoad,
    /// Store a new value into a JsBox.
    /// Operands: JsBox pointer, new value. Result: void.
    BoxStore,

    // === Iterator protocol (7) ===
    /// Get iterator via `Symbol.iterator`. Used by `for..of`.
    IterInit,
    /// Create a `for..in` property enumerator (ES2024 §14.7.5.9).
    ///
    /// Unlike `IterInit` (which uses `Symbol.iterator` for `for..of`),
    /// this creates an iterator over the enumerable string-keyed own AND
    /// inherited properties of the object, per `EnumerateObjectProperties`.
    ForInInit,
    /// Get async iterator via `Symbol.asyncIterator`, falling back to
    /// `Symbol.iterator` if not present. Used by `for await...of`.
    IterInitAsync,
    /// `iterator.next()`.
    IterNext,
    /// Check done flag.
    IterDone,
    /// Extract value.
    IterValue,
    /// Close iterator.
    IterClose,

    // === Promise / Async (4) ===
    /// Create a new Promise object.
    PromiseCreate,
    /// Resolve a promise with a value.
    PromiseResolve,
    /// Reject a promise with a reason.
    PromiseReject,
    /// `await` expression (suspend async function).
    Await,

    // === Generator (3) ===
    /// Create a generator object.
    GeneratorCreate,
    /// `yield` expression (suspend generator).
    Yield,
    /// `yield*` delegation.
    YieldDelegate,

    // === String operations (4) ===
    /// Concatenate two strings.
    StringConcat,
    /// Compare two strings lexicographically.
    StringCompare,
    /// Get string length.
    StringLength,
    /// Get character at index.
    StringCharAt,

    // === Miscellaneous (7) ===
    /// No operation.
    Nop,
    /// `debugger` statement.
    Debugger,
    /// Load `this`.
    ThisValue,
    /// `new.target`.
    NewTarget,
    /// `import.meta`.
    ImportMeta,
    /// `super()`.
    SuperCall,
    /// `with` statement scope.
    WithScope,
}

// Manual PartialEq: treat f64 values bitwise so NaN == NaN.
impl PartialEq for Op {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Op::ConstI32(a), Op::ConstI32(b)) => a == b,
            (Op::ConstI64(a), Op::ConstI64(b)) => a == b,
            (Op::ConstF64(a), Op::ConstF64(b)) => a.to_bits() == b.to_bits(),
            (Op::ConstBool(a), Op::ConstBool(b)) => a == b,
            (Op::ConstString(a), Op::ConstString(b)) => a == b,
            (Op::LoadGlobal(a), Op::LoadGlobal(b)) => a == b,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

impl Eq for Op {}

impl Hash for Op {
    fn hash<H: Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            Op::ConstI32(v) => v.hash(state),
            Op::ConstI64(v) => v.hash(state),
            Op::ConstF64(v) => v.to_bits().hash(state),
            Op::ConstBool(v) => v.hash(state),
            Op::ConstString(v) => v.hash(state),
            Op::LoadGlobal(v) => v.hash(state),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Op utility methods
// ---------------------------------------------------------------------------

impl Op {
    /// Returns `true` if this opcode is a block terminator (transfers control).
    pub fn is_terminator(&self) -> bool {
        matches!(
            self,
            Op::Br | Op::BrIf | Op::Switch | Op::Ret | Op::Unreachable | Op::Throw | Op::Rethrow
        )
    }

    /// Returns `true` if this opcode is a call instruction.
    pub fn is_call(&self) -> bool {
        matches!(
            self,
            Op::Call
                | Op::CallMethod
                | Op::CallNew
                | Op::CallEval
                | Op::CallEvalDirect
                | Op::CallVarargs
                | Op::CallRuntime
                | Op::TailCall
                | Op::Invoke
        )
    }

    /// Returns `true` if this opcode is a memory operation (allocation, free, RC).
    pub fn is_memory(&self) -> bool {
        matches!(
            self,
            Op::AllocZone
                | Op::AllocHeap
                | Op::AllocStack
                | Op::AllocArray
                | Op::FreeZone
                | Op::IncRef
                | Op::DecRef
                | Op::RcIncStrong
                | Op::RcDecStrong
                | Op::RcIncWeak
                | Op::RcDecWeak
        )
    }

    /// Returns `true` if this opcode has observable side effects (calls,
    /// stores, throws, memory ops, etc.).
    pub fn has_side_effects(&self) -> bool {
        if self.is_call() || self.is_memory() || self.is_terminator() {
            return true;
        }
        matches!(
            self,
            Op::StoreField
                | Op::StoreElement
                | Op::StoreLocal
                | Op::SetProp
                | Op::SetPropStrict
                | Op::DeleteProp
                | Op::SetElem
                | Op::DeleteElem
                | Op::SetPropDynamic
                | Op::SetPropDynamicStrict
                | Op::SetSuper
                | Op::SetPrivate
                | Op::PrivateFieldSet
                | Op::InstallPrivateField
                | Op::ICSetProp
                | Op::ObjectDefineProperty
                | Op::ObjectSetPrototype
                | Op::ShapeTransition
                | Op::TryBegin
                | Op::TryEnd
                | Op::Throw
                | Op::Catch
                | Op::Finally
                | Op::Rethrow
                | Op::TdzInit
                | Op::DropFlagSet
                | Op::EnvCreate
                | Op::EnvStore
                | Op::EnvExtend
                | Op::EnvLookup
                | Op::EnvLookupStore
                | Op::AllocBox
                | Op::BoxStore
                | Op::IterClose
                | Op::PromiseResolve
                | Op::PromiseReject
                | Op::Await
                | Op::Yield
                | Op::YieldDelegate
                | Op::Debugger
                | Op::CreateObjectLiteral
                | Op::SuperCall
                | Op::WithScope
        )
    }

    /// Returns the category name of this opcode.
    pub fn category(&self) -> &'static str {
        match self {
            Op::ConstI32(_)
            | Op::ConstI64(_)
            | Op::ConstF64(_)
            | Op::ConstBool(_)
            | Op::ConstNull
            | Op::ConstUndefined
            | Op::ConstString(_)
            | Op::LoadGlobal(_) => "constants",

            Op::AddI32
            | Op::SubI32
            | Op::MulI32
            | Op::DivI32
            | Op::ModI32
            | Op::NegI32
            | Op::AddF64
            | Op::SubF64
            | Op::MulF64
            | Op::DivF64
            | Op::ModF64
            | Op::NegF64
            | Op::AddJS
            | Op::SubJS
            | Op::MulJS
            | Op::DivJS
            | Op::ModJS
            | Op::NegJS
            | Op::ExpJS
            | Op::BitwiseAnd
            | Op::BitwiseOr
            | Op::BitwiseXor
            | Op::BitwiseNot
            | Op::ShiftLeft
            | Op::ShiftRight
            | Op::ShiftRightUnsigned => "arithmetic",

            Op::EqI32
            | Op::EqF64
            | Op::EqStrict
            | Op::EqAbstract
            | Op::NeI32
            | Op::NeF64
            | Op::NeStrict
            | Op::NeAbstract
            | Op::LtI32
            | Op::LtF64
            | Op::LtJS
            | Op::LeI32
            | Op::LeF64
            | Op::LeJS
            | Op::GtI32
            | Op::GtF64
            | Op::GtJS
            | Op::GeI32
            | Op::GeF64
            | Op::GeJS => "comparison",

            Op::ToNumber
            | Op::ToNumeric
            | Op::ToString
            | Op::ToBoolean
            | Op::ToObject
            | Op::ToPrimitive
            | Op::ToPropertyKey
            | Op::ToInt32
            | Op::ToUint32 => "type_conversion",

            Op::BoxI32
            | Op::BoxUnsignedI32
            | Op::BoxF64
            | Op::BoxBool
            | Op::BoxNull
            | Op::BoxUndefined
            | Op::BoxString
            | Op::BoxObject
            | Op::BoxSymbol
            | Op::UnboxI32
            | Op::UnboxF64
            | Op::UnboxBool
            | Op::UnboxString
            | Op::UnboxObject
            | Op::UnboxSymbol
            | Op::TypeofBoxed
            | Op::IsNullish
            | Op::IsFalsy => "nan_boxing",

            Op::Br | Op::BrIf | Op::Switch | Op::Ret | Op::Unreachable => "control_flow",

            Op::Phi => "ssa",

            Op::AllocZone
            | Op::AllocHeap
            | Op::AllocStack
            | Op::AllocArray
            | Op::FreeZone
            | Op::IncRef
            | Op::DecRef => "memory_allocation",

            Op::LoadField
            | Op::StoreField
            | Op::LoadElement
            | Op::StoreElement
            | Op::LoadLocal
            | Op::StoreLocal
            | Op::LoadParam(_) => "field_element_access",

            Op::RcIncStrong | Op::RcDecStrong | Op::RcIncWeak | Op::RcDecWeak | Op::RcIsUnique => {
                "rc_operations"
            }

            Op::GetProp
            | Op::SetProp
            | Op::SetPropStrict
            | Op::DeleteProp
            | Op::HasProp
            | Op::GetElem
            | Op::SetElem
            | Op::DeleteElem
            | Op::GetPropDynamic
            | Op::SetPropDynamic
            | Op::SetPropDynamicStrict
            | Op::GetSuper
            | Op::SetSuper
            | Op::GetPrivate
            | Op::SetPrivate
            | Op::PrivateFieldGet
            | Op::PrivateFieldSet
            | Op::PrivateFieldHas
            | Op::InstallPrivateField
            | Op::ICGetProp
            | Op::ICSetProp => "property_access",

            Op::Call
            | Op::CallMethod
            | Op::CallNew
            | Op::CallEval
            | Op::CallEvalDirect
            | Op::CallVarargs
            | Op::CallRuntime
            | Op::TailCall
            | Op::Invoke => "calls",

            Op::CreateObject
            | Op::CreateObjectLiteral
            | Op::CreateArray
            | Op::CreateRegExp
            | Op::CreateClosure
            | Op::CreateArguments
            | Op::ObjectDefineProperty
            | Op::ObjectGetPrototype
            | Op::ObjectSetPrototype
            | Op::ShapeCheck
            | Op::ShapeTransition
            | Op::InstanceOf => "object_shape",

            Op::GuardType | Op::GuardShape | Op::GuardTruthiness => "type_guards",

            Op::TryBegin
            | Op::TryEnd
            | Op::Throw
            | Op::Catch
            | Op::Finally
            | Op::Rethrow
            | Op::IsException
            | Op::GetException => "exception_handling",

            Op::TdzCheck | Op::TdzInit | Op::DropFlagSet | Op::DropFlagCheck => "tdz_drop_flags",

            Op::EnvCreate
            | Op::EnvLoad
            | Op::EnvStore
            | Op::EnvExtend
            | Op::EnvLookup
            | Op::EnvLookupStore => "closure_environment",

            Op::AllocBox | Op::BoxLoad | Op::BoxStore => "jsbox",

            Op::IterInit
            | Op::ForInInit
            | Op::IterInitAsync
            | Op::IterNext
            | Op::IterDone
            | Op::IterValue
            | Op::IterClose => "iterator_protocol",

            Op::PromiseCreate | Op::PromiseResolve | Op::PromiseReject | Op::Await => {
                "promise_async"
            }

            Op::GeneratorCreate | Op::Yield | Op::YieldDelegate => "generator",

            Op::StringConcat | Op::StringCompare | Op::StringLength | Op::StringCharAt => {
                "string_operations"
            }

            Op::Nop
            | Op::Debugger
            | Op::ThisValue
            | Op::NewTarget
            | Op::ImportMeta
            | Op::SuperCall
            | Op::WithScope => "miscellaneous",
        }
    }
}

// ---------------------------------------------------------------------------
// TypedInstruction
// ---------------------------------------------------------------------------

/// A fully-typed instruction in the new IR representation.
///
/// Coexists with the legacy `Instruction` / `InstructionData` during the
/// migration period. Will become the sole instruction type in a future wave.
#[derive(Debug, Clone)]
pub struct TypedInstruction {
    /// The SSA value produced by this instruction.
    pub id: ValueId,
    /// The opcode (operation to perform).
    pub op: Op,
    /// The result type of this instruction.
    pub ty: IrType,
    /// SSA value operands consumed by this instruction.
    pub operands: Vec<ValueId>,
    /// Branch / switch target blocks.
    pub block_targets: Vec<BlockId>,
    /// Source location for diagnostics and source maps.
    pub span: SourceSpan,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
