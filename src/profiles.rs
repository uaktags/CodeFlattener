use crate::config::CustomProfile;
use crate::wordpress_profile::WordPressProfilePlugin;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use tracing::debug;

/// Core struct representing a fully resolved profile.
#[derive(Debug, Clone)]
pub struct Profile {
    pub description: String,
    pub allowed_extensions: Vec<String>,
    pub allowed_filenames: Vec<String>,
    pub include_globs: Vec<String>,
    pub markdown: Option<bool>,
}

impl Profile {
    pub fn new(
        description: String,
        allowed_extensions: Vec<String>,
        allowed_filenames: Vec<String>,
    ) -> Self {
        Self {
            description,
            allowed_extensions,
            allowed_filenames,
            include_globs: Vec::new(),
            markdown: None,
        }
    }

    /// Merges this profile (parent) with another profile (child).
    /// Child values take precedence or are additive where appropriate.
    pub fn merge_with(&self, child: &Profile) -> Profile {
        let mut merged_extensions = self.allowed_extensions.clone();
        let mut merged_filenames = self.allowed_filenames.clone();
        let mut merged_globs = self.include_globs.clone();

        for ext in &child.allowed_extensions {
            if !merged_extensions.contains(ext) {
                merged_extensions.push(ext.clone());
            }
        }

        for filename in &child.allowed_filenames {
            if !merged_filenames.contains(filename) {
                merged_filenames.push(filename.clone());
            }
        }

        for glob in &child.include_globs {
            if !merged_globs.contains(glob) {
                merged_globs.push(glob.clone());
            }
        }

        Profile {
            description: child.description.clone(),
            allowed_extensions: merged_extensions,
            allowed_filenames: merged_filenames,
            include_globs: merged_globs,
            markdown: child.markdown.or(self.markdown),
        }
    }
}

/// Trait for different sources of profiles (Built-ins, WordPress, Config).
pub trait ProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile>;
    fn list_profiles(&self) -> Vec<String>;
}

/// The Manager that holds all plugins and resolves logic.
pub struct ProfileManager {
    built_ins: HashMap<&'static str, Profile>,
    wordpress: WordPressProfilePlugin,
    custom_profiles: HashMap<String, CustomProfile>,
}

impl ProfileManager {
    pub fn new(custom_profiles: Option<HashMap<String, CustomProfile>>) -> Self {
        Self {
            built_ins: BUILT_IN_PROFILES.clone(),
            wordpress: WordPressProfilePlugin,
            custom_profiles: custom_profiles.unwrap_or_default(),
        }
    }

    /// Resolves a profile by name, handling inheritance (extends) from the config.
    pub fn resolve(&self, name: &str) -> Option<Profile> {
        // 1. Check if it is a custom profile defined in TOML
        if let Some(custom_def) = self.custom_profiles.get(name) {
            return self.resolve_custom(name, custom_def);
        }

        // 2. Check WordPress plugin
        if let Some(p) = self.wordpress.get_profile(name) {
            return Some(p);
        }

        // 3. Check Built-ins
        self.built_ins.get(name).cloned()
    }

    /// Lists all available profile keys from all sources.
    pub fn list_all(&self) -> Vec<(String, String)> {
        let mut list = Vec::new();

        // Built-ins
        for (name, p) in &self.built_ins {
            list.push((name.to_string(), p.description.clone()));
        }

        // WordPress
        for name in self.wordpress.list_profiles() {
            if let Some(p) = self.wordpress.get_profile(&name) {
                list.push((name, p.description));
            }
        }

        // Custom
        for (name, custom) in &self.custom_profiles {
            // If we haven't already added this name (overrides)
            if !list.iter().any(|(n, _)| n == name) {
                let desc = custom
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Custom profile extending {:?}", custom.extends));
                list.push((name.clone(), desc));
            }
        }

        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    fn resolve_custom(&self, name: &str, custom: &CustomProfile) -> Option<Profile> {
        // Create the "child" part of the profile
        let mut child = Profile::new(
            custom.description.clone().unwrap_or_else(|| name.to_string()),
            custom.extensions.clone().unwrap_or_default(),
            custom.allowed_filenames.clone().unwrap_or_default(),
        );
        child.include_globs = custom.include_globs.clone().unwrap_or_default();
        child.markdown = custom.markdown;

        // If it extends something, resolve the parent and merge
        if let Some(parent_name) = &custom.extends {
            debug!("Resolving parent '{}' for custom profile '{}'", parent_name, name);
            
            // Recursion guard: prevent simple loops (A -> A)
            if parent_name == name {
                tracing::warn!("Profile '{}' extends itself. Ignoring parent.", name);
                return Some(child);
            }

            // Recursive call to resolve() allows extending other custom profiles or built-ins
            if let Some(parent_profile) = self.resolve(parent_name) {
                return Some(parent_profile.merge_with(&child));
            } else {
                tracing::warn!("Parent profile '{}' not found for '{}'", parent_name, name);
            }
        }

        Some(child)
    }
    
    /// Specific helper for the WordPress path-aware resolution
    pub fn resolve_wordpress_path_aware(
        &self, 
        name: &str, 
        path: &std::path::Path,
        args: &crate::Args
    ) -> Option<Profile> {
         self.wordpress.get_profile_for_path(
            name,
            path,
            args.wp_exclude_plugins.as_deref(),
            args.wp_include_only_plugins.as_deref(),
            args.wp_include_theme.as_deref(),
        )
    }
}

// --- Built-in Data ---

static BUILT_IN_PROFILES: Lazy<HashMap<&'static str, Profile>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "nextjs-ts-prisma",
        Profile {
            description: "Next.js, TypeScript, Prisma project files.".to_string(),
            allowed_extensions: vec![
                ".ts".to_string(), ".tsx".to_string(), ".js".to_string(), ".jsx".to_string(),
                ".json".to_string(), ".css".to_string(), ".scss".to_string(), ".md".to_string(),
                ".env".to_string(), ".env.local".to_string(), ".prisma".to_string(),
            ],
            allowed_filenames: vec![
                "next.config.js".to_string(), "tailwind.config.js".to_string(),
                "postcss.config.js".to_string(), "middleware.ts".to_string(), "schema.prisma".to_string(),
            ],
            include_globs: Vec::new(),
            markdown: None,
        },
    );
    m.insert(
        "cpp-cmake",
        Profile {
            description: "C/C++ and CMake project files.".to_string(),
            allowed_extensions: vec![
                ".c".to_string(), ".cpp".to_string(), ".h".to_string(), ".hpp".to_string(),
                ".cmake".to_string(), ".txt".to_string(), ".md".to_string(),
            ],
            allowed_filenames: vec!["CMakeLists.txt".to_string()],
            include_globs: Vec::new(),
            markdown: None,
        },
    );
    m.insert(
        "rust",
        Profile {
            description: "Rust project files.".to_string(),
            allowed_extensions: vec![
                ".rs".to_string(), ".toml".to_string(), ".md".to_string(), ".yml".to_string(), ".json".to_string(),
            ],
            allowed_filenames: vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()],
            include_globs: Vec::new(),
            markdown: None,
        },
    );
    m
});