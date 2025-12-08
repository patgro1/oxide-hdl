use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
pub struct OxideConfig {
    // Default: ["build", "sim", "synth", "target", ".git"]
    #[serde(default = "default_ignores")]
    pub ignore: Vec<String>,

    // Default: ["vhd", "vhdl"]
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
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
    pub fn load(root_path: &Path) -> Self {
        let config_path = root_path.join("oxide.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(config_path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_else(|_| Self::default())
        } else {
            Self::default()
        }
    }

    pub fn default() -> Self {
        OxideConfig {
            ignore: default_ignores(),
            extensions: default_ignores(),
        }
    }

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
