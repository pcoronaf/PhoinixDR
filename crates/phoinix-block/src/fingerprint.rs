//! Lightweight source identity used to detect that a source changed between
//! sessions. This is *not* a forensic whole-disk hash.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{BlockError, BlockReader, BlockReaderExt};

const MIB: u64 = 1024 * 1024;

/// Fingerprint of a source: its size and digests of its first and last MiB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    /// Source length in bytes.
    pub size: u64,
    /// SHA-256 of the first MiB (or of the whole source if shorter).
    #[serde(with = "hex_digest")]
    pub first_mib_sha256: [u8; 32],
    /// SHA-256 of the last MiB, if the source is longer than one MiB.
    #[serde(with = "hex_digest_opt")]
    pub last_mib_sha256: Option<[u8; 32]>,
}

impl SourceFingerprint {
    /// Computes the fingerprint by reading at most two MiB.
    ///
    /// # Errors
    ///
    /// Propagates read errors.
    pub fn compute(reader: &dyn BlockReader) -> Result<Self, BlockError> {
        let size = reader.len();
        let head_len = size.min(MIB);
        let head = reader.read_vec(
            0,
            usize::try_from(head_len).map_err(|_| BlockError::IntegerOverflow)?,
        )?;
        let first = Sha256::digest(&head).into();
        let last = if size > MIB {
            let tail = reader.read_vec(
                size - MIB,
                usize::try_from(MIB).map_err(|_| BlockError::IntegerOverflow)?,
            )?;
            Some(Sha256::digest(&tail).into())
        } else {
            None
        };
        Ok(Self {
            size,
            first_mib_sha256: first,
            last_mib_sha256: last,
        })
    }
}

mod hex_digest {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(d)?;
        super::from_hex(&text).ok_or_else(|| serde::de::Error::custom("invalid SHA-256 hex"))
    }
}

mod hex_digest_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => s.serialize_some(&super::to_hex(v)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let text: Option<String> = Option::deserialize(d)?;
        match text {
            None => Ok(None),
            Some(t) => super::from_hex(&t)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("invalid SHA-256 hex")),
        }
    }
}

/// Renders a digest as lowercase hex.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let pair = text.get(i * 2..i * 2 + 2)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]

    use super::*;
    use crate::MemoryReader;

    #[test]
    fn small_source_has_no_tail() {
        let reader = MemoryReader::new(vec![7u8; 4096]);
        let fp = SourceFingerprint::compute(&reader).unwrap();
        assert_eq!(fp.size, 4096);
        assert!(fp.last_mib_sha256.is_none());
        assert_eq!(
            fp.first_mib_sha256,
            Sha256::digest(vec![7u8; 4096]).as_slice()
        );
    }

    #[test]
    fn large_source_has_tail_and_round_trips_json() {
        let mut data = vec![0u8; 3 * 1024 * 1024];
        if let Some(last) = data.last_mut() {
            *last = 0xAA;
        }
        let reader = MemoryReader::new(data);
        let fp = SourceFingerprint::compute(&reader).unwrap();
        assert!(fp.last_mib_sha256.is_some());
        assert_ne!(fp.last_mib_sha256.unwrap(), fp.first_mib_sha256);
        let json = serde_json::to_string(&fp).unwrap();
        let back: SourceFingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fp);
    }
}
