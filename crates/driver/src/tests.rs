//! Tests for driver.

use crate::config::{self, EscConfig};
use crate::error::DriverError;
use crate::phase_timer::PhaseTimer;
use crate::{CompileMode, CompileTarget, CompilerConfig, EmitKind};
use common::Edition;

// ---------------------------------------------------------------------------
// CompilerConfig construction
// ---------------------------------------------------------------------------

#[test]
fn test_compiler_config_new() {
    let config = CompilerConfig::new(vec!["input.js".to_string()]);
    assert_eq!(config.mode, CompileMode::Debug);
    assert_eq!(config.target, CompileTarget::Executable);
    assert_eq!(config.input, vec!["input.js"]);
    assert!(config.output.is_empty());
    assert_eq!(config.emit, None);
    assert!(!config.heap_only);
    assert!(!config.time_phases);
    assert_eq!(config.edition, Edition::ES2025);
}

#[test]
fn test_compiler_config_all_fields() {
    let config = CompilerConfig {
        mode: CompileMode::Release,
        target: CompileTarget::SharedLib,
        input: vec!["a.js".to_string(), "b.js".to_string()],
        output: "out.so".to_string(),
        emit: Some(EmitKind::LlvmIr),
        heap_only: true,
        time_phases: true,
        edition: Edition::ES2020,
        esc_config: None,
        source_map: false,
        out_dir: None,
        config_path: None,
        no_config: false,
        allow_ffi: false,
        ffi_flag: None,
        allow_eval: true,
        allow_jit: true,
        permissions: host::PermissionsConfig::new(),
        permissions_from_cli: false,
    };
    assert_eq!(config.mode, CompileMode::Release);
    assert_eq!(config.target, CompileTarget::SharedLib);
    assert_eq!(config.emit, Some(EmitKind::LlvmIr));
    assert!(config.heap_only);
    assert!(config.time_phases);
    assert_eq!(config.edition, Edition::ES2020);
}

// ---------------------------------------------------------------------------
// Enum equality / debug
// ---------------------------------------------------------------------------

#[test]
fn test_compile_mode_debug_trait() {
    assert_eq!(format!("{:?}", CompileMode::Debug), "Debug");
    assert_eq!(format!("{:?}", CompileMode::Release), "Release");
}

#[test]
fn test_compile_target_equality() {
    assert_eq!(CompileTarget::Executable, CompileTarget::Executable);
    assert_ne!(CompileTarget::Executable, CompileTarget::Wasm);
}

#[test]
fn test_emit_kind_equality() {
    assert_eq!(EmitKind::Ir, EmitKind::Ir);
    assert_ne!(EmitKind::Ast, EmitKind::Asm);
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn test_no_input_error() {
    let config = CompilerConfig::new(vec![]);
    let result = crate::compile(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, DriverError::NoInput),
        "expected NoInput, got: {err}"
    );
}

#[test]
fn test_file_not_found_error() {
    let config = CompilerConfig::new(vec!["/nonexistent/path/to/file.js".to_string()]);
    let result = crate::compile(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, DriverError::FileNotFound(_)),
        "expected FileNotFound, got: {err}"
    );
}

#[test]
fn test_check_no_input_error() {
    let config = CompilerConfig::new(vec![]);
    let result = crate::check(&config);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), DriverError::NoInput));
}

#[test]
fn test_check_file_not_found() {
    let config = CompilerConfig::new(vec!["/nonexistent/bad_file.js".to_string()]);
    let result = crate::check(&config);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), DriverError::FileNotFound(_)));
}

// ---------------------------------------------------------------------------
// Valid source through check (parse + lower + verify)
// ---------------------------------------------------------------------------

#[test]
fn test_check_valid_source() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.mjs");
    std::fs::write(&file_path, "let x = 1 + 2;\n").unwrap();

    let config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    let result = crate::check(&config);
    assert!(
        result.is_ok(),
        "check() should succeed for valid JS: {result:?}"
    );
}

#[test]
fn test_check_invalid_syntax() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bad.mjs");
    std::fs::write(&file_path, "let x = ;\n").unwrap();

    let config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    let result = crate::check(&config);
    // Parse errors become Lowering errors since lower_program wraps the parse step.
    assert!(result.is_err(), "check() should fail for invalid syntax");
}

// ---------------------------------------------------------------------------
// Emit IR early-exit
// ---------------------------------------------------------------------------

#[test]
fn test_emit_ir_flag() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("emit.mjs");
    std::fs::write(&file_path, "let x = 42;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config.emit = Some(EmitKind::Ir);
    // --emit ir should succeed (early exit before codegen)
    let result = crate::compile(&config);
    assert!(
        result.is_ok(),
        "compile with --emit ir should succeed: {result:?}"
    );
    let compile_result = result.unwrap();
    // output_path is empty when emitting IR
    assert!(compile_result.output_path.is_empty());
}

// ---------------------------------------------------------------------------
// Compile pipeline (codegen may fail since Cranelift backend is stub)
// ---------------------------------------------------------------------------

#[test]
fn test_compile_valid_source_reaches_codegen() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("codegen.mjs");
    std::fs::write(&file_path, "let x = 1 + 2;\n").unwrap();

    let config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    let result = crate::compile(&config);
    // Verify the pipeline reaches at least the codegen phase.
    // Linking may fail if the runtime staticlib is not found from the test CWD.
    match result {
        Ok(_) => {}                        // Full pipeline succeeded
        Err(DriverError::Codegen(_)) => {} // Codegen error (e.g., unimplemented op)
        Err(DriverError::Linker(_)) => {}  // Linker error (e.g., missing runtime lib)
        Err(other) => panic!("unexpected error phase: {other}"),
    }
}

// ---------------------------------------------------------------------------
// PhaseTimer
// ---------------------------------------------------------------------------

#[test]
fn test_phase_timer_enabled() {
    let mut timer = PhaseTimer::new(true);
    timer.start("parse");
    timer.end("parse");
    timer.start("codegen");
    timer.end("codegen");
    let timings = timer.finish();
    assert!(timings.is_some());
    let timings = timings.unwrap();
    assert_eq!(timings.phases.len(), 2);
    assert_eq!(timings.phases[0].0, "parse");
    assert_eq!(timings.phases[1].0, "codegen");
}

#[test]
fn test_phase_timer_disabled() {
    let mut timer = PhaseTimer::new(false);
    timer.start("parse");
    timer.end("parse");
    let timings = timer.finish();
    assert!(timings.is_none());
}

#[test]
fn test_phase_timer_records_duration() {
    let mut timer = PhaseTimer::new(true);
    timer.start("slow");
    // Busy-wait briefly to ensure a non-zero duration
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_micros(10) {
        std::hint::spin_loop();
    }
    timer.end("slow");
    let timings = timer.finish().unwrap();
    assert!(
        !timings.phases[0].1.is_zero(),
        "phase duration should be > 0"
    );
}

// ---------------------------------------------------------------------------
// DriverError display
// ---------------------------------------------------------------------------

#[test]
fn test_driver_error_display_no_input() {
    let err = DriverError::NoInput;
    assert_eq!(err.to_string(), "no input files");
}

#[test]
fn test_driver_error_display_file_not_found() {
    let err = DriverError::FileNotFound("missing.js".to_string());
    assert!(err.to_string().contains("missing.js"));
}

#[test]
fn test_driver_error_display_lowering() {
    let err = DriverError::Lowering(vec!["err1".to_string(), "err2".to_string()]);
    let msg = err.to_string();
    assert!(msg.contains("err1"));
    assert!(msg.contains("err2"));
    assert!(msg.contains("; "));
}

#[test]
fn test_driver_error_display_verification() {
    let err = DriverError::Verification(vec!["bad phi".to_string()]);
    assert!(err.to_string().contains("bad phi"));
}

#[test]
fn test_driver_error_display_codegen() {
    let err = DriverError::Codegen("register alloc failed".to_string());
    assert!(err.to_string().contains("register alloc"));
}

// ---------------------------------------------------------------------------
// DriverError -> Vec<CompileError> conversion
// ---------------------------------------------------------------------------

#[test]
fn test_driver_error_to_compile_errors() {
    let err = DriverError::NoInput;
    let compile_errors: Vec<common::CompileError> = err.into();
    assert_eq!(compile_errors.len(), 1);
    assert!(compile_errors[0].to_string().contains("no input files"));
}

// ---------------------------------------------------------------------------
// Runtime library discovery
// ---------------------------------------------------------------------------

#[test]
fn test_find_runtime_lib_returns_option() {
    // find_runtime_lib returns Some when the lib exists on disk, None otherwise.
    // We just verify it doesn't panic and returns a valid Option.
    let result = crate::pipeline::find_runtime_lib();
    if let Some(ref path) = result {
        // The path comes from either an on-disk search (libruntime.a) or
        // the embedded-fallback extraction (libruntime-<ver>-<hash>.a — ESC-59).
        // Both are valid runtime staticlib paths.
        let expected_ext = if cfg!(windows) { ".lib" } else { ".a" };
        assert!(
            path.ends_with(expected_ext),
            "path should end with {expected_ext}, got: {path}"
        );
        // The file should actually exist.
        assert!(
            std::path::Path::new(path).exists(),
            "path should exist: {path}"
        );
    }
}

#[test]
fn test_pipeline_config_includes_runtime_lib_field() {
    // Verify that LinkerConfig has a runtime_lib field that can be set.
    let config = linker::LinkerConfig {
        format: linker::OutputFormat::Executable,
        output_path: "test_out".to_string(),
        objects: vec!["test.o".to_string()],
        runtime_lib: Some("/path/to/libruntime.a".to_string()),
    };
    assert_eq!(config.runtime_lib.as_deref(), Some("/path/to/libruntime.a"));
}

// ---------------------------------------------------------------------------
// Module pipeline: build_module_graph
// ---------------------------------------------------------------------------

#[test]
fn test_module_graph_single_file_no_imports() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.mjs");
    std::fs::write(&entry, "export const x = 42;\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&entry).unwrap();
    assert_eq!(graph.modules().len(), 1);
    // Single module should compile fine in topological order.
    let order = graph.compilation_order().unwrap();
    assert_eq!(order.len(), 1);
}

#[test]
fn test_module_graph_two_file_chain() {
    let dir = tempfile::tempdir().unwrap();

    // Create a dependency: main.mjs imports from utils.mjs
    let utils = dir.path().join("utils.mjs");
    std::fs::write(&utils, "export const helper = 1;\n").unwrap();

    let main = dir.path().join("main.mjs");
    std::fs::write(
        &main,
        "import { helper } from './utils.mjs';\nconsole.log(helper);\n",
    )
    .unwrap();

    let graph = crate::module_pipeline::build_module_graph(&main).unwrap();
    assert_eq!(graph.modules().len(), 2);

    let order = graph.compilation_order().unwrap();
    assert_eq!(order.len(), 2);

    // utils.mjs should be compiled before main.mjs (dependency first)
    let main_canonical = main.canonicalize().unwrap();
    let utils_canonical = utils.canonicalize().unwrap();

    let pos_utils = order
        .iter()
        .position(|&id| graph.get_module(id).unwrap().path == utils_canonical)
        .unwrap();
    let pos_main = order
        .iter()
        .position(|&id| graph.get_module(id).unwrap().path == main_canonical)
        .unwrap();
    assert!(
        pos_utils < pos_main,
        "utils should come before main in compilation order"
    );
}

#[test]
fn test_module_graph_three_file_transitive() {
    let dir = tempfile::tempdir().unwrap();

    // c.mjs (leaf)
    let c = dir.path().join("c.mjs");
    std::fs::write(&c, "export const C = 3;\n").unwrap();

    // b.mjs imports c
    let b = dir.path().join("b.mjs");
    std::fs::write(
        &b,
        "import { C } from './c.mjs';\nexport const B = C + 1;\n",
    )
    .unwrap();

    // a.mjs imports b
    let a = dir.path().join("a.mjs");
    std::fs::write(
        &a,
        "import { B } from './b.mjs';\nexport const A = B + 1;\n",
    )
    .unwrap();

    let graph = crate::module_pipeline::build_module_graph(&a).unwrap();
    assert_eq!(graph.modules().len(), 3);

    let order = graph.compilation_order().unwrap();
    assert_eq!(order.len(), 3);
}

