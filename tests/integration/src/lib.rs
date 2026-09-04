//! Shared helpers for PHOINIX integration tests.
//!
//! The crate exists so that end-to-end tests can live outside any single
//! engine crate. See the `tests/` directory next to this file.

#![forbid(unsafe_code)]

/// Absolute path of the repository's `tests/fixtures` directory.
#[must_use]
pub fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
}
