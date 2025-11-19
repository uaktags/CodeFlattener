// src/wordpress_profile.rs
use crate::profiles::{Profile, ProfilePlugin};
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

pub struct WordPressProfilePlugin;

impl ProfilePlugin for WordPressProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile> {
        if name != "wordpress" {
            return None;
        }
        // Return a conservative default WordPress profile
        Some(Self::create_wordpress_profile())
    }

    fn list_profiles(&self) -> Vec<String> {
        vec!["wordpress".to_string()]
    }
}

impl WordPressProfilePlugin {
    fn create_wordpress_profile() -> Profile {
        let mut profile = Profile::new(
            "WordPress site with active theme and plugins.".to_string(),
            vec![
                ".php".to_string(),
                ".js".to_string(),
                ".css".to_string(),
                ".scss".to_string(),
                ".sass".to_string(),
                ".less".to_string(),
                ".html".to_string(),
                ".htm".to_string(),
                ".md".to_string(),
                ".mdx".to_string(),
                ".json".to_string(),
                ".xml".to_string(),
                ".yml".to_string(),
                ".yaml".to_string(),
                ".ini".to_string(),
                ".env".to_string(),
                ".env.local".to_string(),
                ".env.development".to_string(),
                ".env.production".to_string(),
                ".txt".to_string(),
            ],
            vec![
                "wp-config.php".to_string(),
                "wp-cli.yml".to_string(),
                "composer.json".to_string(),
                "package.json".to_string(),
                "webpack.config.js".to_string(),
                "tailwind.config.js".to_string(),
                "postcss.config.js".to_string(),
            ],
        );
        profile.include_globs = Vec::new();
        profile.markdown = None;
        profile
    }

