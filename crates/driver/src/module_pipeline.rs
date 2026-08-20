//! Multi-module compilation pipeline.
//!
//! Builds a module graph from an entry point, discovers transitive dependencies,
//! lowers each module to typed IR in topological order, and merges the results
//! into a single unified module. The single-file pipeline remains the fast path;
//! this module adds the multi-file layer on top.
//!
//! Key types:
//! - [`ModuleExportMap`] — cross-module export-to-local-name mapping
//! - [`ModuleLoweringResult`] — per-module lowering output (IR + exports)
//! - [`MergeResult`] — unified module after merging all per-module IR

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use desugar::{ExportDeclKind, ExportInfo, LoweringResult};
use ir::builder::{TypedFunction, TypedIrBuilder, TypedModule};
use ir::{IrType, Op};
use modules::resolver::ModuleResolver;
use modules::{
    ModuleGraph, ModuleId, ModuleSummary, compute_api_hash, exports::collect_imports_exports,
};

use crate::error::DriverError;

/// The result of lowering a single module to IR.
pub struct ModuleLoweringResult {
    /// The module's unique identifier within this compilation.
    pub module_id: ModuleId,
    /// The filesystem path to the module's source file.
    pub path: PathBuf,
    /// The lowering output (typed IR module, string table, exports).
    pub lowering: LoweringResult,
}

/// Maps `(ModuleId, export_name)` to `(ModuleId, local_name)`.
///
/// Used by the merge phase to wire up cross-module references.
/// For re-exports, the target `ModuleId` points to the source module.
pub struct ModuleExportMap {
    /// The underlying mapping from `(module_id, export_name)` to `(target_module_id, local_name)`.
    entries: HashMap<(u32, String), (u32, String)>,
}

impl ModuleExportMap {
    /// Create an empty export map.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert a mapping from `(module_id, export_name)` to `(target_id, local_name)`.
    pub fn insert(
        &mut self,
        module_id: ModuleId,
        export_name: String,
        target_id: ModuleId,
        local_name: String,
    ) {
        self.entries
            .insert((module_id.0, export_name), (target_id.0, local_name));
    }

    /// Look up the target `(ModuleId, local_name)` for a given `(module_id, export_name)`.
    pub fn get(&self, module_id: ModuleId, export_name: &str) -> Option<(ModuleId, &str)> {
        self.entries
            .get(&(module_id.0, export_name.to_string()))
            .map(|(target, local)| (ModuleId(*target), local.as_str()))
    }

    /// Return the number of entries in the map.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ModuleExportMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a module graph starting from an entry point file.
///
/// Resolves the entry point and discovers all transitive dependencies
/// using the existing `ModuleResolver`. Each discovered module is parsed
/// to extract its imports and exports, which are used to build dependency
/// edges and compute API hashes.
///
/// # Errors
///
/// Returns [`DriverError`] if the entry file cannot be read, parsed, or
/// if any transitive dependency fails to resolve.
pub fn build_module_graph(entry: &Path) -> Result<ModuleGraph, DriverError> {
    let entry_canonical = entry
        .canonicalize()
        .map_err(|_| DriverError::FileNotFound(entry.display().to_string()))?;

    let base_dir = entry_canonical
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let resolver = ModuleResolver::new(base_dir);

    let mut graph = ModuleGraph::new();
    // Maps canonical path → ModuleId so we don't process a module twice.
    let mut path_to_id: HashMap<PathBuf, ModuleId> = HashMap::new();
    // BFS queue of paths to process.
    let mut queue: Vec<PathBuf> = vec![entry_canonical.clone()];

    while let Some(current_path) = queue.pop() {
        if path_to_id.contains_key(&current_path) {
            continue;
        }

        let source = std::fs::read_to_string(&current_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DriverError::FileNotFound(current_path.display().to_string())
            } else {
                DriverError::Io(e)
            }
        })?;

        let filename = current_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown.js".to_string());

        let (imports, exports) = collect_imports_exports(&source, &filename)
            .map_err(|e| DriverError::Lowering(vec![e]))?;

