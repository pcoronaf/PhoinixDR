//! Deterministic chunk contents.
//!
//! A pass is written in chunks of [`CHUNK_BYTES`]. The bytes of chunk `i`
//! of pass `p` depend only on the pattern, the run's 256-bit seed, `p` and
//! `i`, so the verification stage can regenerate any chunk and compare it
//! with what the device returns, random passes included. Random chunks are
//! produced by ChaCha (rand's `StdRng`) keyed with SHA-256(seed, pass,
//! chunk).

use rand::{RngCore, SeedableRng, TryRngCore};
use sha2::{Digest, Sha256};

use crate::method::PassPattern;

/// Bytes written per chunk (a multiple of every sector size in use).
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// A 256-bit seed for the random passes of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seed(pub [u8; 32]);

impl Seed {
    /// A fresh seed from the operating system's randomness.
    #[must_use]
    pub fn fresh() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .unwrap_or_else(|_| {
                // Fall back to the thread RNG (itself OS-seeded) if the OS
                // source is unavailable.
                rand::rng().fill_bytes(&mut bytes);
            });
        Self(bytes)
    }

    /// Hex rendering for the certificate.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parses the hex rendering.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = hex::decode(text.trim()).ok()?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(Self(arr))
    }
}

/// Fills `buf` with the contents of chunk `chunk` of pass `pass`.
pub fn fill_chunk(pattern: &PassPattern, seed: &Seed, pass: usize, chunk: u64, buf: &mut [u8]) {
    match pattern {
        PassPattern::Fixed { byte } => buf.fill(*byte),
        PassPattern::Sequence { bytes } => {
            if bytes.is_empty() {
                buf.fill(0);
                return;
            }
            // Keep the sequence phase continuous across chunks.
            let start = usize::try_from(chunk % bytes.len() as u64)
                .unwrap_or(0)
                .wrapping_mul(buf.len() % bytes.len())
                % bytes.len();
            for (i, b) in buf.iter_mut().enumerate() {
                *b = bytes.get((start + i) % bytes.len()).copied().unwrap_or(0);
            }
        }
        PassPattern::Random => {
            let mut h = Sha256::new();
            h.update(seed.0);
            h.update((pass as u64).to_le_bytes());
            h.update(chunk.to_le_bytes());
            let key: [u8; 32] = h.finalize().into();
            let mut rng = rand::rngs::StdRng::from_seed(key);
            rng.fill_bytes(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn chunks_are_reproducible_and_distinct() {
        let seed = Seed::fresh();
        let mut a = vec![0u8; 8192];
        let mut b = vec![0u8; 8192];
        fill_chunk(&PassPattern::Random, &seed, 0, 7, &mut a);
        fill_chunk(&PassPattern::Random, &seed, 0, 7, &mut b);
        assert_eq!(a, b);
        fill_chunk(&PassPattern::Random, &seed, 0, 8, &mut b);
        assert_ne!(a, b);
        fill_chunk(&PassPattern::Random, &seed, 1, 7, &mut b);
        assert_ne!(a, b);
        assert!(a.iter().filter(|x| **x == 0).count() < 200);
        let seed2 = Seed::from_hex(&seed.to_hex()).unwrap();
        assert_eq!(seed, seed2);
    }

    #[test]
    fn sequences_keep_their_phase_across_chunks() {
        let seed = Seed::fresh();
        let pat = PassPattern::Sequence {
            bytes: vec![1, 2, 3],
        };
        let mut c0 = vec![0u8; 10];
        let mut c1 = vec![0u8; 10];
        fill_chunk(&pat, &seed, 0, 0, &mut c0);
        fill_chunk(&pat, &seed, 0, 1, &mut c1);
        let mut joined = c0.clone();
        joined.extend(c1);
        for (i, b) in joined.iter().enumerate() {
            assert_eq!(*b, [1u8, 2, 3][i % 3]);
        }
        let mut f = vec![9u8; 4];
        fill_chunk(&PassPattern::Fixed { byte: 0xAA }, &seed, 0, 0, &mut f);
        assert_eq!(f, vec![0xAA; 4]);
    }
}
