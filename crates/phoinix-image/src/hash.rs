//! Hash verification of a whole source against the hashes an acquisition
//! tool stored in the container.

use md5::Md5;
use phoinix_block::{BlockError, BlockReader};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::info::StoredHashes;

/// Bytes hashed per read.
pub const HASH_CHUNK: usize = 8 * 1024 * 1024;

/// Outcome of hashing a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashVerification {
    /// Bytes hashed.
    pub bytes: u64,
    /// Computed MD5, lower-case hex.
    pub md5: String,
    /// Computed SHA-1, lower-case hex.
    pub sha1: String,
    /// Computed SHA-256, lower-case hex.
    pub sha256: String,
    /// Hashes stored in the container.
    pub stored: StoredHashes,
    /// Whether the computed MD5 matches the stored one (None without one).
    pub md5_matches: Option<bool>,
    /// Whether the computed SHA-1 matches the stored one (None without one).
    pub sha1_matches: Option<bool>,
}

impl HashVerification {
    /// Whether every stored hash matched. `None` when nothing was stored.
    #[must_use]
    pub fn verified(&self) -> Option<bool> {
        match (self.md5_matches, self.sha1_matches) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(true) && b.unwrap_or(true)),
        }
    }
}

/// Hashes the whole of `reader` and compares with `stored`. `progress`
/// receives (bytes done, bytes total) after every chunk and may return
/// `false` to cancel.
///
/// # Errors
///
/// Returns [`BlockError`] if the source cannot be read; a cancelled run
/// returns [`BlockError::ShortRead`].
pub fn verify(
    reader: &dyn BlockReader,
    stored: &StoredHashes,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<HashVerification, BlockError> {
    let total = reader.len();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    let mut done = 0u64;
    while done < total {
        let want = usize::try_from((total - done).min(HASH_CHUNK as u64))
            .map_err(|_| BlockError::IntegerOverflow)?;
        let dst = buffer.get_mut(..want).ok_or(BlockError::IntegerOverflow)?;
        let n = reader.read_at(done, dst)?;
        if n == 0 {
            return Err(BlockError::ShortRead {
                expected: want,
                actual: 0,
            });
        }
        let got = dst.get(..n).ok_or(BlockError::IntegerOverflow)?;
        md5.update(got);
        sha1.update(got);
        sha256.update(got);
        done += n as u64;
        if !progress(done, total) {
            return Err(BlockError::ShortRead {
                expected: usize::try_from(total).unwrap_or(usize::MAX),
                actual: usize::try_from(done).unwrap_or(usize::MAX),
            });
        }
    }
    let md5 = hex::encode(md5.finalize());
    let sha1 = hex::encode(sha1.finalize());
    let sha256 = hex::encode(sha256.finalize());
    let md5_matches = stored.md5.as_ref().map(|s| s.eq_ignore_ascii_case(&md5));
    let sha1_matches = stored.sha1.as_ref().map(|s| s.eq_ignore_ascii_case(&sha1));
    Ok(HashVerification {
        bytes: done,
        md5,
        sha1,
        sha256,
        stored: stored.clone(),
        md5_matches,
        sha1_matches,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use phoinix_block::MemoryReader;

    use super::*;

    #[test]
    fn hashes_and_compares() {
        let reader = MemoryReader::new(b"abc".to_vec());
        let stored = StoredHashes {
            md5: Some("900150983cd24fb0d6963f7d28e17f72".into()),
            sha1: Some("A9993E364706816ABA3E25717850C26C9CD0D89D".into()),
        };
        let v = verify(&reader, &stored, &mut |_, _| true).unwrap();
        assert_eq!(v.bytes, 3);
        assert_eq!(v.verified(), Some(true));
        assert_eq!(
            v.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let bad = StoredHashes {
            md5: Some("00".into()),
            sha1: None,
        };
        let v = verify(&reader, &bad, &mut |_, _| true).unwrap();
        assert_eq!(v.verified(), Some(false));
        assert!(verify(&reader, &StoredHashes::default(), &mut |_, _| false).is_err());
    }
}
