// src/main.rs

mod wordpress_profile;
use wordpress_profile::WordPressProfilePlugin;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use glob::Pattern;
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Write};
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

// Lazily initialized static map of predefined profiles
static PROFILES: Lazy<HashMap<&'static str, Profile>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "nextjs-ts-prisma",
        Profile {
            description: "Next.js, TypeScript, Prisma project files.",
            allowed_extensions: &[
                ".ts",
                ".tsx",
                ".js",
                ".jsx",
                ".json",
                ".css",
                ".scss",
                ".sass",
                ".less",
                ".html",
                ".htm",
                ".md",
                ".mdx",
                ".graphql",
                ".gql",
                ".env",
                ".env.local",
                ".env.development",
                ".env.production",
                ".yml",
                ".yaml",
                ".xml",
                ".toml",
                ".ini",
                ".vue",
                ".svelte",
                ".prisma",
            ],
            allowed_filenames: &[
                "next.config.js",
                "tailwind.config.js",
                "postcss.config.js",
                "middleware.ts",
                "middleware.js",
                "schema.prisma",
            ],
        },
    );
    m.insert(
        "cpp-cmake",
        Profile {
            description: "C/C++ and CMake project files.",
            allowed_extensions: &[
                ".c", ".cpp", ".cc", ".cxx", ".h", ".hpp", ".hh", ".ino", ".cmake", ".txt", ".md",
                ".json", ".xml", ".yml", ".yaml", ".ini", ".proto", ".fbs",
            ],
            allowed_filenames: &["CMakeLists.txt"],
        },
    );
    m.insert(
        "rust",
        Profile {
            description: "Rust project files.",
            allowed_extensions: &[
                ".rs", ".toml", ".md", ".yml", ".yaml", ".sh", ".json", ".html",
            ],
            allowed_filenames: &["Cargo.toml", "Cargo.lock", "build.rs", ".rustfmt.toml"],
        },
    );
    m
});

#[derive(Debug, Clone)]
struct Profile {
    description: &'static str,
    allowed_extensions: &'static [&'static str],
    allowed_filenames: &'static [&'static str],
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
    #[arg(short, long, value_enum)]
    profile: Option<ProfileChoice>,

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
    #[arg(long)]
    markdown: bool,

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
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ProfileChoice {
    #[clap(name = "nextjs-ts-prisma")]
    NextjsTsPrisma,
    #[clap(name = "cpp-cmake")]
    CppCmake,
    #[clap(name = "rust")]
    Rust,
    #[clap(name = "wordpress")]
    WordPress,
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
    let args = merge_config_with_args(args, config);

    // Validate configuration
    validate_config(&args)?;

    let result = process_directories(&args)?;

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

fn merge_config_with_args(mut args: Args, config: Option<ConfigFile>) -> Args {
    if let Some(config) = config {
        if args.profile.is_none() {
            if let Some(profile) = config.profile {
                args.profile = match profile.as_str() {
                    "nextjs-ts-prisma" => Some(ProfileChoice::NextjsTsPrisma),
                    "cpp-cmake" => Some(ProfileChoice::CppCmake),
                    "rust" => Some(ProfileChoice::Rust),
                    _ => None,
                };
            }
        }

        // Merge other config options (simplified for brevity)
        if let Some(exts) = config.extensions {
            args.extensions = Some(exts);
        }
        // ... merge other config fields similarly
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

fn process_directories(args: &Args) -> Result<ProcessingResult> {
    let mut extensions: HashSet<String> = HashSet::new();
    let mut allowed_filenames: HashSet<String> = HashSet::new();

    // Load profile settings
    load_profile_settings(args, &mut extensions, &mut allowed_filenames)?;

    if extensions.is_empty() && allowed_filenames.is_empty() {
        return Err(anyhow::anyhow!(
            "No allowed extensions or filenames specified"
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

    let content = {
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
            if let Ok(glob_pattern) = Pattern::new(pattern) {
                if glob_pattern.matches_path(relative_path) {
                    return false;
                }
            }
        }
    }

    if let Some(include_globs) = &args.include_globs {
        let mut included = false;
        for pattern in include_globs {
            if let Ok(glob_pattern) = Pattern::new(pattern) {
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
    let mut formatted_content = if args.markdown {
        format!("\n\n```{}\n# --- File: {} ---\n", extension, file_path_str)
    } else {
        format!("\n\n# --- File: {} ---\n\n", file_path_str)
    };

    formatted_content.push_str(&content);

    if args.markdown {
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

fn load_profile_settings(
    args: &Args,
    extensions: &mut HashSet<String>,
    allowed_filenames: &mut HashSet<String>,
) -> Result<()> {
    let plugin: Box<dyn ProfilePlugin> = Box::new(DefaultProfilePlugin);

    if let Some(profile_choice) = &args.profile {
        let profile_key = match profile_choice {
            ProfileChoice::NextjsTsPrisma => "nextjs-ts-prisma",
            ProfileChoice::CppCmake => "cpp-cmake",
            ProfileChoice::Rust => "rust",
            ProfileChoice::WordPress => "wordpress",
        };

        if let Some(profile) = plugin.get_profile(profile_key) {
            if args.verbose {
                info!("Using '{}' profile.", profile_key);
            }
            extensions.extend(profile.allowed_extensions.iter().map(|s| s.to_string()));
            allowed_filenames.extend(profile.allowed_filenames.iter().map(|s| s.to_string()));
        }
    }

    if let Some(exts) = &args.extensions {
        *extensions = exts
            .iter()
            .map(|e| {
                if e.starts_with('.') {
                    e.clone()
                } else {
                    format!(".{}", e)
                }
            })
            .collect();
        if args.verbose {
            info!("Extensions overridden: {:?}", extensions);
        }
    }

    if let Some(files) = &args.allowed_filenames {
        *allowed_filenames = files.iter().cloned().collect();
        if args.verbose {
            info!("Allowed filenames overridden: {:?}", allowed_filenames);
        }
    }

    Ok(())
}

fn list_profiles() {
    let plugin: Box<dyn ProfilePlugin> = Box::new(DefaultProfilePlugin);
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
    use tempfile::tempdir;

    #[test]
    fn test_is_safe_path() {
        let temp_dir = tempdir().unwrap();
        let base_path = temp_dir.path();
        let safe_path = base_path.join("subdir/file.txt");
        assert!(is_safe_path(&safe_path, base_path));

        // This should be safe even if it doesn't exist yet
        let non_existent = base_path.join("nonexistent");
        assert!(is_safe_path(&non_existent, base_path));
    }

    #[test]
    fn test_should_process_path() {
        let temp_dir = tempdir().unwrap();
        let base_path = temp_dir.path();
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
            markdown: false,
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
        };

        assert!(should_process_path(&file_path, &args, base_path));
    }
}
