//! String interning / atom table for the compiler.
//!
//! Wraps `lasso::ThreadedRodeo` to provide thread-safe string interning.
//! Includes a [`WellKnown`] struct with pre-interned common JavaScript identifiers.

use lasso::{Key, Spur, ThreadedRodeo};

/// An interned string identifier. Cheap to copy and compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Atom(Spur);

impl Atom {
    /// Resolves this atom to a string slice using the given interner.
    pub fn display<'a>(&self, interner: &'a Interner) -> &'a str {
        interner.resolve(*self)
    }

    /// Extracts the raw numeric value of this atom.
    ///
    /// This can be used for serialization or embedding the atom in a NaN-boxed
    /// value. Use [`Atom::from_raw`] to reconstruct the atom later.
    pub fn into_raw(self) -> u32 {
        self.0.into_usize() as u32
    }

    /// Reconstructs an atom from a raw numeric value previously obtained via
    /// [`Atom::into_raw`].
    ///
    /// # Panics
    ///
    /// Panics if the raw value does not correspond to a valid `Spur` key
    /// (e.g., out of the representable range).
    pub fn from_raw(raw: u32) -> Self {
        let Some(spur) = Spur::try_from_usize(raw as usize) else {
            panic!("BUG: invalid raw value {raw} for Atom");
        };
        Atom(spur)
    }
}

/// A thread-safe string interner that maps strings to unique [`Atom`] identifiers.
pub struct Interner {
    rodeo: ThreadedRodeo,
}

impl Interner {
    /// Creates a new, empty interner.
    pub fn new() -> Self {
        Self {
            rodeo: ThreadedRodeo::new(),
        }
    }

    /// Creates a new interner with all [`WellKnown`] atoms pre-populated.
    ///
    /// Returns both the interner and the well-known atom table.
    pub fn with_well_known() -> (Self, WellKnown) {
        let interner = Self::new();
        let well_known = WellKnown::new(&interner);
        (interner, well_known)
    }

    /// Interns a string, returning its [`Atom`]. If the string was previously
    /// interned, the same [`Atom`] is returned.
    pub fn intern(&self, s: &str) -> Atom {
        Atom(self.rodeo.get_or_intern(s))
    }

    /// Interns a static string, returning its [`Atom`].
    ///
    /// This avoids copying the string data when the source is a `&'static str`.
    pub fn intern_static(&self, s: &'static str) -> Atom {
        Atom(self.rodeo.get_or_intern_static(s))
    }

    /// Resolves an [`Atom`] back to its string slice.
    ///
    /// # Panics
    ///
    /// Panics if the atom was not created by this interner.
    pub fn resolve(&self, atom: Atom) -> &str {
        self.rodeo.resolve(&atom.0)
    }

    /// Attempts to resolve an [`Atom`] back to its string slice.
    ///
    /// Returns `None` if the atom is not present in this interner.
    pub fn try_resolve(&self, atom: Atom) -> Option<&str> {
        self.rodeo.try_resolve(&atom.0)
    }

    /// Returns `true` if the given string has already been interned.
    pub fn contains(&self, s: &str) -> bool {
        self.rodeo.contains(s)
    }

    /// Returns the number of interned strings.
    pub fn len(&self) -> usize {
        self.rodeo.len()
    }

