//! Shared types, spans, and error types for the compiler.

use std::fmt;

use miette::Diagnostic;
use thiserror::Error;

// ---------------------------------------------------------------------------
// File and source span
// ---------------------------------------------------------------------------

/// Identifies a source file within the compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// A span within a source file, identified by file, start offset, and end offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    /// The source file this span belongs to.
    pub file_id: FileId,
    /// Byte offset of the start of the span (inclusive).
    pub start: u32,
    /// Byte offset of the end of the span (exclusive).
    pub end: u32,
}

impl SourceSpan {
    /// A dummy span used as a placeholder when no real span is available.
    pub const DUMMY: SourceSpan = SourceSpan {
        file_id: FileId(u32::MAX),
        start: 0,
        end: 0,
    };

    /// Create a new span covering bytes `start..end` in the given file.
    pub fn new(file_id: FileId, start: u32, end: u32) -> Self {
        Self {
            file_id,
            start,
            end,
        }
    }

    /// Returns the byte length of this span.
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    /// Returns true if this span is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Merge two spans into one that covers both.
    ///
    /// If either span is [`DUMMY`](Self::DUMMY), the other is returned.
    /// Panics if the two spans belong to different files (and neither is DUMMY).
    pub fn merge(self, other: SourceSpan) -> SourceSpan {
        if self == Self::DUMMY {
            return other;
        }
        if other == Self::DUMMY {
            return self;
        }
        assert_eq!(
            self.file_id, other.file_id,
            "cannot merge spans from different files"
        );
        SourceSpan {
            file_id: self.file_id,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Returns true if the given byte offset falls within this span.
    ///
    /// The range is half-open: `start <= offset < end`.
    pub fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

// ---------------------------------------------------------------------------
// ID newtypes
// ---------------------------------------------------------------------------

macro_rules! id_newtype {
    ($(#[doc = $doc:expr])* $name:ident) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

id_newtype! {
    /// Interned string identifier.
    AtomId
}

id_newtype! {
    /// Object shape identifier.
    ShapeId
}

id_newtype! {
    /// TypeScript interface identifier.
    InterfaceId
}

id_newtype! {
    /// Temporal dead zone slot identifier.
    TdzSlotId
}

id_newtype! {
    /// Drop flag identifier for deterministic cleanup.
    DropFlagId
}

id_newtype! {
    /// Struct type identifier in the IR.
    StructTypeId
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The top-level error type for the compiler.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    /// A syntax error during parsing.
    #[error("parse error: {message}")]
    Parse { message: String, span: SourceSpan },

    /// A type checking or inference error.
    #[error("type error: {message}")]
    Type { message: String, span: SourceSpan },

    /// An escape analysis failure.
    #[error("escape analysis error: {message}")]
    Escape { message: String, span: SourceSpan },

    /// A code generation error from the Cranelift or LLVM backend.
    #[error("codegen error: {message}")]
    Codegen { message: String, span: SourceSpan },

    /// A runtime library error.
    #[error("runtime error: {message}")]
    Runtime { message: String, span: SourceSpan },

    /// IR construction or verification error (no source span).
    #[error("ir error: {message}")]
    Ir { message: String },

    /// Module resolution error.
    #[error("module error: {message}")]
    Module { message: String, span: SourceSpan },

    /// Internal compiler error (ICE).
    #[error("internal compiler error: {message}")]
    Internal { message: String },
}

/// A convenient result type alias for compiler operations.
pub type CompileResult<T> = Result<T, CompileError>;

// ---------------------------------------------------------------------------
// Configuration enums
// ---------------------------------------------------------------------------

/// Whether the input is JavaScript or TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    /// JavaScript source (`.js`, `.mjs`, `.cjs`).
    JavaScript,
    /// TypeScript source (`.ts`, `.mts`, `.cts`).
    TypeScript,
}

/// Whether we are building in debug (Cranelift) or release (LLVM) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// Debug mode: uses Cranelift for fast compilation (~500ms).
    Debug,
    /// Release mode: uses LLVM with ThinLTO for optimized output.
    Release,
}

/// Memory management mode.
///
/// `Normal` uses both zone and heap allocation worlds.
/// `HeapOnly` disables zone allocation (useful for differential testing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    /// Normal mode: uses both zone and heap allocation worlds.
    Normal,
    /// Heap-only mode: disables zone allocation (useful for differential testing).
    HeapOnly,
}

/// Target ECMAScript edition for compilation.
///
/// Controls which language features are available and what semantics to use.
/// Defaults to [`ES2025`](Edition::ES2025), the latest ratified specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edition {
    /// ECMAScript 5.1 (2011).
    ES5,
    /// ECMAScript 2015 (ES6) — classes, arrow functions, let/const, etc.
    ES2015,
    /// ECMAScript 2016 — `Array.prototype.includes`, exponentiation operator.
    ES2016,
    /// ECMAScript 2017 — async/await, `Object.entries`/`Object.values`.
    ES2017,
    /// ECMAScript 2018 — async iteration, rest/spread properties.
    ES2018,
    /// ECMAScript 2019 — `Array.prototype.flat`, optional catch binding.
    ES2019,
    /// ECMAScript 2020 — `BigInt`, `globalThis`, optional chaining, nullish coalescing.
    ES2020,
    /// ECMAScript 2021 — `String.prototype.replaceAll`, logical assignment operators.
    ES2021,
    /// ECMAScript 2022 — top-level await, class fields, `Object.hasOwn`.
    ES2022,
    /// ECMAScript 2023 — `Array.prototype.findLast`, hashbang grammar.
    ES2023,
    /// ECMAScript 2024 — `Object.groupBy`, `Promise.withResolvers`.
    ES2024,
    /// ECMAScript 2025 — the latest ratified specification.
    #[default]
    ES2025,
    /// Bleeding-edge features beyond the current specification.
    ESNext,
}

impl std::fmt::Display for Edition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ES5 => write!(f, "es5"),
            Self::ES2015 => write!(f, "es2015"),
            Self::ES2016 => write!(f, "es2016"),
            Self::ES2017 => write!(f, "es2017"),
            Self::ES2018 => write!(f, "es2018"),
            Self::ES2019 => write!(f, "es2019"),
            Self::ES2020 => write!(f, "es2020"),
            Self::ES2021 => write!(f, "es2021"),
            Self::ES2022 => write!(f, "es2022"),
            Self::ES2023 => write!(f, "es2023"),
            Self::ES2024 => write!(f, "es2024"),
            Self::ES2025 => write!(f, "es2025"),
            Self::ESNext => write!(f, "esnext"),
        }
    }
}