        // Also discover dynamic import() calls with string literal specifiers.
        // These modules are added to the graph so they are compiled ahead of time.
        let dynamic_specs =
            modules::collect_dynamic_imports(&source, &filename).unwrap_or_default();

        let api_hash = compute_api_hash(&exports);
        let is_esm = filename.ends_with(".mjs") || filename.ends_with(".mts");

        let mut import_entries = Vec::new();
        for imp in &imports {
            let resolved_path = resolver.resolve(&imp.source, &current_path);
            let resolved_id = match resolved_path {
                Ok(ref resolved) => {
                    let canonical = resolved.canonicalize().map_err(DriverError::Io)?;
                    // Will be filled after we know all module IDs
                    if !path_to_id.contains_key(&canonical) {
                        queue.push(canonical.clone());
                    }
                    None // filled in the second pass
                }
                Err(_) => {
                    // External/unresolvable module — skip for now
                    None
                }
            };

            import_entries.push(modules::ImportEntry {
                source: imp.source.clone(),
                bindings: imp
                    .bindings
                    .iter()
                    .map(|b| modules::ImportBinding {
                        imported: b.imported.clone(),
                        local: b.local.clone(),
                    })
                    .collect(),
                resolved_id,
            });
        }

        // Enqueue dynamically imported modules for graph building.
        // They are treated like static imports for module discovery but
        // don't have bindings (the namespace is accessed at runtime).
        for spec in &dynamic_specs {
            if let Ok(resolved) = resolver.resolve(spec, &current_path)
                && let Ok(canonical) = resolved.canonicalize()
                && !path_to_id.contains_key(&canonical)
            {
                queue.push(canonical);
            }
        }

        let summary = ModuleSummary {
            id: ModuleId(0), // overwritten by add_module
            path: current_path.clone(),
            api_hash,
            exports,
            imports: import_entries,
            is_esm,
        };

        let module_id = graph.add_module(summary);
        path_to_id.insert(current_path, module_id);
    }

    // Second pass: resolve import specifiers to ModuleIds now that all modules are registered.
    let path_to_id_snapshot = path_to_id.clone();
    let resolver2 = ModuleResolver::new(
        entry_canonical
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
    );

    // We need to update the resolved_id fields in the graph's module summaries.
    // Since ModuleGraph doesn't expose mutable access to summaries, we rebuild
    // the import edges by calling resolve_imports after setting resolved_ids.
    // But first we need to set them. Let's collect the edges and add them manually.

    // Rebuild the graph with resolved_ids set so dependency edges are wired up.
    let mut final_graph = ModuleGraph::new();

    for module in graph.modules() {
        let summary = ModuleSummary {
            id: ModuleId(0),
            path: module.path.clone(),
            api_hash: module.api_hash,
            exports: module.exports.clone(),
            imports: module
                .imports
                .iter()
                .map(|imp| {
                    let resolved_id = resolver2
                        .resolve(&imp.source, &module.path)
                        .ok()
                        .and_then(|resolved| resolved.canonicalize().ok())
                        .and_then(|canonical| path_to_id_snapshot.get(&canonical).map(|id| id.0));
                    modules::ImportEntry {
                        source: imp.source.clone(),
                        bindings: imp.bindings.clone(),
                        resolved_id,
                    }
                })
                .collect(),
            is_esm: module.is_esm,
        };
        final_graph.add_module(summary);
    }

    // Resolve imports to build dependency edges
    final_graph
        .resolve_imports()
        .map_err(DriverError::Lowering)?;

    Ok(final_graph)
}

