//! Classification rules for escape analysis.
//!
//! Determines which IR opcodes create allocatable values, which can cause
//! values to escape, and which store one value into another.

use ir::Op;

/// Stateless classifier for escape-analysis-relevant operations.
pub struct EscapeClassifier;

impl EscapeClassifier {
    /// Returns `true` if this op creates an allocatable value (an object that
    /// needs memory management decisions).
    pub fn is_allocation(op: &Op) -> bool {
        matches!(
            op,
            Op::AllocZone
                | Op::AllocHeap
                | Op::AllocArray
                | Op::AllocBox
                | Op::CreateObject
                | Op::CreateObjectLiteral
                | Op::CreateArray
                | Op::CreateClosure
                | Op::CreateArguments
                | Op::CreateRegExp
        )
    }

    /// Returns `true` if this op can cause its operand to escape the function.
    ///
    /// Conservative: any value passed to a call or returned from the function
    /// is considered escaped.
    pub fn is_escape_point(op: &Op) -> bool {
        matches!(
            op,
            Op::Ret
                | Op::Call
                | Op::CallMethod
                | Op::CallNew
                | Op::CallEval
                | Op::CallEvalDirect
                | Op::CallVarargs
                | Op::CallRuntime
                | Op::TailCall
                | Op::Invoke
                | Op::Throw
                | Op::Yield
                | Op::YieldDelegate
        )
    }

    /// Returns `true` if this op stores one value into another (creating a
    /// containment relationship for transitive escape propagation).
    pub fn is_store(op: &Op) -> bool {
        matches!(
            op,
            Op::StoreField
                | Op::StoreElement
                | Op::SetProp
                | Op::SetPropStrict
                | Op::SetElem
                | Op::SetPropDynamic
                | Op::SetPropDynamicStrict
                | Op::SetSuper
                | Op::SetPrivate
                | Op::PrivateFieldSet
                | Op::InstallPrivateField
                | Op::ICSetProp
                | Op::EnvStore
                | Op::BoxStore
        )
    }
}