/// Error returned when parsing an invalid edition string.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
#[error("unknown edition: {0}")]
pub struct ParseEditionError(String);

impl std::str::FromStr for Edition {
    type Err = ParseEditionError;

    /// Parse an edition string (case-insensitive) into an [`Edition`].
    ///
    /// Accepts canonical forms (`es2025`, `esnext`) and common aliases
    /// (`es6` for `es2015`, `es7` for `es2016`, etc.).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "es5" => Ok(Self::ES5),
            "es2015" | "es6" => Ok(Self::ES2015),
            "es2016" | "es7" => Ok(Self::ES2016),
            "es2017" | "es8" => Ok(Self::ES2017),
            "es2018" | "es9" => Ok(Self::ES2018),
            "es2019" | "es10" => Ok(Self::ES2019),
            "es2020" | "es11" => Ok(Self::ES2020),
            "es2021" | "es12" => Ok(Self::ES2021),
            "es2022" | "es13" => Ok(Self::ES2022),
            "es2023" | "es14" => Ok(Self::ES2023),
            "es2024" => Ok(Self::ES2024),
            "es2025" => Ok(Self::ES2025),
            "esnext" => Ok(Self::ESNext),
            _ => Err(ParseEditionError(s.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ID Display ---------------------------------------------------------

    #[test]
    fn atom_id_display() {
        assert_eq!(AtomId(3).to_string(), "AtomId(3)");
    }

    #[test]
    fn shape_id_display() {
        assert_eq!(ShapeId(42).to_string(), "ShapeId(42)");
    }

    #[test]
    fn interface_id_display() {
        assert_eq!(InterfaceId(0).to_string(), "InterfaceId(0)");
    }

    #[test]
    fn tdz_slot_id_display() {
        assert_eq!(TdzSlotId(7).to_string(), "TdzSlotId(7)");
    }

    #[test]
    fn drop_flag_id_display() {
        assert_eq!(DropFlagId(100).to_string(), "DropFlagId(100)");
    }

    #[test]
    fn struct_type_id_display() {
        assert_eq!(StructTypeId(99).to_string(), "StructTypeId(99)");
    }

    // -- SourceSpan DUMMY ---------------------------------------------------

    #[test]
    fn source_span_dummy_has_max_file_id() {
        assert_eq!(SourceSpan::DUMMY.file_id, FileId(u32::MAX));
        assert_eq!(SourceSpan::DUMMY.start, 0);
        assert_eq!(SourceSpan::DUMMY.end, 0);
        assert!(SourceSpan::DUMMY.is_empty());
    }

    // -- SourceSpan::merge --------------------------------------------------

    #[test]
    fn merge_same_file() {
        let a = SourceSpan::new(FileId(1), 10, 20);
        let b = SourceSpan::new(FileId(1), 5, 30);
        let merged = a.merge(b);
        assert_eq!(merged.file_id, FileId(1));
        assert_eq!(merged.start, 5);
        assert_eq!(merged.end, 30);
    }

    #[test]
    fn merge_with_dummy_lhs() {
        let real = SourceSpan::new(FileId(2), 10, 20);
        let merged = SourceSpan::DUMMY.merge(real);
        assert_eq!(merged, real);
    }

    #[test]
    fn merge_with_dummy_rhs() {
        let real = SourceSpan::new(FileId(2), 10, 20);
        let merged = real.merge(SourceSpan::DUMMY);
        assert_eq!(merged, real);
    }

    #[test]
    #[should_panic(expected = "cannot merge spans from different files")]
    fn merge_different_files_panics() {
        let a = SourceSpan::new(FileId(1), 0, 10);
        let b = SourceSpan::new(FileId(2), 5, 15);
        let _ = a.merge(b);
    }

    // -- SourceSpan::contains -----------------------------------------------

    #[test]
    fn contains_within_span() {
        let span = SourceSpan::new(FileId(0), 10, 20);
        assert!(span.contains(10));
        assert!(span.contains(15));
        assert!(span.contains(19));
    }

    #[test]
    fn contains_outside_span() {
        let span = SourceSpan::new(FileId(0), 10, 20);
        assert!(!span.contains(9));
        assert!(!span.contains(20)); // half-open: end is exclusive
        assert!(!span.contains(100));
    }

    // -- CompileError formatting ------------------------------------------------

    #[test]
    fn compile_error_ir_format() {
        let err = CompileError::Ir {
            message: "invalid opcode".into(),
        };
        assert_eq!(err.to_string(), "ir error: invalid opcode");
    }

    #[test]
    fn compile_error_module_format() {
        let err = CompileError::Module {
            message: "not found".into(),
            span: SourceSpan::DUMMY,
        };
        assert_eq!(err.to_string(), "module error: not found");
    }

    #[test]
    fn compile_error_internal_format() {
        let err = CompileError::Internal {
            message: "unreachable state".into(),
        };
        assert_eq!(
            err.to_string(),
            "internal compiler error: unreachable state"
        );
    }

    // -- Config types -------------------------------------------------------

    #[test]
    fn config_types_debug() {
        // Verify Debug is implemented and produces expected output.
        assert_eq!(format!("{:?}", SourceType::JavaScript), "JavaScript");
        assert_eq!(format!("{:?}", SourceType::TypeScript), "TypeScript");
        assert_eq!(format!("{:?}", BuildMode::Debug), "Debug");
        assert_eq!(format!("{:?}", BuildMode::Release), "Release");
        assert_eq!(format!("{:?}", MemoryMode::Normal), "Normal");
        assert_eq!(format!("{:?}", MemoryMode::HeapOnly), "HeapOnly");
    }

    #[test]
    fn config_types_equality() {
        assert_eq!(SourceType::JavaScript, SourceType::JavaScript);
        assert_ne!(SourceType::JavaScript, SourceType::TypeScript);
        assert_eq!(BuildMode::Release, BuildMode::Release);
        assert_ne!(BuildMode::Debug, BuildMode::Release);
        assert_eq!(MemoryMode::HeapOnly, MemoryMode::HeapOnly);
        assert_ne!(MemoryMode::Normal, MemoryMode::HeapOnly);
    }

    // -- Edition enum -------------------------------------------------------

    #[test]
    fn test_edition_default_is_es2025() {
        assert_eq!(Edition::default(), Edition::ES2025);
    }

    #[test]
    fn test_edition_display() {
        assert_eq!(Edition::ES5.to_string(), "es5");
        assert_eq!(Edition::ES2015.to_string(), "es2015");
        assert_eq!(Edition::ES2020.to_string(), "es2020");
        assert_eq!(Edition::ES2025.to_string(), "es2025");
        assert_eq!(Edition::ESNext.to_string(), "esnext");
    }

    #[test]
    fn test_edition_from_str_valid() {
        assert_eq!("es5".parse::<Edition>(), Ok(Edition::ES5));
        assert_eq!("es2015".parse::<Edition>(), Ok(Edition::ES2015));
        assert_eq!("es6".parse::<Edition>(), Ok(Edition::ES2015));
        assert_eq!("es2017".parse::<Edition>(), Ok(Edition::ES2017));
        assert_eq!("es8".parse::<Edition>(), Ok(Edition::ES2017));
        assert_eq!("es2020".parse::<Edition>(), Ok(Edition::ES2020));
        assert_eq!("es2025".parse::<Edition>(), Ok(Edition::ES2025));
        assert_eq!("esnext".parse::<Edition>(), Ok(Edition::ESNext));
    }

    #[test]
    fn test_edition_from_str_case_insensitive() {
        assert_eq!("ES2025".parse::<Edition>(), Ok(Edition::ES2025));
        assert_eq!("ESNext".parse::<Edition>(), Ok(Edition::ESNext));
        assert_eq!("ESNEXT".parse::<Edition>(), Ok(Edition::ESNext));
    }

    #[test]
    fn test_edition_from_str_invalid() {
        assert!("".parse::<Edition>().is_err());
        assert!("es4".parse::<Edition>().is_err());
        assert!("es2030".parse::<Edition>().is_err());
        assert!("garbage".parse::<Edition>().is_err());
    }

    #[test]
    fn test_edition_equality() {
        assert_eq!(Edition::ES2025, Edition::ES2025);
        assert_ne!(Edition::ES2025, Edition::ESNext);
        assert_ne!(Edition::ES5, Edition::ES2015);
    }

    #[test]
    fn test_edition_debug() {
        assert_eq!(format!("{:?}", Edition::ES2025), "ES2025");
        assert_eq!(format!("{:?}", Edition::ESNext), "ESNext");
        assert_eq!(format!("{:?}", Edition::ES5), "ES5");
    }

    #[test]
    fn test_edition_all_from_str_roundtrip() {
        let editions = [
            Edition::ES5,
            Edition::ES2015,
            Edition::ES2016,
            Edition::ES2017,
            Edition::ES2018,
            Edition::ES2019,
            Edition::ES2020,
            Edition::ES2021,
            Edition::ES2022,
            Edition::ES2023,
            Edition::ES2024,
            Edition::ES2025,
            Edition::ESNext,
        ];
        for edition in editions {
            let s = edition.to_string();
            assert_eq!(
                s.parse::<Edition>(),
                Ok(edition),
                "roundtrip failed for {edition:?}"
            );
        }
    }

    #[test]
    fn test_parse_edition_error_display() {
        let err = "badval".parse::<Edition>().unwrap_err();
        assert_eq!(err.to_string(), "unknown edition: badval");
    }

    // -- SourceSpan edge cases -----------------------------------------------

    #[test]
    fn test_source_span_zero_length() {
        let span = SourceSpan::new(FileId(0), 5, 5);
        assert!(span.is_empty());
        assert_eq!(span.len(), 0);
        assert!(!span.contains(5)); // half-open: empty span contains nothing
    }

    #[test]
    fn test_source_span_single_byte() {
        let span = SourceSpan::new(FileId(0), 10, 11);
        assert!(!span.is_empty());
        assert_eq!(span.len(), 1);
        assert!(span.contains(10));
        assert!(!span.contains(11));
    }

    #[test]
    fn test_source_span_max_offsets() {
        let span = SourceSpan::new(FileId(0), 0, u32::MAX);
        assert_eq!(span.len(), u32::MAX);
        assert!(span.contains(0));
        assert!(span.contains(u32::MAX - 1));
        assert!(!span.contains(u32::MAX)); // exclusive end
    }

    #[test]
    fn test_source_span_merge_both_dummy() {
        let merged = SourceSpan::DUMMY.merge(SourceSpan::DUMMY);
        assert_eq!(merged, SourceSpan::DUMMY);
    }

    #[test]
    fn test_source_span_merge_overlapping() {
        let a = SourceSpan::new(FileId(0), 5, 15);
        let b = SourceSpan::new(FileId(0), 10, 20);
        let merged = a.merge(b);
        assert_eq!(merged.start, 5);
        assert_eq!(merged.end, 20);
    }

    #[test]
    fn test_source_span_merge_adjacent() {
        let a = SourceSpan::new(FileId(0), 0, 5);
        let b = SourceSpan::new(FileId(0), 5, 10);
        let merged = a.merge(b);
        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 10);
    }

    #[test]
    fn test_source_span_merge_nested() {
        let outer = SourceSpan::new(FileId(0), 0, 100);
        let inner = SourceSpan::new(FileId(0), 10, 20);
        let merged = outer.merge(inner);
        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 100);
    }

