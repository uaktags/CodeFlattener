// src/main.rs

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tiktoken_rs::p50k_base;

// --- Profile Definitions ---
// Using once_cell::sync::Lazy for safe, one-time static initialization.
// This is the Rust equivalent of defining a global constant dictionary in Python.
static PROFILES: Lazy<HashMap<&'static str, Profile>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "nextjs-ts-prisma",
        Profile {
            description: "Next.js, TypeScript, Prisma project files.",
            allowed_extensions: &[
                ".ts", ".tsx", ".js", ".jsx", ".json", ".css", ".scss", ".sass", ".less",
                ".html", ".htm", ".md", ".mdx", ".graphql", ".gql", ".env", ".env.local",
                ".env.development", ".env.production", ".yml", ".yaml", ".xml", ".toml",
                ".ini", ".vue", ".svelte", ".prisma",
            ],
            allowed_filenames: &[
                "next.config.js", "tailwind.config.js", "postcss.config.js",
                "middleware.ts", "middleware.js", "schema.prisma",
            ],
        },
    );
    m.insert(
        "cpp-cmake",
        Profile {
            description: "C/C++ and CMake project files.",
            allowed_extensions: &[
                ".c", ".cpp", ".cc", ".cxx", ".h", ".hpp", ".hh", ".ino", ".cmake",
                ".txt", ".md", ".json", ".xml", ".yml", ".yaml", ".ini", ".proto", ".fbs",
            ],
            allowed_filenames: &["CMakeLists.txt"],
        },
    );
    // Add other profiles here following the same pattern...
    m
});

#[derive(Debug, Clone)]
struct Profile {
    description: &'static str,
    allowed_extensions: &'static [&'static str],
    allowed_filenames: &'static [&'static str],
}

// --- CLI Argument Parsing using `clap` ---
// `clap` with the `derive` feature lets us define our CLI arguments as a struct.
// It's strongly typed and automatically generates help messages.
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

    /// Comma-separated or space-separated list of allowed file extensions.
    #[arg(short, long, value_delimiter = ',', use_value_delimiter = true)]
    extensions: Option<Vec<String>>,

    /// Space-separated list of specific filenames to include regardless of extension.
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
}

// An enum for clap to create a choice of profiles from our HashMap keys.
#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
enum ProfileChoice {
    #[clap(name = "nextjs-ts-prisma")]
    NextjsTsPrisma,
    #[clap(name = "cpp-cmake")]
    CppCmake,
    // Add other profile names here
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.list_profiles {
        list_profiles();
        return Ok(());
    }

    // --- Resolve Configuration (Profile + Overrides) ---
    let mut extensions: HashSet<String> = HashSet::new();
    let mut allowed_filenames: HashSet<String> = HashSet::new();

    if let Some(profile_choice) = &args.profile {
        let profile_key = match profile_choice {
            ProfileChoice::NextjsTsPrisma => "nextjs-ts-prisma",
            ProfileChoice::CppCmake => "cpp-cmake",
        };
        if let Some(profile) = PROFILES.get(profile_key) {
            if args.verbose {
                println!("Using '{}' profile.", profile_key);
            }
            extensions.extend(profile.allowed_extensions.iter().map(|s| s.to_string()));
            allowed_filenames.extend(profile.allowed_filenames.iter().map(|s| s.to_string()));
        }
    }

    // Apply command-line overrides
    if let Some(exts) = args.extensions {
        extensions = exts
            .into_iter()
            .map(|e| if e.starts_with('.') { e } else { format!(".{}", e) })
            .collect();
        if args.verbose {
            println!("Extensions overridden by command line: {:?}", extensions);
        }
    }
    if let Some(files) = args.allowed_filenames {
        allowed_filenames = files.into_iter().collect();
        if args.verbose {
            println!("Allowed filenames overridden by command line: {:?}", allowed_filenames);
        }
    }

    if extensions.is_empty() && allowed_filenames.is_empty() {
        anyhow::bail!("Error: No allowed extensions or filenames specified. Use a profile or --extensions.");
    }

    // --- Start Processing ---
    let mut all_contents = String::new();
    let mut file_count = 0;
    let max_file_size = (args.max_size * 1024.0 * 1024.0) as u64;

    println!("--- Starting Code Flattening ---");
    println!("Target Directories: {:?}", args.target_dirs);
    println!("Allowed Extensions: {:?}", extensions);
    if !allowed_filenames.is_empty() {
        println!("Allowed Filenames: {:?}", allowed_filenames);
    }
    println!("Max File Size: {:.2} MB", args.max_size);

