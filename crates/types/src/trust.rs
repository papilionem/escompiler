//! Trust categories for type information provenance.
//!
//! Each inferred type carries a trust level that indicates how reliable the
//! type information is. Higher trust allows more aggressive optimizations
//! (e.g. skipping runtime type checks).

/// Trust level for an inferred type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustCategory {
    /// D: Unknown/any — no type information available.
    Untyped = 0,
    /// C: External input — function params, imports.
    External = 1,
    /// B: TypeScript annotations present.
    Annotated = 2,
    /// A: Provably correct — constants, compiler-generated.
    Provable = 3,
}

impl TrustCategory {
    /// Returns `true` if this trust level is high enough to skip runtime
    /// type checks (Provable or Annotated).
    pub fn is_trusted(self) -> bool {
        matches!(self, TrustCategory::Provable | TrustCategory::Annotated)
    }

    /// Returns `true` if runtime checks can be elided entirely (Provable only).
    pub fn can_skip_check(self) -> bool {
        self == TrustCategory::Provable
    }

    /// Conservative merge: returns the lower trust level of the two.
    pub fn merge(a: TrustCategory, b: TrustCategory) -> TrustCategory {
        if (a as u8) <= (b as u8) { a } else { b }
    }
}
