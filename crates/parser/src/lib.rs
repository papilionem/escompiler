//! parser — oxc integration wrapper for parsing JavaScript/TypeScript.
//!
//! Wraps the oxc parser and exposes a simplified API for the rest of the
//! compiler. Because the oxc AST borrows from its arena allocator, we use a
//! callback pattern (`parse_with`) to let callers visit the AST without
//! self-referential struct issues. Convenience functions (`parse_js`,
//! `parse_ts`, `parse`) extract an owned summary (`ParsedProgram`).

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_diagnostics::OxcDiagnostic;
pub use oxc_span::SourceType;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// A single parse error with message and optional byte offset.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// Human-readable description of the parse error.
    pub message: String,
    /// Byte offset in the source where the error occurred, if known.
    pub offset: Option<u32>,
}

/// Result type returned by all public parse functions.
pub type ParseResult<T> = Result<T, Vec<ParseError>>;

// ---------------------------------------------------------------------------
// Core: callback-based parsing
// ---------------------------------------------------------------------------

/// Parse `source` and invoke `f` with the resulting AST.
///
/// This is the primary entry point. The callback receives a reference to the
/// oxc `Program` that borrows the arena — it must extract whatever owned data
/// it needs before returning.
pub fn parse_with<F, R>(source: &str, source_type: SourceType, f: F) -> ParseResult<R>
where
    F: FnOnce(&Program<'_>) -> R,
{
    let allocator = Allocator::default();
    let ret = oxc_parser::Parser::new(&allocator, source, source_type).parse();

    if ret.panicked || !ret.errors.is_empty() {
        Err(convert_errors(ret.errors))
    } else {
        Ok(f(&ret.program))
    }
}

// ---------------------------------------------------------------------------
// Owned summary
// ---------------------------------------------------------------------------

/// An owned, simplified representation of a parsed program.
///
/// Downstream passes that need the full AST should use `parse_with` directly.
#[derive(Debug)]
pub struct ParsedProgram {
    /// Whether the parser resolved this file as an ES module.
    pub is_module: bool,
    /// Whether any (possibly recoverable) errors were present.
    pub has_errors: bool,
    /// Number of top-level statements.
    pub statement_count: usize,
    /// A copy of the original source text.
    pub source: String,
}

// ---------------------------------------------------------------------------
// Convenience wrappers
// ---------------------------------------------------------------------------

/// Parse a JavaScript source string (script mode).
pub fn parse_js(source: &str) -> ParseResult<ParsedProgram> {
    let source_type = SourceType::mjs();
    parse_with(source, source_type, |program| {
        to_parsed_program(program, source)
    })
}

/// Parse a TypeScript source string.
pub fn parse_ts(source: &str) -> ParseResult<ParsedProgram> {
    let source_type = SourceType::ts();
    parse_with(source, source_type, |program| {
        to_parsed_program(program, source)
    })
}

/// Parse source, detecting the language from `filename` extension.
///
/// Falls back to JavaScript-module if the extension is unrecognised.
pub fn parse(source: &str, filename: &str) -> ParseResult<ParsedProgram> {
    let source_type = detect_source_type(filename);
    parse_with(source, source_type, |program| {
        to_parsed_program(program, source)
    })
}

/// Detect `SourceType` from a filename extension.
///
/// Returns `SourceType::mjs()` (JavaScript module) for unknown extensions.
pub fn detect_source_type(filename: &str) -> SourceType {
    SourceType::from_path(filename).unwrap_or_else(|_| SourceType::mjs())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn to_parsed_program(program: &Program<'_>, source: &str) -> ParsedProgram {
    ParsedProgram {
        is_module: program.source_type.is_module(),
        has_errors: false,
        statement_count: program.body.len(),
        source: source.to_string(),
    }
}

/// Convert oxc diagnostics into our `ParseError` representation.
fn convert_errors(errors: Vec<OxcDiagnostic>) -> Vec<ParseError> {
    errors
        .into_iter()
        .map(|diag| {
            let offset = diag
                .labels
                .as_ref()
                .and_then(|labels| labels.first())
                .map(|label| label.offset() as u32);
            ParseError {
                message: diag.to_string(),
                offset,
            }
        })
        .collect()
}

// ===========================================================================
// Tests
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Basic statement parsing
    // -----------------------------------------------------------------------

    #[test]
    fn parse_variable_declaration() {
        let result = parse_js("let x = 1;");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_function_declaration() {
        let result = parse_js("function foo() {}");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_class_declaration() {
        let result = parse_js("class Foo {}");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_arrow_function() {
        let result = parse_js("const f = (x) => x + 1;");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_async_await() {
        let result = parse_js("async function f() { await 1; }");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_for_loop() {
        let result = parse_js("for (let i = 0; i < 10; i++) {}");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_destructuring() {
        let result = parse_js("const { a, b } = obj;");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_template_literal() {
        let result = parse_js("const s = `hello ${name}`;");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_try_catch() {
        let result = parse_js("try { throw 1; } catch(e) {}");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    // -----------------------------------------------------------------------
    // Modules
    // -----------------------------------------------------------------------

    #[test]
    fn parse_module_import_export() {
        let source = "import { foo } from 'bar'; export default 42;";
        let result = parse_js(source);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 2);
        assert!(prog.is_module);
    }

    // -----------------------------------------------------------------------
    // TypeScript
    // -----------------------------------------------------------------------

    #[test]
    fn parse_typescript() {
        let source = "function add(a: number, b: number): number { return a + b; }";
        let result = parse_ts(source);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    #[test]
    fn parse_ts_interface() {
        let source = "interface Foo { bar: string; baz: number; }";
        let result = parse_ts(source);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_syntax_error() {
        let result = parse_js("let 123 = 1;");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(!errors.is_empty());
        // The error should have an offset pointing near the problem.
        assert!(errors[0].offset.is_some());
    }

    #[test]
    fn parse_empty_source() {
        let result = parse_js("");
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 0);
    }

    // -----------------------------------------------------------------------
    // Large expression
    // -----------------------------------------------------------------------

    #[test]
    fn parse_large_expression() {
        // Build a chain: 1 + 2 + 3 + ... + 200
        let expr: String = (1..=200)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" + ");
        let source = format!("{expr};");
        let result = parse_js(&source);
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(prog.statement_count, 1);
    }

    // -----------------------------------------------------------------------
    // detect_source_type
    // -----------------------------------------------------------------------

    #[test]
    fn detect_source_type_js() {
        let st = detect_source_type("app.js");
        assert!(st.is_javascript());
    }

    #[test]
    fn detect_source_type_ts() {
        let st = detect_source_type("app.ts");
        assert!(st.is_typescript());
    }

    #[test]
    fn detect_source_type_mjs_is_module() {
        let st = detect_source_type("app.mjs");
        assert!(st.is_module());
        assert!(st.is_javascript());
    }

    #[test]
    fn detect_source_type_tsx() {
        let st = detect_source_type("component.tsx");
        assert!(st.is_typescript());
        assert!(st.is_jsx());
    }

    #[test]
    fn detect_source_type_unknown_fallback() {
        let st = detect_source_type("readme.txt");
        // Falls back to JavaScript module
        assert!(st.is_javascript());
    }

    // -----------------------------------------------------------------------
    // parse_with callback pattern
    // -----------------------------------------------------------------------

    #[test]
    fn parse_with_callback() {
        let count = parse_with("let a = 1; let b = 2;", SourceType::mjs(), |prog| {
            prog.body.len()
        });
        assert_eq!(count.unwrap(), 2);
    }

    #[test]
    fn parse_with_error_returns_err() {
        let result = parse_with("let = ;", SourceType::mjs(), |_| unreachable!());
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // parse_js vs parse_ts
    // -----------------------------------------------------------------------

    #[test]
    fn parse_js_rejects_ts_syntax() {
        // Type annotations are not valid JS.
        let result = parse_js("function f(x: number) {}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_ts_accepts_ts_syntax() {
        let result = parse_ts("function f(x: number) {}");
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // parse (filename-based detection)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_detects_ts_from_filename() {
        let source = "const x: number = 42;";
        let result = parse(source, "app.ts");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_detects_js_from_filename() {
        let source = "const x = 42;";
        let result = parse(source, "app.js");
        assert!(result.is_ok());
    }

    #[test]
    fn parse_source_preserved() {
        let source = "let x = 1;";
        let result = parse(source, "test.js");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().source, source);
    }
}
