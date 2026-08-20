//! ESM import resolution.
//!
//! Resolves import specifiers to filesystem paths following simplified
//! Node.js/ESM resolution rules.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors that can occur during module resolution.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// The module could not be found at any candidate path.
    #[error("module not found: {0}")]
    NotFound(String),
    /// The import specifier is malformed or unsupported.
    #[error("invalid specifier: {0}")]
    InvalidSpecifier(String),
}

/// Module resolver that resolves import specifiers to filesystem paths.
pub struct ModuleResolver {
    /// Base directory used for bare specifier resolution (node_modules lookup).
    base_dir: PathBuf,
}

/// Extensions to try when resolving imports without an extension.
const EXTENSIONS: &[&str] = &[".js", ".ts", ".mjs", ".mts"];

/// Index files to try when resolving a directory import.
const INDEX_FILES: &[&str] = &["index.js", "index.ts", "index.mjs", "index.mts"];

impl ModuleResolver {
    /// Create a new resolver with the given base directory.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Resolve an import specifier relative to the importing module's path.
    ///
    /// Resolution rules:
    /// - Relative paths (`./foo`, `../foo`) are resolved relative to `from`
    /// - Absolute paths (`/foo`) are used as-is
    /// - Bare specifiers (`lodash`) are looked up in node_modules
    ///
    /// For paths without an extension, tries `.js`, `.ts`, `.mjs`, `.mts`,
    /// then `/index.js`, `/index.ts`, `/index.mjs`, `/index.mts`.
    pub fn resolve(&self, specifier: &str, from: &Path) -> Result<PathBuf, ResolveError> {
        if specifier.is_empty() {
            return Err(ResolveError::InvalidSpecifier(
                "empty specifier".to_string(),
            ));
        }

        if specifier.starts_with("./") || specifier.starts_with("../") {
            // Relative import
            let dir = from
                .parent()
                .ok_or_else(|| ResolveError::InvalidSpecifier(specifier.to_string()))?;
            let candidate = dir.join(specifier);
            self.resolve_file_or_dir(&candidate, specifier)
        } else if specifier.starts_with('/') {
            // Absolute import
            let candidate = PathBuf::from(specifier);
            self.resolve_file_or_dir(&candidate, specifier)
        } else {
            // Bare specifier — look in node_modules
            let candidate = self.base_dir.join("node_modules").join(specifier);
            self.resolve_file_or_dir(&candidate, specifier)
        }
    }

    /// Try to resolve a candidate path, trying extensions and index files.
    fn resolve_file_or_dir(
        &self,
        candidate: &Path,
        specifier: &str,
    ) -> Result<PathBuf, ResolveError> {
        // 1. Exact match
        if candidate.is_file() {
            return Ok(candidate.to_path_buf());
        }

        // 2. Try adding extensions
        if candidate.extension().is_none() {
            for ext in EXTENSIONS {
                let with_ext = candidate.with_extension(ext.trim_start_matches('.'));
                if with_ext.is_file() {
                    return Ok(with_ext);
                }
            }

            // 3. Try as directory with index files
            for index in INDEX_FILES {
                let index_path = candidate.join(index);
                if index_path.is_file() {
                    return Ok(index_path);
                }
            }
        }

        Err(ResolveError::NotFound(specifier.to_string()))
    }
}
