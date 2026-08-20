//! modules — Module resolution, dependency graph, and incremental compilation.
//!
//! Provides ESM import resolution, topological ordering for compilation,
//! cycle detection, and API-hash-based incremental recompilation.

/// Import/export collection from JavaScript/TypeScript source.
pub mod exports;
/// Dependency graph with topological sort and cycle detection.
pub mod graph;
/// ESM import specifier resolution to filesystem paths.
pub mod resolver;
#[cfg(test)]
mod tests;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use exports::{ExportEntry, ExportKind, ImportBinding, ImportEntry, collect_dynamic_imports};
pub use graph::{CycleError, DependencyGraph};
pub use resolver::{ModuleResolver, ResolveError};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Unique identifier for a module within a compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModuleId(pub u32);

/// Hash of a module's public API surface (exports).
///
/// Used for incremental compilation: if a module's API hash hasn't changed,
/// its dependents do not need to be recompiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApiHash(pub u64);

/// Summary of a parsed module: its path, exports, imports, and API hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSummary {
    /// The module's unique identifier within this compilation.
    pub id: ModuleId,
    /// Filesystem path to the module's source file.
    pub path: PathBuf,
    /// Hash of the module's public API surface for incremental compilation.
    pub api_hash: ApiHash,
    /// All exports declared by this module.
    pub exports: Vec<ExportEntry>,
    /// All imports consumed by this module.
    pub imports: Vec<ImportEntry>,
    /// Whether this module uses ES module syntax (as opposed to CJS/script).
    pub is_esm: bool,
}

// ---------------------------------------------------------------------------
// API hash computation
// ---------------------------------------------------------------------------

/// Compute the API hash for a set of exports.
///
/// The hash captures export names and kinds, so any change to the public
/// surface of a module produces a different hash.
pub fn compute_api_hash(exports: &[ExportEntry]) -> ApiHash {
    let mut hasher = DefaultHasher::new();
    for export in exports {
        export.name.hash(&mut hasher);
        match &export.kind {
            ExportKind::Named => 0u8.hash(&mut hasher),
            ExportKind::Default => 1u8.hash(&mut hasher),
            ExportKind::ReExport { source } => {
                2u8.hash(&mut hasher);
                source.hash(&mut hasher);
            }
        }
    }
    ApiHash(hasher.finish())
}

// ---------------------------------------------------------------------------
// ModuleGraph
// ---------------------------------------------------------------------------

/// The top-level module graph for a compilation.
///
/// Holds all module summaries and a dependency graph for computing
/// compilation order and detecting cycles.
pub struct ModuleGraph {
    modules: Vec<ModuleSummary>,
    dep_graph: DependencyGraph,
}

impl ModuleGraph {
    /// Create an empty module graph.
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            dep_graph: DependencyGraph::new(),
        }
    }

    /// Add a module to the graph. Returns its assigned `ModuleId`.
    pub fn add_module(&mut self, mut summary: ModuleSummary) -> ModuleId {
        let id = ModuleId(self.modules.len() as u32);
        summary.id = id;
        self.dep_graph.add_node(id);
        self.modules.push(summary);
        id
    }

    /// Resolve imports across all modules and build dependency edges.
    ///
    /// For each import that has a `resolved_id`, adds an edge in the
    /// dependency graph from the importing module to the imported module.
    pub fn resolve_imports(&mut self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Collect edges first to avoid borrow issues
        let mut edges: Vec<(ModuleId, ModuleId)> = Vec::new();

        for module in &self.modules {
            for import in &module.imports {
                if let Some(resolved) = import.resolved_id {
                    let target = ModuleId(resolved);
                    if (target.0 as usize) < self.modules.len() {
                        edges.push((module.id, target));
                    } else {
                        errors.push(format!(
                            "module {:?}: import '{}' resolved to invalid id {}",
                            module.path, import.source, resolved
                        ));
                    }
                }
                // Unresolved imports are not errors here — they may be
                // external/builtin modules.
            }
        }

        for (from, to) in edges {
            self.dep_graph.add_edge(from, to);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Get the compilation order (topological sort, dependencies first).
    pub fn compilation_order(&self) -> Result<Vec<ModuleId>, CycleError> {
        self.dep_graph.topological_sort()
    }

    /// Get all module summaries.
    pub fn modules(&self) -> &[ModuleSummary] {
        &self.modules
    }

    /// Get a specific module by id.
    pub fn get_module(&self, id: ModuleId) -> Option<&ModuleSummary> {
        self.modules.get(id.0 as usize)
    }

    /// Check if a module needs recompilation based on its API hash.
    pub fn needs_recompile(&self, id: ModuleId, new_hash: ApiHash) -> bool {
        self.modules
            .get(id.0 as usize)
            .map(|m| m.api_hash != new_hash)
            .unwrap_or(true)
    }
}

impl Default for ModuleGraph {
    fn default() -> Self {
        Self::new()
    }
}