    /// Build a path-aware WordPress profile using wp-cli when available.
    pub fn get_profile_for_path(
        &self,
        name: &str,
        wp_path: &std::path::Path,
        exclude_plugins: Option<&[String]>,
        include_only_plugins: Option<&[String]>,
        include_theme: Option<&str>,
    ) -> Option<Profile> {
        if name != "wordpress" {
            return None;
        }

        // 1. Handle Explicit Includes (User specific specific plugins/themes)
        if include_only_plugins.is_some() || include_theme.is_some() {
            info!("Using explicit include profile for WordPress");
            let allowed_extensions = vec![
                ".php".to_string(), ".js".to_string(), ".css".to_string(), ".scss".to_string(),
                ".sass".to_string(), ".less".to_string(), ".html".to_string(), ".htm".to_string(),
                ".md".to_string(), ".mdx".to_string(), ".json".to_string(), ".xml".to_string(),
                ".yml".to_string(), ".yaml".to_string(), ".ini".to_string(), ".env".to_string(),
                ".env.local".to_string(), ".env.development".to_string(), ".env.production".to_string(),
                ".txt".to_string(),
            ];
            let mut allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

            if let Some(theme_name) = include_theme {
                let theme_dir = wp_path.join("wp-content/themes").join(theme_name);
                for file in &["functions.php", "style.css"] {
                    let fp = theme_dir.join(file);
                    if fp.exists() {
                        if let Ok(rel) = fp.strip_prefix(wp_path) {
                            allowed_filenames.push(rel.to_string_lossy().replace('\\', "/"));
                        } else {
                            allowed_filenames.push(fp.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
            }

            let mut plugin_names = Vec::new();
            if let Some(includes) = include_only_plugins {
                for p in includes {
                    plugin_names.push(p.to_string());
                }
            } else if let Some(excludes) = exclude_plugins {
                let all = self.get_active_plugins().unwrap_or_default();
                for pd in all {
                    if let Some(n) = pd.file_name().and_then(|s| s.to_str()) {
                        let slug = n.to_lowercase();
                        if !excludes.iter().any(|e| e.to_lowercase() == slug) {
                            plugin_names.push(n.to_string());
                        }
                    }
                }
            } else {
                plugin_names = self
                    .get_active_plugins()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                    .collect();
            }

            for plugin in plugin_names {
                let plugin_dir = wp_path.join("wp-content/plugins").join(&plugin);
                let main = format!("{}.php", plugin);
                let pf = plugin_dir.join(&main);
                if pf.exists() {
                    if let Ok(rel) = pf.strip_prefix(wp_path) {
                        allowed_filenames.push(rel.to_string_lossy().replace('\\', "/"));
                    } else {
                        allowed_filenames.push(pf.to_string_lossy().replace('\\', "/"));
                    }
                }
            }

            let mut profile = Profile::new(
                "WordPress site with specific theme/plugins.".to_string(),
                allowed_extensions,
                allowed_filenames,
            );
            return Some(profile);
        }

        // 2. Default Path-Aware Detection (WP-CLI)
        let mut allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

        info!("Running `wp theme list` in {}", wp_path.display());
        let theme_path = if let Ok(output) = Command::new("wp")
            .args(["theme", "list", "--format=json", "--status=active"])
            .current_dir(wp_path)
            .output()
        {
            if output.status.success() {
                if let Ok(themes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                {
                    themes
                        .first()
                        .and_then(|t| t.get("name").and_then(|n| n.as_str()))
                        .map(|n| wp_path.join("wp-content/themes").join(n))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(tdir) = theme_path {
            for file in &["functions.php", "style.css"] {
                let fp = tdir.join(file);
                if fp.exists() {
                    if let Ok(rel) = fp.strip_prefix(wp_path) {
                        allowed_filenames.push(rel.to_string_lossy().replace('\\', "/"));
                    } else {
                        allowed_filenames.push(fp.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }

        let mut plugin_names: Vec<String> = if let Ok(output) = Command::new("wp")
            .args(["plugin", "list", "--format=json", "--status=active"])
            .current_dir(wp_path)
            .output()
        {
            if output.status.success() {
                if let Ok(pl) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                    pl.iter()
                        .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        if plugin_names.is_empty() {
            if let Ok(av) = self.get_available_plugins() {
                plugin_names = av
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                    .collect();
            }
        }

        for plugin in plugin_names {
             let slug = plugin.split('/').next().unwrap_or(&plugin).to_string();
             if let Some(excludes) = exclude_plugins {
                 if excludes.iter().any(|e| e.to_lowercase() == slug.to_lowercase()) {
                     info!("Excluding plugin '{}'", slug);
                     continue;
                 }
             }

             let plugin_dir = wp_path.join("wp-content/plugins").join(&slug);
             let main = format!("{}.php", slug);
             let pf = plugin_dir.join(&main);
             if pf.exists() {
                 if let Ok(rel) = pf.strip_prefix(wp_path) {
                     allowed_filenames.push(rel.to_string_lossy().replace('\\', "/"));
                 } else {
                     allowed_filenames.push(pf.to_string_lossy().replace('\\', "/"));
                 }
             }
        }

        let allowed_extensions = vec![
            ".js".to_string(), ".css".to_string(), ".scss".to_string(), ".sass".to_string(),
            ".less".to_string(), ".json".to_string(), ".txt".to_string(), ".md".to_string(),
        ];

        Some(Profile::new(
            "WordPress site with active theme and plugins (path-aware).".to_string(),
            allowed_extensions,
            allowed_filenames,
        ))
    }

    pub fn get_active_plugins(&self) -> Result<Vec<PathBuf>> {
        if let Ok(output) = Command::new("wp")
            .args(["plugin", "list", "--format=json", "--status=active"])
            .output()
        {
            if output.status.success() {
                if let Ok(plugins) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                    let paths = plugins
                        .iter()
                        .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(|s| PathBuf::from("wp-content/plugins").join(s)))
                        .collect();
                    return Ok(paths);
                }
            }
        }
        self.get_available_plugins()
    }

    pub fn get_available_plugins(&self) -> Result<Vec<PathBuf>> {
        let plugins_dir = PathBuf::from("wp-content/plugins");
        if !plugins_dir.exists() { return Ok(Vec::new()); }
        let mut res = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        if let Some(n) = entry.file_name().to_str() {
                            if !n.starts_with('.') { res.push(entry.path()); }
                        }
                    }
                }
            }
        }
        Ok(res)
    }
}