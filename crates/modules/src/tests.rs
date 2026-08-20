//! Tests for modules.

use std::path::PathBuf;

use crate::exports::{ExportKind, collect_imports_exports};
use crate::graph::DependencyGraph;
use crate::resolver::ModuleResolver;
use crate::{ApiHash, ModuleGraph, ModuleId, ModuleSummary, compute_api_hash};

// ===========================================================================
// ModuleGraph tests
// ===========================================================================

#[test]
fn test_single_module() {
    let mut graph = ModuleGraph::new();

    let summary = ModuleSummary {
        id: ModuleId(0),
        path: PathBuf::from("src/main.js"),
        api_hash: ApiHash(123),
        exports: vec![crate::exports::ExportEntry {
            name: "foo".to_string(),
            kind: ExportKind::Named,
        }],
        imports: vec![],
        is_esm: true,
    };

    let id = graph.add_module(summary);
    assert_eq!(id, ModuleId(0));
    assert_eq!(graph.modules().len(), 1);
    assert_eq!(
        graph.get_module(id).unwrap().path,
        PathBuf::from("src/main.js")
    );
}

// ===========================================================================
// Resolver tests
// ===========================================================================

#[test]
fn test_import_resolution_relative() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();

    // Create files
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("src/main.js"), "").unwrap();
    std::fs::write(base.join("src/foo.js"), "").unwrap();

    let resolver = ModuleResolver::new(base.to_path_buf());
    let result = resolver.resolve("./foo", &base.join("src/main.js"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), base.join("src/foo.js"));
}

#[test]
fn test_resolver_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();

    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(base.join("src/main.js"), "").unwrap();

    // Test .ts extension
    std::fs::write(base.join("src/utils.ts"), "").unwrap();
    let resolver = ModuleResolver::new(base.to_path_buf());
    let result = resolver.resolve("./utils", &base.join("src/main.js"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), base.join("src/utils.ts"));

    // Test /index.js
    std::fs::create_dir_all(base.join("src/lib")).unwrap();
    std::fs::write(base.join("src/lib/index.js"), "").unwrap();
    let result = resolver.resolve("./lib", &base.join("src/main.js"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), base.join("src/lib/index.js"));

    // Test /index.ts
    std::fs::create_dir_all(base.join("src/core")).unwrap();
    std::fs::write(base.join("src/core/index.ts"), "").unwrap();
    let result = resolver.resolve("./core", &base.join("src/main.js"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), base.join("src/core/index.ts"));
}

#[test]
fn test_resolver_bare_specifier() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();

    std::fs::create_dir_all(base.join("node_modules/react")).unwrap();
    std::fs::write(base.join("node_modules/react/index.js"), "").unwrap();
    std::fs::write(base.join("main.js"), "").unwrap();

    let resolver = ModuleResolver::new(base.to_path_buf());
    let result = resolver.resolve("react", &base.join("main.js"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), base.join("node_modules/react/index.js"));
}

#[test]
fn test_resolver_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::write(base.join("main.js"), "").unwrap();

    let resolver = ModuleResolver::new(base.to_path_buf());
    let result = resolver.resolve("./nonexistent", &base.join("main.js"));
    assert!(result.is_err());
}

// ===========================================================================
// DependencyGraph tests
// ===========================================================================

#[test]
fn test_multi_module_graph() {
    // A imports B, B imports C
    let mut graph = DependencyGraph::new();
    let a = ModuleId(0);
    let b = ModuleId(1);
    let c = ModuleId(2);

    graph.add_node(a);
    graph.add_node(b);
    graph.add_node(c);

    graph.add_edge(a, b); // A depends on B
    graph.add_edge(b, c); // B depends on C

    assert!(graph.has_edge(a, b));
    assert!(graph.has_edge(b, c));
    assert!(!graph.has_edge(a, c));
}

#[test]
fn test_circular_detection() {
    let mut graph = DependencyGraph::new();
    let a = ModuleId(0);
    let b = ModuleId(1);

    graph.add_node(a);
    graph.add_node(b);

    graph.add_edge(a, b); // A depends on B
    graph.add_edge(b, a); // B depends on A (cycle!)

    let result = graph.topological_sort();
    assert!(result.is_err());

    let cycles = graph.find_cycles();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].len(), 2);
}

#[test]
fn test_topological_order() {
    // C depends on B, B depends on A → order should be [A, B, C]
    let mut graph = DependencyGraph::new();
    let a = ModuleId(0);
    let b = ModuleId(1);
    let c = ModuleId(2);

    graph.add_node(a);
    graph.add_node(b);
    graph.add_node(c);

    graph.add_edge(c, b); // C depends on B
    graph.add_edge(b, a); // B depends on A

    let order = graph.topological_sort().unwrap();
    assert_eq!(order, vec![a, b, c]);
}

