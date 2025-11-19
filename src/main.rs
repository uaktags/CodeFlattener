// src/main.rs
mod config;
mod profiles;
mod wordpress_profile;

use crate::config::ConfigFile;
use crate::profiles::ProfileManager;

use anyhow::{Context, Result};
use clap::Parser;
use glob::Pattern;
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tiktoken_rs::p50k_base;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "A blazingly fast code flattener, written in Rust.",
    long_about = "Flattens code files from directories, filters by extension, and counts tokens, with profile support."
)]
pub struct Args {
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

    let args_cli = Args::parse();

    // 1. Load Configuration
    let config = config::load_config(&args_cli.config)?;

    // 2. Initialize Profile Manager (loads built-ins + config profiles)
    let custom_profiles = config.as_ref().and_then(|c| c.profiles.clone());
    let profile_manager = ProfileManager::new(custom_profiles);

    // 3. Handle List Profiles
    if args_cli.list_profiles {
        println!("Available Profiles:");
        for (name, desc) in profile_manager.list_all() {
            println!("  - {}: {}", name, desc);
        }
        return Ok(());
    }

    // 4. Merge Config into Args
    let mut args = merge_config_with_args(args_cli, &config);
    validate_config(&args)?;

    // 5. Process Directories
    let result = process_directories(&mut args, &profile_manager)?;

    // 6. Output Results
    output_results(&result, &args)?;

    info!(
        "Processing complete: {} files, {} tokens",
        result.file_count, result.token_count
    );

    Ok(())
}

fn merge_config_with_args(mut args: Args, config: &Option<ConfigFile>) -> Args {
    if let Some(config) = config {
        if args.profile.is_none() {
            if let Some(profile) = &config.profile {
                args.profile = Some(profile.clone());
            }
        }

        if let Some(exts) = &config.extensions {
            // Only override if not provided via CLI
            if args.extensions.is_none() {
                args.extensions = Some(exts.clone());
            }
        }
        
        if let Some(filenames) = &config.allowed_filenames {
             if args.allowed_filenames.is_none() {
                args.allowed_filenames = Some(filenames.clone());
            }
        }

        if args.include_globs.is_none() {
            args.include_globs = config.include_globs.clone();
        }
        if args.exclude_globs.is_none() {
            args.exclude_globs = config.exclude_globs.clone();
        }
        
        if args.markdown == 0 {
            if let Some(markdown) = config.markdown {
                args.markdown = if markdown { 1 } else { 0 };
            }
        }
        
        // Merge boolean flags if CLI flag is false (default)
        if !args.exclude_node_modules && config.exclude_node_modules.unwrap_or(false) {
            args.exclude_node_modules = true;
        }
        if !args.exclude_build_dirs && config.exclude_build_dirs.unwrap_or(false) {
            args.exclude_build_dirs = true;
        }
        if !args.exclude_hidden_dirs && config.exclude_hidden_dirs.unwrap_or(false) {
            args.exclude_hidden_dirs = true;
        }
        if !args.include_git_changes && config.include_git_changes.unwrap_or(false) {
            args.include_git_changes = true;
        }
    }
    args
}

fn validate_config(args: &Args) -> Result<()> {
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

    if args.max_size > 100.0 {
        return Err(anyhow::anyhow!("Max file size cannot exceed 100MB"));
    }

    Ok(())
}

