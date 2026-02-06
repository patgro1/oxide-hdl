use std::{
    fs,
    path::{Path, PathBuf},
};

use include_dir::{Dir, include_dir};

/// Embedded VHDL standard library files (IEEE, STD, etc.) compiled into the binary.
static VHDL_LIBRARIES: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources/libraries");

/// Manages extraction and caching of built-in VHDL library files.
///
/// The LSP embeds standard library source files (IEEE std_logic_1164, numeric_std, etc.)
/// directly in the binary. This manager extracts them to a temporary cache directory
/// on startup so they can be parsed and indexed like user code.
///
/// # Cache Location
/// Files are extracted to `{temp_dir}/oxide_lsp_cache/` preserving the original
/// directory structure (e.g., `ieee/std_logic_1164.vhdl`).
pub struct BuiltinManager {
    /// Root directory for the extracted library cache.
    cache_root: PathBuf,
}

impl BuiltinManager {
    /// Creates a new BuiltinManager and ensures the cache directory exists.
    ///
    /// # Panics
    /// Panics if the cache directory cannot be created.
    pub fn new() -> Self {
        let mut cache_root = std::env::temp_dir();
        cache_root.push("oxide_lsp_cache");

        if !cache_root.exists() {
            fs::create_dir(&cache_root).expect("Failed to create cache root");
        }

        Self { cache_root }
    }

    /// Extracts all embedded standard library files to the cache directory.
    ///
    /// Files are only written if they don't already exist, making subsequent
    /// startups faster. This should be called once at LSP initialization.
    pub fn initialize(&self) {
        // We just dump the libraries in the cache directory
        extract_embedded_files(&VHDL_LIBRARIES, &self.cache_root);
    }
}

/// Recursively extracts files from an embedded directory to the filesystem.
///
/// # Arguments
/// * `dir` - The embedded directory to extract from.
/// * `sys_path` - The filesystem path to extract to.
fn extract_embedded_files(dir: &Dir, sys_path: &Path) {
    for file in dir.files() {
        let file_path = sys_path.join(file.path());

        if !file_path.exists() {
            if let Some(parent) = file_path.parent() {
                fs::create_dir(parent).ok();
            }
            fs::write(&file_path, file.contents()).ok();
        }
    }
    for subdir in dir.dirs() {
        extract_embedded_files(subdir, sys_path);
    }
}
