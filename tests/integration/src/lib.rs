//! Shared helpers for PHOINIX integration tests.
//!
//! The crate exists so that end-to-end tests can live outside any single
//! engine crate. See the `tests/` directory next to this file.

#![forbid(unsafe_code)]
// This crate only supports tests: failing loudly on a missing fixture is correct.
#![allow(clippy::panic, clippy::expect_used)]

use std::io::Read;
use std::path::{Path, PathBuf};

use phoinix_block::MemoryReader;

/// Absolute path of the repository's `tests/fixtures` directory.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
}

/// Decompresses a `.img.gz` fixture into memory.
///
/// # Panics
///
/// Panics if the fixture is missing or unreadable; fixtures are part of the
/// repository, so this is a test-setup error.
#[must_use]
#[allow(clippy::expect_used)]
pub fn load_gz(relative: &str) -> Vec<u8> {
    let path = fixtures_dir().join(relative);
    let file = std::fs::File::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {}: {e}", path.display()));
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut data = Vec::new();
    decoder.read_to_end(&mut data).expect("decompress fixture");
    data
}

/// Loads a fixture image as an in-memory reader.
#[must_use]
pub fn fixture_reader(relative: &str) -> MemoryReader {
    MemoryReader::new(load_gz(relative))
}

/// Parses a fixture manifest.
///
/// # Panics
///
/// Panics if the manifest is missing or malformed.
#[must_use]
#[allow(clippy::expect_used)]
pub fn manifest(relative: &str) -> serde_json::Value {
    let path = fixtures_dir().join(relative);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("valid manifest JSON")
}

/// A tiny deterministic PRNG (xorshift64*) for mutation tests.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeds the generator; zero is remapped to a fixed constant.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Next 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..bound` (bound > 0).
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }
}
