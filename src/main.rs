// /src/main.rs

mod wordpress_profile;
use wordpress_profile::WordPressProfilePlugin;

use anyhow::{Context, Result};
use clap::{Parser};
use glob::Pattern;
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tiktoken_rs::p50k_base;
use tracing::{Level, debug, info, warn};
use tracing_subscriber::FmtSubscriber;

// Configuration file structure
#[derive(Debug, Deserialize)]
struct ConfigFile {
    profile: Option<String>,
    extensions: Option<Vec<String>>,
    allowed_filenames: Option<Vec<String>>,
    max_size: Option<f64>,
    markdown: Option<bool>,
    gpt4_tokens: Option<bool>,
    include_git_changes: Option<bool>,
    no_staged_diff: Option<bool>,
    no_unstaged_diff: Option<bool>,
    include_dirs: Option<Vec<PathBuf>>,
    exclude_dirs: Option<Vec<PathBuf>>,
    exclude_patterns: Option<Vec<String>>,
    include_patterns: Option<Vec<String>>,
    exclude_globs: Option<Vec<String>>,
    include_globs: Option<Vec<String>>,
    exclude_node_modules: Option<bool>,
    exclude_build_dirs: Option<bool>,
    exclude_hidden_dirs: Option<bool>,
    max_depth: Option<usize>,

    // Custom profiles section
    profiles: Option<std::collections::HashMap<String, CustomProfile>>,
}

#[derive(Debug, Deserialize, Clone)]
struct CustomProfile {
    description: Option<String>,
    #[serde(alias = "profile")]
    extends: Option<String>,
    extensions: Option<Vec<String>>,
    allowed_filenames: Option<Vec<String>>,
    include_globs: Option<Vec<String>>,
    markdown: Option<bool>,
}

// Plugin trait for custom profiles
trait ProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile>;
    fn list_profiles(&self) -> Vec<String>;
}

// Default profile plugin using built-in profiles
struct DefaultProfilePlugin;
impl ProfilePlugin for DefaultProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile> {
        PROFILES.get(name).cloned()
    }

    fn list_profiles(&self) -> Vec<String> {
        PROFILES.keys().map(|s| s.to_string()).collect()
    }
}

// Composite plugin that consults WordPress plugin first, then defaults
struct CompositeProfilePlugin {
    default: DefaultProfilePlugin,
    wordpress: WordPressProfilePlugin,
}

impl CompositeProfilePlugin {
    fn new() -> Self {
        CompositeProfilePlugin {
            default: DefaultProfilePlugin,
            wordpress: WordPressProfilePlugin,
        }
    }
}

impl ProfilePlugin for CompositeProfilePlugin {
    fn get_profile(&self, name: &str) -> Option<Profile> {
        // Prefer wordpress plugin when asked, otherwise fall back to defaults
        self.wordpress
            .get_profile(name)
            .or_else(|| self.default.get_profile(name))
    }

    fn list_profiles(&self) -> Vec<String> {
        let mut profiles = self.default.list_profiles();
        for p in self.wordpress.list_profiles() {
            if !profiles.contains(&p) {
                profiles.push(p);
            }
        }
        profiles.sort();
        profiles
    }
}

// Lazily initialized static map of predefined profiles
static PROFILES: Lazy<HashMap<&'static str, Profile>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "nextjs-ts-prisma",
        Profile {
            description: "Next.js, TypeScript, Prisma project files.".to_string(),
            allowed_extensions: vec![
                ".ts".to_string(),
                ".tsx".to_string(),
                ".js".to_string(),
                ".jsx".to_string(),
                ".json".to_string(),
                ".css".to_string(),
                ".scss".to_string(),
                ".sass".to_string(),
                ".less".to_string(),
                ".html".to_string(),
                ".htm".to_string(),
                ".md".to_string(),
                ".mdx".to_string(),
                ".graphql".to_string(),
                ".gql".to_string(),
                ".env".to_string(),
                ".env.local".to_string(),
                ".env.development".to_string(),
                ".env.production".to_string(),
                ".yml".to_string(),
                ".yaml".to_string(),
                ".xml".to_string(),
                ".toml".to_string(),
                ".ini".to_string(),
                ".vue".to_string(),
                ".svelte".to_string(),
                ".prisma".to_string(),
            ],
            allowed_filenames: vec![
                "next.config.js".to_string(),
                "tailwind.config.js".to_string(),
                "postcss.config.js".to_string(),
                "middleware.ts".to_string(),
                "middleware.js".to_string(),
                "schema.prisma".to_string(),
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
                ".c".to_string(), ".cpp".to_string(), ".cc".to_string(), ".cxx".to_string(), ".h".to_string(), ".hpp".to_string(), ".hh".to_string(), ".ino".to_string(), ".cmake".to_string(), ".txt".to_string(), ".md".to_string(),
                ".json".to_string(), ".xml".to_string(), ".yml".to_string(), ".yaml".to_string(), ".ini".to_string(), ".proto".to_string(), ".fbs".to_string(),
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
                ".rs".to_string(), ".toml".to_string(), ".md".to_string(), ".yml".to_string(), ".yaml".to_string(), ".sh".to_string(), ".json".to_string(), ".html".to_string(),
            ],
            allowed_filenames: vec!["Cargo.toml".to_string(), "Cargo.lock".to_string(), "build.rs".to_string(), ".rustfmt.toml".to_string()],
            include_globs: Vec::new(),
            markdown: None,
        },
    );
    m
});

