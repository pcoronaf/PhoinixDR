//! The jbd2 journal: older copies of metadata blocks, indexed by the
//! filesystem block they were logged for.

use std::collections::HashMap;

use phoinix_block::BlockReaderExt;
use phoinix_core::bytes::ByteView;
use phoinix_fs::{Extent, ExtentStream};
use serde::{Deserialize, Serialize};

use crate::ExtError;

/// Journal block magic.
pub const MAGIC: u32 = 0xC03B_3998;
const DESCRIPTOR: u32 = 1;
const COMMIT: u32 = 2;
const SUPERBLOCK_V1: u32 = 3;
const SUPERBLOCK_V2: u32 = 4;
const REVOKE: u32 = 5;
const FLAG_ESCAPE: u32 = 1;
const FLAG_SAME_UUID: u32 = 2;
const FLAG_LAST_TAG: u32 = 8;
const INCOMPAT_64BIT: u32 = 0x2;
const INCOMPAT_CSUM_V2: u32 = 0x8;
const INCOMPAT_CSUM_V3: u32 = 0x10;
/// Most journal blocks walked.
pub const MAX_JOURNAL_BLOCKS: u64 = 1 << 22;

/// One logged copy of a filesystem block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedBlock {
    /// Transaction sequence number.
    pub sequence: u32,
    /// Journal block index holding the copy.
    pub journal_block: u64,
    /// The copy's checksum matched its tag (None when untagged).
    pub checksum_ok: Option<bool>,
    /// The first four bytes were escaped and must read as the magic.
    pub escaped: bool,
}

/// Journal superblock facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalInfo {
    /// Journal block size.
    pub block_size: u32,
    /// Journal length in blocks.
    pub max_len: u32,
    /// First log block.
    pub first: u32,
    /// Expected sequence of the first transaction.
    pub sequence: u32,
    /// Start block of the log (0 = clean).
    pub start: u32,
    /// Incompatible features.
    pub incompat: u32,
    /// Descriptor blocks seen.
    pub descriptors: usize,
    /// Commit blocks seen.
    pub commits: usize,
    /// Distinct filesystem blocks with at least one logged copy.
    pub logged_blocks: usize,
}

/// A parsed journal.
#[derive(Debug)]
pub struct Journal {
    stream: ExtentStream,
    info: JournalInfo,
    logged: HashMap<u64, Vec<LoggedBlock>>,
}

