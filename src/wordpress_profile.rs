// wordpress_profile.rs
use super::{Profile, ProfilePlugin};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub struct WordPressProfilePlugin;

impl ProfilePlugin for WordPressProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile> {
        if name == "wordpress" {
            Some(WordPressProfilePlugin::create_wordpress_profile())
        } else {
            None
        }
    }

    fn list_profiles(&self) -> Vec<String> {
        vec!["wordpress".to_string()]
    }
}

impl WordPressProfilePlugin {
    fn create_wordpress_profile() -> Profile {
        Profile {
            description: "WordPress site with active theme and plugins.",
            allowed_extensions: &[
                ".php",
                ".js",
                ".css",
                ".scss",
                ".sass",
                ".less",
                ".html",
                ".htm",
                ".md",
                ".mdx",
                ".json",
                ".xml",
                ".yml",
                ".yaml",
                ".ini",
                ".env",
                ".env.local",
                ".env.development",
                ".env.production",
                ".txt",
            ],
            allowed_filenames: &[
                "wp-config.php",
                "wp-cli.yml",
                "composer.json",
                "package.json",
                "webpack.config.js",
                "tailwind.config.js",
                "postcss.config.js",
            ],
        }
    }

    pub fn get_active_theme(&self) -> Result<PathBuf> {
        let output = Command::new("wp")
            .args(["theme", "list", "--format=json", "--status=active"])
            .output()
            .context("Failed to execute wp-cli theme list")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "wp-cli theme list failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let themes: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse wp-cli theme list output")?;

        if let Some(theme) = themes.first() {
            if let Some(theme_name) = theme.get("name").and_then(|n| n.as_str()) {
                return Ok(PathBuf::from("wp-content/themes").join(theme_name));
            }
        }

        Err(anyhow::anyhow!("No active theme found"))
    }

    pub fn get_active_plugins(&self) -> Result<Vec<PathBuf>> {
        let output = Command::new("wp")
            .args(["plugin", "list", "--format=json", "--status=active"])
            .output()
            .context("Failed to execute wp-cli plugin list")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "wp-cli plugin list failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let plugins: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse wp-cli plugin list output")?;

        let mut plugin_paths = Vec::new();
        for plugin in plugins {
            if let Some(plugin_name) = plugin.get("name").and_then(|n| n.as_str()) {
                plugin_paths.push(PathBuf::from("wp-content/plugins").join(plugin_name));
            }
        }

        Ok(plugin_paths)
    }
}
