use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::Path;

/// Diagnostic trigger type
#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticTrigger {
    /// Trigger diagnostic only when the file is saved
    OnSave,
    /// Trigger diagnostic for each change
    OnChange,
}

impl Default for DiagnosticTrigger {
    fn default() -> Self {
        Self::OnSave
    }
}

/// Configuration for the Oxide HDL Language Server.
///
/// This struct dictates which files should be indexed and which should be ignored
/// to improve performance and accuracy (e.g., ignoring simulation artifacts).
/// It is usually loaded from an `oxide.toml` file in the workspace root.
#[derive(Deserialize, Debug, Clone)]
pub struct OxideConfig {
    /// List of glob patterns to ignore during indexing.
    /// Default: `["**/build/**", "**/sim/**", "**/synth/**", "**/target/**", "**/.git/**"]`, ".git"]
    #[serde(default = "default_ignores")]
    pub ignore: Vec<String>,

    /// List of file extensions to treat as VHDL source files.
    /// Default: `["vhd", "vhdl"]`
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,

    /// Trigger Diagnosic
    /// Default: on_save
    #[serde(default)]
    pub diagnostics: DiagnosticTrigger,
}

fn default_ignores() -> Vec<String> {
    vec![
        "**/build/**".to_string(),
        "**/sim/**".to_string(),
        "**/target/**".to_string(),
        "**/.git/**".to_string(),
        "**/work/**".to_string(), // Vivado/Quartus work libs often junk
    ]
}

fn default_extensions() -> Vec<String> {
    vec!["vhd".to_string(), "vhdl".to_string()]
}

impl OxideConfig {
    /// Loads configuration from an `oxide.toml` file in the given root directory.
    ///
    /// If the file does not exist or contains invalid TOML, it falls back to the
    /// default configuration safe defaults.
    ///
    /// # Arguments
    ///
    /// * `root_path` - The root directory of the workspace (where `oxide.toml` looks for).
    ///
    /// # Returns
    ///
    /// An `OxideConfig` struct populated from the file or defaults.
    pub fn load(root_path: &Path) -> Self {
        let config_path = root_path.join("oxide.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(config_path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_else(|_| Self::default())
        } else {
            Self::default()
        }
    }

    /// Returns the default configuration.
    ///
    /// Used when no config file is found or when deserialization fails.
    ///
    /// # Returns
    ///
    /// A new `OxideConfig` with standard ignore patterns and VHDL extensions.
    pub fn default() -> Self {
        OxideConfig {
            ignore: default_ignores(),
            extensions: default_extensions(),
            diagnostics: DiagnosticTrigger::default(),
        }
    }

    /// Compiles the string-based ignore patterns into a highly optimized `GlobSet`.
    ///
    /// This is used by the `workspace` scanner to filter files efficiently during
    /// the directory walk.
    ///
    /// # Returns
    ///
    /// A `globset::GlobSet` ready for matching paths.
    ///
    /// # Panics
    ///
    /// Panics if the `GlobSet` cannot be built (e.g., if valid glob limits are exceeded),
    /// though individual invalid patterns are skipped silently.
    pub fn build_globset(&self) -> GlobSet {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.ignore {
            if let Ok(glob) = Glob::new(pattern) {
                builder.add(glob);
            };
        }
        builder.build().expect("Failed to build glob set")
    }
}
