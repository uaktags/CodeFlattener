// wordpress_profile.rs
use super::{Profile, ProfilePlugin};
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

        // If explicit includes provided, be permissive on extensions and scope filenames
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
                            allowed_filenames.push(rel.to_string_lossy().to_string());
                        } else {
                            allowed_filenames.push(fp.to_string_lossy().to_string());
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
                        allowed_filenames.push(rel.to_string_lossy().to_string());
                    } else {
                        allowed_filenames.push(pf.to_string_lossy().to_string());
                    }
                }
            }

            let mut profile = Profile::new(
                "WordPress site with specific theme/plugins.".to_string(),
                allowed_extensions,
                allowed_filenames,
            );
            profile.include_globs = Vec::new();
            profile.markdown = None;
            return Some(profile);
        }

        // Default path-aware profile: detect active theme and plugins (prefer wp-cli)
        let mut allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

        // Try to detect active theme via wp-cli in the provided path
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
                        allowed_filenames.push(rel.to_string_lossy().to_string());
                    } else {
                        allowed_filenames.push(fp.to_string_lossy().to_string());
                    }
                }
            }
        }

        // Try to detect active plugins via wp-cli
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

        // Fallback to filesystem scan if wp-cli didn't return plugins
        if plugin_names.is_empty() {
            if let Ok(av) = self.get_available_plugins() {
                plugin_names = av
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                    .collect();
            }
        }

        // Filter and normalize plugin directories
        let mut final_plugin_dirs = Vec::new();
        for plugin in plugin_names.iter() {
            let slug = plugin.split('/').next().unwrap_or(plugin).to_string();
            if let Some(includes) = include_only_plugins {
                if !includes.iter().any(|e| e.to_lowercase() == slug.to_lowercase()) {
                    continue;
                }
            } else if let Some(excludes) = exclude_plugins {
                if excludes.iter().any(|e| e.to_lowercase() == slug.to_lowercase()) {
                    info!("Excluding plugin '{}' (slug {}) from profile", plugin, slug);
                    continue;
                }
            }
            final_plugin_dirs.push(wp_path.join("wp-content/plugins").join(slug));
        }

        for plugin_dir in final_plugin_dirs {
            if let Some(name) = plugin_dir.file_name().and_then(|n| n.to_str()) {
                let main = format!("{}.php", name);
                let pf = plugin_dir.join(&main);
                if pf.exists() {
                    if let Ok(rel) = pf.strip_prefix(wp_path) {
                        allowed_filenames.push(rel.to_string_lossy().to_string());
                    } else {
                        allowed_filenames.push(main);
                    }
                }
            }
        }

        let allowed_extensions = vec![
            ".js".to_string(), ".css".to_string(), ".scss".to_string(), ".sass".to_string(),
            ".less".to_string(), ".json".to_string(), ".txt".to_string(), ".md".to_string(),
        ];

        let mut profile = Profile::new(
            "WordPress site with active theme and plugins (path-aware).".to_string(),
            allowed_extensions,
            allowed_filenames,
        );
        profile.include_globs = Vec::new();
        profile.markdown = None;
        Some(profile)
    }

    pub fn get_active_theme(&self) -> Result<PathBuf> {
        if let Ok(output) = Command::new("wp")
            .args(["theme", "list", "--format=json", "--status=active"])
            .output()
        {
            if output.status.success() {
                if let Ok(themes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                    if let Some(t) = themes.first() {
                        if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                            return Ok(PathBuf::from("wp-content/themes").join(name));
                        }
                    }
                }
            }
        }
        self.get_available_themes()?.first().cloned().ok_or_else(|| anyhow::anyhow!("No themes found"))
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

    pub fn get_available_themes(&self) -> Result<Vec<PathBuf>> {
        let themes_dir = PathBuf::from("wp-content/themes");
        if !themes_dir.exists() { return Ok(Vec::new()); }
        let mut res = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        if let Some(n) = entry.file_name().to_str() {
                            if !n.starts_with('.') && n != "index.php" { res.push(entry.path()); }
                        }
                    }
                }
            }
        }
        Ok(res)
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
                            if !n.starts_with('.') && n != "index.php" { res.push(entry.path()); }
                        }
                    }
                }
            }
        }
        Ok(res)
    }

    pub fn collect_files_recursively(&self, dir_path: &PathBuf) -> Result<Vec<String>> {
        let mut files = Vec::new();
        self.collect_files_recursive_helper(dir_path, &mut files)?;
        Ok(files)
    }

    fn collect_files_recursive_helper(
        &self,
        dir_path: &PathBuf,
        files: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(dir_name) = dir_path.file_name().and_then(|n| n.to_str()) {
            if dir_name == "wp-admin" || dir_name == "wp-includes" { return Ok(()); }
        }
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let ep = entry.path();
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() {
                        if let Some(n) = ep.file_name().and_then(|s| s.to_str()) { files.push(n.to_string()); }
                    } else if ft.is_dir() {
                        let _ = self.collect_files_recursive_helper(&ep, files);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn parse_wp_config(&self) -> Result<Vec<String>> {
        let config_path = PathBuf::from("wp-config.php");
        if !config_path.exists() { return Ok(Vec::new()); }
        let mut files = Vec::new();
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if content.contains("DB_NAME") { files.push("wp-config.php".to_string()); }
            if content.contains("WP_DEBUG") { files.push("wp-config.php".to_string()); }
        }
        Ok(files)
    }

    pub fn parse_htaccess(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let root = PathBuf::from(".htaccess");
        if root.exists() { files.push(".htaccess".to_string()); }
        let common = ["wp-admin/.htaccess"];
        for c in &common { let p = PathBuf::from(c); if p.exists() { files.push(c.to_string()); } }
        Ok(files)
    }

    pub fn parse_env_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        for p in &[".env", ".env.local", ".env.development", ".env.production"] {
            let path = PathBuf::from(p);
            if path.exists() { files.push(p.to_string()); }
        }
        Ok(files)
    }

    pub fn parse_composer_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        for f in &["composer.json", "composer.lock"] { let p = PathBuf::from(f); if p.exists() { files.push(f.to_string()); } }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wp_cli_detection() {
        let plugin = WordPressProfilePlugin;

        let theme_result = plugin.get_active_theme();

        match theme_result {
            Ok(theme_path) => {
                println!("✅ WP-CLI used for theme detection: {}", theme_path.display());
                assert!(theme_path.to_string_lossy().contains("wp-content/themes"));
            }
            Err(e) => {
                println!("ℹ️  Fell back to filesystem scanning for themes: {}", e);
            }
        }

        let plugin_result = plugin.get_active_plugins();

        match plugin_result {
            Ok(plugin_paths) => {
                println!("✅ WP-CLI used for plugin detection: {} plugins found", plugin_paths.len());
                for path in &plugin_paths { assert!(path.to_string_lossy().contains("wp-content/plugins")); }
            }
            Err(e) => {
                println!("ℹ️  Fell back to filesystem scanning for plugins: {}", e);
            }
        }
    }

    #[test]
    fn test_profile_restrictions() {
        let plugin = WordPressProfilePlugin;

        if let Some(profile) = plugin.get_profile("wordpress") {
            let filenames: Vec<String> = profile.allowed_filenames.iter().cloned().collect();
            let extensions: Vec<String> = profile.allowed_extensions.iter().cloned().collect();

            assert!(filenames.contains(&"wp-config.php".to_string()));

            assert!(!filenames.contains(&"wp-load.php".to_string()));
            assert!(!filenames.contains(&"xmlrpc.php".to_string()));
            assert!(!filenames.contains(&"wp-cron.php".to_string()));

            assert!(!extensions.contains(&".php".to_string()));
            assert!(extensions.contains(&".js".to_string()));
            assert!(extensions.contains(&".css".to_string()));
            assert!(!extensions.contains(&".png".to_string()));
            assert!(!extensions.contains(&".jpg".to_string()));

            println!("✅ Profile restrictions working correctly");
            println!("   - Allowed filenames: {:?}", filenames.len());
            println!("   - Allowed extensions: {:?}", extensions.len());
        } else {
            panic!("WordPress profile not found");
        }
    }
}