    #[test]
    fn test_source_span_contains_at_boundary() {
        let span = SourceSpan::new(FileId(0), 0, 0);
        assert!(!span.contains(0)); // empty span at 0
    }

    // -- FileId edge cases ---------------------------------------------------

    #[test]
    fn test_file_id_zero() {
        let fid = FileId(0);
        assert_eq!(fid.0, 0);
    }

    #[test]
    fn test_file_id_max() {
        let fid = FileId(u32::MAX);
        assert_eq!(fid.0, u32::MAX);
    }

    #[test]
    fn test_file_id_equality() {
        assert_eq!(FileId(0), FileId(0));
        assert_ne!(FileId(0), FileId(1));
    }

    // -- CompileError variants -----------------------------------------------

    #[test]
    fn test_compile_error_parse_format() {
        let err = CompileError::Parse {
            message: "unexpected token".into(),
            span: SourceSpan::new(FileId(0), 10, 15),
        };
        assert_eq!(err.to_string(), "parse error: unexpected token");
    }

    #[test]
    fn test_compile_error_type_format() {
        let err = CompileError::Type {
            message: "incompatible types".into(),
            span: SourceSpan::DUMMY,
        };
        assert_eq!(err.to_string(), "type error: incompatible types");
    }

    #[test]
    fn test_compile_error_escape_format() {
        let err = CompileError::Escape {
            message: "value escapes scope".into(),
            span: SourceSpan::DUMMY,
        };
        assert_eq!(
            err.to_string(),
            "escape analysis error: value escapes scope"
        );
    }