    for start_dir in &args.target_dirs {
        // `ignore` crate respects .gitignore, .ignore, etc. by default.
        // It's much more efficient and practical than `os.walk`.
        let walker = WalkBuilder::new(start_dir).build();
        for result in walker {
            let entry = match result {
                Ok(entry) => entry,
                Err(e) => {
                    eprintln!("Warning: Failed to process a directory entry: {}", e);
                    continue;
                }
            };
            
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            let extension = path.extension().unwrap_or_default().to_string_lossy();

            let is_allowed_ext = extensions.contains(&format!(".{}", extension));
            let is_allowed_file = allowed_filenames.contains(file_name.as_ref());

            if is_allowed_ext || is_allowed_file {
                match fs::metadata(path) {
                    Ok(metadata) => {
                        if metadata.len() > max_file_size {
                            if args.verbose {
                                println!(
                                    "Skipping large file ({:.2} KB): {}",
                                    metadata.len() as f64 / 1024.0,
                                    path.display()
                                );
                            }
                            continue;
                        }

                        match fs::read_to_string(path) {
                            Ok(content) => {
                                let file_path_str = path.to_string_lossy();
                                if args.markdown {
                                    all_contents.push_str(&format!(
                                        "\n\n```{}\n# --- File: {} ---\n",
                                        extension, file_path_str
                                    ));
                                    all_contents.push_str(&content);
                                    all_contents.push_str("\n```\n");
                                } else {
                                    all_contents.push_str(&format!(
                                        "\n\n# --- File: {} ---\n\n",
                                        file_path_str
                                    ));
                                    all_contents.push_str(&content);
                                }
                                file_count += 1;
                                if args.verbose {
                                    println!("Processed: {}", path.display());
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "Warning: Could not read file {}: {} (maybe not UTF-8?)",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Could not get metadata for {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    // --- Append Git Changes ---
    if args.include_git_changes {
        let git_root = find_git_root(args.target_dirs.get(0).unwrap_or(&PathBuf::from(".")))?;
        if let Some(root) = git_root {
            let git_output = get_git_changes(
                &root,
                !args.no_staged_diff,
                !args.no_unstaged_diff,
                args.verbose,
            )?;
            if let Some(output) = git_output {
                all_contents.push_str(&output);
            }
        } else if args.verbose {
            eprintln!("Warning: Git repository not found for any target directory.");
        }
    }


    // --- Tokenization and Summary ---
    let token_count = if args.gpt4_tokens {
        let bpe = p50k_base().unwrap(); // Using a common tokenizer, change if needed
        bpe.encode_with_special_tokens(&all_contents).len()
    } else {
        all_contents.split_whitespace().count() // Simple whitespace split
    };

    // --- Output ---
    if let Some(output_path) = args.output {
        let parent = output_path.parent().unwrap();
        if !parent.exists() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
        }
        fs::write(&output_path, &all_contents)
            .with_context(|| format!("Failed to write to output file: {}", output_path.display()))?;
        println!("\nFlattened code written to: {}", output_path.display());
    } else {
        // Using io::stdout().lock() is more performant for large outputs.
        let mut stdout = io::stdout().lock();
        write!(stdout, "{}", all_contents)?;
    }
    
    // Final summary printed to stderr to not interfere with stdout piping
    eprintln!("\n--- Processing Complete ---");
    eprintln!("Total files processed: {}", file_count);
    eprintln!("Approximate token count: {}", token_count);

    Ok(())
}

fn list_profiles() {
    println!("Available Profiles:");
    for (name, profile) in PROFILES.iter() {
        println!("  - {}: {}", name, profile.description);
        println!("    Extensions: {}", profile.allowed_extensions.join(", "));
        if !profile.allowed_filenames.is_empty() {
             println!("    Allowed Filenames: {}", profile.allowed_filenames.join(", "));
        }
        println!();
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

    // Git Status
    let status_output = Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(git_repo_path)
        .output()
        .context("Failed to execute 'git status'")?;

    if status_output.status.success() {
        let stdout = String::from_utf8_lossy(&status_output.stdout);
        if !stdout.trim().is_empty() {
            output.push_str("## Git Status (Staged, Unstaged, Untracked):\n```bash\n");
            output.push_str(stdout.trim());
            output.push_str("\n```\n\n");
        } else {
            output.push_str("## Git Status: No uncommitted changes.\n\n");
        }
    } else if verbose {
        eprintln!("'git status' failed: {}", String::from_utf8_lossy(&status_output.stderr));
    }
    
    // Git Diff (Staged)
    if include_staged {
        let diff_output = Command::new("git")
            .args(["diff", "--staged"])
            .current_dir(git_repo_path)
            .output().context("Failed to execute 'git diff --staged'")?;

        if diff_output.status.success() {
             let stdout = String::from_utf8_lossy(&diff_output.stdout);
             if !stdout.trim().is_empty() {
                 output.push_str("## Git Diff (Staged Changes):\n```diff\n");
                 output.push_str(stdout.trim());
                 output.push_str("\n```\n\n");
             }
        } else if verbose {
             eprintln!("'git diff --staged' failed: {}", String::from_utf8_lossy(&diff_output.stderr));
        }
    }

    // Git Diff (Unstaged)
    if include_unstaged {
        let diff_output = Command::new("git")
            .args(["diff"])
            .current_dir(git_repo_path)
            .output().context("Failed to execute 'git diff'")?;

        if diff_output.status.success() {
             let stdout = String::from_utf8_lossy(&diff_output.stdout);
             if !stdout.trim().is_empty() {
                 output.push_str("## Git Diff (Unstaged Changes):\n```diff\n");
                 output.push_str(stdout.trim());
                 output.push_str("\n```\n\n");
             }
        } else if verbose {
             eprintln!("'git diff' failed: {}", String::from_utf8_lossy(&diff_output.stderr));
        }
    }
    
    Ok(Some(output))
}