impl Journal {
    /// Parses the journal whose data is `stream`.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError::Malformed`] if the journal superblock is not
    /// recognised.
    pub fn parse(stream: ExtentStream, fs_uuid: &[u8; 16]) -> Result<Self, ExtError> {
        let malformed = |detail: &str| ExtError::Malformed {
            structure: "journal",
            detail: detail.to_owned(),
        };
        let mut head = vec![0u8; 1024];
        let n = stream
            .read_at(0, &mut head)
            .map_err(|e| ExtError::Malformed {
                structure: "journal",
                detail: e.to_string(),
            })?;
        head.truncate(n);
        let v = ByteView::new(&head);
        if v.u32_be(0) != Some(MAGIC) || !matches!(v.u32_be(4), Some(SUPERBLOCK_V1 | SUPERBLOCK_V2))
        {
            return Err(malformed("superblock magic absent"));
        }
        let block_size = v.u32_be(12).unwrap_or(0);
        let max_len = v.u32_be(16).unwrap_or(0);
        if !(1024..=65536).contains(&block_size) || !block_size.is_power_of_two() || max_len == 0 {
            return Err(malformed("invalid block size or length"));
        }
        let incompat = v.u32_be(40).unwrap_or(0);
        let first = v.u32_be(20).unwrap_or(1);
        let mut info = JournalInfo {
            block_size,
            max_len,
            first,
            sequence: v.u32_be(24).unwrap_or(0),
            start: v.u32_be(28).unwrap_or(0),
            incompat,
            descriptors: 0,
            commits: 0,
            logged_blocks: 0,
        };
        let journal_uuid: [u8; 16] = v
            .slice(48, 16)
            .and_then(|b| b.try_into().ok())
            .unwrap_or(*fs_uuid);
        let csum_seed = crate::crc32c::update(!0, &journal_uuid);
        let mut logged: HashMap<u64, Vec<LoggedBlock>> = HashMap::new();
        let bs = u64::from(block_size);
        let total = u64::from(max_len)
            .min(MAX_JOURNAL_BLOCKS)
            .min(stream.len() / bs);
        let tag_size: usize = if incompat & INCOMPAT_CSUM_V3 != 0 {
            16
        } else {
            8 + if incompat & INCOMPAT_64BIT != 0 { 4 } else { 0 }
        };
        let mut i = u64::from(first).max(1);
        let mut guard = 0u64;
        while i < total && guard < MAX_JOURNAL_BLOCKS {
            guard += 1;
            let block = match stream
                .read_at_vec(i * bs, usize::try_from(bs).map_err(|_| ExtError::Overflow)?)
            {
                Ok(b) => b,
                Err(_) => break,
            };
            let bv = ByteView::new(&block);
            if bv.u32_be(0) != Some(MAGIC) {
                i += 1;
                continue;
            }
            let kind = bv.u32_be(4).unwrap_or(0);
            let sequence = bv.u32_be(8).unwrap_or(0);
            match kind {
                DESCRIPTOR => {
                    info.descriptors += 1;
                    let mut off = 12usize;
                    let mut data = i + 1;
                    let limit = block.len().saturating_sub(
                        if incompat & (INCOMPAT_CSUM_V2 | INCOMPAT_CSUM_V3) != 0 {
                            4
                        } else {
                            0
                        },
                    );
                    let mut tags = 0usize;
                    while off + tag_size <= limit && tags < 4096 {
                        tags += 1;
                        let (fs_block, flags, tag_csum) = if tag_size == 16 {
                            (
                                u64::from(bv.u32_be(off).unwrap_or(0))
                                    | (u64::from(bv.u32_be(off + 8).unwrap_or(0)) << 32),
                                bv.u32_be(off + 4).unwrap_or(0),
                                Some(bv.u32_be(off + 12).unwrap_or(0)),
                            )
                        } else {
                            let hi = if tag_size == 12 {
                                u64::from(bv.u32_be(off + 8).unwrap_or(0)) << 32
                            } else {
                                0
                            };
                            (
                                u64::from(bv.u32_be(off).unwrap_or(0)) | hi,
                                u32::from(bv.u16_be(off + 6).unwrap_or(0)),
                                (incompat & INCOMPAT_CSUM_V2 != 0)
                                    .then(|| u32::from(bv.u16_be(off + 4).unwrap_or(0))),
                            )
                        };
                        off += tag_size;
                        if flags & FLAG_SAME_UUID == 0 {
                            off += 16;
                        }
                        if data >= total {
                            break;
                        }
                        let checksum_ok = tag_csum.and_then(|expected| {
                            let copy = stream
                                .read_at_vec(data * bs, usize::try_from(bs).ok()?)
                                .ok()?;
                            let mut c = crate::crc32c::update(csum_seed, &sequence.to_be_bytes());
                            c = crate::crc32c::update(c, &copy);
                            Some(if tag_size == 16 {
                                c == expected
                            } else {
                                (c & 0xFFFF) == expected
                            })
                        });
                        logged.entry(fs_block).or_default().push(LoggedBlock {
                            sequence,
                            journal_block: data,
                            checksum_ok,
                            escaped: flags & FLAG_ESCAPE != 0,
                        });
                        data += 1;
                        if flags & FLAG_LAST_TAG != 0 {
                            break;
                        }
                    }
                    i = data.max(i + 1);
                }
                COMMIT => {
                    info.commits += 1;
                    i += 1;
                }
                REVOKE | SUPERBLOCK_V1 | SUPERBLOCK_V2 => {
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
        for copies in logged.values_mut() {
            copies.sort_by_key(|c| std::cmp::Reverse(c.sequence));
        }
        info.logged_blocks = logged.len();
        Ok(Self {
            stream,
            info,
            logged,
        })
    }

    /// Journal facts.
    #[must_use]
    pub const fn info(&self) -> &JournalInfo {
        &self.info
    }

    /// Logged copies of `fs_block`, newest first.
    #[must_use]
    pub fn copies(&self, fs_block: u64) -> &[LoggedBlock] {
        self.logged.get(&fs_block).map_or(&[], Vec::as_slice)
    }

    /// Reads a logged copy.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] on read failures.
    pub fn read_copy(&self, copy: &LoggedBlock) -> Result<Vec<u8>, ExtError> {
        let bs = u64::from(self.info.block_size);
        let mut data = self
            .stream
            .read_at_vec(
                copy.journal_block * bs,
                usize::try_from(bs).map_err(|_| ExtError::Overflow)?,
            )
            .map_err(|e| ExtError::Malformed {
                structure: "journal",
                detail: e.to_string(),
            })?;
        if copy.escaped
            && let Some(head) = data.get_mut(..4)
        {
            head.copy_from_slice(&MAGIC.to_be_bytes());
        }
        Ok(data)
    }
}

/// Builds the extents of the journal file for [`Journal::parse`].
#[must_use]
pub fn extents_from_runs(runs: &[crate::extent::Run], block_size: u64) -> Vec<Extent> {
    runs.iter()
        .filter_map(|r| {
            r.physical.map(|p| Extent {
                offset: p.saturating_mul(block_size),
                length: r.count.saturating_mul(block_size),
            })
        })
        .collect()
}

/// A [`BlockReaderExt`]-like helper on extent streams.
trait ReadAtVec {
    fn read_at_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>, phoinix_fs::FsError>;
}

impl ReadAtVec for ExtentStream {
    fn read_at_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>, phoinix_fs::FsError> {
        let mut buf = vec![0u8; len];
        let n = self.read_at(offset, &mut buf)?;
        buf.truncate(n);
        if n < len {
            return Err(phoinix_fs::FsError::Malformed {
                structure: "journal",
                detail: format!("short read at {offset}"),
            });
        }
        Ok(buf)
    }
}

#[allow(dead_code)]
fn _uses_ext(_: &dyn BlockReaderExt) {}