#[test]
fn test_dependency_graph_edges() {
    let mut graph = DependencyGraph::new();
    let a = ModuleId(0);
    let b = ModuleId(1);
    let c = ModuleId(2);

    graph.add_node(a);
    graph.add_node(b);
    graph.add_node(c);

    graph.add_edge(a, b);
    graph.add_edge(a, c);

    assert!(graph.has_edge(a, b));
    assert!(graph.has_edge(a, c));
    assert!(!graph.has_edge(b, a));
    assert!(!graph.has_edge(b, c));

    let deps = graph.dependencies(a);
    assert_eq!(deps.len(), 2);
    assert!(deps.contains(&b));
    assert!(deps.contains(&c));
}

#[test]
fn test_dependents() {
    let mut graph = DependencyGraph::new();
    let a = ModuleId(0);
    let b = ModuleId(1);
    let c = ModuleId(2);

    graph.add_node(a);
    graph.add_node(b);
    graph.add_node(c);

    graph.add_edge(b, a); // B depends on A
    graph.add_edge(c, a); // C depends on A

    let dependents = graph.dependents(a);
    assert_eq!(dependents.len(), 2);
    assert!(dependents.contains(&b));
    assert!(dependents.contains(&c));
}

// ===========================================================================
// API hash tests
// ===========================================================================

#[test]
fn test_api_hash_changes() {
    use crate::exports::{ExportEntry, ExportKind};

    let exports1 = vec![ExportEntry {
        name: "foo".to_string(),
        kind: ExportKind::Named,
    }];

    let exports2 = vec![ExportEntry {
        name: "bar".to_string(),
        kind: ExportKind::Named,
    }];

    let hash1 = compute_api_hash(&exports1);
    let hash2 = compute_api_hash(&exports2);
    assert_ne!(hash1, hash2);
}

#[test]
fn test_api_hash_stable() {
    use crate::exports::{ExportEntry, ExportKind};

    let exports = vec![
        ExportEntry {
            name: "foo".to_string(),
            kind: ExportKind::Named,
        },
        ExportEntry {
            name: "bar".to_string(),
            kind: ExportKind::Default,
        },
    ];

    let hash1 = compute_api_hash(&exports);
    let hash2 = compute_api_hash(&exports);
    assert_eq!(hash1, hash2);
}

#[test]
fn test_needs_recompile() {
    use crate::exports::{ExportEntry, ExportKind};

    let mut graph = ModuleGraph::new();

    let exports = vec![ExportEntry {
        name: "foo".to_string(),
        kind: ExportKind::Named,
    }];

    let hash = compute_api_hash(&exports);

    let summary = ModuleSummary {
        id: ModuleId(0),
        path: PathBuf::from("mod.js"),
        api_hash: hash,
        exports,
        imports: vec![],
        is_esm: true,
    };

    let id = graph.add_module(summary);

    // Same hash → no recompile
    assert!(!graph.needs_recompile(id, hash));

    // Different hash → needs recompile
    assert!(graph.needs_recompile(id, ApiHash(hash.0.wrapping_add(1))));
}

// ===========================================================================
// Export/import collection tests
// ===========================================================================

#[test]
fn test_collect_imports_exports() {
    let source = r#"
import { foo, bar } from './utils';
import baz from 'lodash';

export const x = 1;
export function hello() {}
export default 42;
"#;

    let (imports, exports) = collect_imports_exports(source, "test.js").unwrap();

    // Check imports
    assert_eq!(imports.len(), 2);
    assert_eq!(imports[0].source, "./utils");
    assert_eq!(imports[0].bindings.len(), 2);
    assert_eq!(imports[0].bindings[0].imported, "foo");
    assert_eq!(imports[0].bindings[0].local, "foo");
    assert_eq!(imports[0].bindings[1].imported, "bar");
    assert_eq!(imports[0].bindings[1].local, "bar");

    assert_eq!(imports[1].source, "lodash");
    assert_eq!(imports[1].bindings.len(), 1);
    assert_eq!(imports[1].bindings[0].imported, "default");
    assert_eq!(imports[1].bindings[0].local, "baz");

    // Check exports
    assert_eq!(exports.len(), 3);
    assert_eq!(exports[0].name, "x");
    assert!(matches!(exports[0].kind, ExportKind::Named));
    assert_eq!(exports[1].name, "hello");
    assert!(matches!(exports[1].kind, ExportKind::Named));
    assert_eq!(exports[2].name, "default");
    assert!(matches!(exports[2].kind, ExportKind::Default));
}

#[test]
fn test_re_exports() {
    let source = r#"
export { foo } from './bar';
export * from './baz';
"#;

    let (_, exports) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(exports.len(), 2);

    assert_eq!(exports[0].name, "foo");
    match &exports[0].kind {
        ExportKind::ReExport { source } => assert_eq!(source, "./bar"),
        other => panic!("expected ReExport, got {other:?}"),
    }

    assert_eq!(exports[1].name, "*");
    match &exports[1].kind {
        ExportKind::ReExport { source } => assert_eq!(source, "./baz"),
        other => panic!("expected ReExport, got {other:?}"),
    }
}