#[test]
fn test_module_graph_circular_dependency_detected() {
    let dir = tempfile::tempdir().unwrap();

    // a.mjs imports b, b.mjs imports a — cycle!
    let a = dir.path().join("a.mjs");
    let b = dir.path().join("b.mjs");
    std::fs::write(&a, "import { y } from './b.mjs';\nexport const x = y;\n").unwrap();
    std::fs::write(&b, "import { x } from './a.mjs';\nexport const y = x;\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&a).unwrap();
    assert_eq!(graph.modules().len(), 2);

    // Compilation order should fail due to cycle
    let result = graph.compilation_order();
    assert!(result.is_err(), "should detect circular dependency");
}

#[test]
fn test_module_graph_missing_entry_file() {
    let dir = tempfile::tempdir().unwrap();
    let nonexistent = dir.path().join("nonexistent.mjs");

    let result = crate::module_pipeline::build_module_graph(&nonexistent);
    assert!(result.is_err());
}

#[test]
fn test_module_graph_unresolvable_import_skipped() {
    let dir = tempfile::tempdir().unwrap();

    // Import from a non-existent module — treated as external, not an error
    let main = dir.path().join("main.mjs");
    std::fs::write(&main, "import { foo } from 'nonexistent-package';\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&main).unwrap();
    // Should have 1 module (the entry), the unresolvable import is ignored
    assert_eq!(graph.modules().len(), 1);
}

// ---------------------------------------------------------------------------
// Module pipeline: lower_all_modules
// ---------------------------------------------------------------------------

#[test]
fn test_lower_single_module() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.mjs");
    std::fs::write(&entry, "export const x = 42;\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&entry).unwrap();
    let (results, export_map) = crate::module_pipeline::lower_all_modules(&graph).unwrap();

    assert_eq!(results.len(), 1);
    // The module should have an export named "x"
    assert!(
        !export_map.is_empty(),
        "export map should have entries for 'x'"
    );
    let module_id = results[0].module_id;
    let lookup = export_map.get(module_id, "x");
    assert!(lookup.is_some(), "should find export 'x' in map");
}

#[test]
fn test_lower_two_modules_topological_order() {
    let dir = tempfile::tempdir().unwrap();

    let utils = dir.path().join("utils.mjs");
    std::fs::write(&utils, "export const helper = 1;\n").unwrap();

    let main = dir.path().join("main.mjs");
    std::fs::write(
        &main,
        "import { helper } from './utils.mjs';\nexport const x = helper;\n",
    )
    .unwrap();

    let graph = crate::module_pipeline::build_module_graph(&main).unwrap();
    let (results, export_map) = crate::module_pipeline::lower_all_modules(&graph).unwrap();

    assert_eq!(results.len(), 2);

    // utils should be lowered first (dependency first in topological order)
    let utils_canonical = utils.canonicalize().unwrap();
    assert_eq!(
        results[0].path, utils_canonical,
        "dependency should be lowered first"
    );

    // Both modules should have exports in the map
    assert!(export_map.get(results[0].module_id, "helper").is_some());
    assert!(export_map.get(results[1].module_id, "x").is_some());
}

#[test]
fn test_lower_module_with_default_export() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.mjs");
    std::fs::write(&entry, "export default 42;\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&entry).unwrap();
    let (results, export_map) = crate::module_pipeline::lower_all_modules(&graph).unwrap();

    assert_eq!(results.len(), 1);
    let module_id = results[0].module_id;
    let lookup = export_map.get(module_id, "default");
    assert!(lookup.is_some(), "should find default export in map");
    let (_, local) = lookup.unwrap();
    assert_eq!(local, "__default");
}

#[test]
fn test_lower_circular_dependency_error() {
    let dir = tempfile::tempdir().unwrap();

    let a = dir.path().join("a.mjs");
    let b = dir.path().join("b.mjs");
    std::fs::write(&a, "import { y } from './b.mjs';\nexport const x = y;\n").unwrap();
    std::fs::write(&b, "import { x } from './a.mjs';\nexport const y = x;\n").unwrap();

    let graph = crate::module_pipeline::build_module_graph(&a).unwrap();
    let result = crate::module_pipeline::lower_all_modules(&graph);

    assert!(result.is_err(), "should fail on circular dependency");
}

// ---------------------------------------------------------------------------
// Export recording during lowering
// ---------------------------------------------------------------------------

#[test]
fn test_export_recording_named() {
    let source = "export const x = 1;\nexport function foo() {}\n";
    let result = desugar::lower_program(source).unwrap();

    // Should have recorded two named exports: "x" and "foo"
    assert_eq!(result.exports.len(), 2);
    assert_eq!(result.exports[0].name, "x");
    assert_eq!(result.exports[0].kind, desugar::ExportKind::Named);
    assert_eq!(result.exports[1].name, "foo");
    assert_eq!(result.exports[1].kind, desugar::ExportKind::Named);
}

#[test]
fn test_export_recording_default() {
    let source = "export default 42;\n";
    let result = desugar::lower_program(source).unwrap();

    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "default");
    assert_eq!(result.exports[0].kind, desugar::ExportKind::Default);
}

#[test]
fn test_export_recording_re_export() {
    let source = "export { foo } from './bar.mjs';\n";
    let result = desugar::lower_program(source).unwrap();

    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "foo");
    assert!(matches!(
        &result.exports[0].kind,
        desugar::ExportKind::ReExport { source } if source == "./bar.mjs"
    ));
}

#[test]
fn test_export_recording_export_all() {
    let source = "export * from './mod.mjs';\n";
    let result = desugar::lower_program(source).unwrap();

    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "*");
    assert!(matches!(
        &result.exports[0].kind,
        desugar::ExportKind::ReExport { source } if source == "./mod.mjs"
    ));
}

#[test]
fn test_export_recording_mixed() {
    let source = r#"
export const x = 1;
export default class Foo {}
export { x as y };
"#;
    let result = desugar::lower_program(source).unwrap();

    // Should have 3 exports: "x" (named), "default", "y" (named)
    assert_eq!(result.exports.len(), 3);

    let names: Vec<&str> = result.exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"x"));
    assert!(names.contains(&"default"));
    assert!(names.contains(&"y"));
}

#[test]
fn test_export_recording_script_mode_empty() {
    // Script mode has no exports
    let source = "const x = 1;\n";
    let result = desugar::lower_script(source).unwrap();
    assert!(result.exports.is_empty());
}

// ---------------------------------------------------------------------------
// ModuleExportMap
// ---------------------------------------------------------------------------

#[test]
fn test_module_export_map_basic() {
    use modules::ModuleId;

    let mut map = crate::module_pipeline::ModuleExportMap::new();
    assert!(map.is_empty());

    map.insert(
        ModuleId(0),
        "foo".to_string(),
        ModuleId(0),
        "foo".to_string(),
    );
    assert_eq!(map.len(), 1);
    assert!(!map.is_empty());

    let result = map.get(ModuleId(0), "foo");
    assert!(result.is_some());
    let (target_id, local) = result.unwrap();
    assert_eq!(target_id, ModuleId(0));
    assert_eq!(local, "foo");
}

#[test]
fn test_module_export_map_missing_key() {
    use modules::ModuleId;

    let map = crate::module_pipeline::ModuleExportMap::new();
    assert!(map.get(ModuleId(0), "nonexistent").is_none());
}

// ---------------------------------------------------------------------------
// Module merging: merge_modules
// ---------------------------------------------------------------------------

/// Helper: build a minimal `LoweringResult` with the given functions, string
/// table, and optional entry index using the `TypedIrBuilder`.
fn build_lowering_result(
    func_names: &[&str],
    string_table: Vec<String>,
    entry: Option<usize>,
) -> desugar::LoweringResult {
    use ir::IrType;
    use ir::builder::TypedIrBuilder;

    let mut b = TypedIrBuilder::new();
    for name in func_names {
        b.begin_function(name, vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        b.ret(None);
        b.end_function();
    }
    if let Some(e) = entry {
        b.set_entry(e);
    }
    let module = b.finish();
    desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table,
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    }
}

/// Helper: build a `LoweringResult` with a `ConstString` instruction in the
/// first function for testing string index rewriting.
fn build_lowering_result_with_const_string(
    func_names: &[&str],
    string_table: Vec<String>,
    entry: Option<usize>,
    const_string_idx: u32,
) -> desugar::LoweringResult {
    use ir::IrType;
    use ir::builder::TypedIrBuilder;

    let mut b = TypedIrBuilder::new();
    for (i, name) in func_names.iter().enumerate() {
        b.begin_function(name, vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);

        // Add ConstString to the first function
        if i == 0 {
            b.const_string(const_string_idx);
        }

        b.ret(None);
        b.end_function();
    }
    if let Some(e) = entry {
        b.set_entry(e);
    }
    let module = b.finish();
    desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table,
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    }
}

/// Helper: create a simple `ModuleGraph` with the given number of modules
/// in a chain (0→1→2→...) for merge testing. Does not use filesystem.
fn build_test_graph(num_modules: usize) -> modules::ModuleGraph {
    use modules::{ModuleGraph, ModuleId, ModuleSummary};

    let mut graph = ModuleGraph::new();
    for i in 0..num_modules {
        let summary = ModuleSummary {
            id: ModuleId(0),
            path: std::path::PathBuf::from(format!("/test/mod_{i}.mjs")),
            api_hash: modules::ApiHash(i as u64),
            exports: vec![],
            imports: if i > 0 {
                vec![modules::ImportEntry {
                    source: format!("./mod_{}.mjs", i - 1),
                    bindings: vec![],
                    resolved_id: Some((i - 1) as u32),
                }]
            } else {
                vec![]
            },
            is_esm: true,
        };
        graph.add_module(summary);
    }
    graph.resolve_imports().unwrap_or_default();
    graph
}

#[test]
fn test_merge_single_module_identity() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result(&["main"], vec!["hello".to_string()], Some(0));

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    assert_eq!(result.module.functions.len(), 1);
    assert_eq!(result.module.functions[0].name, "main");
    assert_eq!(result.module.entry, Some(0));
    assert_eq!(result.string_table, vec!["hello".to_string()]);
    assert_eq!(result.module_offsets.len(), 1);
    assert_eq!(result.module_offsets[&modules::ModuleId(0)], 0);
}

#[test]
fn test_merge_two_modules_functions_concatenated() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["dep_fn"], vec!["a".to_string()], Some(0));
    let lr_b = build_lowering_result(&["main_fn", "helper"], vec!["b".to_string()], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // dep (module 0) comes first in topological order, then main (module 1).
    assert_eq!(result.module.functions.len(), 3);
    assert_eq!(result.module.functions[0].name, "dep_fn");
    assert_eq!(result.module.functions[1].name, "main_fn");
    assert_eq!(result.module.functions[2].name, "helper");

    // Module offsets
    assert_eq!(result.module_offsets[&modules::ModuleId(0)], 0);
    assert_eq!(result.module_offsets[&modules::ModuleId(1)], 1);
}

#[test]
fn test_merge_string_table_deduplication() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(
        &["fn_a"],
        vec!["shared".to_string(), "only_a".to_string()],
        Some(0),
    );
    let lr_b = build_lowering_result(
        &["fn_b"],
        vec!["shared".to_string(), "only_b".to_string()],
        Some(0),
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // "shared" should appear only once in the merged table.
    assert_eq!(result.string_table.len(), 3);
    assert!(result.string_table.contains(&"shared".to_string()));
    assert!(result.string_table.contains(&"only_a".to_string()));
    assert!(result.string_table.contains(&"only_b".to_string()));
}

