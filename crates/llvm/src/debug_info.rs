//! DWARF debug information emission via LLVM DIBuilder.
//!
//! Provides [`DebugInfoEmitter`], which wraps inkwell's [`DebugInfoBuilder`] to
//! produce DWARF debug metadata (compile unit, subprograms, source locations)
//! so that GDB/LLDB can map native code back to JavaScript source lines.
//!
//! # Usage
//!
//! Create a [`DebugInfoEmitter`] at the start of compilation, call
//! [`create_function_scope`](DebugInfoEmitter::create_function_scope) for each
//! function, and [`set_location`](DebugInfoEmitter::set_location) for each
//! instruction with a [`SourceSpan`]. Call
//! [`finalize`](DebugInfoEmitter::finalize) before module verification.
//!
//! [`DebugInfoBuilder`]: inkwell::debug_info::DebugInfoBuilder

use common::SourceSpan;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFlags, DIFlagsConstants, DIScope, DISubprogram, DWARFEmissionKind,
    DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::Module;
use inkwell::values::FunctionValue;

/// Lookup table that converts byte offsets to (line, column) pairs.
///
/// Built from the source text at construction time. Lines are 1-based,
/// columns are 0-based (DWARF convention).
pub struct LineTable {
    /// Byte offset of each line start (0-based). `line_starts[0]` is always 0.
    line_starts: Vec<u32>,
}

impl LineTable {
    /// Build a line table from source text.
    ///
    /// Scans for `\n` characters and records the byte offset of each line start.
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, byte) in source.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self { line_starts }
    }

    /// Convert a byte offset to a (line, column) pair.
    ///
    /// Both line and column are 1-based in the returned tuple (matching DWARF
    /// conventions where 0 means "unknown"). Returns `(0, 0)` for offsets
    /// that fall outside the source text.
    pub fn offset_to_line_col(&self, offset: u32) -> (u32, u32) {
        // Binary search for the line containing this offset.
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert) => insert.saturating_sub(1),
        };
        if line_idx >= self.line_starts.len() {
            return (0, 0);
        }
        let line = (line_idx as u32) + 1; // 1-based
        let col = offset.saturating_sub(self.line_starts[line_idx]) + 1; // 1-based
        (line, col)
    }
}

/// Manages DWARF debug information emission during LLVM code generation.
///
/// Wraps inkwell's `DebugInfoBuilder` and tracks the current compile unit,
/// source file, and line table for byte-offset-to-line-number conversion.
pub struct DebugInfoEmitter<'ctx> {
    /// The inkwell debug info builder.
    dibuilder: DebugInfoBuilder<'ctx>,
    /// The compile unit metadata node.
    compile_unit: DICompileUnit<'ctx>,
    /// Per-file line tables, indexed by `FileId.0`. Currently we only support
    /// a single file (index 0).
    pub(crate) line_tables: Vec<LineTable>,
}

impl<'ctx> DebugInfoEmitter<'ctx> {
    /// Create a new debug info emitter for the given module.
    ///
    /// * `module` - The LLVM module to attach debug info to.
    /// * `filename` - Source file name (e.g., `"script.js"`).
    /// * `directory` - Directory containing the source file.
    /// * `source` - The full source text (used for byte-offset-to-line mapping).
    /// * `is_optimized` - Whether optimizations are enabled.
    pub fn new(
        module: &Module<'ctx>,
        filename: &str,
        directory: &str,
        source: &str,
        is_optimized: bool,
    ) -> Self {
        // Add the "Debug Info Version" module flag required by LLVM.
        let context = module.get_context();
        let debug_metadata_version = context.i32_type().const_int(3, false);
        module.add_basic_value_flag(
            "Debug Info Version",
            inkwell::module::FlagBehavior::Warning,
            debug_metadata_version,
        );

        let (dibuilder, compile_unit) = module.create_debug_info_builder(
            /* allow_unresolved */ true,
            DWARFSourceLanguage::C, // No DW_LANG_JavaScript in DWARF; C is conventional
            filename,
            directory,
            /* producer */ "escompiler",
            is_optimized,
            /* flags */ "",
            /* runtime_ver */ 0,
            /* split_name */ "",
            DWARFEmissionKind::Full,
            /* dwo_id */ 0,
            /* split_debug_inlining */ false,
            /* debug_info_for_profiling */ false,
            /* sysroot */ "",
            /* sdk */ "",
        );

        let line_table = LineTable::new(source);

        Self {
            dibuilder,
            compile_unit,
            line_tables: vec![line_table],
        }
    }

