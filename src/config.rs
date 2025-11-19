use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Represents the structure of the .flattener.toml configuration file.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct ConfigFile {
    pub profile: Option<String>,
    pub extensions: Option<Vec<String>>,
    pub allowed_filenames: Option<Vec<String>>,
    pub max_size: Option<f64>,
    pub markdown: Option<bool>,
    pub gpt4_tokens: Option<bool>,
    pub include_git_changes: Option<bool>,
    pub no_staged_diff: Option<bool>,
    pub no_unstaged_diff: Option<bool>,
    pub include_dirs: Option<Vec<PathBuf>>,
    pub exclude_dirs: Option<Vec<PathBuf>>,
    pub exclude_patterns: Option<Vec<String>>,
    pub include_patterns: Option<Vec<String>>,
    pub exclude_globs: Option<Vec<String>>,
    pub include_globs: Option<Vec<String>>,
    pub exclude_node_modules: Option<bool>,
    pub exclude_build_dirs: Option<bool>,
    pub exclude_hidden_dirs: Option<bool>,
    pub max_depth: Option<usize>,

    // Custom profiles section: [profiles.my-profile]
    pub profiles: Option<HashMap<String, CustomProfile>>,
}

/// Represents a custom profile definition within the config file.
#[derive(Debug, Deserialize, Clone)]
pub struct CustomProfile {
    pub description: Option<String>,
    /// The name of the profile this one extends (e.g., "rust" or another custom one)
    #[serde(alias = "profile")]
    pub extends: Option<String>,
    pub extensions: Option<Vec<String>>,
    pub allowed_filenames: Option<Vec<String>>,
    pub include_globs: Option<Vec<String>>,
    pub markdown: Option<bool>,
}

/// Loads the configuration file from the given path or defaults to .flattener.toml
pub fn load_config(config_path: &Option<PathBuf>) -> Result<Option<ConfigFile>> {
    let path = config_path
        .as_ref()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(".flattener.toml"));

    if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: ConfigFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        return Ok(Some(config));
    }
    
    // If user explicitly provided a path that doesn't exist, that's an error.
    // If it's the default path and it doesn't exist, we just return None.
    if let Some(p) = config_path {
        anyhow::bail!("Configuration file not found at: {}", p.display());
    }

    Ok(None)
}