#[test]
fn test_merge_const_string_index_rewriting() {
    use ir::Op;

    let graph = build_test_graph(2);

    // Module 0 has string table: ["alpha", "beta"], ConstString(0) references "alpha"
    // Module 1 has string table: ["beta", "gamma"], ConstString(1) references "gamma"
    // After merge, "beta" is deduplicated. Module 1's ConstString(1) for "gamma"
    // should point to the merged index of "gamma".

    let lr_a = build_lowering_result_with_const_string(
        &["fn_a"],
        vec!["alpha".to_string(), "beta".to_string()],
        Some(0),
        0, // ConstString(0) → "alpha"
    );
    let lr_b = build_lowering_result_with_const_string(
        &["fn_b"],
        vec!["beta".to_string(), "gamma".to_string()],
        Some(0),
        1, // ConstString(1) → "gamma"
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Verify "gamma" is at the correct merged index.
    let gamma_idx = result
        .string_table
        .iter()
        .position(|s| s == "gamma")
        .unwrap();

    // Module 1's fn_b had ConstString(1) for "gamma". After merge, it should
    // point to gamma's merged index.
    let fn_b = &result.module.functions[1]; // module 1's function at offset 1
    let const_string_ops: Vec<&ir::TypedInstruction> = fn_b
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter(|i| matches!(i.op, Op::ConstString(_)))
        .collect();
    assert!(!const_string_ops.is_empty(), "should have ConstString ops");

    if let Op::ConstString(idx) = const_string_ops[0].op {
        assert_eq!(
            idx as usize, gamma_idx,
            "ConstString index should be rewritten to merged index"
        );
    } else {
        panic!("expected ConstString op");
    }
}

#[test]
fn test_merge_function_index_rewriting_create_closure() {
    use ir::{IrType, Op};

    let graph = build_test_graph(2);

    // Module 0: 1 function (dep_fn)
    // Module 1: 2 functions (main_fn, closure_fn)
    //   main_fn has: ConstI32(1) as func_ref → CreateClosure(func_ref, ...)
    //   This references closure_fn at index 1 in module 1.
    //   After merge, module 1's offset is 1, so closure_fn becomes index 2.
    //   The ConstI32(1) should be rewritten to ConstI32(2).

    let lr_a = build_lowering_result(&["dep_fn"], vec![], Some(0));

    // Build module 1 manually with CreateClosure referencing func index 1
    let mut b = ir::builder::TypedIrBuilder::new();

    // Function 0: main_fn
    b.begin_function("main_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    let func_ref = b.const_i32(1); // references closure_fn at index 1
    let null_env = b.const_null();
    let flags = b.const_i32(0);
    b.create_closure(func_ref, null_env, flags);
    b.ret(None);
    b.end_function();

    // Function 1: closure_fn
    b.begin_function("closure_fn", vec![], IrType::JSValue);
    let bb2 = b.create_block();
    b.switch_to_block(bb2);
    b.seal_block(bb2);
    let undef = b.const_undefined();
    b.ret(Some(undef));
    b.end_function();

    b.set_entry(0);
    let module = b.finish();

    let lr_b = desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table: vec![],
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    };

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Module 1's offset is 1 (module 0 has 1 function).
    assert_eq!(result.module_offsets[&modules::ModuleId(1)], 1);

    // Find the ConstI32 instruction in main_fn (now at index 1 in merged)
    let main_fn = &result.module.functions[1];
    let const_i32_ops: Vec<&ir::TypedInstruction> = main_fn
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter(|i| matches!(i.op, Op::ConstI32(_)))
        .collect();

    // The func_ref ConstI32(1) should now be ConstI32(2) (offset 1 + original 1)
    let func_ref_instr = const_i32_ops.iter().find(|i| {
        if let Op::ConstI32(v) = i.op {
            v == 2 // rewritten from 1 to 2
        } else {
            false
        }
    });
    assert!(
        func_ref_instr.is_some(),
        "ConstI32(1) should be rewritten to ConstI32(2) after merge, got: {:?}",
        const_i32_ops.iter().map(|i| &i.op).collect::<Vec<_>>()
    );
}

#[test]
fn test_merge_three_module_chain_topological_order() {
    // A→B→C chain: C is leaf, A is entry.
    // Topological order: C, B, A.
    let graph = build_test_graph(3);

    let lr_c = build_lowering_result(&["fn_c"], vec![], None);
    let lr_b = build_lowering_result(&["fn_b"], vec![], None);
    let lr_a = build_lowering_result(&["fn_a"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_c),
            (modules::ModuleId(1), lr_b),
            (modules::ModuleId(2), lr_a),
        ],
        &graph,
    )
    .unwrap();

    // Topological order: 0 (leaf, no deps), then 1 (deps on 0), then 2 (deps on 1).
    assert_eq!(result.module.functions.len(), 3);
    assert_eq!(result.module.functions[0].name, "fn_c");
    assert_eq!(result.module.functions[1].name, "fn_b");
    assert_eq!(result.module.functions[2].name, "fn_a");

    assert_eq!(result.module_offsets[&modules::ModuleId(0)], 0);
    assert_eq!(result.module_offsets[&modules::ModuleId(1)], 1);
    assert_eq!(result.module_offsets[&modules::ModuleId(2)], 2);
}

#[test]
fn test_merge_entry_point_correctly_offset() {
    let graph = build_test_graph(2);

    // Module 0: 2 functions, no entry
    let lr_a = build_lowering_result(&["dep_1", "dep_2"], vec![], None);
    // Module 1: 1 function, entry at 0
    let lr_b = build_lowering_result(&["main"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Module 1's entry (0) + offset (2) = 2
    assert_eq!(result.module.entry, Some(2));
}

#[test]
fn test_merge_module_offsets_map_populated() {
    let graph = build_test_graph(3);

    let lr_0 = build_lowering_result(&["a", "b"], vec![], None);
    let lr_1 = build_lowering_result(&["c"], vec![], None);
    let lr_2 = build_lowering_result(&["d", "e", "f"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    assert_eq!(result.module_offsets.len(), 3);
    assert_eq!(result.module_offsets[&modules::ModuleId(0)], 0);
    assert_eq!(result.module_offsets[&modules::ModuleId(1)], 2);
    assert_eq!(result.module_offsets[&modules::ModuleId(2)], 3);
    assert_eq!(result.module.functions.len(), 6);
}

#[test]
fn test_merge_empty_string_tables() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["fn_a"], vec![], Some(0));
    let lr_b = build_lowering_result(&["fn_b"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert!(result.string_table.is_empty());
}

#[test]
fn test_merge_const_i32_non_func_ref_not_rewritten() {
    use ir::{IrType, Op};

    let graph = build_test_graph(2);

    // Module 0: 1 function
    let lr_a = build_lowering_result(&["dep_fn"], vec![], Some(0));

    // Module 1: 1 function with ConstI32(0) NOT used by CreateClosure.
    // This should NOT be rewritten even though it's in the func index range.
    let mut b = ir::builder::TypedIrBuilder::new();
    b.begin_function("main_fn", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    b.const_i32(0); // Plain integer constant, NOT a func ref
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    let lr_b = desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table: vec![],
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    };

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // The ConstI32(0) in main_fn should remain 0 (not rewritten to 1).
    let main_fn = &result.module.functions[1];
    let const_ops: Vec<&ir::TypedInstruction> = main_fn
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter(|i| matches!(i.op, Op::ConstI32(0)))
        .collect();
    assert!(
        !const_ops.is_empty(),
        "ConstI32(0) that is NOT a func ref should NOT be rewritten"
    );
}

#[test]
fn test_merge_verifier_passes() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["dep_fn"], vec!["hello".to_string()], Some(0));
    let lr_b = build_lowering_result(&["main_fn"], vec!["world".to_string()], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // The merged module should pass the IR verifier.
    let verify_result = ir::verify::verify_typed_module(&result.module);
    assert!(
        verify_result.is_ok(),
        "merged module should pass verification: {:?}",
        verify_result.err()
    );
}

#[test]
fn test_merge_struct_types_combined() {
    use ir::IrType;
    use ir::builder::TypedIrBuilder;

    let graph = build_test_graph(2);

    // Module 0 with a struct type
    let mut b = TypedIrBuilder::new();
    b.begin_function("fn_a", vec![], IrType::Void);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    b.ret(None);
    b.end_function();
    b.set_entry(0);
    b.add_struct_type("Point", vec![("x", IrType::F64), ("y", IrType::F64)]);
    let module_a = b.finish();
    let lr_a = desugar::LoweringResult {
        module: module_a,
        errors: vec![],
        refusals: vec![],
        string_table: vec![],
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    };

    // Module 1 with a different struct type
    let mut b2 = TypedIrBuilder::new();
    b2.begin_function("fn_b", vec![], IrType::Void);
    let bb2 = b2.create_block();
    b2.switch_to_block(bb2);
    b2.seal_block(bb2);
    b2.ret(None);
    b2.end_function();
    b2.set_entry(0);
    b2.add_struct_type("Color", vec![("r", IrType::I32)]);
    let module_b = b2.finish();
    let lr_b = desugar::LoweringResult {
        module: module_b,
        errors: vec![],
        refusals: vec![],
        string_table: vec![],
        exports: vec![],
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    };

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Both struct types should be present.
    assert_eq!(result.module.struct_types.len(), 2);
    let names: Vec<&str> = result
        .module
        .struct_types
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(names.contains(&"Point"));
    assert!(names.contains(&"Color"));
}

#[test]
fn test_merge_last_entry_wins() {
    // When multiple modules have entries, the last in topological order wins.
    let graph = build_test_graph(3);

    let lr_0 = build_lowering_result(&["fn_0"], vec![], Some(0));
    let lr_1 = build_lowering_result(&["fn_1"], vec![], Some(0));
    let lr_2 = build_lowering_result(&["fn_2"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    // Module 2 is last in topological order, its entry is 0 + offset 2 = 2.
    assert_eq!(result.module.entry, Some(2));
}

#[test]
fn test_merge_no_entry_in_any_module() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["fn_a"], vec![], None);
    let lr_b = build_lowering_result(&["fn_b"], vec![], None);

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert_eq!(result.module.entry, None);
}

#[test]
fn test_merge_with_real_lowered_modules() {
    // Integration-style test: lower real JS sources and merge them.
    let source_a = "export const x = 42;\n";
    let source_b = "export const y = 'hello';\n";

    let lr_a = desugar::lower_program(source_a).unwrap();
    let lr_b = desugar::lower_program(source_b).unwrap();

    let graph = build_test_graph(2);

    let total_funcs = lr_a.module.functions.len() + lr_b.module.functions.len();

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert_eq!(result.module.functions.len(), total_funcs);
    // Verify the merged module passes verification.
    let verify_result = ir::verify::verify_typed_module(&result.module);
    assert!(
        verify_result.is_ok(),
        "merged real modules should pass verification: {:?}",
        verify_result.err()
    );
}

#[test]
fn test_merge_string_dedup_preserves_all_unique() {
    // Three modules with overlapping string tables.
    let graph = build_test_graph(3);

    let lr_0 = build_lowering_result(&["f0"], vec!["a".to_string(), "b".to_string()], None);
    let lr_1 = build_lowering_result(&["f1"], vec!["b".to_string(), "c".to_string()], None);
    let lr_2 = build_lowering_result(
        &["f2"],
        vec!["c".to_string(), "d".to_string(), "a".to_string()],
        Some(0),
    );

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    // Should have exactly 4 unique strings: a, b, c, d
    assert_eq!(result.string_table.len(), 4);
    assert!(result.string_table.contains(&"a".to_string()));
    assert!(result.string_table.contains(&"b".to_string()));
    assert!(result.string_table.contains(&"c".to_string()));
    assert!(result.string_table.contains(&"d".to_string()));
}

#[test]
fn test_merge_module_not_in_results_error() {
    // If the graph references a module that isn't in the lowering results, error.
    let graph = build_test_graph(2);

    // Only provide module 0, not module 1.
    let lr_0 = build_lowering_result(&["fn_0"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr_0)], &graph);

    assert!(
        result.is_err(),
        "should error when module is missing from results"
    );
}

#[test]
fn test_merge_multiple_functions_per_module() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["a1", "a2", "a3"], vec![], None);
    let lr_b = build_lowering_result(&["b1", "b2"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert_eq!(result.module.functions.len(), 5);
    assert_eq!(result.module.functions[0].name, "a1");
    assert_eq!(result.module.functions[1].name, "a2");
    assert_eq!(result.module.functions[2].name, "a3");
    assert_eq!(result.module.functions[3].name, "b1");
    assert_eq!(result.module.functions[4].name, "b2");

    // Entry should be at offset 3 (module 1 offset) + 0 = 3
    assert_eq!(result.module.entry, Some(3));
}

// ---------------------------------------------------------------------------
// Pipeline backward compatibility: single-file still works
// ---------------------------------------------------------------------------

#[test]
fn test_single_file_compilation_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("simple.mjs");
    std::fs::write(&file_path, "let x = 1 + 2;\n").unwrap();

    let config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    let result = crate::check(&config);
    assert!(
        result.is_ok(),
        "single-file .mjs should still work: {result:?}"
    );
}

#[test]
fn test_single_file_script_compilation_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("simple.js");
    std::fs::write(&file_path, "var x = 1 + 2;\n").unwrap();

    let config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    let result = crate::check(&config);
    assert!(
        result.is_ok(),
        "single-file .js should still work: {result:?}"
    );
}