#[derive(Debug, Clone)]
struct Profile {
    description: String,
    allowed_extensions: Vec<String>,
    allowed_filenames: Vec<String>,
    include_globs: Vec<String>,
    markdown: Option<bool>,
}

impl Profile {
    fn new(description: String, allowed_extensions: Vec<String>, allowed_filenames: Vec<String>) -> Self {
        Self {
            description,
            allowed_extensions,
            allowed_filenames,
            include_globs: Vec::new(),
            markdown: None,
        }
    }

    fn merge_with(&self, other: &Profile) -> Profile {
        let mut merged_extensions = self.allowed_extensions.clone();
        let mut merged_filenames = self.allowed_filenames.clone();

        for ext in &other.allowed_extensions {
            if !merged_extensions.contains(ext) {
                merged_extensions.push(ext.clone());
            }
        }

        for filename in &other.allowed_filenames {
            if !merged_filenames.contains(filename) {
                merged_filenames.push(filename.clone());
            }
        }

        let mut merged_include_globs = self.include_globs.clone();
        for glob in &other.include_globs {
            if !merged_include_globs.contains(glob) {
                merged_include_globs.push(glob.clone());
            }
        }

        Profile {
            description: other.description.clone(), // Use the child's description
            allowed_extensions: merged_extensions,
            allowed_filenames: merged_filenames,
            include_globs: merged_include_globs,
            markdown: other.markdown.or(self.markdown),
        }
    }
}
 
fn resolve_custom_profile(
    name: &str,
    custom_profiles: &HashMap<String, CustomProfile>,
    plugin: &dyn ProfilePlugin,
) -> Result<Profile> {
    info!("Resolving custom profile '{}'", name);
    // Lookup custom profile by name
    let custom = custom_profiles
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Custom profile '{}' not found", name))?;
 
    // Resolve parent if present (either built-in or another custom)
    let parent_profile: Option<Profile> = if let Some(parent_name) = &custom.extends {
        info!("Custom profile '{}' extends '{}'", name, parent_name);
        if custom_profiles.contains_key(parent_name) {
            Some(resolve_custom_profile(parent_name, custom_profiles, plugin)?)
        } else if let Some(built) = plugin.get_profile(parent_name.as_str()) {
            Some(built)
        } else {
            return Err(anyhow::anyhow!("Cannot resolve parent profile '{}'", parent_name));
        }
    } else {
        None
    };
 
    // Build child profile from custom definition
    let mut child = Profile::new(
        custom
            .description
            .clone()
            .unwrap_or_else(|| name.to_string()),
        custom.extensions.clone().unwrap_or_default(),
        custom.allowed_filenames.clone().unwrap_or_default(),
    );

    child.include_globs = custom.include_globs.clone().unwrap_or_default();
    child.markdown = custom.markdown;
 
    // Merge parent (if any) with child, giving child's values precedence where applicable
    Ok(match parent_profile {
        Some(p) => {
            info!("Merging custom profile '{}' with parent profile", name);
            p.merge_with(&child)
        }
        None => child,
    })
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "A blazingly fast code flattener, written in Rust.",
    long_about = "Flattens code files from directories, filters by extension, and counts tokens, with profile support."
)]
struct Args {
    /// One or more directories to scan. Defaults to current directory.
    #[arg(default_value = ".")]
    target_dirs: Vec<PathBuf>,

    /// Output file path for the flattened code. If not specified, prints to console.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Use a predefined profile for a specific project type.
    #[arg(short, long)]
    profile: Option<String>,

    /// List all available profiles and their descriptions.
    #[arg(long)]
    list_profiles: bool,

    /// Comma-separated list of allowed file extensions (overrides profile).
    #[arg(short, long, value_delimiter = ',', use_value_delimiter = true)]
    extensions: Option<Vec<String>>,

