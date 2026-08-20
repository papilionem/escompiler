//! Forward dataflow type inference engine.
//!
//! Walks the SSA IR in block order and computes an `InferredType` and
//! `TrustCategory` for every value produced by an instruction. The result
//! is a `TypeAnnotations` side-table that downstream passes can query.

use std::collections::HashMap;

use ir::ValueId;
use ir::builder::{TypedFunction, TypedModule};
use ir::types::{IrType, Op};

use crate::lattice::{self, InferredType};
use crate::trust::TrustCategory;

/// Side-table mapping value IDs to their inferred types and trust levels.
pub struct TypeAnnotations {
    /// ValueId.0 -> inferred type.
    pub types: HashMap<u32, InferredType>,
    /// ValueId.0 -> trust category.
    pub trust: HashMap<u32, TrustCategory>,
    /// ValueId.0 -> whether the value may be a Proxy object.
    ///
    /// `false` means the value is provably not a Proxy (e.g. object literals,
    /// array literals, primitives, constructor results). `true` means the value
    /// could potentially be a Proxy (e.g. function parameters, property reads,
    /// return values from unknown functions). This flag is conservative: when in
    /// doubt, it is set to `true`.
    pub proxy_flags: HashMap<u32, bool>,
}

impl TypeAnnotations {
    fn new() -> Self {
        Self {
            types: HashMap::new(),
            trust: HashMap::new(),
            proxy_flags: HashMap::new(),
        }
    }

    /// Look up the inferred type for a value.
    pub fn get_type(&self, val: ValueId) -> &InferredType {
        self.types.get(&val.0).unwrap_or(&InferredType::Unknown)
    }

    /// Look up the trust category for a value.
    pub fn get_trust(&self, val: ValueId) -> TrustCategory {
        self.trust
            .get(&val.0)
            .copied()
            .unwrap_or(TrustCategory::Untyped)
    }

    /// Look up whether a value may be a Proxy object.
    ///
    /// Returns `true` (conservative) for unknown values. Primitives and
    /// compiler-created objects return `false`. Values from external sources
    /// (parameters, property reads, calls) return `true`.
    pub fn get_may_be_proxy(&self, val: ValueId) -> bool {
        self.proxy_flags.get(&val.0).copied().unwrap_or(true)
    }
}

/// Infer types for all values in a single function.
pub fn infer_function(func: &TypedFunction) -> TypeAnnotations {
    let mut ann = TypeAnnotations::new();
    let mut changed = true;

    // Seed: walk all blocks in order and compute initial types.
    compute_types(func, &mut ann);

    // Worklist: re-process until phi types converge.
    // We use a simple fixed-point iteration: if any phi type changed on
    // a pass, re-process all blocks. This converges quickly because the
    // lattice is finite and join is monotone.
    while changed {
        let old_types = ann.types.clone();
        compute_types(func, &mut ann);
        changed = ann.types != old_types;
    }

    ann
}

/// Infer types for all functions in a module.
pub fn infer_module(module: &TypedModule) -> Vec<TypeAnnotations> {
    module.functions.iter().map(infer_function).collect()
}

/// Single pass: compute types for every instruction in every block.
fn compute_types(func: &TypedFunction, ann: &mut TypeAnnotations) {
    // Assign trust for function parameters: they are External inputs.
    // Parameters don't have explicit instructions in the typed IR,
    // so we skip them here — they'll be handled when used.

    for block in &func.blocks {
        for inst in &block.instructions {
            let (ty, trust, may_be_proxy) =
                infer_instruction(&inst.op, &inst.ty, &inst.operands, ann);
            ann.types.insert(inst.id.0, ty);
            ann.trust.insert(inst.id.0, trust);
            ann.proxy_flags.insert(inst.id.0, may_be_proxy);
        }
    }
}