#[test]
fn test_emit_ir_with_module_graph_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("emit.mjs");
    std::fs::write(&file_path, "export const x = 42;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config.emit = Some(EmitKind::Ir);
    let result = crate::compile(&config);
    assert!(
        result.is_ok(),
        "emit IR with module file should succeed: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Config integration: esc.json loading and merging
// ---------------------------------------------------------------------------

#[test]
fn test_no_esc_json_config_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut config);
    assert!(
        config.esc_config.is_none(),
        "esc_config should be None when no esc.json exists"
    );
}

#[test]
fn test_esc_json_target_overrides_default_edition() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{ "compilerOptions": { "target": "es2020" } }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut config);

    assert!(config.esc_config.is_some());
    assert_eq!(
        config.edition,
        Edition::ES2020,
        "esc.json target should override default edition"
    );
}

#[test]
fn test_cli_edition_overrides_esc_json_target() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{ "compilerOptions": { "target": "es2020" } }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    // Simulate CLI setting a non-default edition
    config.edition = Edition::ESNext;
    config::load_and_merge_config(&mut config);

    assert_eq!(
        config.edition,
        Edition::ESNext,
        "CLI edition should take precedence over esc.json"
    );
}

#[test]
fn test_esc_json_source_map_merged() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(&esc_json, r#"{ "compilerOptions": { "sourceMap": true } }"#).unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut config);

    assert!(config.source_map, "source_map should be set from esc.json");
}

#[test]
fn test_esc_json_out_dir_merged() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(&esc_json, r#"{ "compilerOptions": { "outDir": "dist" } }"#).unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut config);

    assert_eq!(
        config.out_dir.as_deref(),
        Some("dist"),
        "out_dir should be set from esc.json"
    );
}

#[test]
fn test_no_config_flag_skips_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{ "compilerOptions": { "target": "es2020", "sourceMap": true } }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config.no_config = true;
    config::load_and_merge_config(&mut config);

    assert!(
        config.esc_config.is_none(),
        "esc_config should be None when --no-config is set"
    );
    assert_eq!(
        config.edition,
        Edition::ES2025,
        "edition should remain default when --no-config skips discovery"
    );
    assert!(
        !config.source_map,
        "source_map should remain false when --no-config skips discovery"
    );
}

#[test]
fn test_explicit_config_path() {
    let dir = tempfile::tempdir().unwrap();
    // Put esc.json in a subdirectory that wouldn't be found by discovery
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    let esc_json = sub.join("custom.json");
    std::fs::write(
        &esc_json,
        r#"{ "compilerOptions": { "target": "es2020" } }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config.config_path = Some(esc_json.to_string_lossy().to_string());
    config::load_and_merge_config(&mut config);

    assert!(config.esc_config.is_some());
    assert_eq!(
        config.edition,
        Edition::ES2020,
        "explicit config path should be loaded"
    );
}

#[test]
fn test_explicit_config_path_missing_warns() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config.config_path = Some("/nonexistent/esc.json".to_string());
    config::load_and_merge_config(&mut config);

    assert!(
        config.esc_config.is_none(),
        "esc_config should be None when explicit path doesn't exist"
    );
}

#[test]
fn test_merge_config_with_all_fields() {
    let esc = EscConfig {
        compiler_options: Some(config::CompilerOptions {
            target: Some("es2020".to_string()),
            module: Some("esm".to_string()),
            out_dir: Some("build".to_string()),
            source_map: Some(true),
        }),
        host: None,
        eval: None,
        permissions: None,
    };

    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut config, &esc);

    assert_eq!(config.edition, Edition::ES2020);
    assert_eq!(config.out_dir.as_deref(), Some("build"));
    assert!(config.source_map);
}

#[test]
fn test_merge_config_cli_out_dir_takes_precedence() {
    let esc = EscConfig {
        compiler_options: Some(config::CompilerOptions {
            target: None,
            module: None,
            out_dir: Some("from-config".to_string()),
            source_map: None,
        }),
        host: None,
        eval: None,
        permissions: None,
    };

    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.out_dir = Some("from-cli".to_string());
    config::merge_config(&mut config, &esc);

    assert_eq!(
        config.out_dir.as_deref(),
        Some("from-cli"),
        "CLI out_dir should take precedence over esc.json"
    );
}

#[test]
fn test_esc_json_with_comments_parsed() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{
            // This is a comment
            "compilerOptions": {
                "target": "es2020" /* inline comment */
            }
        }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut config);

    assert!(config.esc_config.is_some(), "JSONC should be parsed");
    assert_eq!(config.edition, Edition::ES2020);
}

#[test]
fn test_compiler_config_new_includes_config_fields() {
    let config = CompilerConfig::new(vec!["input.js".to_string()]);
    assert!(config.esc_config.is_none());
    assert!(!config.source_map);
    assert!(config.out_dir.is_none());
    assert!(config.config_path.is_none());
    assert!(!config.no_config);
}

// ---------------------------------------------------------------------------
// Live bindings: ExportDeclKind classification
// ---------------------------------------------------------------------------

#[test]
fn test_export_decl_kind_const_no_getter() {
    assert!(
        !desugar::ExportDeclKind::Const.needs_getter(),
        "const exports should not need a getter"
    );
}

#[test]
fn test_export_decl_kind_function_no_getter() {
    assert!(
        !desugar::ExportDeclKind::Function.needs_getter(),
        "function exports should not need a getter"
    );
}

#[test]
fn test_export_decl_kind_class_no_getter() {
    assert!(
        !desugar::ExportDeclKind::Class.needs_getter(),
        "class exports should not need a getter"
    );
}

#[test]
fn test_export_decl_kind_let_needs_getter() {
    assert!(
        desugar::ExportDeclKind::Let.needs_getter(),
        "let exports should need a getter"
    );
}

#[test]
fn test_export_decl_kind_var_needs_getter() {
    assert!(
        desugar::ExportDeclKind::Var.needs_getter(),
        "var exports should need a getter"
    );
}

#[test]
fn test_export_decl_kind_unknown_no_getter() {
    assert!(
        !desugar::ExportDeclKind::Unknown.needs_getter(),
        "unknown exports should not need a getter (conservative default)"
    );
}

// ---------------------------------------------------------------------------
// Live bindings: ExportInfo carries decl_kind from lowering
// ---------------------------------------------------------------------------

/// Helper: build a `LoweringResult` with the given functions and exports.
fn build_lowering_result_with_exports(
    func_names: &[&str],
    string_table: Vec<String>,
    entry: Option<usize>,
    exports: Vec<desugar::ExportInfo>,
) -> desugar::LoweringResult {
    use ir::IrType;
    use ir::builder::TypedIrBuilder;

    let mut b = TypedIrBuilder::new();
    for name in func_names {
        b.begin_function(name, vec![], IrType::Void);
        let bb = b.create_block();
        b.switch_to_block(bb);
        b.seal_block(bb);
        b.ret(None);
        b.end_function();
    }
    if let Some(e) = entry {
        b.set_entry(e);
    }
    let module = b.finish();
    desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table,
        exports,
        has_top_level_await: false,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    }
}

#[test]
fn test_lowering_const_export_records_decl_kind() {
    let source = "export const x = 42;";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "x");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Const);
}

#[test]
fn test_lowering_let_export_records_decl_kind() {
    let source = "export let y = 10;";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "y");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Let);
}

#[test]
fn test_lowering_var_export_records_decl_kind() {
    let source = "export var z = 5;";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "z");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Var);
}

#[test]
fn test_lowering_function_export_records_decl_kind() {
    let source = "export function foo() {}";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "foo");
    assert_eq!(
        result.exports[0].decl_kind,
        desugar::ExportDeclKind::Function
    );
}

#[test]
fn test_lowering_class_export_records_decl_kind() {
    let source = "export class Bar {}";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "Bar");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Class);
}

#[test]
fn test_lowering_default_function_export_records_decl_kind() {
    let source = "export default function foo() {}";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "default");
    assert_eq!(
        result.exports[0].decl_kind,
        desugar::ExportDeclKind::Function
    );
}

#[test]
fn test_lowering_default_class_export_records_decl_kind() {
    let source = "export default class Foo {}";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "default");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Class);
}

#[test]
fn test_lowering_default_expression_export_records_const() {
    let source = "export default 42;";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "default");
    assert_eq!(result.exports[0].decl_kind, desugar::ExportDeclKind::Const);
}

#[test]
fn test_lowering_reexport_records_unknown_decl_kind() {
    let source = "export { foo } from './other.mjs';";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "foo");
    assert_eq!(
        result.exports[0].decl_kind,
        desugar::ExportDeclKind::Unknown
    );
}

#[test]
fn test_lowering_named_specifier_export_records_unknown() {
    let source = "const a = 1; export { a };";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "a");
    // Named specifier `export { a }` does not carry declaration kind info
    assert_eq!(
        result.exports[0].decl_kind,
        desugar::ExportDeclKind::Unknown
    );
}

// ---------------------------------------------------------------------------
// Live bindings: merge_modules classification
// ---------------------------------------------------------------------------

#[test]
fn test_merge_const_export_produces_direct_binding() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "x".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let binding = result.live_bindings.get(&(0, "x".to_string()));
    assert!(binding.is_some(), "const export should produce a binding");
    assert!(
        matches!(
            binding.unwrap(),
            crate::module_pipeline::BindingKind::Direct { .. }
        ),
        "const export should produce a Direct binding"
    );
}

#[test]
fn test_merge_function_export_produces_direct_binding() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main", "foo"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "foo".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Function,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let binding = result.live_bindings.get(&(0, "foo".to_string()));
    assert!(
        matches!(
            binding.unwrap(),
            crate::module_pipeline::BindingKind::Direct { .. }
        ),
        "function export should produce a Direct binding"
    );
}

#[test]
fn test_merge_let_export_produces_getter_binding() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "y".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let binding = result.live_bindings.get(&(0, "y".to_string()));
    assert!(binding.is_some(), "let export should produce a binding");
    assert!(
        matches!(
            binding.unwrap(),
            crate::module_pipeline::BindingKind::Getter { .. }
        ),
        "let export should produce a Getter binding"
    );
}

#[test]
fn test_merge_var_export_produces_getter_binding() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "z".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Var,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let binding = result.live_bindings.get(&(0, "z".to_string()));
    assert!(binding.is_some(), "var export should produce a binding");
    assert!(
        matches!(
            binding.unwrap(),
            crate::module_pipeline::BindingKind::Getter { .. }
        ),
        "var export should produce a Getter binding"
    );
}

// ---------------------------------------------------------------------------
// Live bindings: getter function generation
// ---------------------------------------------------------------------------

#[test]
fn test_merge_let_export_generates_getter_function() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "counter".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    // The merged module should have 2 functions: "main" + the generated getter.
    assert_eq!(
        result.module.functions.len(),
        2,
        "getter function should be appended"
    );

    let getter = &result.module.functions[1];
    assert!(
        getter.name.contains("__live_getter"),
        "getter function should have __live_getter prefix: got {}",
        getter.name
    );
    assert!(
        getter.name.contains("counter"),
        "getter function name should contain the export name: got {}",
        getter.name
    );
}

#[test]
fn test_merge_const_export_no_getter_generated() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "PI".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    // No getter function should be generated for const exports.
    assert_eq!(
        result.module.functions.len(),
        1,
        "const export should not generate a getter function"
    );
}