    /// Space-separated list of specific filenames to include (overrides profile).
    #[arg(short, long, value_delimiter = ' ')]
    allowed_filenames: Option<Vec<String>>,

    /// Maximum file size to process in megabytes (MB).
    #[arg(long, default_value_t = 2.0)]
    max_size: f64,

    /// Format the output content using Markdown code blocks.
    #[arg(long, action = clap::ArgAction::Count)]
    markdown: u8,

    /// Use GPT-4 tokenizer for more accurate token counting.
    #[arg(long)]
    gpt4_tokens: bool,

    /// Append a section with current Git status and diffs.
    #[arg(short = 'g', long = "include-git-changes")]
    include_git_changes: bool,

    /// Do NOT include staged changes (git diff --staged).
    #[arg(long, requires = "include_git_changes")]
    no_staged_diff: bool,

    /// Do NOT include unstaged changes (git diff).
    #[arg(long, requires = "include_git_changes")]
    no_unstaged_diff: bool,

    /// Print verbose output during processing.
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Comma-separated list of directories to include (relative to target)
    #[arg(long, value_delimiter = ',')]
    include_dirs: Option<Vec<PathBuf>>,

    /// Comma-separated list of directories to exclude (relative to target)
    #[arg(long, value_delimiter = ',')]
    exclude_dirs: Option<Vec<PathBuf>>,

    /// Exclude node_modules directories (common in JS projects)
    #[arg(long)]
    exclude_node_modules: bool,

    /// Exclude target/ and build/ directories (common in compiled projects)
    #[arg(long)]
    exclude_build_dirs: bool,

    /// Exclude hidden directories (starting with .)
    #[arg(long)]
    exclude_hidden_dirs: bool,

    /// Maximum directory depth to traverse
    #[arg(long, default_value_t = 100)]
    max_depth: usize,

    /// Comma-separated list of patterns to exclude
    #[arg(long, value_delimiter = ',')]
    exclude_patterns: Option<Vec<String>>,

    /// Comma-separated list of patterns to include
    #[arg(long, value_delimiter = ',')]
    include_patterns: Option<Vec<String>>,

    /// Comma-separated list of glob patterns to exclude
    #[arg(long, value_delimiter = ',')]
    exclude_globs: Option<Vec<String>>,

    /// Comma-separated list of glob patterns to include
    #[arg(long, value_delimiter = ',')]
    include_globs: Option<Vec<String>>,

    /// Enable parallel processing
    #[arg(long)]
    parallel: bool,

    /// Show progress bar
    #[arg(long)]
    progress: bool,

    /// Dry run: log which files would be processed but don't read them
    #[arg(long)]
    dry_run: bool,

    /// WordPress-profile-specific: comma-separated list of plugin slugs to exclude (e.g. woocommerce,elementor-pro)
    #[arg(long, value_delimiter = ',', use_value_delimiter = true)]
    wp_exclude_plugins: Option<Vec<String>>,

    /// WordPress-profile-specific: comma-separated list of plugin slugs to exclusively include
    #[arg(long, value_delimiter = ',', use_value_delimiter = true)]
    wp_include_only_plugins: Option<Vec<String>>,

    /// WordPress-profile-specific: theme to include
    #[arg(long)]
    wp_include_theme: Option<String>,
}


#[derive(Debug)]
struct ProcessingResult {
    content: String,
    file_count: usize,
    token_count: usize,
}

fn main() -> Result<()> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let args = Args::parse();

    if args.list_profiles {
        list_profiles();
        return Ok(());
    }

    // Load configuration from file if specified
    let config = load_config(&args.config)?;
    let mut args = merge_config_with_args(args, &config);

    // Validate configuration
    validate_config(&args)?;

    let result = process_directories(&mut args, &config)?;

    // Output results
    output_results(&result, &args)?;

    info!(
        "Processing complete: {} files, {} tokens",
        result.file_count, result.token_count
    );

    Ok(())
}

fn load_config(config_path: &Option<PathBuf>) -> Result<Option<ConfigFile>> {
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
    Ok(None)
}

fn merge_config_with_args(mut args: Args, config: &Option<ConfigFile>) -> Args {
    if let Some(config) = config {
        if args.profile.is_none() {
            if let Some(profile) = &config.profile {
                // Preserve the raw profile name for dynamic/custom profiles
                args.profile = Some(profile.clone());
            }
        }

        // Merge other config options (simplified for brevity)
        if let Some(exts) = &config.extensions {
            args.extensions = Some(exts.clone());
        }
        if args.include_globs.is_none() {
            args.include_globs = config.include_globs.clone();
        }
        if args.markdown == 0 {
            if let Some(markdown) = config.markdown {
                args.markdown = if markdown { 1 } else { 0 };
            }
        }
    }
    args
}