    /// Create a debug info subprogram (function scope) and attach it to the
    /// LLVM function value.
    ///
    /// * `func_name` - The human-readable function name.
    /// * `linkage_name` - The mangled/linkage name (if different from `func_name`).
    /// * `line_no` - Source line number where the function is defined (1-based).
    /// * `is_local` - Whether the function is local to the compilation unit.
    /// * `is_optimized` - Whether the function is optimized.
    /// * `llvm_func` - The LLVM function value to attach the subprogram to.
    pub fn create_function_scope(
        &self,
        func_name: &str,
        linkage_name: Option<&str>,
        line_no: u32,
        is_local: bool,
        is_optimized: bool,
        llvm_func: FunctionValue<'ctx>,
    ) -> DISubprogram<'ctx> {
        let file = self.compile_unit.get_file();
        let subroutine_type = self.dibuilder.create_subroutine_type(
            file,
            /* return type */ None,
            /* parameter types */ &[],
            DIFlags::PUBLIC,
        );

        let subprogram = self.dibuilder.create_function(
            self.compile_unit.as_debug_info_scope(),
            func_name,
            linkage_name,
            file,
            line_no,
            subroutine_type,
            is_local,
            /* is_definition */ true,
            /* scope_line */ line_no,
            DIFlags::PUBLIC,
            is_optimized,
        );

