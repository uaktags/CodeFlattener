use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use tempfile::tempdir;

// This integration test creates temporary project layouts and runs the
// code-flattener binary with different profiles to ensure expected behavior.

#[test]
fn gold_standard_profile_processes_expected_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let root = dir.path();

    // Create a small Rust project structure
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src").join("lib.rs"), "pub fn hello() { println!(\"hi\"); }")?;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"temp_proj\"\n")?;
    fs::write(root.join("Cargo.lock"), "# lockfile")?;

    // Create a .flattener.toml specifying gold-standard profile
    let conf = r#"
[profiles.gold-standard]
profile = "rust"
include_globs = ["src/**", "Cargo.toml", "Cargo.lock"]
extensions = [".rs"]
allowed_filenames = ["Cargo.toml", "Cargo.lock"]
"#;
    fs::write(root.join(".flattener.toml"), conf)?;

    // Run the binary with dry-run to avoid writing output files
    let mut cmd = Command::cargo_bin("code-flattener")?;
    cmd.current_dir(root)
        .arg("--profile")
        .arg("gold-standard")
        .arg("--dry-run")
        .arg(".");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("DRY-RUN: would process"));

    dir.close()?;
    Ok(())
}

#[test]
fn rust2_profile_matches_src_and_cargo_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let root = dir.path();

    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src").join("main.rs"), "fn main() {}")?;
    fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n")?;
    fs::write(root.join("Cargo.lock"), "# lock")?;

    let conf = r#"
[profiles.rust2]
include_globs = ["src/**", "Cargo.toml", "Cargo.lock"]
max_size = 1.0
"#;
    fs::write(root.join(".flattener.toml"), conf)?;

    let mut cmd = Command::cargo_bin("code-flattener")?;
    cmd.current_dir(root)
        .arg("--profile")
        .arg("rust2")
        .arg("--dry-run")
        .arg(".");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("DRY-RUN: would process"));

    dir.close()?;
    Ok(())
}