fn process_directories(args: &mut Args, profile_manager: &ProfileManager) -> Result<ProcessingResult> {
    // Apply Profile Settings
    if let Some(profile_name) = &args.profile {
        let profile = if profile_name == "wordpress" {
             // Special handling for WordPress to enable path-aware resolution
             let default_path = PathBuf::from(".");
             let path = args.target_dirs.first().unwrap_or(&default_path);
             profile_manager.resolve_wordpress_path_aware(profile_name, path, args)
        } else {
             profile_manager.resolve(profile_name)
        };

        if let Some(p) = profile {
            if args.verbose {
                info!("Applied profile: {}", p.description);
            }
            // Merge profile settings into args if args are empty
            if args.extensions.is_none() {
                args.extensions = Some(p.allowed_extensions);
            }
            if args.allowed_filenames.is_none() {
                args.allowed_filenames = Some(p.allowed_filenames);
            }
            // Append globs from profile to any existing args
            if !p.include_globs.is_empty() {
                 let mut current_globs = args.include_globs.clone().unwrap_or_default();
                 for g in p.include_globs {
                     if !current_globs.contains(&g) {
                         current_globs.push(g);
                     }
                 }
                 args.include_globs = Some(current_globs);
            }
            
            if args.markdown == 0 {
                if let Some(markdown) = p.markdown {
                    args.markdown = if markdown { 1 } else { 0 };
                }
            }

            // Merge additional profile settings
            if args.max_size == 0.0 {
                if let Some(max_size) = p.max_size {
                    args.max_size = max_size;
                }
            }
            if !args.gpt4_tokens {
                if let Some(gpt4_tokens) = p.gpt4_tokens {
                    args.gpt4_tokens = gpt4_tokens;
                }
            }
            if !args.include_git_changes {
                if let Some(include_git_changes) = p.include_git_changes {
                    args.include_git_changes = include_git_changes;
                }
            }
            if !args.no_staged_diff {
                if let Some(no_staged_diff) = p.no_staged_diff {
                    args.no_staged_diff = no_staged_diff;
                }
            }
            if !args.no_unstaged_diff {
                if let Some(no_unstaged_diff) = p.no_unstaged_diff {
                    args.no_unstaged_diff = no_unstaged_diff;
                }
            }
            if args.include_dirs.is_none() {
                args.include_dirs = p.include_dirs.clone();
            }
            if args.exclude_dirs.is_none() {
                args.exclude_dirs = p.exclude_dirs.clone();
            }
            if args.exclude_patterns.is_none() {
                args.exclude_patterns = p.exclude_patterns.clone();
            }
            if args.include_patterns.is_none() {
                args.include_patterns = p.include_patterns.clone();
            }
            if args.exclude_globs.is_none() {
                args.exclude_globs = p.exclude_globs.clone();
            }
            if !args.exclude_node_modules {
                if let Some(exclude_node_modules) = p.exclude_node_modules {
                    args.exclude_node_modules = exclude_node_modules;
                }
            }
            if !args.exclude_build_dirs {
                if let Some(exclude_build_dirs) = p.exclude_build_dirs {
                    args.exclude_build_dirs = exclude_build_dirs;
                }
            }
            if !args.exclude_hidden_dirs {
                if let Some(exclude_hidden_dirs) = p.exclude_hidden_dirs {
                    args.exclude_hidden_dirs = exclude_hidden_dirs;
                }
            }
            if args.max_depth == 0 {
                if let Some(max_depth) = p.max_depth {
                    args.max_depth = max_depth;
                }
            }
        } else {
            warn!("Profile '{}' not found. Using provided arguments only.", profile_name);
        }
    }

    info!(
        "Settings - extensions: {:?}, filenames: {:?}, include_globs: {:?}, max_size: {}MB",
        args.extensions, args.allowed_filenames, args.include_globs, args.max_size
    );

    // Prepare lookup sets
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

    info!("Starting processing...");

    for start_dir in &args.target_dirs {
        let start_dir = fs::canonicalize(start_dir)
            .with_context(|| format!("Failed to canonicalize path: {}", start_dir.display()))?;

        if !is_safe_path(&start_dir, &start_dir) {
             return Err(anyhow::anyhow!("Path traversal detected: {}", start_dir.display()));
        }

        let walker = build_walker(&start_dir, args);
        let entries: Vec<_> = walker.build().filter_map(Result::ok).collect();

        if args.parallel {
            entries.par_iter().for_each(|entry| {
                let path = entry.path();
                if should_process_path(path, args, &start_dir) {
                    if let Err(e) = process_single_file(
                        path,
                        &extensions,
                        &allowed_filenames,
                        max_file_size,
                        args,
                        &all_contents,
                        &file_count,
                    ) {
                        warn!("Failed to process {}: {}", path.display(), e);
                    }
                }
            });
        } else {
            for entry in entries {
                let path = entry.path();
                if should_process_path(path, args, &start_dir) {
                    process_single_file(
                        path,
                        &extensions,
                        &allowed_filenames,
                        max_file_size,
                        args,
                        &all_contents,
                        &file_count,
                    )?;
                }
            }
        }
    }

    // Final content assembly
    let content = if args.dry_run {
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
        walker.filter_entry(|entry| entry.file_name() != "node_modules");
    }

    if args.exclude_build_dirs {
        walker.filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(name.as_ref(), "target" | "build" | "dist")
        });
    }

    if args.exclude_hidden_dirs {
        walker.filter_entry(|entry| {
            !entry.file_name().to_string_lossy().starts_with('.')
        });
    }

    // Always filter specific WP dirs to avoid massive dumps unless explicitly crawled
    walker.filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        name != "wp-admin" && name != "wp-includes"
    });

    walker
}