fn validate_config(args: &Args) -> Result<()> {
    // Validate directory conflicts
    if let (Some(include_dirs), Some(exclude_dirs)) = (&args.include_dirs, &args.exclude_dirs) {
        for include_dir in include_dirs {
            for exclude_dir in exclude_dirs {
                if exclude_dir.starts_with(include_dir) {
                    return Err(anyhow::anyhow!(
                        "Conflict: exclude directory '{}' is within include directory '{}'",
                        exclude_dir.display(),
                        include_dir.display()
                    ));
                }
            }
        }
    }

    // Validate max size
    if args.max_size > 100.0 {
        return Err(anyhow::anyhow!("Max file size cannot exceed 100MB"));
    }

    Ok(())
}
fn process_directories(args: &mut Args, config: &Option<ConfigFile>) -> Result<ProcessingResult> {
    apply_profile_settings(args, config)?;
 
    // Debug log the effective profile-derived settings so we can diagnose missing files (helps on Windows where globs
    // may use forward slashes).
    info!(
        "Effective profile settings - extensions: {:?}, allowed_filenames: {:?}, include_globs: {:?}, max_size: {}MB",
        args.extensions, args.allowed_filenames, args.include_globs, args.max_size
    );
 
    let mut extensions: HashSet<String> = HashSet::new();
    if let Some(exts) = &args.extensions {
        extensions = exts
            .iter()
            .map(|e| if e.starts_with('.') { e.clone() } else { format!(".{}", e) })
            .collect();
    }
    let mut allowed_filenames: HashSet<String> = HashSet::new();
    if let Some(files) = &args.allowed_filenames {
        allowed_filenames = files.iter().cloned().collect();
    }

    if extensions.is_empty() && allowed_filenames.is_empty() && args.include_globs.is_none() {
        return Err(anyhow::anyhow!(
            "No allowed extensions, filenames, or include globs specified"
        ));
    }

    let max_file_size = (args.max_size * 1024.0 * 1024.0) as u64;
    let all_contents = Arc::new(Mutex::new(String::new()));
    let file_count = Arc::new(Mutex::new(0));

    info!("Starting code flattening");
    debug!("Target directories: {:?}", args.target_dirs);
    debug!("Allowed extensions: {:?}", extensions);
    debug!("Max file size: {:.2} MB", args.max_size);

    for start_dir in &args.target_dirs {
        let start_dir = fs::canonicalize(start_dir)
            .with_context(|| format!("Failed to canonicalize path: {}", start_dir.display()))?;

        if !is_safe_path(&start_dir, &start_dir) {
            return Err(anyhow::anyhow!(
                "Path traversal detected: {}",
                start_dir.display()
            ));
        }

        let walker = build_walker(&start_dir, args);
        let entries: Vec<_> = walker.build().filter_map(Result::ok).collect();

        if args.parallel {
            process_entries_parallel(
                &entries,
                &start_dir,
                &extensions,
                &allowed_filenames,
                max_file_size,
                args,
                &all_contents,
                &file_count,
            )?;
        } else {
            process_entries_sequential(
                &entries,
                &start_dir,
                &extensions,
                &allowed_filenames,
                max_file_size,
                args,
                &all_contents,
                &file_count,
            )?;
        }
    }

    // If dry-run, do not fetch git diffs or assemble file contents.
    // We still rely on process_single_file to have incremented file_count.
    let content = if args.dry_run {
        // Do not collect git changes or file contents in dry-run.
        String::new()
    } else {
        let mut git_output = String::new();
        if args.include_git_changes {
            if let Ok(Some(root)) =
                find_git_root(args.target_dirs.first().unwrap_or(&PathBuf::from(".")))
            {
                if let Ok(Some(output)) = get_git_changes(
                    &root,
                    !args.no_staged_diff,
                    !args.no_unstaged_diff,
                    args.verbose,
                ) {
                    git_output = output;
                }
            }
        }

        let mut content = all_contents.lock().unwrap().clone();
        content.push_str(&git_output);
        content
    };

    let token_count = if args.gpt4_tokens {
        p50k_base()
            .unwrap()
            .encode_with_special_tokens(&content)
            .len()
    } else {
        content.split_whitespace().count()
    };

    Ok(ProcessingResult {
        content,
        file_count: *file_count.lock().unwrap(),
        token_count,
    })
}

