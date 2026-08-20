//! Symbol built-in — well-known symbols.
//!
//! Defines the [`WellKnownSymbol`] enum for the standard well-known symbol
//! identifiers used throughout the runtime (e.g. `Symbol.iterator`,
//! `Symbol.toPrimitive`).

/// Well-known Symbol IDs matching the ECMAScript specification.
///
/// These correspond to the `@@iterator`, `@@toPrimitive`, etc. abstract
/// names used in the spec. At runtime they are mapped to unique symbol
/// values via the interner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WellKnownSymbol {
    /// `Symbol.iterator` — used by for-of and spread.
    Iterator,
    /// `Symbol.toPrimitive` — used by type coercion.
    ToPrimitive,
    /// `Symbol.toStringTag` — used by `Object.prototype.toString`.
    ToStringTag,
    /// `Symbol.hasInstance` — used by `instanceof`.
    HasInstance,
    /// `Symbol.isConcatSpreadable` — used by `Array.prototype.concat`.
    IsConcatSpreadable,
    /// `Symbol.species` — used by built-in methods to create derived objects.
    Species,
}

impl WellKnownSymbol {
    /// Returns the description string for this symbol (e.g. `"Symbol.iterator"`).
    pub fn description(&self) -> &'static str {
        match self {
            Self::Iterator => "Symbol.iterator",
            Self::ToPrimitive => "Symbol.toPrimitive",
            Self::ToStringTag => "Symbol.toStringTag",
            Self::HasInstance => "Symbol.hasInstance",
            Self::IsConcatSpreadable => "Symbol.isConcatSpreadable",
            Self::Species => "Symbol.species",
        }
    }

    /// Returns all well-known symbols.
    pub fn all() -> &'static [WellKnownSymbol] {
        &[
            Self::Iterator,
            Self::ToPrimitive,
            Self::ToStringTag,
            Self::HasInstance,
            Self::IsConcatSpreadable,
            Self::Species,
        ]
    }
}
