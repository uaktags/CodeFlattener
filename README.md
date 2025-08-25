# CodeFlattener

A small, blazingly-fast Rust CLI that "flattens" a codebase into a single, uploadable text file tailored for AI code analysis (for example, uploading to AIStudio or other code-review/analysis tools).

This README gives a short overview, build instructions, usage examples, and configuration notes so you can quickly integrate CodeFlattener into your workflow.

## Key features

- Scan one or more target directories and collect source files.
- Filter files by extension, filename, glob patterns, or predefined "profiles" (examples: `rust`, `nextjs-ts-prisma`, `cpp-cmake`).
- Optional path-aware WordPress profile that can use wp-cli to discover active theme/plugins.
- Optional inclusion of current Git status and diffs.
- Token counting support (tiktoken tokenizer) with optional GPT-4 token mode.
- Parallel processing using Rayon for speed.

## Build

Requires Rust toolchain (stable). From the repository root:

```powershell
cargo build --release
```

Or to run without a release build during development:

```powershell
cargo run -- <args>
```

You can also install locally using:

```powershell
cargo install --path .
```

## Basic usage

Run `code-flattener` (or `cargo run --`) with a target directory (defaults to `.`):

```powershell
cargo run -- --profile rust -o flattened.txt .
```

Common useful flags:

- `--profile <name>` — use a predefined profile (`rust`, `nextjs-ts-prisma`, `cpp-cmake`, `wordpress` via plugin).
- `--output, -o <file>` — write flattened output to a file.
- `--markdown` — wrap file contents in Markdown code blocks for nicer display in viewers.
- `--include-git-changes, -g` — append git status and diffs to the output.
- `--extensions` — comma-separated list of extensions to allow (overrides profile).
- `--allowed-filenames` — space-separated list of specific filenames to include (overrides profile).
- `--max-size` — maximum file size in megabytes to process (default ~2 MB).

For full CLI help, run:

```powershell
cargo run -- --help
```

## Configuration file

CodeFlattener respects a TOML configuration file (default path: `.flattener.toml`). Example:

```toml
profile = "nextjs-ts-prisma"
extensions = [".ts", ".tsx", ".js", ".json", ".css", ".md"]
allowed_filenames = ["next.config.js", "schema.prisma"]
max_size = 2.0
include_git_changes = true
exclude_node_modules = true
# custom profiles may be defined in the config under `profiles` (see source for structure)
```

You can pass a specific config path with `--config <path>`.

## Profiles

Built-in profiles include (at time of writing):

- `rust` — Rust projects (includes `.rs`, `Cargo.toml`, `Cargo.lock`, etc.)
- `nextjs-ts-prisma` — Next.js + TypeScript + Prisma projects
- `cpp-cmake` — C/C++ and CMake projects
- `wordpress` — provided by a WordPress profile plugin (`src/wordpress_profile.rs`) that can:
  - return a conservative WordPress profile, or
  - use `wp-cli` (when present) to detect active theme/plugins and produce a path-aware profile that includes theme/plugin entry files.

Custom profiles can also be defined in the TOML config and may extend built-in profiles.

## How it works (brief)

1. Load CLI args and optional `.flattener.toml` config.
2. Resolve the active profile (built-in, WordPress plugin, or custom profile in config).
3. Walk target directories (respecting include/exclude globs and directory depth).
4. Read allowed files (subject to max size), concatenate them into a single flattened output, optionally wrapped in Markdown code blocks.
5. Optionally append git diffs and token-counting information.

See `src/main.rs` for the full implementation and the available options.

## Examples

Flatten the current repository using the Rust profile and write to `flattened.md` with Markdown blocks:

```powershell
cargo run -- --profile rust --markdown -o flattened.md .
```

Flatten a project and include current git diffs:

```powershell
cargo run -- --profile nextjs-ts-prisma -g -o project_flat.txt /path/to/project
```

Create a custom run overriding extensions:

```powershell
cargo run -- --extensions .py,.md -o scripts_flat.txt /path/to/repo
```

## Development notes

- Source: `src/main.rs`, plugin in `src/wordpress_profile.rs`.
- Dependencies are declared in `Cargo.toml` (rayon, tiktoken-rs, clap, ignore, serde, tracing, etc.).

## License

This project is licensed under the terms in `LICENSE`.

## Contributing

Contributions are welcome. Open issues or PRs for bugs, improvements, or feature requests.

## Contact / Issues

If you find a bug or want a feature, please open an issue in the repository.

---

Requirements coverage:

- Read the project and sources: Done (inspected `src/main.rs`, `src/wordpress_profile.rs`, `Cargo.toml`).
- Produce a revamped `README.md`: Done (this file).
- Sanity-check build after change: Will run `cargo build` and report results.

"Done" means the README was updated. Next step: run a build to validate repository still builds.

## Profiles quick start

- Use a built-in profile:
  - Create a `.flattener.toml` in your project root with:
    ```
    [profiles.myproject]
    profile = "rust"
    ```
  - Or pass --profile rust on the CLI: `code-flattener --profile rust .`

- Create a custom profile that extends a built-in profile:
  ```
  [profiles.custom-rust-like]
  description = "Custom profile extending the built-in rust profile"
  profile = "rust"                  # extends built-in 'rust'
  extensions = [".rs", ".toml", ".ron"]
  allowed_filenames = ["Cargo.toml"]
  include_globs = ["src/**", "examples/**"]
  ```
  Merge precedence: child profile values are merged into the parent; extensions and allowed_filenames are combined, and child's values are preferred where conflicts exist. If a child provides include_globs, they are merged with the parent's globs.

- CLI examples:
  - Dry run to see which files would be processed:
    `code-flattener --profile rust --dry-run .`
  - Include git diffs in the output:
    `code-flattener --profile rust --include-git-changes .`

- WordPress notes:
  - When using `profile = "wordpress"`, the tool will try to use `wp-cli` if available to detect the active theme and plugins. If `wp` is not available it will fall back to filesystem scanning.
  - To include only specific plugins or a theme, use:
    `--wp-include-only-plugins=plugin-slug1,plugin-slug2 --wp-include-theme=theme-name`
  - When writing include_globs on Windows, prefer forward slashes in globs (e.g., "wp-content/plugins/**") — the tool normalizes separators but this avoids surprises.
