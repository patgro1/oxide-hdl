use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Diagnostic trigger type
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticTrigger {
    /// Trigger diagnostic only when the file is saved
    #[default]
    OnSave,
    /// Trigger diagnostic for each change
    OnChange,
}

/// Configuration for the Oxide HDL Language Server.
///
/// This struct dictates which files should be indexed and which should be ignored
/// to improve performance and accuracy (e.g., ignoring simulation artifacts).
/// It is usually loaded from an `oxide.toml` file in the workspace root.
#[derive(Deserialize, Debug, Clone)]
pub struct OxideConfig {
    /// List of glob patterns to ignore during indexing.
    /// Default: `["**/build/**", "**/sim/**", "**/target/**", "**/.git/**", "**/work/**"]`
    #[serde(default = "default_ignores")]
    pub ignore: Vec<String>,

    /// List of file extensions to treat as VHDL source files.
    /// Default: `["vhd", "vhdl"]`
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,

    /// Trigger for running diagnostics.
    /// Default: `on_save`
    #[serde(default)]
    pub diagnostics: DiagnosticTrigger,

    /// List of regex pattern for identifiers to ignore during undeclared variable checks
    /// Example: ["^REG_.*", "^AUTO_.*", "BUILD_ID"]
    #[serde(default)]
    pub ignored_identifiers: Vec<String>,

    /// List of external workspace directories to include for indexing.
    /// This is useful when a repository depends on another repository.
    #[serde(default)]
    pub include_workspace: Vec<String>,

    /// Maps VHDL library names to the path globs whose files belong to that library.
    ///
    /// Globs are matched against the path relative to the workspace root and, failing
    /// that, against the absolute path — the latter is what lets vendor libraries
    /// outside the workspace be declared. Files matching nothing belong to `work`.
    ///
    /// ```toml
    /// [libraries]
    /// rtl_lib = ["rtl/**/*.vhd"]
    /// unisim    = ["/opt/Xilinx/**/unisims/**/*.vhd"]
    /// ```
    #[serde(default)]
    pub libraries: HashMap<String, Vec<String>>,

    /// Workspace root, captured by `load()`. Not part of the TOML schema.
    /// Used to resolve relative library globs. Empty for `default()`.
    #[serde(skip)]
    pub root: PathBuf,
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
    /// If the file does not exist or contains invalid TOML, it falls back to
    /// default configuration with safe defaults.
    ///
    /// # Arguments
    ///
    /// * `root_path` - The root directory of the workspace (where to look for `oxide.toml`).
    ///
    /// # Returns
    ///
    /// An `OxideConfig` struct populated from the file or defaults.
    pub fn load(root_path: &Path) -> Self {
        let config_path = root_path.join("oxide.toml");

        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(config_path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_else(|_| Self::default())
        } else {
            Self::default()
        };
        config.root = root_path.to_path_buf();
        config
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
            ignored_identifiers: vec![],
            include_workspace: vec![],
            libraries: HashMap::new(),
            root: PathBuf::new(),
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

    /// Compiles the ignored identifiers patterns into Regex objects.
    /// Invalid patterns are silently skipped to preven crashing.
    pub fn ignored_identifiers_regex(&self) -> Vec<Regex> {
        self.ignored_identifiers
            .iter()
            .filter_map(|s| RegexBuilder::new(s).case_insensitive(true).build().ok())
            .collect()
    }
}

/// Resolves which VHDL library a source file belongs to, based on the
/// `[libraries]` globs from `oxide.toml`.
///
/// Entries are sorted by library name at construction so that a file matching
/// several libraries always resolves to the same one (the alphabetically first).
/// Files matching no library belong to `work`, which is also the behaviour when
/// no `[libraries]` section is present at all.
#[derive(Debug, Clone)]
pub struct LibraryMatcher {
    /// (lowercase library name, compiled globs), sorted by name.
    entries: Vec<(String, GlobSet)>,
    root: PathBuf,
}

impl LibraryMatcher {
    /// Builds a matcher from raw (library name, globs) pairs.
    ///
    /// Invalid glob patterns are skipped silently, consistent with `build_globset`.
    ///
    /// # Arguments
    /// * `entries` - Library name paired with its list of glob patterns.
    /// * `root` - Workspace root, used to relativize paths before matching.
    pub fn new(entries: Vec<(String, Vec<String>)>, root: PathBuf) -> Self {
        let mut compiled: Vec<(String, GlobSet)> = entries
            .into_iter()
            .filter_map(|(name, globs)| {
                let mut builder = GlobSetBuilder::new();
                for pattern in &globs {
                    // `literal_separator(true)` stops `*` from crossing `/`, which is
                    // globset's default. Without it `rtl/*.vhd` also matches
                    // `rtl/a/b/deep.vhd`, so two libraries with patterns the user
                    // believes are disjoint would both match and be silently resolved
                    // by the alphabetical tie-break. `**` still spans directories,
                    // including zero of them.
                    //
                    // NOTE: `build_globset()` above deliberately keeps the loose
                    // default. Tightening it would change which files existing users
                    // have indexed; `[libraries]` is new, so it has no such history.
                    if let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() {
                        builder.add(glob);
                    }
                }
                builder.build().ok().map(|set| (name.to_lowercase(), set))
            })
            .collect();
        compiled.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            entries: compiled,
            root,
        }
    }