#[test]
fn test_merge_getter_function_has_env_load_op() {
    use ir::Op;

    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "val".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let getter = &result.module.functions[1];
    let has_env_load = getter
        .blocks
        .iter()
        .any(|b| b.instructions.iter().any(|i| matches!(i.op, Op::EnvLoad)));
    assert!(
        has_env_load,
        "getter function should contain an EnvLoad instruction"
    );
}

#[test]
fn test_merge_getter_function_has_ret_op() {
    use ir::Op;

    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "val".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let getter = &result.module.functions[1];
    let has_ret = getter
        .blocks
        .iter()
        .any(|b| b.instructions.iter().any(|i| matches!(i.op, Op::Ret)));
    assert!(has_ret, "getter function should contain a Ret instruction");
}

// ---------------------------------------------------------------------------
// Live bindings: two modules with let exports each get own getter
// ---------------------------------------------------------------------------

#[test]
fn test_merge_two_modules_let_exports_each_get_own_getter() {
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["dep_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "a".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );
    let lr_b = build_lowering_result_with_exports(
        &["entry_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "b".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // 2 original functions + 2 getters = 4 total
    assert_eq!(
        result.module.functions.len(),
        4,
        "each let export should generate its own getter"
    );

    // Both bindings should be Getter
    let binding_a = result.live_bindings.get(&(0, "a".to_string()));
    let binding_b = result.live_bindings.get(&(1, "b".to_string()));
    assert!(
        matches!(
            binding_a,
            Some(crate::module_pipeline::BindingKind::Getter { .. })
        ),
        "module 0 let export 'a' should be Getter"
    );
    assert!(
        matches!(
            binding_b,
            Some(crate::module_pipeline::BindingKind::Getter { .. })
        ),
        "module 1 let export 'b' should be Getter"
    );

    // The getter indices should be different
    if let (
        Some(crate::module_pipeline::BindingKind::Getter {
            getter_func_idx: idx_a,
        }),
        Some(crate::module_pipeline::BindingKind::Getter {
            getter_func_idx: idx_b,
        }),
    ) = (binding_a, binding_b)
    {
        assert_ne!(
            idx_a, idx_b,
            "each module should have its own getter function"
        );
    }
}

// ---------------------------------------------------------------------------
// Namespace exports
// ---------------------------------------------------------------------------

#[test]
fn test_merge_produces_namespace_exports() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "x".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
            desugar::ExportInfo {
                name: "y".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Let,
            },
        ],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let ns = result.namespace_exports.get(&0);
    assert!(ns.is_some(), "module 0 should have namespace exports");
    let ns = ns.unwrap();
    assert_eq!(ns.len(), 2, "should have 2 namespace exports");
    assert_eq!(ns[0].name, "x");
    assert_eq!(ns[1].name, "y");
}

#[test]
fn test_namespace_export_const_is_direct() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "PI".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let ns = &result.namespace_exports[&0];
    assert!(
        matches!(
            ns[0].binding,
            crate::module_pipeline::BindingKind::Direct { .. }
        ),
        "const namespace export should be Direct"
    );
}

#[test]
fn test_namespace_export_let_is_getter() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "count".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let ns = &result.namespace_exports[&0];
    assert!(
        matches!(
            ns[0].binding,
            crate::module_pipeline::BindingKind::Getter { .. }
        ),
        "let namespace export should be Getter"
    );
}

#[test]
fn test_namespace_exports_empty_when_no_exports() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result(&["main"], vec![], Some(0));

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    assert!(
        !result.namespace_exports.contains_key(&0),
        "module with no exports should not have namespace exports entry"
    );
}

// ---------------------------------------------------------------------------
// Live bindings: mixed exports in one module
// ---------------------------------------------------------------------------

#[test]
fn test_merge_mixed_exports_correct_classification() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main", "add"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "MAX".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
            desugar::ExportInfo {
                name: "count".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Let,
            },
            desugar::ExportInfo {
                name: "add".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Function,
            },
            desugar::ExportInfo {
                name: "state".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Var,
            },
        ],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    // const → Direct
    assert!(matches!(
        result.live_bindings.get(&(0, "MAX".to_string())),
        Some(crate::module_pipeline::BindingKind::Direct { .. })
    ));

    // let → Getter
    assert!(matches!(
        result.live_bindings.get(&(0, "count".to_string())),
        Some(crate::module_pipeline::BindingKind::Getter { .. })
    ));

    // function → Direct
    assert!(matches!(
        result.live_bindings.get(&(0, "add".to_string())),
        Some(crate::module_pipeline::BindingKind::Direct { .. })
    ));

    // var → Getter
    assert!(matches!(
        result.live_bindings.get(&(0, "state".to_string())),
        Some(crate::module_pipeline::BindingKind::Getter { .. })
    ));

    // Should have 2 getter functions generated (for "count" and "state")
    // Original: 2 functions + 2 getters = 4
    assert_eq!(result.module.functions.len(), 4);
}

// ---------------------------------------------------------------------------
// Live bindings: single-module compilation unchanged
// ---------------------------------------------------------------------------

#[test]
fn test_merge_single_module_no_exports_unchanged() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result(&["main"], vec!["hello".to_string()], Some(0));

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    // No exports → no live bindings, no namespace exports, same function count
    assert!(result.live_bindings.is_empty());
    assert!(result.namespace_exports.is_empty() || !result.namespace_exports.contains_key(&0));
    assert_eq!(result.module.functions.len(), 1);
    assert_eq!(result.module.entry, Some(0));
}

// ---------------------------------------------------------------------------
// Live bindings: getter function index in binding map matches function list
// ---------------------------------------------------------------------------

#[test]
fn test_merge_getter_func_idx_matches_function_list() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main", "helper"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "mutable_val".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    let binding = result
        .live_bindings
        .get(&(0, "mutable_val".to_string()))
        .unwrap();
    if let crate::module_pipeline::BindingKind::Getter { getter_func_idx } = binding {
        assert!(
            *getter_func_idx < result.module.functions.len(),
            "getter func_idx should be a valid index into the function list"
        );
        assert!(
            result.module.functions[*getter_func_idx]
                .name
                .contains("__live_getter"),
            "function at getter_func_idx should be a getter"
        );
    } else {
        panic!("expected Getter binding for let export");
    }
}

// ---------------------------------------------------------------------------
// Live bindings: export * (re-export) gets Unknown decl_kind
// ---------------------------------------------------------------------------

#[test]
fn test_lowering_export_star_records_unknown_decl_kind() {
    let source = "export * from './other.mjs';";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 1);
    assert_eq!(result.exports[0].name, "*");
    assert_eq!(
        result.exports[0].decl_kind,
        desugar::ExportDeclKind::Unknown
    );
}

// ---------------------------------------------------------------------------
// Live bindings: multiple exports from one declaration
// ---------------------------------------------------------------------------

#[test]
fn test_lowering_destructured_const_export_all_const() {
    let source = "export const { a, b } = { a: 1, b: 2 };";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 2);
    for export in &result.exports {
        assert_eq!(
            export.decl_kind,
            desugar::ExportDeclKind::Const,
            "destructured const exports should all be Const"
        );
    }
}

#[test]
fn test_lowering_destructured_let_export_all_let() {
    let source = "export let { c, d } = { c: 3, d: 4 };";
    let result = desugar::lower_program(source).unwrap();
    assert_eq!(result.exports.len(), 2);
    for export in &result.exports {
        assert_eq!(
            export.decl_kind,
            desugar::ExportDeclKind::Let,
            "destructured let exports should all be Let"
        );
    }
}

// =========================================================================
// Step 0.5.6: Circular Import TDZ tracking
// =========================================================================

#[test]
fn test_tdz_export_set_new_is_empty() {
    let set = crate::module_pipeline::TdzExportSet::new();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
}

#[test]
fn test_tdz_export_set_mark_initialized() {
    let mut set = crate::module_pipeline::TdzExportSet::new();
    assert!(!set.is_initialized(modules::ModuleId(0), "foo"));

    set.mark_initialized(modules::ModuleId(0), "foo");
    assert!(set.is_initialized(modules::ModuleId(0), "foo"));
    assert_eq!(set.len(), 1);
}

#[test]
fn test_tdz_export_set_different_modules_independent() {
    let mut set = crate::module_pipeline::TdzExportSet::new();
    set.mark_initialized(modules::ModuleId(0), "foo");

    // Same name in different module is not initialized.
    assert!(!set.is_initialized(modules::ModuleId(1), "foo"));
    assert!(set.is_initialized(modules::ModuleId(0), "foo"));
}

#[test]
fn test_tdz_export_set_uninitialized_export_not_found() {
    let set = crate::module_pipeline::TdzExportSet::new();
    assert!(!set.is_initialized(modules::ModuleId(0), "bar"));
}

#[test]
fn test_merge_const_export_starts_initialized() {
    // const exports should be marked initialized immediately since they
    // are immutable and available once the module is compiled.
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "CONST_VAL".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();
    assert!(
        result
            .tdz_exports
            .is_initialized(modules::ModuleId(0), "CONST_VAL"),
        "const export should be initialized in TDZ set"
    );
}

#[test]
fn test_merge_let_export_starts_uninitialized() {
    // let exports start uninitialized (in TDZ) — they only become
    // initialized after the declaring module's init code runs.
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "mutableVal".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Let,
        }],
    );
    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();
    assert!(
        !result
            .tdz_exports
            .is_initialized(modules::ModuleId(0), "mutableVal"),
        "let export should NOT be initialized in TDZ set (starts in TDZ)"
    );
}

#[test]
fn test_merge_function_export_starts_initialized() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "myFunc".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Function,
        }],
    );
    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();
    assert!(
        result
            .tdz_exports
            .is_initialized(modules::ModuleId(0), "myFunc"),
        "function export should be initialized"
    );
}

#[test]
fn test_merge_var_export_starts_uninitialized() {
    let graph = build_test_graph(1);
    let lr = build_lowering_result_with_exports(
        &["main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "varVal".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Var,
        }],
    );
    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();
    assert!(
        !result
            .tdz_exports
            .is_initialized(modules::ModuleId(0), "varVal"),
        "var export should NOT be initialized in TDZ set"
    );
}

#[test]
fn test_tdz_mark_initialized_then_access_succeeds() {
    let mut set = crate::module_pipeline::TdzExportSet::new();
    // Start uninitialized
    assert!(!set.is_initialized(modules::ModuleId(0), "x"));

    // Mark as initialized
    set.mark_initialized(modules::ModuleId(0), "x");

    // Now accessible
    assert!(set.is_initialized(modules::ModuleId(0), "x"));
}

// =========================================================================
// Step 0.5.7: import.meta module paths in merge
// =========================================================================

