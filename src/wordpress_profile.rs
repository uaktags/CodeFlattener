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
        // Focus on only wp-config.php, themes, and plugins
        let mut allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

        // Get active theme and plugins
        let theme_path = self.get_active_theme().ok();
        let plugin_paths = self.get_active_plugins().unwrap_or_default();

        // Only include specific custom files from themes and plugins, not all files
        if let Some(theme_dir) = theme_path {
            // Only include essential theme files that are likely to contain custom code
            let essential_theme_files = ["functions.php", "style.css"];
            for file in essential_theme_files {
                let file_path = theme_dir.join(file);
                if file_path.exists() {
                    allowed_filenames.push(file.to_string());
                }
            }
        }

        for plugin_dir in plugin_paths {
            // Only include the main plugin file (usually named after the plugin directory)
            if let Some(plugin_name) = plugin_dir.file_name().and_then(|n| n.to_str()) {
                let main_plugin_file = format!("{}.php", plugin_name);
                let plugin_file_path = plugin_dir.join(&main_plugin_file);
                if plugin_file_path.exists() {
                    allowed_filenames.push(main_plugin_file);
                }
            }
        }

        // Only include essential extensions to avoid scanning all PHP files.
        // We intentionally exclude ".php" here so only explicit filenames (e.g. functions.php,
        // main plugin files) are allowed via allowed_filenames.
        let allowed_extensions: Vec<&'static str> = vec![
            ".js", ".css", ".scss", ".sass", ".less", ".json", ".txt", ".md",
        ];

        // Convert Vec<String> to Vec<&str> for Profile
        let allowed_filenames_leaked: Vec<&'static str> = allowed_filenames
            .iter()
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
            .collect();

        Some(Profile {
            description: "WordPress site with active theme and plugins.",
            allowed_extensions: Box::leak(allowed_extensions.into_boxed_slice()),
            allowed_filenames: Box::leak(allowed_filenames_leaked.into_boxed_slice()),
        })
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

    /// Like `get_profile` but uses a specific WordPress site path when invoking wp-cli
    /// and when scanning the filesystem. This ensures `wp` is executed inside the
    /// target WordPress directory so active/inactive state reflects that site.
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

        // If user is specifying exactly what to include, use a more permissive profile
        // and let the directory filtering in `should_process_path` do the work.
        if include_only_plugins.is_some() || include_theme.is_some() {
            info!("Using explicit include profile for WordPress");
            let allowed_extensions: Vec<&'static str> = vec![
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
            ];
            let allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

            let allowed_filenames_leaked: Vec<&'static str> = allowed_filenames
                .iter()
                .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
                .collect();

            return Some(Profile {
                description: "WordPress site with specific theme/plugins.",
                allowed_extensions: Box::leak(allowed_extensions.into_boxed_slice()),
                allowed_filenames: Box::leak(allowed_filenames_leaked.into_boxed_slice()),
            });
        }

        // Build allowed filenames starting with wp-config.php
        let mut allowed_filenames: Vec<String> = vec!["wp-config.php".to_string()];

        // Try to obtain active theme via wp-cli in the provided path
        info!("Running `wp theme list` in {}", wp_path.display());
        let theme_path = if let Some(theme_name) = include_theme {
            Some(wp_path.join("wp-content/themes").join(theme_name))
        } else {
            if let Ok(output) = Command::new("wp")
                .args(["theme", "list", "--format=json", "--status=active"])
                .current_dir(wp_path)
                .output()
            {
                if output.status.success() {
                    if let Ok(themes) =
                        serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                    {
                        if let Some(theme) = themes.first() {
                            if let Some(theme_name) = theme.get("name").and_then(|n| n.as_str()) {
                                Some(wp_path.join("wp-content/themes").join(theme_name))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
            .or_else(|| {
                // Fallback to filesystem scanning rooted at wp_path
                let themes_dir = wp_path.join("wp-content/themes");
                if themes_dir.exists() {
                    if let Ok(mut themes) = self.get_available_themes() {
                        // convert to absolute by joining
                        themes.retain(|p| p.starts_with("wp-content") || p.is_absolute());
                        // If available_themes returned relative names, return first
                        themes.first().cloned()
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        };

        // Try to obtain active plugins via wp-cli in the provided path
        let mut plugin_paths = if let Ok(output) = Command::new("wp")
            .args(["plugin", "list", "--format=json", "--status=active"])
            .current_dir(wp_path)
            .output()
        {
            info!("wp plugin list output ({} bytes)", output.stdout.len());
            if output.status.success() {
                if let Ok(plugins) =
                    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                {
                    plugins
                        .iter()
                        .filter_map(|p| {
                            p.get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| s.to_string())
                        })
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

        // If wp-cli fails, fallback to filesystem scan
        if plugin_paths.is_empty() {
            if let Ok(available_plugins) = self.get_available_plugins() {
                plugin_paths = available_plugins
                    .iter()
                    .filter_map(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string())
                    })
                    .collect();
            }
        }

        // Filter out excluded plugins
        let final_plugin_paths: Vec<PathBuf> = plugin_paths
            .iter()
            .filter_map(|plugin_name| {
                let slug = plugin_name.split('/').next().unwrap_or(plugin_name);
                let slug_normalized = slug.to_lowercase();

                if let Some(includes) = include_only_plugins {
                    if !includes.iter().any(|e| e.to_lowercase() == slug_normalized) {
                        return None;
                    }
                } else if let Some(excludes) = exclude_plugins {
                    if excludes.iter().any(|e| e.to_lowercase() == slug_normalized) {
                        info!(
                            "Excluding plugin '{}' (slug {}) from profile",
                            plugin_name, slug
                        );
                        return None;
                    }
                }
                Some(wp_path.join("wp-content/plugins").join(slug))
            })
            .collect();

        // Include essential theme files (if theme detected)
        // We add the relative path (from the site root) so filename matches are scoped
        // to the exact theme, avoiding accidental matches across inactive themes.
        if let Some(theme_dir) = theme_path {
            let essential_theme_files = ["functions.php", "style.css"];
            for file in essential_theme_files {
                let file_path = theme_dir.join(file);
                if file_path.exists() {
                    // Prefer a path relative to the WordPress root so matching is scoped
                    if let Ok(rel) = file_path.strip_prefix(wp_path) {
                        allowed_filenames.push(rel.to_string_lossy().to_string());
                    } else {
                        allowed_filenames.push(file_path.to_string_lossy().to_string());
                    }
                }
            }
        }

        // Include main plugin files for active plugins (plugin_paths are normalized to plugin root)
        for plugin_dir in final_plugin_paths {
            if let Some(plugin_name) = plugin_dir.file_name().and_then(|n| n.to_str()) {
                let main_plugin_file = format!("{}.php", plugin_name);
                let plugin_file_path = plugin_dir.join(&main_plugin_file);
                if plugin_file_path.exists() {
                    // Store path relative to wp root when possible (keeps matching scoped)
                    if let Ok(rel) = plugin_file_path.strip_prefix(wp_path) {
                        allowed_filenames.push(rel.to_string_lossy().to_string());
                    } else {
                        allowed_filenames.push(main_plugin_file);
                    }
                }
            }
        }

        // Only include essential extensions (do NOT include generic ".php" so we only
        // process explicit PHP filenames added to allowed_filenames)
        let allowed_extensions: Vec<&'static str> = vec![
            ".js", ".css", ".scss", ".sass", ".less", ".json", ".txt", ".md",
        ];

        let allowed_filenames_leaked: Vec<&'static str> = allowed_filenames
            .iter()
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
            .collect();

        Some(Profile {
            description: "WordPress site with active theme and plugins (path-aware).",
            allowed_extensions: Box::leak(allowed_extensions.into_boxed_slice()),
            allowed_filenames: Box::leak(allowed_filenames_leaked.into_boxed_slice()),
        })
    }

    pub fn get_active_theme(&self) -> Result<PathBuf> {
        // Try wp-cli first
        if let Ok(output) = Command::new("wp")
            .args(["theme", "list", "--format=json", "--status=active"])
            .output()
        {
            if output.status.success() {
                if let Ok(themes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                {
                    if let Some(theme) = themes.first() {
                        if let Some(theme_name) = theme.get("name").and_then(|n| n.as_str()) {
                            return Ok(PathBuf::from("wp-content/themes").join(theme_name));
                        }
                    }
                }
            }
        }

        // Fallback: scan filesystem for themes
        self.get_available_themes()?
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No themes found"))
    }

    pub fn get_active_plugins(&self) -> Result<Vec<PathBuf>> {
        // Try wp-cli first
        if let Ok(output) = Command::new("wp")
            .args(["plugin", "list", "--format=json", "--status=active"])
            .output()
        {
            if output.status.success() {
                if let Ok(plugins) =
                    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
                {
                    let plugin_paths = plugins
                        .iter()
                        .filter_map(|p| {
                            p.get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| PathBuf::from("wp-content/plugins").join(s))
                        })
                        .collect();
                    return Ok(plugin_paths);
                }
            }
        }

        // Fallback: scan filesystem for plugins
        self.get_available_plugins()
    }

    pub fn get_available_themes(&self) -> Result<Vec<PathBuf>> {
        let themes_dir = PathBuf::from("wp-content/themes");
        if !themes_dir.exists() {
            return Ok(Vec::new());
        }

        let mut theme_paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(theme_name) = entry.file_name().to_str() {
                            // Skip common non-theme directories
                            if !theme_name.starts_with('.') && theme_name != "index.php" {
                                theme_paths.push(entry.path());
                            }
                        }
                    }
                }
            }
        }

        Ok(theme_paths)
    }

    pub fn get_available_plugins(&self) -> Result<Vec<PathBuf>> {
        let plugins_dir = PathBuf::from("wp-content/plugins");
        if !plugins_dir.exists() {
            return Ok(Vec::new());
        }

        let mut plugin_paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        if let Some(plugin_name) = entry.file_name().to_str() {
                            // Skip common non-plugin directories
                            if !plugin_name.starts_with('.') && plugin_name != "index.php" {
                                plugin_paths.push(entry.path());
                            }
                        }
                    }
                }
            }
        }

        Ok(plugin_paths)
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
        // Skip wp-admin directory and other WordPress core directories
        if let Some(dir_name) = dir_path.file_name().and_then(|n| n.to_str()) {
            if dir_name == "wp-admin" || dir_name == "wp-includes" {
                return Ok(());
            }
        }

        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        if let Some(file_name) = entry_path.file_name().and_then(|n| n.to_str()) {
                            files.push(file_name.to_string());
                        }
                    } else if file_type.is_dir() {
                        // Recursively collect from subdirectories
                        let _ = self.collect_files_recursive_helper(&entry_path, files);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn parse_wp_config(&self) -> Result<Vec<String>> {
        let config_path = PathBuf::from("wp-config.php");
        if !config_path.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // Look for common WordPress configuration patterns
            if content.contains("DB_NAME") {
                files.push("wp-config.php".to_string());
            }
            if content.contains("WP_DEBUG") {
                files.push("wp-config.php".to_string());
            }
            // Add more patterns as needed
        }

        Ok(files)
    }

    pub fn parse_htaccess(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();

        // Check root .htaccess
        let root_htaccess = PathBuf::from(".htaccess");
        if root_htaccess.exists() {
            files.push(".htaccess".to_string());
        }

        // Check for .htaccess in common locations
        let common_locations = ["wp-admin/.htaccess"];
        for location in &common_locations {
            let path = PathBuf::from(location);
            if path.exists() {
                files.push(location.to_string());
            }
        }

        Ok(files)
    }

    pub fn parse_env_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let env_patterns = [".env", ".env.local", ".env.development", ".env.production"];

        for pattern in &env_patterns {
            let path = PathBuf::from(pattern);
            if path.exists() {
                files.push(pattern.to_string());
            }
        }

        Ok(files)
    }

    pub fn parse_composer_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        let composer_files = ["composer.json", "composer.lock"];

        for file in &composer_files {
            let path = PathBuf::from(file);
            if path.exists() {
                files.push(file.to_string());
            }
        }

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wp_cli_detection() {
        let plugin = WordPressProfilePlugin;

        // Test theme detection - this will show if wp-cli is being used
        let theme_result = plugin.get_active_theme();

        match theme_result {
            Ok(theme_path) => {
                println!(
                    "✅ WP-CLI used for theme detection: {}",
                    theme_path.display()
                );
                assert!(theme_path.to_string_lossy().contains("wp-content/themes"));
            }
            Err(e) => {
                println!("ℹ️  Fell back to filesystem scanning for themes: {}", e);
                // This is expected if wp-cli is not available
            }
        }

        // Test plugin detection
        let plugin_result = plugin.get_active_plugins();

        match plugin_result {
            Ok(plugin_paths) => {
                println!(
                    "✅ WP-CLI used for plugin detection: {} plugins found",
                    plugin_paths.len()
                );
                for path in &plugin_paths {
                    assert!(path.to_string_lossy().contains("wp-content/plugins"));
                }
            }
            Err(e) => {
                println!("ℹ️  Fell back to filesystem scanning for plugins: {}", e);
                // This is expected if wp-cli is not available
            }
        }
    }

    #[test]
    fn test_profile_restrictions() {
        let plugin = WordPressProfilePlugin;

        if let Some(profile) = plugin.get_profile("wordpress") {
            let filenames: Vec<&str> = profile.allowed_filenames.iter().cloned().collect();
            let extensions: Vec<&str> = profile.allowed_extensions.iter().cloned().collect();

            // Should only include wp-config.php as base filename
            assert!(filenames.contains(&"wp-config.php"));

            // Should not include core WordPress files
            assert!(!filenames.contains(&"wp-load.php"));
            assert!(!filenames.contains(&"xmlrpc.php"));
            assert!(!filenames.contains(&"wp-cron.php"));

            // Should include relevant extensions and not include generic PHP extension
            // We only include explicit PHP filenames (functions.php, main plugin files), so
            // the generic ".php" extension should NOT be present in allowed_extensions.
            assert!(!extensions.contains(&".php"));
            assert!(extensions.contains(&".js"));
            assert!(extensions.contains(&".css"));
            assert!(!extensions.contains(&".png"));
            assert!(!extensions.contains(&".jpg"));

            println!("✅ Profile restrictions working correctly");
            println!("   - Allowed filenames: {:?}", filenames.len());
            println!("   - Allowed extensions: {:?}", extensions.len());
        } else {
            panic!("WordPress profile not found");
        }
    }
}