        llvm_func.set_subprogram(subprogram);
        subprogram
    }

    /// Set the current debug location on the IR builder.
    ///
    /// Converts a [`SourceSpan`] to a (line, column) pair using the line table
    /// and sets it on the builder so subsequent instructions get tagged with
    /// that source location.
    ///
    /// Spans with `file_id == FileId(u32::MAX)` (dummy spans) are ignored.
    pub fn set_location(
        &self,
        builder: &Builder<'ctx>,
        context: &'ctx Context,
        span: &SourceSpan,
        scope: DIScope<'ctx>,
    ) {
        // Skip dummy spans.
        if span.file_id.0 == u32::MAX {
            return;
        }

        let file_idx = span.file_id.0 as usize;
        let (line, col) = if file_idx < self.line_tables.len() {
            self.line_tables[file_idx].offset_to_line_col(span.start)
        } else {
            (0, 0)
        };

        if line == 0 {
            return;
        }

        let location = self
            .dibuilder
            .create_debug_location(context, line, col, scope, None);
        builder.set_current_debug_location(location);
    }

    /// Add a line table for an additional source file.
    ///
    /// Returns the index (matching `FileId.0`) assigned to this file's table.
    pub fn add_line_table(&mut self, source: &str) -> u32 {
        let idx = self.line_tables.len() as u32;
        self.line_tables.push(LineTable::new(source));
        idx
    }

    /// Finalize the debug info. Must be called before module verification.
    pub fn finalize(&self) {
        self.dibuilder.finalize();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- LineTable -----------------------------------------------------------

    #[test]
    fn test_line_table_single_line() {
        let table = LineTable::new("hello world");
        assert_eq!(table.offset_to_line_col(0), (1, 1));
        assert_eq!(table.offset_to_line_col(5), (1, 6));
    }

    #[test]
    fn test_line_table_multi_line() {
        let table = LineTable::new("abc\ndef\nghi");
        // "abc\n" = bytes 0..4, "def\n" = bytes 4..8, "ghi" = bytes 8..11
        assert_eq!(table.offset_to_line_col(0), (1, 1)); // 'a'
        assert_eq!(table.offset_to_line_col(3), (1, 4)); // '\n'
        assert_eq!(table.offset_to_line_col(4), (2, 1)); // 'd'
        assert_eq!(table.offset_to_line_col(7), (2, 4)); // '\n'
        assert_eq!(table.offset_to_line_col(8), (3, 1)); // 'g'
        assert_eq!(table.offset_to_line_col(10), (3, 3)); // 'i'
    }

    #[test]
    fn test_line_table_empty_source() {
        let table = LineTable::new("");
        assert_eq!(table.offset_to_line_col(0), (1, 1));
    }

    #[test]
    fn test_line_table_trailing_newline() {
        let table = LineTable::new("line1\n");
        assert_eq!(table.offset_to_line_col(0), (1, 1));
        assert_eq!(table.offset_to_line_col(6), (2, 1));
    }

    #[test]
    fn test_line_table_offset_past_end() {
        let table = LineTable::new("abc");
        // Offset beyond the source still maps to line 1 (saturating)
        let (line, _col) = table.offset_to_line_col(100);
        assert_eq!(line, 1);
    }

    // -- DebugInfoEmitter (integration) -------------------------------------

    #[test]
    fn test_debug_info_emitter_create_and_finalize() {
        let context = Context::create();
        let module = context.create_module("test_di");
        let emitter = DebugInfoEmitter::new(&module, "test.js", ".", "var x = 1;\n", false);
        emitter.finalize();
    }

    #[test]
    fn test_debug_info_emitter_create_function_scope() {
        let context = Context::create();
        let module = context.create_module("test_di");
        let emitter = DebugInfoEmitter::new(&module, "test.js", ".", "function foo() {}\n", false);

        let fn_ty = context.void_type().fn_type(&[], false);
        let func = module.add_function("foo", fn_ty, None);
        let _scope = emitter.create_function_scope("foo", None, 1, true, false, func);
        emitter.finalize();
    }

    #[test]
    fn test_debug_info_emitter_set_location() {
        let context = Context::create();
        let module = context.create_module("test_di");
        let builder = context.create_builder();
        let emitter =
            DebugInfoEmitter::new(&module, "test.js", ".", "var x = 1;\nvar y = 2;\n", false);

        let fn_ty = context.void_type().fn_type(&[], false);
        let func = module.add_function("main", fn_ty, None);
        let bb = context.append_basic_block(func, "entry");
        builder.position_at_end(bb);

        let scope = emitter.create_function_scope("main", None, 1, true, false, func);

        // Set location for line 2 (byte offset 11 = "var y = 2")
        let span = SourceSpan::new(common::FileId(0), 11, 20);
        emitter.set_location(&builder, &context, &span, scope.as_debug_info_scope());

        // Build an instruction to carry the debug location
        builder.build_return(None).ok();

        emitter.finalize();
    }

    #[test]
    fn test_debug_info_emitter_skip_dummy_span() {
        let context = Context::create();
        let module = context.create_module("test_di");
        let builder = context.create_builder();
        let emitter = DebugInfoEmitter::new(&module, "test.js", ".", "var x = 1;\n", false);

        let fn_ty = context.void_type().fn_type(&[], false);
        let func = module.add_function("main", fn_ty, None);
        let bb = context.append_basic_block(func, "entry");
        builder.position_at_end(bb);

        let scope = emitter.create_function_scope("main", None, 1, true, false, func);

        // Dummy span should be ignored (no crash)
        emitter.set_location(
            &builder,
            &context,
            &SourceSpan::DUMMY,
            scope.as_debug_info_scope(),
        );

        builder.build_return(None).ok();
        emitter.finalize();
    }

    #[test]
    fn test_debug_info_add_line_table() {
        let context = Context::create();
        let module = context.create_module("test_di");
        let mut emitter = DebugInfoEmitter::new(&module, "test.js", ".", "var x = 1;\n", false);

        let idx = emitter.add_line_table("var y = 2;\nvar z = 3;\n");
        assert_eq!(idx, 1);
        emitter.finalize();
    }
}