fn should_process_path(path: &Path, args: &Args, base_dir: &Path) -> bool {
    if path.is_dir() { return false; }

    let relative_path = match path.strip_prefix(base_dir) {
        Ok(p) => p,
        Err(_) => path,
    };

    if is_ignored_by_file(path, base_dir) { return false; }

    // Directory Exclusions
    if let Some(exclude_dirs) = &args.exclude_dirs {
        for exclude_dir in exclude_dirs {
            if relative_path.starts_with(exclude_dir) { return false; }
        }
    }

    // Directory Inclusions (Exclusive)
    if let Some(include_dirs) = &args.include_dirs {
        let mut included = false;
        for include_dir in include_dirs {
            if relative_path.starts_with(include_dir) {
                included = true;
                break;
            }
        }
        if !included { return false; }
    }

    // Exclude Globs
    if let Some(exclude_globs) = &args.exclude_globs {
        for pattern in exclude_globs {
            // Check matches against OS path and forward-slash normalized path
            if match_glob(pattern, relative_path) { return false; }
        }
    }

    // Include Globs
    if let Some(include_globs) = &args.include_globs {
        let mut matches = false;
        for pattern in include_globs {
             if match_glob(pattern, relative_path) {
                matches = true;
                break;
            }
        }
        if !matches {
            return false;
        }
    }

    // WordPress-specific Exclusions
    if let Some(excludes) = &args.wp_exclude_plugins {
        if let Ok(rel) = path.strip_prefix(base_dir) {
            let rel_str = rel.to_string_lossy().to_lowercase();
            for raw in excludes {
                let slug = raw.split('/').next().unwrap_or(raw).to_lowercase();
                let plugin_prefix = format!("wp-content/plugins/{}", slug);
                if rel_str.starts_with(&plugin_prefix) { return false; }
            }
        }
    }

    if is_binary_file(path) { return false; }

    // WordPress Inclusion Logic (Strict Mode)
    if args.profile.as_deref() == Some("wordpress") 
       && (args.wp_include_only_plugins.is_some() || args.wp_include_theme.is_some()) {
        if let Ok(rel) = path.strip_prefix(base_dir) {
            let rel_str_lower = rel.to_string_lossy().to_lowercase();
            if rel_str_lower == "wp-config.php" { return true; }
            
            if let Some(includes) = &args.wp_include_only_plugins {
                for raw in includes {
                    let slug = raw.split('/').next().unwrap_or(raw).to_lowercase();
                    let prefix = format!("wp-content/plugins/{}", slug);
                    if rel_str_lower.starts_with(&prefix) { return true; }
                }
            }
            
            if let Some(theme) = &args.wp_include_theme {
                 let prefix = format!("wp-content/themes/{}", theme.to_lowercase());
                 if rel_str_lower.starts_with(&prefix) { return true; }
            }
            return false; // Strict mode active and no match
        }
    }

    // Core WP File Exclusion
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        let core_wp_files = [
            "xmlrpc.php", "wp-activate.php", "wp-cron.php", "wp-load.php",
            "wp-blog-header.php", "wp-settings.php", "wp-login.php", "wp-signup.php",
            "wp-trackback.php", "wp-comments-post.php", "wp-links-opml.php", "wp-mail.php",
        ];
        if core_wp_files.contains(&file_name) { return false; }
    }

    true
}

fn match_glob(pattern: &str, path: &Path) -> bool {
    let pat_os = pattern.replace('/', &std::path::MAIN_SEPARATOR.to_string());
    if let Ok(glob) = Pattern::new(&pat_os) {
        if glob.matches_path(path) { return true; }
    }
    // Fallback for Windows: match against forward-slash string
    let rel_forward = path.to_string_lossy().replace('\\', "/");
    if let Ok(glob) = Pattern::new(pattern) {
        if glob.matches_path(Path::new(&rel_forward)) { return true; }
    }
    false
}

fn is_ignored_by_file(path: &Path, base_dir: &Path) -> bool {
    let patterns = load_ignore_patterns();
    let relative_path = match path.strip_prefix(base_dir) {
        Ok(p) => p,
        Err(_) => path,
    };
    patterns.iter().any(|p| p.matches_path(relative_path))
}

fn load_ignore_patterns() -> Vec<Pattern> {
    let mut patterns = Vec::new();
    if let Ok(content) = fs::read_to_string(".flattenerignore") {
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                if let Ok(p) = Pattern::new(line) { patterns.push(p); }
            }
        }
    }
    patterns
}