    #[test]
    fn test_compile_error_codegen_format() {
        let err = CompileError::Codegen {
            message: "unsupported instruction".into(),
            span: SourceSpan::DUMMY,
        };
        assert_eq!(err.to_string(), "codegen error: unsupported instruction");
    }

    #[test]
    fn test_compile_error_runtime_format() {
        let err = CompileError::Runtime {
            message: "stack overflow".into(),
            span: SourceSpan::DUMMY,
        };
        assert_eq!(err.to_string(), "runtime error: stack overflow");
    }

    #[test]
    fn test_compile_error_empty_message() {
        let err = CompileError::Ir {
            message: String::new(),
        };
        assert_eq!(err.to_string(), "ir error: ");
    }

    // -- ID newtype edge cases -----------------------------------------------

    #[test]
    fn test_id_newtype_max_values() {
        assert_eq!(
            AtomId(u32::MAX).to_string(),
            format!("AtomId({})", u32::MAX)
        );
        assert_eq!(
            ShapeId(u32::MAX).to_string(),
            format!("ShapeId({})", u32::MAX)
        );
        assert_eq!(
            StructTypeId(u32::MAX).to_string(),
            format!("StructTypeId({})", u32::MAX)
        );
    }

    #[test]
    fn test_id_newtype_zero() {
        assert_eq!(AtomId(0).to_string(), "AtomId(0)");
        assert_eq!(TdzSlotId(0).to_string(), "TdzSlotId(0)");
        assert_eq!(DropFlagId(0).to_string(), "DropFlagId(0)");
    }

    #[test]
    fn test_id_newtype_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(AtomId(1));
        set.insert(AtomId(1));
        set.insert(AtomId(2));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_id_newtype_clone_and_copy() {
        let id = AtomId(42);
        let copied = id; // Copy
        assert_eq!(id, copied);
    }
}
