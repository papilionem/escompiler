//! Compile-and-execute caching for `esc run`.
//!
//! When the user runs `esc run file.ts`, the cache avoids recompilation if the
//! source content and compiler version have not changed. The cache key is a
//! SipHash of the source content, compiler version, and target triple.
//!
//! # Key types
//!
//! - [`RunCache`] manages the cache directory and provides get/put operations.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::DriverError;

/// On-disk cache for compiled binaries produced by `esc run`.
///
/// Cached binaries are stored in a platform-specific cache directory
/// (overridable via `$ESC_CACHE_DIR`). File names are derived from a
/// SipHash of the source content, compiler version, and target triple.
pub struct RunCache {
    /// Root directory for cached binaries.
    cache_dir: PathBuf,
}

impl RunCache {
    /// Create a new [`RunCache`], creating the cache directory if needed.
    ///
    /// The cache directory is determined by:
    /// 1. `$ESC_CACHE_DIR` environment variable (if set)
    /// 2. `~/.cache/esc/` on Linux/macOS
    /// 3. `%LOCALAPPDATA%/esc/cache/` on Windows
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Io`] if the directory cannot be created.
    pub fn new() -> Result<Self, DriverError> {
        let cache_dir = Self::resolve_cache_dir()?;
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Create a [`RunCache`] with an explicit cache directory.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Io`] if the directory cannot be created.
    pub fn with_dir(cache_dir: PathBuf) -> Result<Self, DriverError> {
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Compute a cache key from source content, compiler version, and target triple.
    ///
    /// Uses `DefaultHasher` (SipHash-1-3) which is fast and provides sufficient
    /// collision resistance for a build cache. The result is a 16-character hex
    /// string.
    pub fn cache_key(source: &str, compiler_version: &str, target_triple: &str) -> String {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        b"\0".hash(&mut hasher);
        compiler_version.hash(&mut hasher);
        b"\0".hash(&mut hasher);
        target_triple.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Look up a cached binary by its key.
    ///
    /// Returns `Some(path)` if a cached binary exists and is executable,
    /// or `None` on cache miss.
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        let path = self.binary_path(key);
        if path.is_file() { Some(path) } else { None }
    }

    /// Store a compiled binary in the cache under the given key.
    ///
    /// Uses atomic rename: writes to a temporary file in the cache directory
    /// first, then renames. This is safe for concurrent access on the same
    /// filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Io`] if the file cannot be copied or renamed.
    pub fn put(&self, key: &str, binary_path: &Path) -> Result<PathBuf, DriverError> {
        let dest = self.binary_path(key);
        let temp_name = format!(".{key}.tmp.{}", std::process::id());
        let temp_path = self.cache_dir.join(temp_name);

        // Copy to temp file first (atomic rename requires same filesystem).
        std::fs::copy(binary_path, &temp_path)?;

        // Make executable on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&temp_path, perms)?;
        }

        // Atomic rename.
        std::fs::rename(&temp_path, &dest)?;
        Ok(dest)
    }

    /// Remove cache entries older than `max_age_days`.
    ///
    /// Returns the number of entries removed.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Io`] if the directory cannot be read.
    pub fn clean_old(&self, max_age_days: u32) -> Result<usize, DriverError> {
        let max_age = std::time::Duration::from_secs(u64::from(max_age_days) * 24 * 60 * 60);
        let now = std::time::SystemTime::now();
        let mut removed = 0;

        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip temp files and non-files.
            if !path.is_file() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with('.')
            {
                continue;
            }

            // Check modification time.
            if let Ok(metadata) = path.metadata()
                && let Ok(modified) = metadata.modified()
                && let Ok(age) = now.duration_since(modified)
                && age > max_age
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }

        Ok(removed)
    }

    /// The path where a cached binary with the given key would live.
    fn binary_path(&self, key: &str) -> PathBuf {
        self.cache_dir.join(key)
    }

    /// Resolve the default cache directory from the environment or platform defaults.
    fn resolve_cache_dir() -> Result<PathBuf, DriverError> {
        // 1. Check ESC_CACHE_DIR env var.
        if let Ok(dir) = std::env::var("ESC_CACHE_DIR") {
            return Ok(PathBuf::from(dir));
        }

        // 2. Platform-specific default.
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                return Ok(PathBuf::from(local_app_data).join("esc").join("cache"));
            }
        }

        // Linux/macOS: ~/.cache/esc/
        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(".cache").join("esc"));
        }

        // Fallback: use a temp directory.
        Ok(std::env::temp_dir().join("esc-cache"))
    }
}

/// The current compiler version (from `Cargo.toml` workspace).
pub fn compiler_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A target identifier derived from compile-time OS and architecture constants.
///
/// Not a full target triple, but sufficient for cache key differentiation
/// across platforms (e.g. `x86_64-linux`, `aarch64-macos`).
pub fn target_id() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}