fn is_binary_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        let binary_extensions = [
            "png", "jpg", "jpeg", "gif", "ico", "webp", "svg", "bmp", "tiff", "tif", "mp4", "avi",
            "mov", "wmv", "flv", "webm", "mkv", "mp3", "wav", "ogg", "zip", "tar", "gz", "bz2",
            "7z", "rar", "pdf", "doc", "docx", "xls", "xlsx", "exe", "dll", "so", "dylib", "woff",
            "woff2", "ttf", "eot",
        ];
        if binary_extensions.contains(&ext_str.as_str()) { return true; }
    }
    // Byte check
    if let Ok(mut file) = fs::File::open(path) {
        let mut buffer = [0u8; 1024];
        if let Ok(n) = file.read(&mut buffer) {
            for &byte in &buffer[..n] {
                if byte == 0 || (byte < 32 && byte != 9 && byte != 10 && byte != 13) {
                    return true;
                }
            }
        }
    }
    false
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

    // Logic: Allowed if extension matches OR filename matches OR include_globs matches.
    // However, include_globs matching happens in `should_process_path` (mostly). 
    // To support profiles that *only* have include_globs (no extensions), we must be permissive here 
    // if include_globs are present.
    
    let is_allowed_ext = extensions.contains(&format!(".{}", extension));
    let is_allowed_file = allowed_filenames.contains(file_name.as_ref());
    let is_allowed_by_glob = args.include_globs.is_some();

    if !is_allowed_ext && !is_allowed_file && !is_allowed_by_glob {
        return Ok(());
    }

    if args.dry_run {
        info!("DRY-RUN: would process {}", path.display());
        let mut c = file_count.lock().unwrap();
        *c += 1;
        return Ok(());
    }

    let metadata = fs::metadata(path)
        .with_context(|| format!("Failed to get metadata for {}", path.display()))?;

    if metadata.len() > max_file_size {
        if args.verbose { info!("Skipping large file: {}", path.display()); }
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

    let mut ac = all_contents.lock().unwrap();
    ac.push_str(&formatted_content);
    
    let mut c = file_count.lock().unwrap();
    *c += 1;

    if args.verbose { info!("Processed: {}", path.display()); }
    Ok(())
}

fn output_results(result: &ProcessingResult, args: &Args) -> Result<()> {
    if let Some(output_path) = &args.output {
        if let Some(parent) = output_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        let file = fs::File::create(output_path)?;
        let mut writer = BufWriter::new(file);
        write!(writer, "{}", result.content)?;
        info!("Flattened code written to: {}", output_path.display());
    } else {
        let mut stdout = io::stdout().lock();
        write!(stdout, "{}", result.content)?;
    }
    Ok(())
}

fn find_git_root(start_path: &Path) -> Result<Option<PathBuf>> {
    let mut current_path = fs::canonicalize(start_path)?;
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
    repo_path: &Path,
    include_staged: bool,
    include_unstaged: bool,
    verbose: bool,
) -> Result<Option<String>> {
    let mut output = String::new();
    output.push_str("\n\n# --- Git Changes ---\n");
    output.push_str(&format!("# Repository: {}\n\n", repo_path.display()));

    let status_out = Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(repo_path)
        .output()?;

    if status_out.status.success() {
        let s = String::from_utf8_lossy(&status_out.stdout);
        if !s.trim().is_empty() {
            output.push_str("## Git Status:\n```bash\n");
            output.push_str(s.trim());
            output.push_str("\n```\n\n");
        }
    } else if verbose {
        warn!("git status failed");
    }

    if include_staged {
        let diff = Command::new("git")
            .args(["diff", "--staged"])
            .current_dir(repo_path)
            .output()?;
        if diff.status.success() {
             let s = String::from_utf8_lossy(&diff.stdout);
             if !s.trim().is_empty() {
                 output.push_str("## Git Diff (Staged):\n```diff\n");
                 output.push_str(s.trim());
                 output.push_str("\n```\n\n");
             }
        }
    }
    
    if include_unstaged {
        let diff = Command::new("git")
            .args(["diff"])
            .current_dir(repo_path)
            .output()?;
        if diff.status.success() {
             let s = String::from_utf8_lossy(&diff.stdout);
             if !s.trim().is_empty() {
                 output.push_str("## Git Diff (Unstaged):\n```diff\n");
                 output.push_str(s.trim());
                 output.push_str("\n```\n\n");
             }
        }
    }

    Ok(Some(output))
}

fn is_safe_path(path: &Path, base_dir: &Path) -> bool {
    if path.strip_prefix(base_dir).is_ok() { return true; }
    let base_abs = base_dir.canonicalize().unwrap_or_else(|_| base_dir.to_path_buf());
    if path.starts_with(&base_abs) { return true; }
    if !path.is_absolute() {
        let candidate = base_abs.join(path);
        return candidate.starts_with(&base_abs);
    }
    false
}