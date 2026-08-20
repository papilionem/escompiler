//! Type narrowing — refine types based on runtime guards.
//!
//! Functions in this module take an existing inferred type and a condition
//! (typeof check, truthiness, nullish check) and return a more specific type.

use ir::types::IrType;

use crate::lattice::InferredType;

/// Narrow a type based on a `typeof` check result.
///
/// Maps JavaScript `typeof` strings to their corresponding IR types.
pub fn narrow_typeof(ty: &InferredType, typeof_result: &str) -> InferredType {
    let narrowed = match typeof_result {
        "number" => IrType::F64,
        "string" => IrType::JSString,
        "boolean" => IrType::Bool,
        "object" => IrType::JSObject,
        "function" => IrType::JSFunction,
        "symbol" => IrType::JSSymbol,
        "undefined" => IrType::JSValue,
        _ => return ty.clone(),
    };

    match ty {
        InferredType::Unknown | InferredType::Concrete(IrType::JSValue) => {
            InferredType::Concrete(narrowed)
        }
        InferredType::Union(types) => {
            if types.contains(&narrowed) {
                InferredType::Concrete(narrowed)
            } else {
                // The typeof check constrains to this type even if the union
                // didn't previously contain it (the union was an approximation).
                InferredType::Concrete(narrowed)
            }
        }
        InferredType::Concrete(existing) if *existing == narrowed => {
            InferredType::Concrete(narrowed)
        }
        InferredType::Unreachable => InferredType::Unreachable,
        _ => InferredType::Concrete(narrowed),
    }
}

/// Narrow a type based on a truthiness check.
///
/// When `is_truthy` is true, removes null and undefined from unions.
/// When `is_truthy` is false, narrows to null/undefined if those are possible.
pub fn narrow_truthiness(ty: &InferredType, is_truthy: bool) -> InferredType {
    match ty {
        InferredType::Unreachable => InferredType::Unreachable,

        InferredType::Union(types) if is_truthy => {
            // Remove null/undefined-like types (JSValue when it represents
            // null/undefined). In practice we remove JSValue from unions since
            // it is the only type that encompasses null/undefined.
            let filtered: Vec<IrType> = types
                .iter()
                .filter(|t| !matches!(t, IrType::JSValue | IrType::Void))
                .cloned()
                .collect();
            match filtered.as_slice() {
                [] => InferredType::Unreachable,
                [single] => InferredType::Concrete(single.clone()),
                _ => InferredType::Union(filtered),
            }
        }

        // Non-union truthy check: if the type could be null/undefined, narrow.
        InferredType::Concrete(IrType::JSValue) if is_truthy => {
            // JSValue is the box type — after truthiness check, it's still
            // JSValue but known to be truthy. We just return it as-is since
            // we don't have a "truthy JSValue" variant.
            InferredType::Concrete(IrType::JSValue)
        }

        _ => ty.clone(),
    }
}

/// Narrow a type based on a nullish check (`== null` / `!= null`).
///
/// When `is_nullish` is true, narrows to the nullish portion.
/// When `is_nullish` is false, removes null/undefined from the type.
pub fn narrow_nullish(ty: &InferredType, is_nullish: bool) -> InferredType {
    match ty {
        InferredType::Unreachable => InferredType::Unreachable,

        InferredType::Union(types) if !is_nullish => {
            let filtered: Vec<IrType> = types
                .iter()
                .filter(|t| !matches!(t, IrType::JSValue | IrType::Void))
                .cloned()
                .collect();
            match filtered.as_slice() {
                [] => InferredType::Unreachable,
                [single] => InferredType::Concrete(single.clone()),
                _ => InferredType::Union(filtered),
            }
        }

        InferredType::Union(_) if is_nullish => {
            // Narrowed to the nullish portion.
            InferredType::Concrete(IrType::JSValue)
        }

        InferredType::Unknown if !is_nullish => {
            // We know it's not null/undefined but we don't know what it is.
            InferredType::Unknown
        }

        InferredType::Unknown if is_nullish => InferredType::Concrete(IrType::JSValue),

        _ => ty.clone(),
    }
}