fn build_walker(start_dir: &Path, args: &Args) -> WalkBuilder {
    let mut walker = WalkBuilder::new(start_dir);

    walker.max_depth(Some(args.max_depth));

    if args.exclude_node_modules {
        walker.filter_entry(|entry| {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name != "node_modules"
        });
    }

    if args.exclude_build_dirs {
        walker.filter_entry(|entry| {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            !matches!(name.as_ref(), "target" | "build" | "dist")
        });
    }

    if args.exclude_hidden_dirs {
        walker.filter_entry(|entry| {
            let path = entry.path();
            !path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with('.')
        });
    }

    // Add WordPress-specific exclusions
    walker.filter_entry(|entry| {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        name != "wp-admin" && name != "wp-includes"
    });

    walker
}

fn should_process_path(path: &Path, args: &Args, base_dir: &Path) -> bool {
    if path.is_dir() {
        return false;
    }

    let relative_path = match path.strip_prefix(base_dir) {
        Ok(p) => p,
        Err(_) => path,
    };

    // Check .flattenerignore
    if is_ignored_by_file(path, base_dir) {
        return false;
    }

    // Check directory inclusions/exclusions
    if let Some(exclude_dirs) = &args.exclude_dirs {
        for exclude_dir in exclude_dirs {
            if relative_path.starts_with(exclude_dir) {
                return false;
            }
        }
    }

    if let Some(include_dirs) = &args.include_dirs {
        let mut included = false;
        for include_dir in include_dirs {
            if relative_path.starts_with(include_dir) {
                included = true;
                break;
            }
        }
        if !included {
            return false;
        }
    }

    // Check glob patterns
    if let Some(exclude_globs) = &args.exclude_globs {
        for pattern in exclude_globs {
            // Normalize pattern for the host OS (allow toml patterns like "src/*" to work on Windows).
            let pat_os = pattern.replace('/', &std::path::MAIN_SEPARATOR.to_string());
            if let Ok(glob_pattern) = Pattern::new(&pat_os) {
                if glob_pattern.matches_path(relative_path) {
                    return false;
                }
                // Also try matching against a forward-slash-normalized path string as a fallback.
                let rel_forward = relative_path.to_string_lossy().replace('\\', "/");
                if glob_pattern.matches_path(std::path::Path::new(&rel_forward)) {
                    return false;
                }
            } else if let Ok(glob_pattern) = Pattern::new(pattern) {
                if glob_pattern.matches_path(relative_path) {
                    return false;
                }
            }
        }
    }

    if let Some(include_globs) = &args.include_globs {
        let mut included = false;
        for pattern in include_globs {
            // Normalize pattern to account for Windows path separators in TOML authored globs.
            let pat_os = pattern.replace('/', &std::path::MAIN_SEPARATOR.to_string());
            if let Ok(glob_pattern) = Pattern::new(&pat_os) {
                if glob_pattern.matches_path(relative_path) {
                    included = true;
                    break;
                }
                // Fallback: try matching against a forward-slash-normalized string path
                let rel_forward = relative_path.to_string_lossy().replace('\\', "/");
                if glob_pattern.matches_path(std::path::Path::new(&rel_forward)) {
                    included = true;
                    break;
                }
            } else if let Ok(glob_pattern) = Pattern::new(pattern) {
                if glob_pattern.matches_path(relative_path) {
                    included = true;
                    break;
                }
            }
        }
        if !included {
            return false;
        }
    }

    // WordPress-profile specific: exclude plugins specified via --wp-exclude-plugins
    if let Some(excludes) = &args.wp_exclude_plugins {
        if let Ok(rel) = path.strip_prefix(base_dir) {
            // Normalize the relative path to lowercase for slug matching
            let rel_str = rel.to_string_lossy().to_lowercase();
            for raw in excludes {
                // Normalize exclude entries (allow values like "woocommerce/packages/..." or "Woocommerce")
                let slug = raw.split('/').next().unwrap_or(raw).to_lowercase();
                let plugin_prefix = format!("wp-content/plugins/{}", slug);
                if rel_str.starts_with(&plugin_prefix) {
                    if args.verbose {
                        info!("Excluding plugin '{}' path: {}", raw, rel.display());
                    }
                    return false;
                }
            }
        }
    }

    // Check if file is binary (skip binary files)
    if is_binary_file(path) {
        return false;
    }

    // WordPress-profile specific: if --wp-include-only-plugins or --wp-include-theme is used,
    // we should ONLY include those directories and wp-config.php.
    if args.profile.as_deref() == Some("wordpress")
        && (args.wp_include_only_plugins.is_some() || args.wp_include_theme.is_some())
    {
        if let Ok(rel) = path.strip_prefix(base_dir) {
            let rel_str_lower = rel.to_string_lossy().to_lowercase();

            // Always allow wp-config.php
            if rel_str_lower == "wp-config.php" {
                return true;
            }

            // Check if path is inside one of the included plugins
            if let Some(includes) = &args.wp_include_only_plugins {
                for raw in includes {
                    let slug = raw.split('/').next().unwrap_or(raw).to_lowercase();
                    let plugin_prefix = format!("wp-content/plugins/{}", slug);
                    if rel_str_lower.starts_with(&plugin_prefix) {
                        return true;
                    }
                }
            }

            // Check if path is inside the included theme
            if let Some(theme_name) = &args.wp_include_theme {
                let theme_prefix = format!("wp-content/themes/{}", theme_name.to_lowercase());
                if rel_str_lower.starts_with(&theme_prefix) {
                    return true;
                }
            }

            // If we are here, the path is not in any of the allowed include lists, so deny it.
            return false;
        } else {
            // If we can't get a relative path, deny it to be safe.
            return false;
        }
    }

    // For WordPress profile, exclude common core WordPress files
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        let core_wp_files = [
            "xmlrpc.php",
            "wp-activate.php",
            "wp-cron.php",
            "wp-load.php",
            "wp-blog-header.php",
            "wp-settings.php",
            "wp-login.php",
            "wp-signup.php",
            "wp-trackback.php",
            "wp-comments-post.php",
            "wp-links-opml.php",
            "wp-mail.php",
        ];

        if core_wp_files.contains(&file_name) {
            return false;
        }
    }

    true
}