#[test]
fn test_default_export() {
    let source = "export default class Foo {}";
    let (_, exports) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].name, "default");
    assert!(matches!(exports[0].kind, ExportKind::Default));
}

#[test]
fn test_namespace_import() {
    let source = "import * as ns from './mod';";
    let (imports, _) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].bindings.len(), 1);
    assert_eq!(imports[0].bindings[0].imported, "*");
    assert_eq!(imports[0].bindings[0].local, "ns");
}

#[test]
fn test_export_destructuring() {
    let source = "export const { a, b } = obj;";
    let (_, exports) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(exports.len(), 2);
    assert_eq!(exports[0].name, "a");
    assert_eq!(exports[1].name, "b");
}

#[test]
fn test_export_renamed() {
    let source = "const x = 1; export { x as y };";
    let (_, exports) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].name, "y");
    assert!(matches!(exports[0].kind, ExportKind::Named));
}

#[test]
fn test_import_renamed() {
    let source = "import { foo as bar } from './mod';";
    let (imports, _) = collect_imports_exports(source, "test.js").unwrap();

    assert_eq!(imports[0].bindings[0].imported, "foo");
    assert_eq!(imports[0].bindings[0].local, "bar");
}

// ===========================================================================
// ModuleGraph integration
// ===========================================================================

#[test]
fn test_module_graph_compilation_order() {
    let mut graph = ModuleGraph::new();

    let sum_a = ModuleSummary {
        id: ModuleId(0),
        path: PathBuf::from("a.js"),
        api_hash: ApiHash(1),
        exports: vec![],
        imports: vec![],
        is_esm: true,
    };
    let sum_b = ModuleSummary {
        id: ModuleId(0),
        path: PathBuf::from("b.js"),
        api_hash: ApiHash(2),
        exports: vec![],
        imports: vec![crate::exports::ImportEntry {
            source: "./a".to_string(),
            bindings: vec![],
            resolved_id: Some(0),
        }],
        is_esm: true,
    };
    let sum_c = ModuleSummary {
        id: ModuleId(0),
        path: PathBuf::from("c.js"),
        api_hash: ApiHash(3),
        exports: vec![],
        imports: vec![crate::exports::ImportEntry {
            source: "./b".to_string(),
            bindings: vec![],
            resolved_id: Some(1),
        }],
        is_esm: true,
    };

    let id_a = graph.add_module(sum_a);
    let id_b = graph.add_module(sum_b);
    let id_c = graph.add_module(sum_c);

    graph.resolve_imports().unwrap();
    let order = graph.compilation_order().unwrap();

    // A should come before B, B before C
    let pos_a = order.iter().position(|&id| id == id_a).unwrap();
    let pos_b = order.iter().position(|&id| id == id_b).unwrap();
    let pos_c = order.iter().position(|&id| id == id_c).unwrap();

    assert!(pos_a < pos_b);
    assert!(pos_b < pos_c);
}

// ===========================================================================
// Dynamic import collection tests
// ===========================================================================

#[test]
fn test_collect_dynamic_imports_string_literal() {
    let source = r#"import("./mod.js");"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./mod.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_template_literal_no_interpolation() {
    let source = "import(`./mod.js`);";
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./mod.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_variable_not_collected() {
    let source = r#"const x = "./mod.js"; import(x);"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert!(
        specifiers.is_empty(),
        "variable specifiers should not be collected"
    );
}

#[test]
fn test_collect_dynamic_imports_multiple() {
    let source = r#"import("./a.js"); import("./b.js");"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./a.js".to_string(), "./b.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_deduplication() {
    let source = r#"import("./mod.js"); import("./mod.js");"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(
        specifiers.len(),
        1,
        "duplicate specifiers should be deduplicated"
    );
}

#[test]
fn test_collect_dynamic_imports_inside_function() {
    let source = r#"function load() { return import("./lazy.js"); }"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./lazy.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_inside_arrow_function() {
    let source = r#"const load = () => import("./lazy.js");"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./lazy.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_inside_if() {
    let source = r#"if (true) { import("./mod.js"); }"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./mod.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_inside_try() {
    let source = r#"try { import("./mod.js"); } catch(e) {}"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert_eq!(specifiers, vec!["./mod.js".to_string()]);
}

#[test]
fn test_collect_dynamic_imports_interpolated_template_not_collected() {
    let source = r#"const name = "foo"; import(`./mod_${name}.js`);"#;
    let result = crate::exports::collect_dynamic_imports(source, "test.mjs");
    assert!(result.is_ok());
    let specifiers = result.unwrap();
    assert!(
        specifiers.is_empty(),
        "interpolated template should not be collected"
    );
}