    /// Returns `true` if no strings have been interned.
    pub fn is_empty(&self) -> bool {
        self.rodeo.is_empty()
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-interned atoms for common JavaScript identifiers and well-known names.
///
/// Created via [`WellKnown::new`] or [`Interner::with_well_known`]. All fields
/// are guaranteed to be distinct atoms that resolve to their corresponding
/// JavaScript string.
#[derive(Debug, Clone, Copy)]
pub struct WellKnown {
    /// `"undefined"`
    pub undefined: Atom,
    /// `"null"`
    pub null_: Atom,
    /// `"true"`
    pub true_: Atom,
    /// `"false"`
    pub false_: Atom,
    /// `"prototype"`
    pub prototype: Atom,
    /// `"constructor"`
    pub constructor: Atom,
    /// `"length"`
    pub length: Atom,
    /// `"toString"`
    pub to_string: Atom,
    /// `"valueOf"`
    pub value_of: Atom,
    /// `"Symbol.hasInstance"`
    pub has_instance: Atom,
    /// `"Symbol.iterator"`
    pub iterator: Atom,
    /// `"Symbol.toPrimitive"`
    pub to_primitive: Atom,
    /// `"name"`
    pub name: Atom,
    /// `"message"`
    pub message: Atom,
    /// `"stack"`
    pub stack: Atom,
    /// `"cause"`
    pub cause: Atom,
    /// `"this"`
    pub this: Atom,
    /// `"arguments"`
    pub arguments: Atom,
    /// `"callee"`
    pub callee: Atom,
    /// `"caller"`
    pub caller: Atom,
    /// `"apply"`
    pub apply: Atom,
    /// `"call"`
    pub call: Atom,
    /// `"bind"`
    pub bind: Atom,
    /// `"default"`
    pub default: Atom,
    /// `"exports"`
    pub exports: Atom,
}

impl WellKnown {
    /// Interns all well-known JavaScript strings into the given interner and
    /// returns a [`WellKnown`] table holding their atoms.
    pub fn new(interner: &Interner) -> Self {
        Self {
            undefined: interner.intern("undefined"),
            null_: interner.intern("null"),
            true_: interner.intern("true"),
            false_: interner.intern("false"),
            prototype: interner.intern("prototype"),
            constructor: interner.intern("constructor"),
            length: interner.intern("length"),
            to_string: interner.intern("toString"),
            value_of: interner.intern("valueOf"),
            has_instance: interner.intern("Symbol.hasInstance"),
            iterator: interner.intern("Symbol.iterator"),
            to_primitive: interner.intern("Symbol.toPrimitive"),
            name: interner.intern("name"),
            message: interner.intern("message"),
            stack: interner.intern("stack"),
            cause: interner.intern("cause"),
            this: interner.intern("this"),
            arguments: interner.intern("arguments"),
            callee: interner.intern("callee"),
            caller: interner.intern("caller"),
            apply: interner.intern("apply"),
            call: interner.intern("call"),
            bind: interner.intern("bind"),
            default: interner.intern("default"),
            exports: interner.intern("exports"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_resolve_round_trip() {
        let interner = Interner::new();
        let atom = interner.intern("hello");
        assert_eq!(interner.resolve(atom), "hello");
    }

    #[test]
    fn same_string_same_atom() {
        let interner = Interner::new();
        let a1 = interner.intern("foo");
        let a2 = interner.intern("foo");
        assert_eq!(a1, a2);
    }

    #[test]
    fn different_strings_different_atoms() {
        let interner = Interner::new();
        let a1 = interner.intern("foo");
        let a2 = interner.intern("bar");
        assert_ne!(a1, a2);
    }

    #[test]
    fn try_resolve_valid() {
        let interner = Interner::new();
        let atom = interner.intern("test");
        assert_eq!(interner.try_resolve(atom), Some("test"));
    }

    #[test]
    fn try_resolve_invalid() {
        let interner = Interner::new();
        // Create an atom from a different interner.
        let other = Interner::new();
        let atom = other.intern("only_in_other");
        // The first interner may or may not resolve it (depending on internal
        // key allocation), but try_resolve must not panic. We just verify it
        // returns something reasonable.
        let _ = interner.try_resolve(atom);
    }

    #[test]
    fn contains_before_and_after() {
        let interner = Interner::new();
        assert!(!interner.contains("xyz"));
        interner.intern("xyz");
        assert!(interner.contains("xyz"));
    }

    #[test]
    fn len_and_is_empty() {
        let interner = Interner::new();
        assert!(interner.is_empty());
        assert_eq!(interner.len(), 0);

        interner.intern("a");
        assert!(!interner.is_empty());
        assert_eq!(interner.len(), 1);

        interner.intern("b");
        assert_eq!(interner.len(), 2);

        // Duplicate does not increase length.
        interner.intern("a");
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn into_raw_from_raw_round_trip() {
        let interner = Interner::new();
        let atom = interner.intern("round_trip");
        let raw = atom.into_raw();
        let reconstructed = Atom::from_raw(raw);
        assert_eq!(atom, reconstructed);
        assert_eq!(interner.resolve(reconstructed), "round_trip");
    }

    #[test]
    fn into_raw_deterministic() {
        let interner = Interner::new();
        let atom = interner.intern("det");
        let r1 = atom.into_raw();
        let r2 = atom.into_raw();
        assert_eq!(r1, r2);
    }

    #[test]
    fn intern_static_works() {
        let interner = Interner::new();
        let atom = interner.intern_static("static_str");
        assert_eq!(interner.resolve(atom), "static_str");
    }

    #[test]
    fn intern_static_deduplicates() {
        let interner = Interner::new();
        let a1 = interner.intern("shared");
        let a2 = interner.intern_static("shared");
        assert_eq!(a1, a2);
    }

    #[test]
    fn atom_display() {
        let interner = Interner::new();
        let atom = interner.intern("display_test");
        assert_eq!(atom.display(&interner), "display_test");
    }

    #[test]
    fn default_interner() {
        let interner = Interner::default();
        assert!(interner.is_empty());
    }

    #[test]
    fn well_known_atoms_all_distinct() {
        let (_, wk) = Interner::with_well_known();
        let atoms = [
            wk.undefined,
            wk.null_,
            wk.true_,
            wk.false_,
            wk.prototype,
            wk.constructor,
            wk.length,
            wk.to_string,
            wk.value_of,
            wk.has_instance,
            wk.iterator,
            wk.to_primitive,
            wk.name,
            wk.message,
            wk.stack,
            wk.cause,
            wk.this,
            wk.arguments,
            wk.callee,
            wk.caller,
            wk.apply,
            wk.call,
            wk.bind,
            wk.default,
            wk.exports,
        ];
        // All atoms must be unique.
        let mut seen = std::collections::HashSet::new();
        for atom in &atoms {
            assert!(seen.insert(atom), "duplicate well-known atom: {atom:?}");
        }
    }

    #[test]
    fn well_known_atoms_resolve_correctly() {
        let (interner, wk) = Interner::with_well_known();
        assert_eq!(interner.resolve(wk.undefined), "undefined");
        assert_eq!(interner.resolve(wk.null_), "null");
        assert_eq!(interner.resolve(wk.true_), "true");
        assert_eq!(interner.resolve(wk.false_), "false");
        assert_eq!(interner.resolve(wk.prototype), "prototype");
        assert_eq!(interner.resolve(wk.constructor), "constructor");
        assert_eq!(interner.resolve(wk.length), "length");
        assert_eq!(interner.resolve(wk.to_string), "toString");
        assert_eq!(interner.resolve(wk.value_of), "valueOf");
        assert_eq!(interner.resolve(wk.has_instance), "Symbol.hasInstance");
        assert_eq!(interner.resolve(wk.iterator), "Symbol.iterator");
        assert_eq!(interner.resolve(wk.to_primitive), "Symbol.toPrimitive");
        assert_eq!(interner.resolve(wk.name), "name");
        assert_eq!(interner.resolve(wk.message), "message");
        assert_eq!(interner.resolve(wk.stack), "stack");
        assert_eq!(interner.resolve(wk.cause), "cause");
        assert_eq!(interner.resolve(wk.this), "this");
        assert_eq!(interner.resolve(wk.arguments), "arguments");
        assert_eq!(interner.resolve(wk.callee), "callee");
        assert_eq!(interner.resolve(wk.caller), "caller");
        assert_eq!(interner.resolve(wk.apply), "apply");
        assert_eq!(interner.resolve(wk.call), "call");
        assert_eq!(interner.resolve(wk.bind), "bind");
        assert_eq!(interner.resolve(wk.default), "default");
        assert_eq!(interner.resolve(wk.exports), "exports");
    }

    #[test]
    fn well_known_populates_interner() {
        let (interner, _) = Interner::with_well_known();
        assert_eq!(interner.len(), 25);
        assert!(interner.contains("undefined"));
        assert!(interner.contains("toString"));
        assert!(interner.contains("Symbol.iterator"));
    }

    #[test]
    fn well_known_idempotent() {
        let interner = Interner::new();
        let wk1 = WellKnown::new(&interner);
        let wk2 = WellKnown::new(&interner);
        // Creating WellKnown twice yields the same atoms.
        assert_eq!(wk1.undefined, wk2.undefined);
        assert_eq!(wk1.length, wk2.length);
        assert_eq!(wk1.exports, wk2.exports);
        // And does not double the count.
        assert_eq!(interner.len(), 25);
    }

    #[test]
    fn empty_string() {
        let interner = Interner::new();
        let atom = interner.intern("");
        assert_eq!(interner.resolve(atom), "");
        assert!(interner.contains(""));
    }

    #[test]
    fn unicode_strings() {
        let interner = Interner::new();
        let atom = interner.intern("こんにちは");
        assert_eq!(interner.resolve(atom), "こんにちは");
    }

    // -- Edge cases: empty and whitespace strings ----------------------------

    #[test]
    fn test_intern_whitespace_string() {
        let interner = Interner::new();
        let atom = interner.intern("   ");
        assert_eq!(interner.resolve(atom), "   ");
    }

    #[test]
    fn test_intern_newline_string() {
        let interner = Interner::new();
        let atom = interner.intern("\n\r\t");
        assert_eq!(interner.resolve(atom), "\n\r\t");
    }

    #[test]
    fn test_intern_very_long_string() {
        let interner = Interner::new();
        let long_str = "a".repeat(100_000);
        let atom = interner.intern(&long_str);
        assert_eq!(interner.resolve(atom), long_str);
    }

    // -- Edge cases: intern same string via different methods ----------------

    #[test]
    fn test_intern_empty_string_deduplicates() {
        let interner = Interner::new();
        let a1 = interner.intern("");
        let a2 = interner.intern("");
        assert_eq!(a1, a2);
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn test_intern_static_empty_string() {
        let interner = Interner::new();
        let a1 = interner.intern_static("");
        let a2 = interner.intern("");
        assert_eq!(a1, a2);
    }

    // -- Edge cases: contains ------------------------------------------------

    #[test]
    fn test_contains_empty_string_initially() {
        let interner = Interner::new();
        // Empty string is not in the interner until explicitly interned.
        assert!(!interner.contains(""));
        interner.intern("");
        assert!(interner.contains(""));
    }

    // -- Edge cases: multiple interners --------------------------------------

    #[test]
    fn test_two_interners_independent() {
        let i1 = Interner::new();
        let i2 = Interner::new();
        let a1 = i1.intern("hello");
        let a2 = i2.intern("hello");
        // Atoms from different interners may or may not be equal (depends on
        // internal allocation), but each interner should be able to resolve
        // its own atoms.
        assert_eq!(i1.resolve(a1), "hello");
        assert_eq!(i2.resolve(a2), "hello");
    }

    // -- Edge cases: Atom raw round-trip boundary ----------------------------

    #[test]
    fn test_atom_raw_multiple_atoms() {
        let interner = Interner::new();
        let atoms: Vec<Atom> = (0..100)
            .map(|i| interner.intern(&format!("str_{i}")))
            .collect();
        for atom in &atoms {
            let raw = atom.into_raw();
            let reconstructed = Atom::from_raw(raw);
            assert_eq!(*atom, reconstructed);
        }
    }

    // -- Edge cases: well-known with pre-existing interned strings ----------

    #[test]
    fn test_well_known_after_manual_intern() {
        let interner = Interner::new();
        // Manually intern one well-known string before creating WellKnown.
        let manual_undefined = interner.intern("undefined");
        let wk = WellKnown::new(&interner);
        // Should reuse the same atom.
        assert_eq!(manual_undefined, wk.undefined);
    }

    // -- Edge cases: special characters --------------------------------------

    #[test]
    fn test_intern_null_byte_string() {
        let interner = Interner::new();
        let atom = interner.intern("hello\0world");
        assert_eq!(interner.resolve(atom), "hello\0world");
    }

    #[test]
    fn test_intern_emoji() {
        let interner = Interner::new();
        let atom = interner.intern("\u{1F600}\u{1F601}");
        assert_eq!(interner.resolve(atom), "\u{1F600}\u{1F601}");
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn arbitrary_strings_round_trip(s in ".*") {
                let interner = Interner::new();
                let atom = interner.intern(&s);
                prop_assert_eq!(interner.resolve(atom), s.as_str());
            }

            #[test]
            fn interning_same_string_twice_returns_same_atom(s in ".*") {
                let interner = Interner::new();
                let a1 = interner.intern(&s);
                let a2 = interner.intern(&s);
                prop_assert_eq!(a1, a2);
            }

            #[test]
            fn raw_round_trip_for_arbitrary_strings(s in ".*") {
                let interner = Interner::new();
                let atom = interner.intern(&s);
                let raw = atom.into_raw();
                let reconstructed = Atom::from_raw(raw);
                prop_assert_eq!(atom, reconstructed);
                prop_assert_eq!(interner.resolve(reconstructed), s.as_str());
            }
        }
    }
}