fn is_ignored_by_file(path: &Path, base_dir: &Path) -> bool {
    let ignore_patterns = load_ignore_patterns();
    let relative_path = match path.strip_prefix(base_dir) {
        Ok(p) => p,
        Err(_) => path,
    };

    ignore_patterns
        .iter()
        .any(|pattern| pattern.matches_path(relative_path))
}

fn load_ignore_patterns() -> Vec<Pattern> {
    let mut patterns = Vec::new();
    if let Ok(content) = fs::read_to_string(".flattenerignore") {
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                if let Ok(pattern) = Pattern::new(line) {
                    patterns.push(pattern);
                }
            }
        }
    }
    patterns
}

fn is_binary_file(path: &Path) -> bool {
    // Check file extension for known binary files
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "png", "jpg", "jpeg", "gif", "ico", "webp", "svg", "bmp", "tiff", "tif", "mp4", "avi",
            "mov", "wmv", "flv", "webm", "mkv", "mp3", "wav", "ogg", "zip", "tar", "gz", "bz2",
            "7z", "rar", "pdf", "doc", "docx", "xls", "xlsx", "exe", "dll", "so", "dylib", "woff",
            "woff2", "ttf", "eot",
        ];

        if binary_extensions.contains(&ext_str.as_str()) {
            return true;
        }
    }

    // For files without extensions or unknown extensions, try to detect by reading first 1024 bytes
    if let Ok(mut file) = fs::File::open(path) {
        let mut buffer = [0u8; 1024];
        if let Ok(bytes_read) = file.read(&mut buffer) {
            // Check for null bytes or non-printable characters
            for &byte in &buffer[..bytes_read] {
                if byte == 0 || (byte < 32 && byte != 9 && byte != 10 && byte != 13) {
                    return true;
                }
            }
        }
    }

    false
}

fn process_entries_parallel(
    entries: &[ignore::DirEntry],
    start_dir: &Path,
    extensions: &HashSet<String>,
    allowed_filenames: &HashSet<String>,
    max_file_size: u64,
    args: &Args,
    all_contents: &Arc<Mutex<String>>,
    file_count: &Arc<Mutex<usize>>,
) -> Result<()> {
    entries.par_iter().for_each(|entry| {
        let path = entry.path();

        if !should_process_path(path, args, start_dir) {
            return;
        }

        if let Err(e) = process_single_file(
            path,
            extensions,
            allowed_filenames,
            max_file_size,
            args,
            all_contents,
            file_count,
        ) {
            warn!("Failed to process {}: {}", path.display(), e);
        }
    });
    Ok(())
}

fn process_entries_sequential(
    entries: &[ignore::DirEntry],
    start_dir: &Path,
    extensions: &HashSet<String>,
    allowed_filenames: &HashSet<String>,
    max_file_size: u64,
    args: &Args,
    all_contents: &Arc<Mutex<String>>,
    file_count: &Arc<Mutex<usize>>,
) -> Result<()> {
    for entry in entries {
        let path = entry.path();

        if !should_process_path(path, args, start_dir) {
            continue;
        }

        process_single_file(
            path,
            extensions,
            allowed_filenames,
            max_file_size,
            args,
            all_contents,
            file_count,
        )?;
    }
    Ok(())
}