#[test]
fn test_merge_populates_module_paths() {
    let graph = build_test_graph(2);
    let lr_a = build_lowering_result(&["dep_fn"], vec![], Some(0));
    let lr_b = build_lowering_result(&["main_fn"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // module_paths should contain entries for both modules.
    assert_eq!(result.module_paths.len(), 2);
    assert!(
        result.module_paths.contains_key(&0),
        "module_paths should contain module 0"
    );
    assert!(
        result.module_paths.contains_key(&1),
        "module_paths should contain module 1"
    );
}

// =========================================================================
// Step 0.5.8: Re-export aggregation
// =========================================================================

#[test]
fn test_export_star_copies_named_exports() {
    // Module 0: export const a = 1; export const b = 2;
    // Module 1: export * from "./mod_0" (depends on 0)
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["mod_a_main"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "a".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
            desugar::ExportInfo {
                name: "b".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
        ],
    );
    let lr_b = build_lowering_result_with_exports(
        &["mod_b_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "*".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_0.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Module 1 should have re-exported "a" and "b" from module 0.
    let re_exports = result.resolved_re_exports.get(&1).unwrap();
    let names: Vec<&str> = re_exports.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"a"), "export * should copy 'a'");
    assert!(names.contains(&"b"), "export * should copy 'b'");
}

#[test]
fn test_export_star_excludes_default() {
    // Module 0: export default 42; export const a = 1;
    // Module 1: export * from "./mod_0"
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["mod_a_main"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "default".to_string(),
                kind: desugar::ExportKind::Default,
                decl_kind: desugar::ExportDeclKind::Const,
            },
            desugar::ExportInfo {
                name: "a".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
        ],
    );
    let lr_b = build_lowering_result_with_exports(
        &["mod_b_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "*".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_0.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    let re_exports = result.resolved_re_exports.get(&1).unwrap();
    let names: Vec<&str> = re_exports.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"a"), "export * should copy 'a'");
    assert!(
        !names.contains(&"default"),
        "export * should NOT copy 'default'"
    );
}

#[test]
fn test_export_named_re_export() {
    // Module 0: export const foo = 1;
    // Module 1: export { foo as bar } from "./mod_0"
    // Note: named re-exports record the exported name, not the local name.
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["mod_a_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "foo".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    // The desugar pass records: name="bar", kind=ReExport{source}
    // Since the lowerer doesn't track the original name, the re-export
    // resolution uses the export name as the source export name.
    // For `export { foo } from "./mod"` this is correct.
    // For `export { foo as bar } from "./mod"`, we need to handle the rename.
    // But the current desugar only records the exported name.
    let lr_b = build_lowering_result_with_exports(
        &["mod_b_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "foo".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_0.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    let re_exports = result.resolved_re_exports.get(&1).unwrap();
    assert_eq!(re_exports.len(), 1);
    assert_eq!(re_exports[0].name, "foo");
    assert_eq!(re_exports[0].source_module_id, modules::ModuleId(0));
    assert_eq!(re_exports[0].source_export_name, "foo");
}

#[test]
fn test_export_star_ambiguity_excludes_name() {
    // Module 0: export const x = 1;
    // Module 1: export const x = 2;
    // Module 2: export * from "./mod_0"; export * from "./mod_1"
    // The name "x" should be excluded (ambiguous).

    let mut graph = modules::ModuleGraph::new();
    // Module 0: no imports
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_0.mjs"),
        api_hash: modules::ApiHash(0),
        exports: vec![],
        imports: vec![],
        is_esm: true,
    });
    // Module 1: no imports
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_1.mjs"),
        api_hash: modules::ApiHash(1),
        exports: vec![],
        imports: vec![],
        is_esm: true,
    });
    // Module 2: imports from 0 and 1
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_2.mjs"),
        api_hash: modules::ApiHash(2),
        exports: vec![],
        imports: vec![
            modules::ImportEntry {
                source: "./mod_0.mjs".to_string(),
                bindings: vec![],
                resolved_id: Some(0),
            },
            modules::ImportEntry {
                source: "./mod_1.mjs".to_string(),
                bindings: vec![],
                resolved_id: Some(1),
            },
        ],
        is_esm: true,
    });
    graph.resolve_imports().unwrap_or_default();

    let lr_0 = build_lowering_result_with_exports(
        &["mod0_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "x".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let lr_1 = build_lowering_result_with_exports(
        &["mod1_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "x".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let lr_2 = build_lowering_result_with_exports(
        &["mod2_main"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "*".to_string(),
                kind: desugar::ExportKind::ReExport {
                    source: "./mod_0.mjs".to_string(),
                },
                decl_kind: desugar::ExportDeclKind::Unknown,
            },
            desugar::ExportInfo {
                name: "*".to_string(),
                kind: desugar::ExportKind::ReExport {
                    source: "./mod_1.mjs".to_string(),
                },
                decl_kind: desugar::ExportDeclKind::Unknown,
            },
        ],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    // "x" should be excluded due to ambiguity.
    let re_exports = result.resolved_re_exports.get(&2);
    if let Some(re_exports) = re_exports {
        let names: Vec<&str> = re_exports.iter().map(|r| r.name.as_str()).collect();
        assert!(
            !names.contains(&"x"),
            "ambiguous export 'x' should be excluded from re-exports"
        );
    }
    // If no re-exports at all, that's also correct (nothing to re-export).
}

#[test]
fn test_re_export_chain_a_from_b_from_c() {
    // Module 0 (C): export const val = 42;
    // Module 1 (B): export * from "./mod_0"
    // Module 2 (A): export * from "./mod_1"
    // A should transitively re-export "val" from C.
    let mut graph = modules::ModuleGraph::new();
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_0.mjs"),
        api_hash: modules::ApiHash(0),
        exports: vec![],
        imports: vec![],
        is_esm: true,
    });
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_1.mjs"),
        api_hash: modules::ApiHash(1),
        exports: vec![],
        imports: vec![modules::ImportEntry {
            source: "./mod_0.mjs".to_string(),
            bindings: vec![],
            resolved_id: Some(0),
        }],
        is_esm: true,
    });
    graph.add_module(modules::ModuleSummary {
        id: modules::ModuleId(0),
        path: std::path::PathBuf::from("/test/mod_2.mjs"),
        api_hash: modules::ApiHash(2),
        exports: vec![],
        imports: vec![modules::ImportEntry {
            source: "./mod_1.mjs".to_string(),
            bindings: vec![],
            resolved_id: Some(1),
        }],
        is_esm: true,
    });
    graph.resolve_imports().unwrap_or_default();

    let lr_0 = build_lowering_result_with_exports(
        &["modc_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "val".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let lr_1 = build_lowering_result_with_exports(
        &["modb_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "*".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_0.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );
    let lr_2 = build_lowering_result_with_exports(
        &["moda_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "*".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_1.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    // Module 1 (B) should re-export "val" from module 0 (C).
    let re_exports_b = result.resolved_re_exports.get(&1).unwrap();
    assert_eq!(re_exports_b.len(), 1);
    assert_eq!(re_exports_b[0].name, "val");
    assert_eq!(re_exports_b[0].source_module_id, modules::ModuleId(0));

    // Module 2 (A) should re-export "val" transitively through B.
    let re_exports_a = result.resolved_re_exports.get(&2).unwrap();
    assert_eq!(re_exports_a.len(), 1);
    assert_eq!(re_exports_a[0].name, "val");
    // The transitive re-export should point back to the original source (module 0).
    assert_eq!(re_exports_a[0].source_module_id, modules::ModuleId(0));
}

#[test]
fn test_re_export_live_bindings_propagated() {
    // Module 0: export const x = 1;
    // Module 1: export * from "./mod_0"
    // Module 1 should have a live binding for "x" that points to module 0's binding.
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["mod_a_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "x".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let lr_b = build_lowering_result_with_exports(
        &["mod_b_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "*".to_string(),
            kind: desugar::ExportKind::ReExport {
                source: "./mod_0.mjs".to_string(),
            },
            decl_kind: desugar::ExportDeclKind::Unknown,
        }],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    // Module 1 should have a live binding entry for "x".
    let binding = result.live_bindings.get(&(1, "x".to_string()));
    assert!(
        binding.is_some(),
        "re-exported name 'x' should have a live binding in module 1"
    );
    // The binding should be Direct (since const in source module).
    match binding.unwrap() {
        crate::module_pipeline::BindingKind::Direct { .. } => { /* expected */ }
        other => panic!("expected Direct binding for re-exported const, got: {other:?}"),
    }
}

#[test]
fn test_re_export_namespace_exports_include_re_exported_names() {
    // Module 0: export const alpha = 1;
    // Module 1: export * from "./mod_0"; export const beta = 2;
    // Module 1's namespace exports should include both "beta" (own) and "alpha" (re-exported).
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result_with_exports(
        &["mod_a_main"],
        vec![],
        Some(0),
        vec![desugar::ExportInfo {
            name: "alpha".to_string(),
            kind: desugar::ExportKind::Named,
            decl_kind: desugar::ExportDeclKind::Const,
        }],
    );
    let lr_b = build_lowering_result_with_exports(
        &["mod_b_main"],
        vec![],
        Some(0),
        vec![
            desugar::ExportInfo {
                name: "beta".to_string(),
                kind: desugar::ExportKind::Named,
                decl_kind: desugar::ExportDeclKind::Const,
            },
            desugar::ExportInfo {
                name: "*".to_string(),
                kind: desugar::ExportKind::ReExport {
                    source: "./mod_0.mjs".to_string(),
                },
                decl_kind: desugar::ExportDeclKind::Unknown,
            },
        ],
    );

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    let ns = result.namespace_exports.get(&1).unwrap();
    let ns_names: Vec<&str> = ns.iter().map(|n| n.name.as_str()).collect();
    assert!(
        ns_names.contains(&"beta"),
        "namespace should include own export 'beta'"
    );
    assert!(
        ns_names.contains(&"alpha"),
        "namespace should include re-exported 'alpha'"
    );
}

// ---------------------------------------------------------------------------
// RunCache
// ---------------------------------------------------------------------------

use crate::run_cache::RunCache;

#[test]
fn test_cache_key_consistent_for_same_input() {
    let key1 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "x86_64-linux");
    let key2 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "x86_64-linux");
    assert_eq!(key1, key2, "same inputs must produce the same cache key");
}

#[test]
fn test_cache_key_differs_for_different_source() {
    let key1 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "x86_64-linux");
    let key2 = RunCache::cache_key("console.log(2)", "0.5.0-dev", "x86_64-linux");
    assert_ne!(
        key1, key2,
        "different source content must produce different keys"
    );
}

#[test]
fn test_cache_key_differs_for_different_version() {
    let key1 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "x86_64-linux");
    let key2 = RunCache::cache_key("console.log(1)", "0.6.0-dev", "x86_64-linux");
    assert_ne!(
        key1, key2,
        "different compiler versions must produce different keys"
    );
}

#[test]
fn test_cache_key_differs_for_different_target() {
    let key1 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "x86_64-linux");
    let key2 = RunCache::cache_key("console.log(1)", "0.5.0-dev", "aarch64-macos");
    assert_ne!(
        key1, key2,
        "different target triples must produce different keys"
    );
}

#[test]
fn test_cache_key_is_hex_string() {
    let key = RunCache::cache_key("hello", "1.0.0", "x86_64-linux");
    assert_eq!(key.len(), 16, "cache key should be 16 hex characters");
    assert!(
        key.chars().all(|c| c.is_ascii_hexdigit()),
        "cache key must be hex: {key}"
    );
}

#[test]
fn test_run_cache_new_creates_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("test-cache");
    assert!(!cache_dir.exists());
    let _cache = RunCache::with_dir(cache_dir.clone()).unwrap();
    assert!(
        cache_dir.is_dir(),
        "with_dir must create the cache directory"
    );
}

#[test]
fn test_run_cache_get_empty_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = RunCache::with_dir(tmp.path().join("empty-cache")).unwrap();
    assert!(
        cache.get("nonexistent_key").is_none(),
        "get on empty cache must return None"
    );
}

#[test]
fn test_run_cache_put_get_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = RunCache::with_dir(tmp.path().join("roundtrip-cache")).unwrap();

    // Create a dummy "binary" file.
    let binary_path = tmp.path().join("dummy_binary");
    std::fs::write(&binary_path, b"#!/bin/sh\necho hello").unwrap();

    let key = "abc123def456789a";
    let cached_path = cache.put(key, &binary_path).unwrap();
    assert!(cached_path.is_file(), "put must store the file");

    // Verify content matches.
    let cached_content = std::fs::read(&cached_path).unwrap();
    assert_eq!(
        cached_content, b"#!/bin/sh\necho hello",
        "cached file must match original"
    );

    // get should return the path.
    let found = cache.get(key);
    assert!(found.is_some(), "get must find the cached binary");
    assert_eq!(found.unwrap(), cached_path);
}

#[test]
fn test_run_cache_put_overwrites_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = RunCache::with_dir(tmp.path().join("overwrite-cache")).unwrap();

    let key = "overwrite_test_key";

    // First put.
    let bin1 = tmp.path().join("bin1");
    std::fs::write(&bin1, b"version-1").unwrap();
    cache.put(key, &bin1).unwrap();

    // Second put with different content.
    let bin2 = tmp.path().join("bin2");
    std::fs::write(&bin2, b"version-2").unwrap();
    let cached = cache.put(key, &bin2).unwrap();

    let content = std::fs::read(cached).unwrap();
    assert_eq!(content, b"version-2", "put must overwrite existing entry");
}

#[test]
fn test_run_cache_env_var_override() {
    // This test verifies that with_dir works for arbitrary paths,
    // simulating the ESC_CACHE_DIR override behavior.
    let tmp = tempfile::tempdir().unwrap();
    let custom_dir = tmp.path().join("custom").join("cache").join("dir");
    let cache = RunCache::with_dir(custom_dir.clone()).unwrap();
    assert!(
        custom_dir.is_dir(),
        "custom cache directory must be created"
    );

    // Verify it works for operations.
    let bin = tmp.path().join("test_bin");
    std::fs::write(&bin, b"test").unwrap();
    cache.put("env_test_key_0000", &bin).unwrap();
    assert!(cache.get("env_test_key_0000").is_some());
}