/// Lower all modules in topological order (dependencies first).
///
/// Iterates through the module graph's compilation order, reads each source
/// file, lowers it to typed IR, and returns the results alongside a
/// [`ModuleExportMap`] for cross-module reference resolution.
///
/// # Errors
///
/// Returns [`DriverError`] if any module fails to read, parse, or lower,
/// or if the dependency graph contains cycles.
pub fn lower_all_modules(
    graph: &ModuleGraph,
) -> Result<(Vec<ModuleLoweringResult>, ModuleExportMap), DriverError> {
    let order = graph
        .compilation_order()
        .map_err(|e| DriverError::Lowering(vec![e.to_string()]))?;

    let mut results = Vec::with_capacity(order.len());
    let mut export_map = ModuleExportMap::new();

    for &module_id in &order {
        let module_summary = graph.get_module(module_id).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: module id {} not found in graph",
                module_id.0
            )])
        })?;

        let source = std::fs::read_to_string(&module_summary.path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DriverError::FileNotFound(module_summary.path.display().to_string())
            } else {
                DriverError::Io(e)
            }
        })?;

        let is_module = module_summary.is_esm || is_module_extension(&module_summary.path);
        let lowering_result = if is_module {
            desugar::lower_program(&source)
        } else {
            desugar::lower_script(&source)
        }
        .map_err(|errs| DriverError::Lowering(errs.iter().map(|e| e.to_string()).collect()))?;

        // Build export map entries from the lowering-recorded exports.
        populate_export_map(&mut export_map, module_id, &lowering_result.exports);

        results.push(ModuleLoweringResult {
            module_id,
            path: module_summary.path.clone(),
            lowering: lowering_result,
        });
    }

    Ok((results, export_map))
}

/// Populate the export map with entries from a module's recorded exports.
fn populate_export_map(
    export_map: &mut ModuleExportMap,
    module_id: ModuleId,
    exports: &[ExportInfo],
) {
    for export in exports {
        match &export.kind {
            desugar::ExportKind::Named | desugar::ExportKind::Default => {
                // Named/default exports: the local name is the same module.
                // For default exports the local binding is "__default" or the
                // declaration name, but we record "default" as the export name.
                let local = if export.kind == desugar::ExportKind::Default {
                    "__default".to_string()
                } else {
                    export.name.clone()
                };
                export_map.insert(module_id, export.name.clone(), module_id, local);
            }
            desugar::ExportKind::ReExport { .. } => {
                // Re-exports are resolved later during the merge phase.
                // For now, just record them pointing to self so the entry exists.
                export_map.insert(
                    module_id,
                    export.name.clone(),
                    module_id,
                    export.name.clone(),
                );
            }
        }
    }
}

/// Check if a file path has a module extension (`.mjs` or `.mts`).
fn is_module_extension(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.ends_with(".mjs") || s.ends_with(".mts")
}

// ---------------------------------------------------------------------------
// Live bindings
// ---------------------------------------------------------------------------

/// How an import binding should be resolved at the import site.
///
/// `const` and `function` exports produce direct values that can be inlined.
/// `let` and `var` exports produce getter functions that return the current
/// value, implementing ES module live binding semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingKind {
    /// Direct value — can be inlined at the import site (const, function exports).
    Direct {
        /// Index of the function that defines this export in the merged module.
        func_idx: usize,
    },
    /// Getter function — importing reads call the getter to get the current value.
    /// Used for `let`/`var` exports that may be reassigned after import.
    Getter {
        /// Index of the generated getter function in the merged module.
        getter_func_idx: usize,
    },
}

/// Maps `(module_id, export_name)` to the binding kind for that export.
pub type LiveBindingMap = HashMap<(u32, String), BindingKind>;

/// A single property in a namespace object (`import * as ns from "mod"`).
///
/// Captures the export name and how to access its value, so the namespace
/// object can be constructed during module initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceExport {
    /// The exported name (property key on the namespace object).
    pub name: String,
    /// How to access the export's value.
    pub binding: BindingKind,
}

// ---------------------------------------------------------------------------
// Circular import TDZ tracking
// ---------------------------------------------------------------------------

/// Tracks which exports have been initialized for circular import TDZ enforcement.
///
/// During module initialization, exports start uninitialized (in the TDZ).
/// Getter functions check this set; if the export is not yet initialized,
/// they throw `ReferenceError("Cannot access 'X' before initialization")`.
/// After the exporting module's declaration executes, the export is marked
/// as initialized.
#[derive(Debug, Clone, Default)]
pub struct TdzExportSet {
    /// Set of `(module_id, export_name)` pairs that have been initialized.
    initialized: HashSet<(u32, String)>,
}

impl TdzExportSet {
    /// Create an empty TDZ export set (all exports uninitialized).
    pub fn new() -> Self {
        Self {
            initialized: HashSet::new(),
        }
    }