fn process_single_file(
    path: &Path,
    extensions: &HashSet<String>,
    allowed_filenames: &HashSet<String>,
    max_file_size: u64,
    args: &Args,
    all_contents: &Arc<Mutex<String>>,
    file_count: &Arc<Mutex<usize>>,
) -> Result<()> {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();

    let is_allowed_ext = extensions.contains(&format!(".{}", extension));
    let is_allowed_file = allowed_filenames.contains(file_name.as_ref());

    if !is_allowed_ext && !is_allowed_file {
        return Ok(());
    }

    // For dry-run we want to avoid any file reads. Log a single "DRY-RUN" line and
    // increment the counter. Do not attempt metadata or content access.
    if args.dry_run {
        info!("DRY-RUN: would process {}", path.display());
        let mut count = file_count.lock().unwrap();
        *count += 1;
        return Ok(());
    }

    let metadata = fs::metadata(path)
        .with_context(|| format!("Failed to get metadata for {}", path.display()))?;

    if metadata.len() > max_file_size {
        if args.verbose {
            info!("Skipping large file: {}", path.display());
        }
        return Ok(());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file {}", path.display()))?;

    let file_path_str = path.to_string_lossy();
    let mut formatted_content = if args.markdown > 0 {
        format!("\n\n```{}\n# --- File: {} ---\n", extension, file_path_str)
    } else {
        format!("\n\n# --- File: {} ---\n\n", file_path_str)
    };

    formatted_content.push_str(&content);

    if args.markdown > 0 {
        formatted_content.push_str("\n```\n");
    }

    let mut all_contents = all_contents.lock().unwrap();
    all_contents.push_str(&formatted_content);

    let mut count = file_count.lock().unwrap();
    *count += 1;

    if args.verbose {
        info!("Processed: {}", path.display());
    }

    Ok(())
}

fn output_results(result: &ProcessingResult, args: &Args) -> Result<()> {
    if let Some(output_path) = &args.output {
        if let Some(parent) = output_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create output directory: {}", parent.display())
                })?;
            }
        }

        let file = fs::File::create(output_path)
            .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;
        let mut writer = BufWriter::new(file);
        write!(writer, "{}", result.content).with_context(|| {
            format!("Failed to write to output file: {}", output_path.display())
        })?;

        info!("Flattened code written to: {}", output_path.display());
    } else {
        let mut stdout = io::stdout().lock();
        write!(stdout, "{}", result.content)?;
    }

    info!("Total files processed: {}", result.file_count);
    info!("Approximate token count: {}", result.token_count);

    Ok(())
}

fn apply_profile_settings(args: &mut Args, config: &Option<ConfigFile>) -> Result<()> {
    if let Some(profile_name) = &args.profile.clone() {
        let plugin: Box<dyn ProfilePlugin> = Box::new(CompositeProfilePlugin::new());
        let custom_profiles = config
            .as_ref()
            .and_then(|c| c.profiles.as_ref())
            .cloned()
            .unwrap_or_default();

        let profile = if custom_profiles.contains_key(profile_name) {
            if args.verbose {
                info!("Using custom profile '{}'", profile_name);
            }
            Some(resolve_custom_profile(
                profile_name,
                &custom_profiles,
                plugin.as_ref(),
            )?)
        } else {
            plugin.get_profile(profile_name)
        };

        if let Some(p) = profile {
            if args.extensions.is_none() {
                args.extensions = Some(p.allowed_extensions);
            }
            if args.allowed_filenames.is_none() {
                args.allowed_filenames = Some(p.allowed_filenames);
            }
            // Only apply include_globs from a profile when the profile actually provides globs.
            // An empty Vec means "no globs" and should not override absence of include_globs on args,
            // because an empty Some(Vec::new()) would block all files later when checked.
            if args.include_globs.is_none() && !p.include_globs.is_empty() {
                args.include_globs = Some(p.include_globs);
            }
            if args.markdown == 0 {
                if let Some(markdown) = p.markdown {
                    args.markdown = if markdown { 1 } else { 0 };
                }
            }
        }
    }
    Ok(())
}
fn list_profiles() {
    let plugin: Box<dyn ProfilePlugin> = Box::new(CompositeProfilePlugin::new());
    println!("Available Profiles:");
    for name in plugin.list_profiles() {
        if let Some(profile) = plugin.get_profile(&name) {
            println!("  - {}: {}", name, profile.description);
            println!("    Extensions: {}", profile.allowed_extensions.join(", "));
            if !profile.allowed_filenames.is_empty() {
                println!(
                    "    Allowed Filenames: {}",
                    profile.allowed_filenames.join(", ")
                );
            }
            println!();
        }
    }
}