#[test]
fn test_run_cache_clean_old_removes_nothing_when_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = RunCache::with_dir(tmp.path().join("clean-cache")).unwrap();

    // Put a binary.
    let bin = tmp.path().join("fresh_bin");
    std::fs::write(&bin, b"fresh").unwrap();
    cache.put("fresh_key_0000000", &bin).unwrap();

    // Clean with a large age — nothing should be removed.
    let removed = cache.clean_old(365).unwrap();
    assert_eq!(removed, 0, "fresh entries should not be removed");
    assert!(
        cache.get("fresh_key_0000000").is_some(),
        "fresh entry must still exist"
    );
}

// ---------------------------------------------------------------------------
// Top-level await (TLA) tracking in merge
// ---------------------------------------------------------------------------

/// Helper: build a `LoweringResult` with an async entry function containing
/// an `Await` opcode, simulating a module with top-level await.
fn build_tla_lowering_result(
    func_name: &str,
    string_table: Vec<String>,
) -> desugar::LoweringResult {
    use ir::IrType;
    use ir::builder::TypedIrBuilder;

    let mut b = TypedIrBuilder::new();
    b.begin_function(func_name, vec![], IrType::JSValue);
    b.set_async(true);
    let bb = b.create_block();
    b.switch_to_block(bb);
    b.seal_block(bb);
    // Emit an Await opcode to simulate top-level await
    let undef = b.const_undefined();
    let _awaited = b.await_(undef);
    b.ret(Some(undef));
    b.end_function();
    b.set_entry(0);
    let module = b.finish();
    desugar::LoweringResult {
        module,
        errors: vec![],
        refusals: vec![],
        string_table,
        exports: vec![],
        has_top_level_await: true,
        dynamic_imports: vec![],
        has_ffi_usage: false,
        has_eval: false,
        has_function_constructor: false,
    }
}

#[test]
fn test_merge_tla_module_tracked() {
    // Single module with TLA: tla_modules should contain module 0.
    let graph = build_test_graph(1);
    let lr = build_tla_lowering_result("main", vec![]);

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    assert!(
        result.tla_modules.contains(&0),
        "tla_modules should contain module 0 when it has TLA"
    );
}

#[test]
fn test_merge_no_tla_modules_empty_set() {
    // Single module without TLA: tla_modules should be empty.
    let graph = build_test_graph(1);
    let lr = build_lowering_result(&["main"], vec![], Some(0));

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    assert!(
        result.tla_modules.is_empty(),
        "tla_modules should be empty when no module has TLA"
    );
}

#[test]
fn test_merge_two_modules_one_has_tla() {
    // Module 0 is dependency (no TLA), module 1 is entry (has TLA).
    let graph = build_test_graph(2);

    let lr_a = build_lowering_result(&["dep_fn"], vec![], Some(0));
    let lr_b = build_tla_lowering_result("main_fn", vec![]);

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert!(
        !result.tla_modules.contains(&0),
        "module 0 (no TLA) should not be in tla_modules"
    );
    assert!(
        result.tla_modules.contains(&1),
        "module 1 (has TLA) should be in tla_modules"
    );
}

#[test]
fn test_merge_dependency_has_tla() {
    // Module 0 is dependency (has TLA), module 1 is entry (no TLA).
    // Module 1 imports module 0, so module 1's init must await module 0's init.
    let graph = build_test_graph(2);

    let lr_a = build_tla_lowering_result("dep_fn", vec![]);
    let lr_b = build_lowering_result(&["main_fn"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![(modules::ModuleId(0), lr_a), (modules::ModuleId(1), lr_b)],
        &graph,
    )
    .unwrap();

    assert!(
        result.tla_modules.contains(&0),
        "module 0 (has TLA) should be in tla_modules"
    );
    assert!(
        !result.tla_modules.contains(&1),
        "module 1 (no TLA) should not be in tla_modules"
    );
}

#[test]
fn test_merge_three_modules_chain_tla() {
    // Module 0→1→2 chain. Module 1 has TLA.
    // 0: dependency with no TLA
    // 1: middle module with TLA, depends on 0
    // 2: entry module, depends on 1
    let graph = build_test_graph(3);

    let lr_0 = build_lowering_result(&["dep_fn"], vec![], Some(0));
    let lr_1 = build_tla_lowering_result("mid_fn", vec![]);
    let lr_2 = build_lowering_result(&["entry_fn"], vec![], Some(0));

    let result = crate::module_pipeline::merge_modules(
        vec![
            (modules::ModuleId(0), lr_0),
            (modules::ModuleId(1), lr_1),
            (modules::ModuleId(2), lr_2),
        ],
        &graph,
    )
    .unwrap();

    assert_eq!(result.tla_modules.len(), 1, "only module 1 has TLA");
    assert!(
        result.tla_modules.contains(&1),
        "module 1 should be in tla_modules"
    );
}

#[test]
fn test_merge_tla_entry_function_is_async() {
    // Verify that a TLA module's entry function is preserved as async in the merged module.
    let graph = build_test_graph(1);
    let lr = build_tla_lowering_result("main", vec![]);

    let result =
        crate::module_pipeline::merge_modules(vec![(modules::ModuleId(0), lr)], &graph).unwrap();

    assert!(
        result.module.functions[0].is_async,
        "TLA module's entry function should remain is_async after merge"
    );
}

#[test]
fn test_compiler_version_is_nonempty() {
    let v = crate::run_cache::compiler_version();
    assert!(!v.is_empty(), "compiler version must not be empty");
}

#[test]
fn test_target_id_contains_arch_and_os() {
    let id = crate::run_cache::target_id();
    assert!(
        id.contains('-'),
        "target_id must contain arch-os separator: {id}"
    );
    // Should contain known arch and os substrings.
    assert!(!id.is_empty(), "target_id must not be empty");
}

// ---------------------------------------------------------------------------
// FFI security gate
// ---------------------------------------------------------------------------

#[test]
fn test_ffi_default_is_disabled() {
    let config = CompilerConfig::new(vec!["test.js".to_string()]);
    assert!(!config.allow_ffi, "FFI should be disabled by default");
    assert!(
        !config.ffi_allowed(),
        "ffi_allowed() should return false by default"
    );
    assert!(
        config.ffi_flag.is_none(),
        "ffi_flag should be None when no CLI flag is set"
    );
}

#[test]
fn test_ffi_flag_allow_enables_ffi() {
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_ffi = true;
    config.ffi_flag = Some(true);
    assert!(config.ffi_allowed());
}

#[test]
fn test_ffi_flag_no_ffi_disables_ffi() {
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_ffi = false;
    config.ffi_flag = Some(false);
    assert!(!config.ffi_allowed());
}

#[test]
fn test_ffi_gate_no_ffi_usage_no_error() {
    // When code does NOT use FFI and FFI is NOT allowed, should pass
    let config = CompilerConfig::new(vec!["test.js".to_string()]);
    let result = crate::pipeline::check_ffi_gate(&config, false);
    assert!(result.is_ok(), "no FFI usage + no permission should pass");
}

#[test]
fn test_ffi_gate_ffi_usage_without_permission_errors() {
    // When code uses FFI but permission is NOT granted, should error (ESC-E700)
    let config = CompilerConfig::new(vec!["test.js".to_string()]);
    let result = crate::pipeline::check_ffi_gate(&config, true);
    assert!(result.is_err(), "FFI usage without permission should error");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ESC-E700"),
        "error should contain ESC-E700: {msg}"
    );
}

#[test]
fn test_ffi_gate_ffi_usage_with_permission_passes() {
    // When code uses FFI and permission IS granted, should pass
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_ffi = true;
    let result = crate::pipeline::check_ffi_gate(&config, true);
    assert!(result.is_ok(), "FFI usage with permission should pass");
}

#[test]
fn test_ffi_gate_ffi_allowed_no_usage_passes() {
    // When FFI is allowed but code doesn't use it, should pass (just warning)
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_ffi = true;
    let result = crate::pipeline::check_ffi_gate(&config, false);
    assert!(result.is_ok(), "FFI allowed with no usage should pass");
}

#[test]
fn test_ffi_config_merge_allows_ffi_from_config() {
    // When esc.json has permissions.allowFfi = true and no CLI flag, FFI is enabled
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: Some(true),
            allow_eval: None,
            allow_jit: None,
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };

    let mut cfg = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut cfg, &esc);

    assert!(
        cfg.allow_ffi,
        "esc.json permissions.allowFfi should enable FFI"
    );
}

#[test]
fn test_ffi_config_merge_cli_overrides_config() {
    // When CLI has --no-ffi (ffi_flag=Some(false)) and config has allowFfi=true,
    // CLI should win
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: Some(true),
            allow_eval: None,
            allow_jit: None,
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };

    let mut cfg = CompilerConfig::new(vec!["test.js".to_string()]);
    cfg.ffi_flag = Some(false); // CLI says --no-ffi
    cfg.allow_ffi = false; // CLI resolved value
    config::merge_config(&mut cfg, &esc);

    assert!(
        !cfg.allow_ffi,
        "CLI --no-ffi should override config permissions.allowFfi"
    );
}

#[test]
fn test_ffi_config_merge_cli_allow_overrides_config_deny() {
    // When CLI has --allow-ffi and config has allowFfi=false, CLI should win
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: Some(false),
            allow_eval: None,
            allow_jit: None,
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };

    let mut cfg = CompilerConfig::new(vec!["test.js".to_string()]);
    cfg.ffi_flag = Some(true); // CLI says --allow-ffi
    cfg.allow_ffi = true; // CLI resolved value
    config::merge_config(&mut cfg, &esc);

    assert!(
        cfg.allow_ffi,
        "CLI --allow-ffi should override config permissions.allowFfi=false"
    );
}

#[test]
fn test_ffi_driver_error_display() {
    let err = crate::error::DriverError::FfiNotAllowed;
    let msg = err.to_string();
    assert!(msg.contains("ESC-E700"), "should contain ESC-E700: {msg}");
    assert!(
        msg.contains("--allow-ffi"),
        "should mention --allow-ffi: {msg}"
    );
}

#[test]
fn test_ffi_config_permissions_none_leaves_default() {
    // When esc.json has no permissions section, FFI stays at default (false)
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: None,
    };

    let mut cfg = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut cfg, &esc);

    assert!(
        !cfg.allow_ffi,
        "no permissions section should leave FFI disabled"
    );
}

#[test]
fn test_ffi_esc_json_permissions_parsed() {
    // Verify that esc.json with permissions.allowFfi is correctly parsed and merged
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{
            // permissions section
            "permissions": {
                "allowFfi": true
            }
        }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut cfg = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut cfg);

    assert!(cfg.esc_config.is_some(), "esc.json should be parsed");
    assert!(cfg.allow_ffi, "permissions.allowFfi=true should enable FFI");
}

#[test]
fn test_ffi_esc_json_permissions_false_parsed() {
    // Verify that esc.json with permissions.allowFfi=false leaves FFI disabled
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(&esc_json, r#"{ "permissions": { "allowFfi": false } }"#).unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut cfg = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut cfg);

    assert!(cfg.esc_config.is_some(), "esc.json should be parsed");
    assert!(
        !cfg.allow_ffi,
        "permissions.allowFfi=false should leave FFI disabled"
    );
}

// ---------------------------------------------------------------------------
// Eval/JIT permission checks (--no-eval, --no-jit)
// ---------------------------------------------------------------------------

#[test]
fn test_eval_permission_default_allows_eval() {
    // Default config (allow_eval=true, allow_jit=true) should permit eval usage.
    let result = desugar::lower_script("eval('1 + 2')").unwrap();
    assert!(result.has_eval, "eval call should be detected by lowering");
    let config = CompilerConfig::new(vec!["test.js".to_string()]);
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_ok(), "default config should allow eval");
}