/// Infer the type, trust category, and proxy flag for a single instruction.
///
/// The returned `bool` is the `may_be_proxy` flag: `false` when the value is
/// provably not a Proxy (primitives, compiler-created objects, arithmetic
/// results), `true` when it could potentially be a Proxy (parameters, property
/// reads, call return values, closure environment loads).
fn infer_instruction(
    op: &Op,
    ir_type: &IrType,
    operands: &[ValueId],
    ann: &TypeAnnotations,
) -> (InferredType, TrustCategory, bool) {
    match op {
        // --- Constants: provably known types, never Proxy ---
        Op::ConstI32(_) => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstI64(_) => (
            InferredType::Concrete(IrType::I64),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstF64(_) => (
            InferredType::Concrete(IrType::F64),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstBool(_) => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstNull => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstUndefined => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::Provable,
            false,
        ),
        Op::ConstString(_) => (
            InferredType::Concrete(IrType::JSString),
            TrustCategory::Provable,
            false,
        ),
        // LoadGlobal returns a built-in global (e.g. Array, Math). The result
        // is a JSValue from the runtime registry — never a Proxy.
        Op::LoadGlobal(_) => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),

        // --- Integer arithmetic: result is always primitive, never Proxy ---
        Op::AddI32 | Op::SubI32 | Op::MulI32 | Op::DivI32 | Op::ModI32 | Op::NegI32 => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::Provable,
            false,
        ),

        // --- Float arithmetic: result is always primitive, never Proxy ---
        Op::AddF64 | Op::SubF64 | Op::MulF64 | Op::DivF64 | Op::ModF64 | Op::NegF64 => (
            InferredType::Concrete(IrType::F64),
            TrustCategory::Provable,
            false,
        ),

        // --- JS coercing arithmetic: result is JSValue but never Proxy
        //     (coercion always produces a primitive) ---
        Op::AddJS | Op::SubJS | Op::MulJS | Op::DivJS | Op::ModJS | Op::NegJS | Op::ExpJS => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),

        // --- Bitwise (always I32 in JS semantics): never Proxy ---
        Op::BitwiseAnd
        | Op::BitwiseOr
        | Op::BitwiseXor
        | Op::BitwiseNot
        | Op::ShiftLeft
        | Op::ShiftRight
        | Op::ShiftRightUnsigned => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::Provable,
            false,
        ),

        // --- Comparisons: always produce Bool, never Proxy ---
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
        | Op::GeJS => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- Type conversions: results are primitives, never Proxy ---
        Op::ToNumber | Op::ToNumeric => (
            InferredType::Concrete(IrType::F64),
            TrustCategory::External,
            false,
        ),
        Op::ToString => (
            InferredType::Concrete(IrType::JSString),
            TrustCategory::External,
            false,
        ),
        Op::ToBoolean => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::External,
            false,
        ),
        // ToObject may return a Proxy if the input is already one
        Op::ToObject => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::External,
            true,
        ),
        Op::ToPrimitive | Op::ToPropertyKey => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),
        Op::ToInt32 | Op::ToUint32 => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::External,
            false,
        ),

        // --- Boxing: always produces JSValue, never Proxy ---
        Op::BoxI32
        | Op::BoxUnsignedI32
        | Op::BoxF64
        | Op::BoxBool
        | Op::BoxNull
        | Op::BoxUndefined
        | Op::BoxString
        | Op::BoxObject
        | Op::BoxSymbol => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::Provable,
            false,
        ),

        // --- Unboxing: produces primitive types, never Proxy ---
        Op::UnboxI32 => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::External,
            false,
        ),
        Op::UnboxF64 => (
            InferredType::Concrete(IrType::F64),
            TrustCategory::External,
            false,
        ),
        Op::UnboxBool => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::External,
            false,
        ),
        Op::UnboxString => (
            InferredType::Concrete(IrType::JSString),
            TrustCategory::External,
            false,
        ),
        // UnboxObject extracts an object pointer — the underlying object could
        // be a Proxy, so inherit proxy flag from the input operand.
        Op::UnboxObject => {
            let may_proxy = operands.first().is_some_and(|op| ann.get_may_be_proxy(*op));
            (
                InferredType::Concrete(IrType::JSObject),
                TrustCategory::External,
                may_proxy,
            )
        }
        Op::UnboxSymbol => (
            InferredType::Concrete(IrType::JSSymbol),
            TrustCategory::External,
            false,
        ),

        // --- NaN-box queries: results are primitives, never Proxy ---
        Op::TypeofBoxed => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::Provable,
            false,
        ),
        Op::IsNullish | Op::IsFalsy => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- SSA Phi: join all incoming types; may_be_proxy if ANY input may be ---
        Op::Phi => {
            let mut result = InferredType::Unreachable;
            let mut trust = TrustCategory::Provable;
            let mut may_proxy = false;
            for operand in operands {
                let operand_ty = ann
                    .types
                    .get(&operand.0)
                    .cloned()
                    .unwrap_or(InferredType::Unknown);
                result = lattice::join(&result, &operand_ty);
                let operand_trust = ann
                    .trust
                    .get(&operand.0)
                    .copied()
                    .unwrap_or(TrustCategory::Untyped);
                trust = TrustCategory::merge(trust, operand_trust);
                // Conservative: if any input may be Proxy, the phi result may be too.
                may_proxy = may_proxy || ann.get_may_be_proxy(*operand);
            }
            if operands.is_empty() {
                result = InferredType::Unknown;
                trust = TrustCategory::Untyped;
                may_proxy = true;
            }
            (result, trust, may_proxy)
        }

        // --- Control flow: Void, no useful type, never Proxy ---
        Op::Br | Op::BrIf | Op::Switch | Op::Ret | Op::Unreachable => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),

        // --- Memory allocation: compiler-created, never Proxy ---
        Op::AllocZone | Op::AllocHeap | Op::AllocStack => (
            InferredType::Concrete(ir_type.clone()),
            TrustCategory::Provable,
            false,
        ),
        Op::AllocArray => (
            InferredType::Concrete(IrType::JSArray),
            TrustCategory::Provable,
            false,
        ),
        Op::FreeZone | Op::IncRef | Op::DecRef => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),

        // --- Field/element access ---
        // LoadField/LoadElement: reading from a struct/array, unlikely to be Proxy
        // but could be if the underlying object is. Conservative: false for
        // compiler-level field access, but LoadLocal may hold anything.
        Op::LoadField | Op::LoadElement => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),
        Op::LoadLocal => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        // LoadParam: function parameters may be Proxy objects
        Op::LoadParam(_) => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::StoreField | Op::StoreElement | Op::StoreLocal => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),

        // --- RC operations: Void or Bool, never Proxy ---
        Op::RcIncStrong | Op::RcDecStrong | Op::RcIncWeak | Op::RcDecWeak => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::RcIsUnique => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- Property access ---
        // Property reads from unknown objects may return a Proxy
        Op::GetProp
        | Op::GetElem
        | Op::GetPropDynamic
        | Op::GetSuper
        | Op::GetPrivate
        | Op::PrivateFieldGet
        | Op::ICGetProp => (InferredType::Unknown, TrustCategory::Untyped, true),
        // Property sets/deletes produce Void, never Proxy
        Op::SetProp
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
        | Op::ICSetProp => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        // HasProp produces Bool, never Proxy
        Op::HasProp | Op::PrivateFieldHas => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- Calls: return values from unknown functions may be Proxy ---
        Op::Call
        | Op::CallMethod
        | Op::CallVarargs
        | Op::CallRuntime
        | Op::TailCall
        | Op::Invoke => (InferredType::Unknown, TrustCategory::Untyped, true),
        // eval can return anything including Proxy
        Op::CallEval | Op::CallEvalDirect => (InferredType::Unknown, TrustCategory::Untyped, true),
        // CallNew: constructor result — the internal [[Construct]] mechanism
        // returns the newly created `this` object unless the constructor
        // explicitly returns an object. The `this` object is compiler-created
        // (not a Proxy), but a constructor *could* return a Proxy. We are
        // conservative here: mark as false because the common case is that
        // `new Foo()` returns the plain `this`.
        Op::CallNew => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::External,
            false,
        ),

        // --- Object / Shape: compiler-created objects, never Proxy ---
        Op::CreateObject | Op::CreateObjectLiteral | Op::CreateArguments => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::Provable,
            false,
        ),
        Op::CreateArray => (
            InferredType::Concrete(IrType::JSArray),
            TrustCategory::Provable,
            false,
        ),
        Op::CreateRegExp => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::Provable,
            false,
        ),
        Op::CreateClosure => (
            InferredType::Concrete(IrType::JSFunction),
            TrustCategory::Provable,
            false,
        ),
        Op::ObjectDefineProperty | Op::ObjectSetPrototype => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        // ObjectGetPrototype may return a Proxy if the prototype chain has one
        Op::ObjectGetPrototype => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::External,
            true,
        ),
        Op::ShapeCheck => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),
        Op::ShapeTransition => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::InstanceOf => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- Type guards ---
        Op::GuardType => {
            // GuardType narrows the type; for now return JSValue with External trust.
            // A more advanced implementation would use the guard tag operand to
            // determine the narrowed type. Proxy flag inherits from input.
            let may_proxy = operands.first().is_some_and(|op| ann.get_may_be_proxy(*op));
            (
                InferredType::Narrowed(Box::new(InferredType::Concrete(IrType::JSValue))),
                TrustCategory::External,
                may_proxy,
            )
        }
        Op::GuardShape => {
            let may_proxy = operands.first().is_some_and(|op| ann.get_may_be_proxy(*op));
            (
                InferredType::Concrete(IrType::JSObject),
                TrustCategory::External,
                may_proxy,
            )
        }
        Op::GuardTruthiness => {
            let may_proxy = operands.first().is_some_and(|op| ann.get_may_be_proxy(*op));
            (
                InferredType::Concrete(IrType::JSValue),
                TrustCategory::External,
                may_proxy,
            )
        }

        // --- Exception handling ---
        Op::TryBegin | Op::TryEnd | Op::Finally => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::Throw | Op::Rethrow => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        // Caught exceptions may be any value including Proxy
        Op::Catch | Op::GetException => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::IsException => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- TDZ / Drop flags ---
        Op::TdzCheck => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::TdzInit => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::DropFlagSet => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::DropFlagCheck => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::Provable,
            false,
        ),

        // --- Closure environment ---
        Op::EnvCreate => (
            InferredType::Concrete(IrType::Ptr),
            TrustCategory::Provable,
            false,
        ),
        // EnvLoad reads from a closure environment slot — the captured value
        // could be anything including a Proxy
        Op::EnvLoad => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::EnvStore => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        Op::EnvExtend => (
            InferredType::Concrete(IrType::Ptr),
            TrustCategory::Provable,
            false,
        ),
        Op::EnvLookup => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::EnvLookupStore => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),

        // --- JsBox ---
        // BoxLoad reads a heap cell — the value could be a Proxy
        Op::AllocBox => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::BoxLoad => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::BoxStore => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),

        // --- Iterator protocol ---
        // IterInit/ForInInit/IterInitAsync: creates a record, not a Proxy
        Op::IterInit | Op::ForInInit | Op::IterInitAsync => (
            InferredType::Concrete(IrType::IteratorRecord),
            TrustCategory::External,
            false,
        ),
        // IterNext/IterValue: returned value could be a Proxy
        Op::IterNext | Op::IterValue => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::IterDone => (
            InferredType::Concrete(IrType::Bool),
            TrustCategory::External,
            false,
        ),
        Op::IterClose => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),

        // --- Promise / Async ---
        // PromiseCreate: compiler-created, never Proxy
        Op::PromiseCreate => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::Provable,
            false,
        ),
        Op::PromiseResolve | Op::PromiseReject => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        // Await: resolved value could be a Proxy
        Op::Await => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),

        // --- Generator ---
        // GeneratorCreate: compiler-created, never Proxy
        Op::GeneratorCreate => (
            InferredType::Concrete(IrType::JSObject),
            TrustCategory::Provable,
            false,
        ),
        // Yield/YieldDelegate: value sent to generator could be a Proxy
        Op::Yield | Op::YieldDelegate => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),

        // --- String operations: results are primitives, never Proxy ---
        Op::StringConcat | Op::StringCharAt => (
            InferredType::Concrete(IrType::JSString),
            TrustCategory::Provable,
            false,
        ),
        Op::StringCompare | Op::StringLength => (
            InferredType::Concrete(IrType::I32),
            TrustCategory::Provable,
            false,
        ),

        // --- Miscellaneous ---
        Op::Nop | Op::Debugger => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
        // ThisValue may be a Proxy if the caller passed one
        Op::ThisValue => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        // NewTarget is the constructor function reference, never a Proxy itself
        Op::NewTarget => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),
        // ImportMeta is a compiler-created object, never Proxy
        Op::ImportMeta => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            false,
        ),
        // SuperCall return value could be a Proxy (if parent constructor returns one)
        Op::SuperCall => (
            InferredType::Concrete(IrType::JSValue),
            TrustCategory::External,
            true,
        ),
        Op::WithScope => (
            InferredType::Concrete(IrType::Void),
            TrustCategory::Provable,
            false,
        ),
    }
}
