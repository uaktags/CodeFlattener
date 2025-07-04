# Development Plan & TODO

This document outlines future enhancements to make the code flattener more powerful and intuitive for users.

## 1. External Profile Configuration (High Priority)

The current method of hardcoding profiles in a `static HashMap` is inflexible. The tool should support user-defined profiles loaded from an external file.

-   **Goal:** Allow users to add or customize profiles without recompiling the application.
-   **Proposed Solution:**
    1.  **Config File:** Use a TOML file (e.g., `profiles.toml`).
    2.  **Location:** Search for the config file in standard locations (e.g., user's config directory, project root). The `dirs` crate can help find platform-specific config paths.
    3.  **Implementation:**
        -   Use `serde` for deserializing the TOML file into structs.
        -   At startup, load the default built-in profiles, then load and merge profiles from the user's config file, with user-defined profiles overriding defaults.
    4.  **Dynamic CLI:** The `--profile` argument needs to be dynamically populated from the loaded profile names. This is a known challenge with `clap::value_enum` and may require using `clap::builder::PossibleValuesParser` or a similar dynamic approach instead of the derive macro.

### 2. Project Type Auto-Detection

-   **Goal:** Improve usability by suggesting a profile if none is provided.
-   **Proposed Solution:**
    -   If the `--profile` flag is omitted, scan the target directory for key indicator files (`Cargo.toml`, `package.json`, `CMakeLists.txt`, `pom.xml`, etc.).
    -   If a likely project type is identified, either automatically apply that profile or prompt the user to confirm.

### 3. Enhanced Git Integration

-   **Goal:** Provide more granular control over what Git information is included.
-   **Proposed Solution:**
    -   Add a flag to include the output of `git log -n <count>` to show recent commit history.
    -   Add a flag to diff against a specific branch (e.g., `main` or `develop`) instead of just local staged/unstaged changes.

### 4. Code Quality & Refinements

-   **Configuration Loading:** Abstract the configuration logic (profiles, overrides, etc.) into its own module (`config.rs`) to clean up `main.rs`.
-   **Error Handling:** Improve warnings. For example, when a file can't be read, explicitly state if it's being skipped due to a non-UTF8 encoding issue.
