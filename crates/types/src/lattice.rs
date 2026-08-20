//! Type lattice for inferred types.
//!
//! Defines the `InferredType` enum and lattice operations (join, meet,
//! subtype) used by the forward dataflow inference engine.

use common::ShapeId;
use ir::FunctionId;
use ir::types::IrType;

/// An inferred type for a value in the IR.
#[derive(Debug, Clone, PartialEq)]
pub enum InferredType {
    /// Concrete IR type (known precisely).
    Concrete(IrType),
    /// Union of possible types.
    Union(Vec<IrType>),
    /// Object with known shape.
    ObjectShape(ShapeId),
    /// Typed array with known element type.
    TypedArray(Box<InferredType>),
    /// Known function (for direct call optimization).
    KnownFunction(FunctionId),
    /// Narrowed type (from guard/typeof check).
    Narrowed(Box<InferredType>),
    /// Type is completely unknown.
    Unknown,
    /// Unreachable code — no type possible.
    Unreachable,
}

impl InferredType {
    /// Returns `true` if this is a `Concrete` variant.
    pub fn is_concrete(&self) -> bool {
        matches!(self, InferredType::Concrete(_))
    }

    /// Returns `true` if this is `Unknown`.
    pub fn is_unknown(&self) -> bool {
        matches!(self, InferredType::Unknown)
    }

    /// Returns `true` if this is `Unreachable`.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, InferredType::Unreachable)
    }
}

/// Least upper bound: combine two types into the most specific type that
/// contains both.
pub fn join(a: &InferredType, b: &InferredType) -> InferredType {
    match (a, b) {
        // Unreachable is the bottom element.
        (InferredType::Unreachable, other) | (other, InferredType::Unreachable) => other.clone(),

        // Unknown is the top element.
        (InferredType::Unknown, _) | (_, InferredType::Unknown) => InferredType::Unknown,

        // Identical concrete types stay concrete.
        (InferredType::Concrete(x), InferredType::Concrete(y)) if x == y => {
            InferredType::Concrete(x.clone())
        }

        // Different concrete types form a union.
        (InferredType::Concrete(x), InferredType::Concrete(y)) => {
            InferredType::Union(vec![x.clone(), y.clone()])
        }

        // Union + Concrete: add to union if not already present.
        (InferredType::Union(xs), InferredType::Concrete(y)) => {
            let mut result = xs.clone();
            if !result.contains(y) {
                result.push(y.clone());
            }
            InferredType::Union(result)
        }
        (InferredType::Concrete(x), InferredType::Union(ys)) => {
            let mut result = ys.clone();
            if !result.contains(x) {
                result.insert(0, x.clone());
            }
            InferredType::Union(result)
        }

        // Union + Union: merge.
        (InferredType::Union(xs), InferredType::Union(ys)) => {
            let mut result = xs.clone();
            for y in ys {
                if !result.contains(y) {
                    result.push(y.clone());
                }
            }
            InferredType::Union(result)
        }

        // Everything else falls back to Unknown.
        _ => InferredType::Unknown,
    }
}

/// Greatest lower bound: intersect two types to find the most general type
/// that is a subtype of both.
pub fn meet(a: &InferredType, b: &InferredType) -> InferredType {
    match (a, b) {
        // Unknown is top — meet with top yields the other.
        (InferredType::Unknown, other) | (other, InferredType::Unknown) => other.clone(),

        // Unreachable is bottom — meet with bottom yields bottom.
        (InferredType::Unreachable, _) | (_, InferredType::Unreachable) => {
            InferredType::Unreachable
        }

        // Same concrete type.
        (InferredType::Concrete(x), InferredType::Concrete(y)) if x == y => {
            InferredType::Concrete(x.clone())
        }

        // Different concrete types — no overlap.
        (InferredType::Concrete(_), InferredType::Concrete(_)) => InferredType::Unreachable,

        // Union meets Concrete: keep only if present.
        (InferredType::Union(xs), InferredType::Concrete(y)) => {
            if xs.contains(y) {
                InferredType::Concrete(y.clone())
            } else {
                InferredType::Unreachable
            }
        }
        (InferredType::Concrete(x), InferredType::Union(ys)) => {
            if ys.contains(x) {
                InferredType::Concrete(x.clone())
            } else {
                InferredType::Unreachable
            }
        }

        // Union meets Union: intersection.
        (InferredType::Union(xs), InferredType::Union(ys)) => {
            let common: Vec<IrType> = xs.iter().filter(|x| ys.contains(x)).cloned().collect();
            match common.as_slice() {
                [] => InferredType::Unreachable,
                [single] => InferredType::Concrete(single.clone()),
                _ => InferredType::Union(common),
            }
        }

        // Fallback.
        _ => InferredType::Unreachable,
    }
}

/// Returns `true` if `a` is a subtype of `b` in the lattice.
pub fn is_subtype(a: &InferredType, b: &InferredType) -> bool {
    match (a, b) {
        // Unreachable is a subtype of everything.
        (InferredType::Unreachable, _) => true,

        // Everything is a subtype of Unknown.
        (_, InferredType::Unknown) => true,

        // Same concrete type.
        (InferredType::Concrete(x), InferredType::Concrete(y)) => x == y,

        // Concrete is subtype of Union if the union contains it.
        (InferredType::Concrete(x), InferredType::Union(ys)) => ys.contains(x),

        // Union is subtype of Union if all members are contained.
        (InferredType::Union(xs), InferredType::Union(ys)) => xs.iter().all(|x| ys.contains(x)),

        _ => false,
    }
}