    /// Builds a matcher from a loaded configuration.
    pub fn from_config(config: &OxideConfig) -> Self {
        let entries = config
            .libraries
            .iter()
            .map(|(name, globs)| (name.clone(), globs.clone()))
            .collect();
        Self::new(entries, config.root.clone())
    }

    /// Returns the lowercase library name owning `path`, or `"work"`.
    ///
    /// Each glob set is tried against the workspace-relative path first, then
    /// against the absolute path so that vendor libraries outside the workspace
    /// can be declared with absolute globs.
    pub fn library_for(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).ok();
        for (name, set) in &self.entries {
            if let Some(rel) = relative
                && set.is_match(rel)
            {
                return name.clone();
            }
            if set.is_match(path) {
                return name.clone();
            }
        }
        "work".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn matcher(entries: &[(&str, &[&str])], root: &str) -> LibraryMatcher {
        LibraryMatcher::new(
            entries
                .iter()
                .map(|(name, globs)| {
                    (
                        name.to_string(),
                        globs.iter().map(|g| g.to_string()).collect(),
                    )
                })
                .collect(),
            PathBuf::from(root),
        )
    }

    #[test]
    fn test_no_libraries_configured_yields_work() {
        let m = matcher(&[], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "work");
    }

    #[test]
    fn test_relative_glob_match() {
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/core/cpu.vhd")),
            "rtl_lib"
        );
    }

    #[test]
    fn test_non_matching_path_falls_back_to_work() {
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/tb/cpu_tb.vhd")), "work");
    }

    #[test]
    fn test_absolute_glob_matches_outside_workspace() {
        let m = matcher(&[("unisim", &["/opt/Xilinx/**/unisims/**/*.vhd"])], "/ws");
        assert_eq!(
            m.library_for(&PathBuf::from("/opt/Xilinx/2024/data/unisims/prims/BUFG.vhd")),
            "unisim"
        );
    }

    #[test]
    fn test_library_name_is_lowercased() {
        let m = matcher(&[("Rtl_Lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "rtl_lib");
    }

    #[test]
    fn test_ambiguous_match_picks_alphabetically_first() {
        // Both patterns match; "alpha" must win deterministically over "zeta".
        let m = matcher(
            &[("zeta", &["rtl/**/*.vhd"]), ("alpha", &["rtl/**/*.vhd"])],
            "/ws",
        );
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "alpha");
    }

    #[test]
    fn test_star_does_not_cross_directory_separator() {
        // globset's `*` crosses `/` by DEFAULT. For library assignment that is a
        // silent-misassignment footgun, so library globs are built with
        // `literal_separator(true)`. `rtl/*.vhd` must mean "directly in rtl/".
        let m = matcher(&[("rtl_lib", &["rtl/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/top.vhd")), "rtl_lib");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/core/cpu.vhd")),
            "work",
            "`*` must not descend into subdirectories"
        );
    }

    #[test]
    fn test_double_star_matches_zero_directories() {
        // `rtl/**/*.vhd` must also match a file sitting directly in rtl/, otherwise
        // every top-level file silently falls into `work`.
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/top.vhd")), "rtl_lib");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/a/b/deep.vhd")),
            "rtl_lib"
        );
    }

    #[test]
    fn test_invalid_glob_is_skipped_not_panicking() {
        // "[" is an unterminated character class — must be skipped silently.
        let m = matcher(&[("broken", &["["]), ("good", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "good");
    }

    #[test]
    fn test_default_config_has_empty_libraries() {
        let c = OxideConfig::default();
        assert!(c.libraries.is_empty());
        assert_eq!(c.root, PathBuf::new());
    }

    #[test]
    fn test_from_config_reads_libraries_table() {
        let toml_src = r#"
[libraries]
rtl_lib = ["rtl/**/*.vhd"]
"#;
        let mut c: OxideConfig = toml::from_str(toml_src).unwrap();
        c.root = PathBuf::from("/ws");
        let m = LibraryMatcher::from_config(&c);
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "rtl_lib");
    }
}