fn find_git_root(start_path: &Path) -> Result<Option<PathBuf>> {
    let mut current_path = fs::canonicalize(start_path)
        .with_context(|| format!("Failed to find canonical path for {}", start_path.display()))?;

    loop {
        if current_path.join(".git").is_dir() {
            return Ok(Some(current_path));
        }
        if !current_path.pop() {
            return Ok(None);
        }
    }
}

fn get_git_changes(
    git_repo_path: &Path,
    include_staged: bool,
    include_unstaged: bool,
    verbose: bool,
) -> Result<Option<String>> {
    let mut output = String::new();
    output.push_str("\n\n# --- Git Changes ---\n");
    output.push_str(&format!("# Repository: {}\n\n", git_repo_path.display()));

    let status_output = Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(git_repo_path)
        .output()
        .context("Failed to execute 'git status'")?;

    if status_output.status.success() {
        let stdout = String::from_utf8_lossy(&status_output.stdout);
        if !stdout.trim().is_empty() {
            output.push_str("## Git Status:\n```bash\n");
            output.push_str(stdout.trim());
            output.push_str("\n```\n\n");
        } else {
            output.push_str("## Git Status: No uncommitted changes.\n\n");
        }
    } else if verbose {
        warn!(
            "'git status' failed: {}",
            String::from_utf8_lossy(&status_output.stderr)
        );
    }

    if include_staged {
        let diff_output = Command::new("git")
            .args(["diff", "--staged"])
            .current_dir(git_repo_path)
            .output()
            .context("Failed to execute 'git diff --staged'")?;

        if diff_output.status.success() {
            let stdout = String::from_utf8_lossy(&diff_output.stdout);
            if !stdout.trim().is_empty() {
                output.push_str("## Git Diff (Staged):\n```diff\n");
                output.push_str(stdout.trim());
                output.push_str("\n```\n\n");
            }
        } else if verbose {
            warn!(
                "'git diff --staged' failed: {}",
                String::from_utf8_lossy(&diff_output.stderr)
            );
        }
    }

    if include_unstaged {
        let diff_output = Command::new("git")
            .args(["diff"])
            .current_dir(git_repo_path)
            .output()
            .context("Failed to execute 'git diff'")?;

        if diff_output.status.success() {
            let stdout = String::from_utf8_lossy(&diff_output.stdout);
            if !stdout.trim().is_empty() {
                output.push_str("## Git Diff (Unstaged):\n```diff\n");
                output.push_str(stdout.trim());
                output.push_str("\n```\n\n");
            }
        } else if verbose {
            warn!(
                "'git diff' failed: {}",
                String::from_utf8_lossy(&diff_output.stderr)
            );
        }
    }

    Ok(Some(output))
}

fn is_safe_path(path: &Path, base_dir: &Path) -> bool {
    path.canonicalize()
        .ok()
        .map(|p| p.starts_with(base_dir))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_is_safe_path() {
        let base_path = env::temp_dir();
        // create a unique subdir for test
        let temp_dir = base_path.join("codeflattener_test_tmp");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let base_path = temp_dir.as_path();
        let safe_path = base_path.join("subdir/file.txt");
        assert!(is_safe_path(&safe_path, base_path));

        // This should be safe even if it doesn't exist yet
        let non_existent = base_path.join("nonexistent");
        assert!(is_safe_path(&non_existent, base_path));
    }

    #[test]
    fn test_should_process_path() {
        let base = env::temp_dir();
        let temp_dir = base.join("codeflattener_test_tmp_2");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let base_path = temp_dir.as_path();
        let file_path = base_path.join("src/main.rs");

        fs::create_dir_all(base_path.join("src")).unwrap();
        fs::write(&file_path, "fn main() {}").unwrap();

        // Create a minimal Args instance for testing
        let args = Args {
            target_dirs: vec![base_path.to_path_buf()],
            output: None,
            profile: None,
            list_profiles: false,
            extensions: None,
            allowed_filenames: None,
            max_size: 2.0,
            markdown: 0,
            gpt4_tokens: false,
            include_git_changes: false,
            no_staged_diff: false,
            no_unstaged_diff: false,
            verbose: false,
            config: None,
            include_dirs: None,
            exclude_dirs: None,
            exclude_node_modules: false,
            exclude_build_dirs: false,
            exclude_hidden_dirs: false,
            max_depth: 100,
            exclude_patterns: None,
            include_patterns: None,
            exclude_globs: None,
            include_globs: None,
            parallel: false,
            progress: false,
            dry_run: false,
            wp_exclude_plugins: None,
            wp_include_only_plugins: None,
            wp_include_theme: None,
        };

        assert!(should_process_path(&file_path, &args, base_path));
    }
}