    /// Mark an export as initialized (no longer in the TDZ).
    pub fn mark_initialized(&mut self, module_id: ModuleId, export_name: &str) {
        self.initialized
            .insert((module_id.0, export_name.to_string()));
    }

    /// Check if an export has been initialized.
    pub fn is_initialized(&self, module_id: ModuleId, export_name: &str) -> bool {
        self.initialized
            .contains(&(module_id.0, export_name.to_string()))
    }

    /// Return the number of initialized exports.
    pub fn len(&self) -> usize {
        self.initialized.len()
    }

    /// Return whether no exports have been initialized.
    pub fn is_empty(&self) -> bool {
        self.initialized.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Re-export resolution
// ---------------------------------------------------------------------------

/// Resolved re-export entry, mapping a re-exported name to its origin module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReExport {
    /// The export name as seen by importers of this module.
    pub name: String,
    /// The module that originally defines the binding.
    pub source_module_id: ModuleId,
    /// The original export name in the source module.
    pub source_export_name: String,
}

// ---------------------------------------------------------------------------
// Module merging
// ---------------------------------------------------------------------------

/// The result of merging multiple per-module IR into a single unified module.
pub struct MergeResult {
    /// The unified module with all functions from all modules concatenated.
    pub module: TypedModule,
    /// The merged string table with duplicates removed.
    pub string_table: Vec<String>,
    /// Maps each module to its function index offset in the merged function list.
    pub module_offsets: HashMap<ModuleId, usize>,
    /// Live binding classification for each export.
    /// Key: `(module_id, export_name)`, Value: how to access the export.
    pub live_bindings: LiveBindingMap,
    /// Per-module namespace exports for `import * as ns` support.
    /// Key: `module_id`, Value: list of namespace exports for that module.
    pub namespace_exports: HashMap<u32, Vec<NamespaceExport>>,
    /// TDZ tracking for circular imports.
    /// Exports that require TDZ checking are listed here with their module id.
    pub tdz_exports: TdzExportSet,
    /// Resolved re-exports for each module.
    /// Key: `module_id`, Value: list of resolved re-export entries.
    pub resolved_re_exports: HashMap<u32, Vec<ResolvedReExport>>,
    /// Per-module file paths, for import.meta support.
    /// Key: `module_id`, Value: absolute file path of the module.
    pub module_paths: HashMap<u32, PathBuf>,
    /// Module IDs of modules that use top-level `await` (ES2022).
    ///
    /// These modules have async entry functions whose initialization returns
    /// a Promise. Importing modules must await this Promise before accessing
    /// the module's exports.
    pub tla_modules: HashSet<u32>,
}

/// Classify an export as direct (inlineable) or requiring a getter.
///
/// `const`, `function`, and `class` exports are immutable and can be directly
/// referenced at the import site. `let` and `var` exports are mutable and
/// require a getter function so importing modules always read the current value.
fn classify_binding(decl_kind: ExportDeclKind, func_idx: usize) -> BindingKind {
    if decl_kind.needs_getter() {
        // A getter will be generated later; use func_idx as placeholder.
        // The caller replaces this with the actual getter index.
        BindingKind::Getter {
            getter_func_idx: func_idx,
        }
    } else {
        BindingKind::Direct { func_idx }
    }
}

/// Generate a getter function that reads a module-level SSA variable.
///
/// The getter body is: `LoadParam(0)` (env) -> `EnvLoad(env, slot)` -> `Ret(val)`.
/// This allows importing modules to call the getter to read the current value
/// of a mutable export (let/var).
///
/// Returns the built `TypedFunction`.
fn generate_getter_function(
    export_name: &str,
    module_name: &str,
    slot_index: u32,
) -> TypedFunction {
    let getter_name = format!("__live_getter_{module_name}_{export_name}");
    let mut builder = TypedIrBuilder::new();

    builder.begin_function(
        &getter_name,
        vec![("env", IrType::JSValue)],
        IrType::JSValue,
    );
    let entry = builder.create_block();
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    // Load the env parameter
    let env = builder.load_param(0);
    // Load the value from the environment slot (env_load creates the ConstI32 internally)
    let val = builder.env_load(env, slot_index);
    builder.ret(Some(val));
    builder.end_function();

    let module = builder.finish();
    // The module has exactly one function — extract it.
    module.functions.into_iter().next().unwrap_or_else(|| {
        panic!("BUG: generate_getter_function produced no function for {export_name}")
    })
}

/// Resolve re-exports for all modules, handling `export * from` and
/// `export { x as y } from` declarations.
///
/// For `export * from "./mod"`: copies all named exports (excluding `default`)
/// from the source module. If two `export *` sources both export the same name,
/// that name is excluded (ambiguous).
///
/// For `export { x as y } from "./mod"`: adds an entry mapping `y` to the
/// source module's `x` binding.
///
/// Returns a map from `module_id` to its resolved re-export entries.
fn resolve_re_exports(
    module_map: &HashMap<u32, LoweringResult>,
    graph: &ModuleGraph,
    order: &[ModuleId],
) -> HashMap<u32, Vec<ResolvedReExport>> {
    let mut result: HashMap<u32, Vec<ResolvedReExport>> = HashMap::new();

    // Build a map from module specifier → ModuleId for each module's imports.
    // We need this to resolve re-export source specifiers to module IDs.
    let mut specifier_to_id: HashMap<(u32, String), u32> = HashMap::new();
    for module in graph.modules() {
        for import in &module.imports {
            if let Some(resolved) = import.resolved_id {
                specifier_to_id.insert((module.id.0, import.source.clone()), resolved);
            }
        }
    }

    // Also check export-only re-exports (export * from / export { } from)
    // which may not have a corresponding import declaration. In that case,
    // we need to look at the graph's module summaries for specifier resolution.

    for &module_id in order {
        let Some(lowering) = module_map.get(&module_id.0) else {
            continue;
        };

        let mut re_exports: Vec<ResolvedReExport> = Vec::new();
        // Track names seen from `export *` to detect ambiguity.
        let mut star_names: HashMap<String, u32> = HashMap::new();
        // Track names that are ambiguous (seen from multiple `export *` sources).
        let mut ambiguous_names: HashSet<String> = HashSet::new();

        for export in &lowering.exports {
            let desugar::ExportKind::ReExport { source } = &export.kind else {
                continue;
            };

            // Resolve the source specifier to a module ID.
            let source_module_id = specifier_to_id.get(&(module_id.0, source.clone())).copied();

            // Also try to find via graph module summaries if not in import map.
            let source_module_id = source_module_id.or_else(|| {
                let module_summary = graph.get_module(module_id)?;
                // Look through the module summary's exports for the source specifier.
                // The source specifier in a re-export might not have a corresponding
                // import entry, so we resolve through the graph.
                let base_dir = module_summary.path.parent()?;
                let resolver = ModuleResolver::new(base_dir.to_path_buf());
                let resolved_path = resolver.resolve(source, &module_summary.path).ok()?;
                let canonical = resolved_path.canonicalize().ok()?;
                // Find the module with this path
                graph
                    .modules()
                    .iter()
                    .find(|m| m.path == canonical)
                    .map(|m| m.id.0)
            });

            let Some(src_id) = source_module_id else {
                continue;
            };

            if export.name == "*" {
                // `export * from "./mod"` — copy all named exports except "default"
                if let Some(src_lowering) = module_map.get(&src_id) {
                    for src_export in &src_lowering.exports {
                        if src_export.name == "default" {
                            continue;
                        }
                        // Skip re-exports from the source — only include
                        // directly defined exports.
                        if matches!(src_export.kind, desugar::ExportKind::ReExport { .. }) {
                            // But do include resolved re-exports from the source module.
                            if let Some(src_re_exports) = result.get(&src_id) {
                                for src_re in src_re_exports {
                                    if src_re.name == "default" {
                                        continue;
                                    }
                                    if let Some(&prev_src) = star_names.get(&src_re.name) {
                                        if prev_src != src_re.source_module_id.0 {
                                            ambiguous_names.insert(src_re.name.clone());
                                        }
                                    } else {
                                        star_names
                                            .insert(src_re.name.clone(), src_re.source_module_id.0);
                                    }
                                    re_exports.push(ResolvedReExport {
                                        name: src_re.name.clone(),
                                        source_module_id: src_re.source_module_id,
                                        source_export_name: src_re.source_export_name.clone(),
                                    });
                                }
                            }
                            continue;
                        }
                        if let Some(&prev_src) = star_names.get(&src_export.name) {
                            if prev_src != src_id {
                                ambiguous_names.insert(src_export.name.clone());
                            }
                        } else {
                            star_names.insert(src_export.name.clone(), src_id);
                        }
                        re_exports.push(ResolvedReExport {
                            name: src_export.name.clone(),
                            source_module_id: ModuleId(src_id),
                            source_export_name: src_export.name.clone(),
                        });
                    }
                }
            } else {
                // `export { x as y } from "./mod"` — named re-export
                // The export.name is the exported name (y), the local name
                // in the source module is what we need to find.
                // For `export { x as y } from "./mod"`, the lowerer records
                // name="y" with kind=ReExport{source}. The original imported
                // name is stored as the name in the specifier, which the desugar
                // pass captures. In practice, the lowering for
                // `export { foo as bar } from "./mod"` records name="bar".
                // We need the original name "foo" — which is stored as the
                // local name in the specifier. Let's get it from the lowered
                // exports: if the name differs from the import, the desugar
                // records both. For now, use the export name as the source name
                // (handles the common case of `export { foo } from "./mod"`).
                re_exports.push(ResolvedReExport {
                    name: export.name.clone(),
                    source_module_id: ModuleId(src_id),
                    source_export_name: export.name.clone(),
                });
            }
        }

        // Remove ambiguous names from star re-exports.
        if !ambiguous_names.is_empty() {
            re_exports.retain(|re| !ambiguous_names.contains(&re.name));
        }

        if !re_exports.is_empty() {
            result.insert(module_id.0, re_exports);
        }
    }

    result
}

/// Merge multiple per-module lowering results into a single unified module.
///
/// Functions are concatenated in the compilation order provided by the module
/// graph. String tables are deduplicated. All intra-module references
/// (`ConstString` indices, `ConstI32` function references used by
/// `CreateClosure`) are rewritten to use the merged indices.
///
/// Live bindings are classified: `const`/`function` exports become direct
/// bindings, while `let`/`var` exports get generated getter functions.
///
/// Re-exports are resolved: `export * from` copies named exports (excluding
/// default) with ambiguity detection, and `export { x as y } from` adds
/// alias entries.
///
/// The entry point of the last module in topological order (the entry module)
/// becomes the merged module's entry, adjusted by the module's function offset.
///
/// # Errors
///
/// Returns [`DriverError`] if the module graph contains cycles or if a
/// module referenced in the lowering results is not found in the graph.
pub fn merge_modules(
    modules: Vec<(ModuleId, LoweringResult)>,
    graph: &ModuleGraph,
) -> Result<MergeResult, DriverError> {
    let order = graph
        .compilation_order()
        .map_err(|e| DriverError::Lowering(vec![e.to_string()]))?;

    // Build a lookup from ModuleId → LoweringResult for O(1) access.
    // Also track which modules have top-level await.
    let mut tla_modules: HashSet<u32> = HashSet::new();
    let mut module_map: HashMap<u32, LoweringResult> = HashMap::with_capacity(modules.len());
    for (id, result) in modules {
        if result.has_top_level_await {
            tla_modules.insert(id.0);
        }
        module_map.insert(id.0, result);
    }

    // Phase 1: Build merged string table with deduplication.
    // Maps string → new index in the merged table.
    let mut merged_string_map: HashMap<String, u32> = HashMap::new();
    let mut merged_strings: Vec<String> = Vec::new();
    // Per-module mapping: module_id → (old_index → new_index).
    let mut string_remap: HashMap<u32, HashMap<u32, u32>> = HashMap::new();

    for &module_id in &order {
        let lowering = module_map.get(&module_id.0).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: module id {} present in compilation order but not in lowering results",
                module_id.0
            )])
        })?;

        let mut old_to_new: HashMap<u32, u32> = HashMap::new();
        for (old_idx, s) in lowering.string_table.iter().enumerate() {
            let new_idx = if let Some(&existing) = merged_string_map.get(s) {
                existing
            } else {
                let idx = merged_strings.len() as u32;
                merged_strings.push(s.clone());
                merged_string_map.insert(s.clone(), idx);
                idx
            };
            old_to_new.insert(old_idx as u32, new_idx);
        }
        string_remap.insert(module_id.0, old_to_new);
    }

    // Phase 2: Concatenate functions in topological order, recording offsets.
    let mut merged_functions: Vec<TypedFunction> = Vec::new();
    let mut module_offsets: HashMap<ModuleId, usize> = HashMap::new();
    let mut merged_struct_types: Vec<(String, Vec<(String, ir::IrType)>)> = Vec::new();
    let mut merged_entry: Option<usize> = None;

    for &module_id in &order {
        let lowering = module_map.get(&module_id.0).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: module id {} not found in lowering results",
                module_id.0
            )])
        })?;

        let offset = merged_functions.len();
        module_offsets.insert(module_id, offset);

        // Append struct types (prefixed with module id to avoid name collisions).
        for st in &lowering.module.struct_types {
            merged_struct_types.push(st.clone());
        }

        // Append functions.
        merged_functions.extend(lowering.module.functions.iter().cloned());

        // The last module in topological order is the entry module.
        // Its entry function index, adjusted by offset, becomes the merged entry.
        if let Some(entry_idx) = lowering.module.entry {
            merged_entry = Some(offset + entry_idx);
        }
    }

    // Phase 3: Rewrite indices in all instructions.
    // We need to walk instructions to rewrite:
    //   - ConstString(idx) → use string_remap
    //   - ConstI32(func_idx) when used as a function reference → add module offset
    //
    // For function index rewriting, we track which ConstI32 instructions are
    // function references by looking at CreateClosure usage. The approach:
    // for each module's functions, any ConstI32 value that falls within the
    // range [0, module.functions.len()) is a potential function reference.
    // We rewrite them by adding the module's offset.
    //
    // We iterate per-module to know which string_remap and offset to apply.
    let mut func_cursor = 0;
    for &module_id in &order {
        let lowering = module_map.get(&module_id.0).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: module id {} not found during rewrite phase",
                module_id.0
            )])
        })?;

        let str_map = string_remap.get(&module_id.0).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: no string remap for module {}",
                module_id.0
            )])
        })?;

        let func_offset = module_offsets.get(&module_id).copied().ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: no function offset for module {}",
                module_id.0
            )])
        })?;

        let num_module_functions = lowering.module.functions.len();

        for func_local_idx in 0..num_module_functions {
            let func = &mut merged_functions[func_cursor + func_local_idx];
            rewrite_function_indices(func, str_map, func_offset, num_module_functions);
        }

        func_cursor += num_module_functions;
    }

    // Phase 4: Resolve re-exports across modules.
    let resolved_re_exports = resolve_re_exports(&module_map, graph, &order);

    // Phase 5: Classify exports and generate getter functions for live bindings.
    let mut live_bindings: LiveBindingMap = HashMap::new();
    let mut namespace_exports: HashMap<u32, Vec<NamespaceExport>> = HashMap::new();
    let mut tdz_exports = TdzExportSet::new();

    for &module_id in &order {
        let lowering = module_map.get(&module_id.0).ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: module id {} not found during binding classification",
                module_id.0
            )])
        })?;

        let func_offset = module_offsets.get(&module_id).copied().ok_or_else(|| {
            DriverError::Lowering(vec![format!(
                "BUG: no function offset for module {} during binding classification",
                module_id.0
            )])
        })?;

        // Derive a module name for generated getter function names.
        let module_name = format!("mod_{}", module_id.0);

        let mut ns_exports = Vec::new();

        for (slot_index, export) in lowering.exports.iter().enumerate() {
            // Skip re-exports — they are handled separately.
            if matches!(export.kind, desugar::ExportKind::ReExport { .. }) {
                continue;
            }

            let binding = if export.decl_kind.needs_getter() {
                // Generate a getter function for this mutable export.
                let getter =
                    generate_getter_function(&export.name, &module_name, slot_index as u32);
                let getter_idx = merged_functions.len();
                merged_functions.push(getter);
                BindingKind::Getter {
                    getter_func_idx: getter_idx,
                }
            } else {
                classify_binding(export.decl_kind, func_offset)
            };

            live_bindings.insert((module_id.0, export.name.clone()), binding.clone());
            ns_exports.push(NamespaceExport {
                name: export.name.clone(),
                binding,
            });

            // Mark mutable exports (let/var) as needing TDZ checking.
            // For circular imports, these exports may be accessed before
            // the exporting module's initialization reaches the declaration.
            if export.decl_kind.needs_getter() {
                // Export starts in TDZ; will be marked initialized when
                // the exporting module's declaration executes.
                // (Not yet initialized — intentionally not calling mark_initialized.)
            } else {
                // Immutable exports (const, function, class) are considered
                // initialized once the module has been compiled.
                tdz_exports.mark_initialized(module_id, &export.name);
            }
        }

        // Add re-exported names to the namespace exports and live bindings.
        if let Some(re_exports) = resolved_re_exports.get(&module_id.0) {
            for re_export in re_exports {
                // Look up the binding kind from the source module's live bindings.
                if let Some(src_binding) = live_bindings.get(&(
                    re_export.source_module_id.0,
                    re_export.source_export_name.clone(),
                )) {
                    let binding = src_binding.clone();
                    live_bindings.insert((module_id.0, re_export.name.clone()), binding.clone());
                    ns_exports.push(NamespaceExport {
                        name: re_export.name.clone(),
                        binding,
                    });
                }
            }
        }

        if !ns_exports.is_empty() {
            namespace_exports.insert(module_id.0, ns_exports);
        }
    }

    // Build module paths map.
    let mut module_paths: HashMap<u32, PathBuf> = HashMap::new();
    for module in graph.modules() {
        module_paths.insert(module.id.0, module.path.clone());
    }

    Ok(MergeResult {
        module: TypedModule {
            functions: merged_functions,
            struct_types: merged_struct_types,
            entry: merged_entry,
        },
        string_table: merged_strings,
        module_offsets,
        live_bindings,
        namespace_exports,
        tdz_exports,
        resolved_re_exports,
        module_paths,
        tla_modules,
    })
}