#[test]
fn test_eval_with_no_eval_flag_produces_e400() {
    let result = desugar::lower_script("eval('1 + 2')").unwrap();
    assert!(result.has_eval);
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_err());
    let err = check.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ESC-E400"),
        "should produce ESC-E400, got: {msg}"
    );
    assert!(
        msg.contains("--no-eval"),
        "should mention --no-eval, got: {msg}"
    );
}

#[test]
fn test_function_constructor_with_no_eval_flag_produces_e400() {
    let result = desugar::lower_script("var f = new Function('return 1')").unwrap();
    assert!(
        result.has_function_constructor,
        "Function constructor should be detected"
    );
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_err());
    let err = check.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ESC-E400"),
        "should produce ESC-E400, got: {msg}"
    );
}

#[test]
fn test_function_call_without_new_with_no_eval_flag_produces_e400() {
    // Function() without new is also dynamic code execution.
    let result = desugar::lower_script("var f = Function('return 1')").unwrap();
    assert!(
        result.has_function_constructor,
        "Function() call (without new) should be detected"
    );
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_err());
    let msg = check.unwrap_err().to_string();
    assert!(
        msg.contains("ESC-E400"),
        "should produce ESC-E400, got: {msg}"
    );
}

#[test]
fn test_no_eval_no_dynamic_code_compiles_fine() {
    // Code without eval/Function + --no-eval should compile successfully.
    let result = desugar::lower_script("var x = 1 + 2;").unwrap();
    assert!(!result.has_eval);
    assert!(!result.has_function_constructor);
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(
        check.is_ok(),
        "code without eval should pass --no-eval check"
    );
}

#[test]
fn test_eval_with_no_jit_flag_produces_e401() {
    let result = desugar::lower_script("eval('1 + 2')").unwrap();
    assert!(result.has_eval);
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_jit = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_err());
    let err = check.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ESC-E401"),
        "should produce ESC-E401, got: {msg}"
    );
    assert!(
        msg.contains("--no-jit"),
        "should mention --no-jit, got: {msg}"
    );
}

#[test]
fn test_no_jit_no_dynamic_code_compiles_fine() {
    // Code without eval/Function + --no-jit should compile successfully.
    let result = desugar::lower_script("var x = 'hello';").unwrap();
    assert!(!result.has_eval);
    assert!(!result.has_function_constructor);
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_jit = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(
        check.is_ok(),
        "code without eval should pass --no-jit check"
    );
}

#[test]
fn test_no_eval_takes_precedence_over_no_jit() {
    // When both --no-eval and --no-jit are set, ESC-E400 should be reported
    // (eval check runs first).
    let result = desugar::lower_script("eval('x')").unwrap();
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false;
    config.allow_jit = false;
    let check = crate::pipeline::check_eval_permissions(&config, &result);
    assert!(check.is_err());
    let msg = check.unwrap_err().to_string();
    assert!(
        msg.contains("ESC-E400"),
        "--no-eval should take precedence, got: {msg}"
    );
}

#[test]
fn test_config_permissions_allow_eval_false() {
    // Verify that esc.json permissions.allowEval merges into config.
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: None,
            allow_eval: Some(false),
            allow_jit: None,
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut config, &esc);
    assert!(
        !config.allow_eval,
        "allowEval: false in config should set allow_eval to false"
    );
    assert!(config.allow_jit, "allow_jit should remain true (not set)");
}

#[test]
fn test_config_permissions_allow_jit_false() {
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: None,
            allow_eval: None,
            allow_jit: Some(false),
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut config, &esc);
    assert!(config.allow_eval, "allow_eval should remain true");
    assert!(
        !config.allow_jit,
        "allowJit: false in config should set allow_jit to false"
    );
}

#[test]
fn test_cli_no_eval_overrides_config_allow_eval() {
    // CLI --no-eval (allow_eval=false) should not be overridden by config's
    // allowEval: true.
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: None,
            allow_eval: Some(true),
            allow_jit: None,
            allow_read: None,
            allow_write: None,
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };
    let mut config = CompilerConfig::new(vec!["test.js".to_string()]);
    config.allow_eval = false; // CLI --no-eval
    config::merge_config(&mut config, &esc);
    assert!(
        !config.allow_eval,
        "CLI --no-eval should take precedence over config allowEval: true"
    );
}

#[test]
fn test_eval_detected_in_module_mode() {
    let result = desugar::lower_program("eval('1')").unwrap();
    assert!(
        result.has_eval,
        "eval should be detected in ES module mode too"
    );
}

#[test]
fn test_jsonc_config_permissions_parsing() {
    // Verify that the permissions section is correctly parsed from JSONC.
    let json = r#"{
        // Security settings
        "permissions": {
            "allowEval": false,
            "allowJit": false
        }
    }"#;
    let stripped = config::strip_jsonc_comments(json);
    let esc: EscConfig = serde_json::from_str(&stripped).unwrap();
    let perms = esc.permissions.as_ref().unwrap();
    assert_eq!(perms.allow_eval, Some(false));
    assert_eq!(perms.allow_jit, Some(false));
}

// =========================================================================
// Permission system tests — config parsing (Step 0.6.20)
// =========================================================================

#[test]
fn test_esc_json_permissions_allow_read_true() {
    let json = r#"{"permissions": {"allowRead": true}}"#;
    let esc: EscConfig = serde_json::from_str(json).unwrap();
    let perms = esc.permissions.unwrap();
    assert!(matches!(
        perms.allow_read,
        Some(config::PermissionJsonValue::Bool(true))
    ));
}

#[test]
fn test_esc_json_permissions_allow_read_false() {
    let json = r#"{"permissions": {"allowRead": false}}"#;
    let esc: EscConfig = serde_json::from_str(json).unwrap();
    let perms = esc.permissions.unwrap();
    assert!(matches!(
        perms.allow_read,
        Some(config::PermissionJsonValue::Bool(false))
    ));
}

#[test]
fn test_esc_json_permissions_allow_read_array() {
    let json = r#"{"permissions": {"allowRead": ["/tmp", "/home"]}}"#;
    let esc: EscConfig = serde_json::from_str(json).unwrap();
    let perms = esc.permissions.unwrap();
    match &perms.allow_read {
        Some(config::PermissionJsonValue::List(items)) => {
            assert_eq!(items, &vec!["/tmp".to_string(), "/home".to_string()]);
        }
        other => panic!("expected List, got: {other:?}"),
    }
}

#[test]
fn test_esc_json_permissions_all_runtime_fields() {
    let json = r#"{
        "permissions": {
            "allowRead": true,
            "allowWrite": false,
            "allowNet": ["localhost"],
            "allowEnv": ["PATH", "HOME"],
            "allowRun": true
        }
    }"#;
    let esc: EscConfig = serde_json::from_str(json).unwrap();
    let perms = esc.permissions.unwrap();
    assert!(matches!(
        perms.allow_read,
        Some(config::PermissionJsonValue::Bool(true))
    ));
    assert!(matches!(
        perms.allow_write,
        Some(config::PermissionJsonValue::Bool(false))
    ));
    match &perms.allow_net {
        Some(config::PermissionJsonValue::List(items)) => {
            assert_eq!(items, &vec!["localhost".to_string()]);
        }
        other => panic!("expected List, got: {other:?}"),
    }
    match &perms.allow_env {
        Some(config::PermissionJsonValue::List(items)) => {
            assert_eq!(items, &vec!["PATH".to_string(), "HOME".to_string()]);
        }
        other => panic!("expected List, got: {other:?}"),
    }
    assert!(matches!(
        perms.allow_run,
        Some(config::PermissionJsonValue::Bool(true))
    ));
}

#[test]
fn test_json_perm_to_host_perm_true() {
    let val = config::PermissionJsonValue::Bool(true);
    assert_eq!(
        config::json_perm_to_host_perm(&val),
        host::PermissionValue::Granted
    );
}

#[test]
fn test_json_perm_to_host_perm_false() {
    let val = config::PermissionJsonValue::Bool(false);
    assert_eq!(
        config::json_perm_to_host_perm(&val),
        host::PermissionValue::Denied
    );
}

#[test]
fn test_json_perm_to_host_perm_list() {
    let val = config::PermissionJsonValue::List(vec!["/tmp".to_string()]);
    assert_eq!(
        config::json_perm_to_host_perm(&val),
        host::PermissionValue::Restricted(vec!["/tmp".to_string()])
    );
}

#[test]
fn test_json_permissions_to_host_conversion() {
    let json_perms = config::PermissionsJsonConfig {
        allow_ffi: None,
        allow_eval: None,
        allow_jit: None,
        allow_read: Some(config::PermissionJsonValue::Bool(true)),
        allow_write: Some(config::PermissionJsonValue::Bool(false)),
        allow_net: Some(config::PermissionJsonValue::List(vec![
            "localhost".to_string(),
        ])),
        allow_env: None,
        allow_run: None,
    };
    let host_perms = config::json_permissions_to_host(&json_perms);
    assert_eq!(host_perms.allow_read, host::PermissionValue::Granted);
    assert_eq!(host_perms.allow_write, host::PermissionValue::Denied);
    assert_eq!(
        host_perms.allow_net,
        host::PermissionValue::Restricted(vec!["localhost".to_string()])
    );
    // Unspecified fields default to Granted
    assert_eq!(host_perms.allow_env, host::PermissionValue::Granted);
    assert_eq!(host_perms.allow_run, host::PermissionValue::Granted);
}

#[test]
fn test_merge_config_runtime_permissions_from_json() {
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: None,
            allow_eval: None,
            allow_jit: None,
            allow_read: Some(config::PermissionJsonValue::Bool(true)),
            allow_write: Some(config::PermissionJsonValue::Bool(false)),
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };
    let mut compiler_config = CompilerConfig::new(vec!["test.js".to_string()]);
    config::merge_config(&mut compiler_config, &esc);
    assert_eq!(
        compiler_config.permissions.allow_read,
        host::PermissionValue::Granted
    );
    assert_eq!(
        compiler_config.permissions.allow_write,
        host::PermissionValue::Denied
    );
}

#[test]
fn test_merge_config_cli_runtime_permissions_override_json() {
    let esc = EscConfig {
        compiler_options: None,
        host: None,
        eval: None,
        permissions: Some(config::PermissionsJsonConfig {
            allow_ffi: None,
            allow_eval: None,
            allow_jit: None,
            allow_read: Some(config::PermissionJsonValue::Bool(false)),
            allow_write: Some(config::PermissionJsonValue::Bool(false)),
            allow_net: None,
            allow_env: None,
            allow_run: None,
        }),
    };
    let mut compiler_config = CompilerConfig::new(vec!["test.js".to_string()]);
    // Simulate CLI setting permissions (from_cli = true)
    compiler_config.permissions_from_cli = true;
    compiler_config.permissions.allow_read = host::PermissionValue::Granted;
    config::merge_config(&mut compiler_config, &esc);

    // CLI permissions should NOT be overridden by JSON
    assert_eq!(
        compiler_config.permissions.allow_read,
        host::PermissionValue::Granted,
        "CLI permissions should take precedence over esc.json"
    );
}

#[test]
fn test_esc_json_with_runtime_permissions_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let esc_json = dir.path().join("esc.json");
    std::fs::write(
        &esc_json,
        r#"{
            "permissions": {
                "allowRead": ["/tmp"],
                "allowWrite": false,
                "allowEnv": true
            }
        }"#,
    )
    .unwrap();
    let file_path = dir.path().join("test.js");
    std::fs::write(&file_path, "var x = 1;\n").unwrap();

    let mut compiler_config = CompilerConfig::new(vec![file_path.to_string_lossy().to_string()]);
    config::load_and_merge_config(&mut compiler_config);

    assert!(compiler_config.esc_config.is_some());
    assert_eq!(
        compiler_config.permissions.allow_read,
        host::PermissionValue::Restricted(vec!["/tmp".to_string()])
    );
    assert_eq!(
        compiler_config.permissions.allow_write,
        host::PermissionValue::Denied
    );
    assert_eq!(
        compiler_config.permissions.allow_env,
        host::PermissionValue::Granted
    );
    // Unspecified fields default to Granted
    assert_eq!(
        compiler_config.permissions.allow_net,
        host::PermissionValue::Granted
    );
    assert_eq!(
        compiler_config.permissions.allow_run,
        host::PermissionValue::Granted
    );
}