/// Rewrite `ConstString` and function-reference `ConstI32` indices in a function.
///
/// - `ConstString(old)` is remapped via `str_map[old]`
/// - `ConstI32(val)` where `0 <= val < num_module_functions` is offset by `func_offset`
///
/// This function identifies function references by scanning for `CreateClosure`
/// instructions and tracking which `ConstI32` values they consume as their
/// first operand. Only those values get function offset rewriting, avoiding
/// false positives on integer constants that happen to be in range.
fn rewrite_function_indices(
    func: &mut TypedFunction,
    str_map: &HashMap<u32, u32>,
    func_offset: usize,
    num_module_functions: usize,
) {
    // If the function offset is 0 and there are no string remappings that
    // change indices, we can skip the rewrite (identity case for single module).
    let identity_strings = str_map.iter().all(|(k, v)| k == v);
    if func_offset == 0 && identity_strings {
        return;
    }

    // First pass: collect ValueIds that are function references.
    // A ConstI32 is a function reference if it's consumed as the first operand
    // of CreateClosure.
    let mut func_ref_values: std::collections::HashSet<ir::ValueId> =
        std::collections::HashSet::new();

    for block in &func.blocks {
        for instr in &block.instructions {
            if instr.op == Op::CreateClosure && !instr.operands.is_empty() {
                func_ref_values.insert(instr.operands[0]);
            }
        }
    }

    // Second pass: rewrite instructions.
    for block in &mut func.blocks {
        for instr in &mut block.instructions {
            match &mut instr.op {
                Op::ConstString(idx) => {
                    if let Some(&new_idx) = str_map.get(idx) {
                        *idx = new_idx;
                    }
                }
                Op::ConstI32(val)
                    if func_ref_values.contains(&instr.id)
                        && *val >= 0
                        && (*val as usize) < num_module_functions =>
                {
                    *val += func_offset as i32;
                }
                _ => {}
            }
        }
    }
}
