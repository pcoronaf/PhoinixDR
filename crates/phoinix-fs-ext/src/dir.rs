//! Linear directory blocks, including the entries hidden in the slack of
//! live entries: `ext4_delete_entry` folds a removed entry into the
//! previous entry's record length without erasing its bytes.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

/// A directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    /// Inode number (0 = unused slot or a deleted first entry).
    pub inode: u32,
    /// Name.
    pub name: String,
    /// File type from the entry (1 regular, 2 directory, 7 symlink), if
    /// the filesystem stores it.
    pub file_type: Option<u8>,
    /// The entry was found in slack space: it was deleted.
    pub deleted: bool,
    /// Byte offset of the entry inside the volume.
    pub offset: u64,
    /// First and last journal transaction whose copy of the block holds
    /// the entry live, when the journal has copies of the block.
    pub alive_in: Option<(u32, u32)>,
}

impl DirEntry {
    /// Whether the name is `.` or `..`.
    #[must_use]
    pub fn is_dot(&self) -> bool {
        self.name == "." || self.name == ".."
    }

    /// Whether the entry names a directory, when the type is known.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        matches!(self.file_type, Some(2))
    }
}

/// Whether `name` looks like a real file name.
fn plausible_name(name: &[u8]) -> bool {
    !name.is_empty()
        && !name.contains(&0)
        && !name.contains(&b'/')
        && name.iter().filter(|b| **b < 0x20 || **b == 0x7F).count() == 0
}

/// Parses one directory block at volume offset `base`, yielding live
/// entries and the deleted ones hidden inside their slack.
///
/// `filetype` says whether entries carry a file type byte; `indexed_root`
/// marks the first block of an htree directory (its `..` entry's slack is
/// index data, not entries); `inodes_count` bounds inode numbers.
#[must_use]
pub fn parse_block(
    block: &[u8],
    base: u64,
    filetype: bool,
    indexed_root: bool,
    inodes_count: u32,
) -> Vec<DirEntry> {
    let mut out = Vec::new();
    let v = ByteView::new(block);
    let mut off = 0usize;
    let mut guard = 0usize;
    // An htree interior node looks like one entry with inode 0 spanning the
    // block: nothing to parse behind it.
    if v.u32_le(0) == Some(0) && v.u16_le(4).map(usize::from) == Some(block.len()) {
        return out;
    }
    while off + 8 <= block.len() && guard < 100_000 {
        guard += 1;
        let inode = v.u32_le(off).unwrap_or(0);
        let rec_len = usize::from(v.u16_le(off + 4).unwrap_or(0));
        let (name_len, file_type) = if filetype {
            (
                usize::from(
                    v.slice(off + 6, 1)
                        .and_then(|b| b.first().copied())
                        .unwrap_or(0),
                ),
                v.slice(off + 7, 1).and_then(|b| b.first().copied()),
            )
        } else {
            (usize::from(v.u16_le(off + 6).unwrap_or(0)), None)
        };
        if rec_len < 8 || off + rec_len > block.len() {
            break;
        }
        let name = v.slice(off + 8, name_len.min(rec_len - 8)).unwrap_or(&[]);
        let live_is_entry = inode != 0 && inode <= inodes_count && plausible_name(name);
        if live_is_entry {
            out.push(DirEntry {
                inode,
                name: String::from_utf8_lossy(name).into_owned(),
                file_type,
                deleted: false,
                offset: base + off as u64,
                alive_in: None,
            });
        } else if inode == 0 && plausible_name(name) && name_len > 0 && off != 0 {
            // A deleted first-of-block entry keeps its name with inode 0.
            out.push(DirEntry {
                inode: 0,
                name: String::from_utf8_lossy(name).into_owned(),
                file_type,
                deleted: true,
                offset: base + off as u64,
                alive_in: None,
            });
        }
        // Slack: bytes between the end of this entry's name and the next
        // record may hold deleted entries.
        let used = (8 + name_len).div_ceil(4) * 4;
        let skip_slack = indexed_root && off == 12 && name == b"..";
        if !skip_slack && rec_len > used + 8 {
            let mut s = off + used;
            let end = off + rec_len;
            let mut inner = 0usize;
            while s + 8 <= end && inner < 4096 {
                inner += 1;
                let h_inode = v.u32_le(s).unwrap_or(0);
                let h_rec = usize::from(v.u16_le(s + 4).unwrap_or(0));
                let (h_name_len, h_type) = if filetype {
                    (
                        usize::from(
                            v.slice(s + 6, 1)
                                .and_then(|b| b.first().copied())
                                .unwrap_or(0),
                        ),
                        v.slice(s + 7, 1).and_then(|b| b.first().copied()),
                    )
                } else {
                    (usize::from(v.u16_le(s + 6).unwrap_or(0)), None)
                };
                let h_name = v.slice(s + 8, h_name_len).unwrap_or(&[]);
                let sane = h_rec >= 8
                    && h_rec % 4 == 0
                    && s + h_rec <= end
                    && h_name_len > 0
                    && h_name_len <= 255
                    && 8 + h_name_len <= h_rec
                    && h_type.is_none_or(|t| t <= 7)
                    && h_inode <= inodes_count
                    && plausible_name(h_name);
                if !sane {
                    // Try the next 4-byte position: entries were aligned.
                    s += 4;
                    continue;
                }
                out.push(DirEntry {
                    inode: h_inode,
                    name: String::from_utf8_lossy(h_name).into_owned(),
                    file_type: h_type,
                    deleted: true,
                    offset: base + s as u64,
                    alive_in: None,
                });
                s += h_rec;
            }
        }
        off += rec_len;
    }
    out
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

    fn entry(inode: u32, rec_len: u16, name: &str, ft: u8) -> Vec<u8> {
        let mut v = inode.to_le_bytes().to_vec();
        v.extend_from_slice(&rec_len.to_le_bytes());
        v.push(name.len() as u8);
        v.push(ft);
        v.extend_from_slice(name.as_bytes());
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }

    #[test]
    fn finds_live_and_hidden_entries() {
        let mut block = Vec::new();
        block.extend(entry(2, 12, ".", 2));
        block.extend(entry(2, 12, "..", 2));
        // "keep.txt" whose rec_len swallows the deleted "gone.bin" behind it.
        let keep = entry(12, 16, "keep.txt", 1);
        let gone = entry(13, 16, "gone.bin", 1);
        let mut merged = keep.clone();
        merged[4..6].copy_from_slice(&((4096 - 24) as u16).to_le_bytes());
        block.extend(merged);
        block.extend(gone);
        block.resize(4096, 0);
        let entries = parse_block(&block, 0, true, false, 100);
        let names: Vec<(&str, bool, u32)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.deleted, e.inode))
            .collect();
        assert_eq!(
            names,
            vec![
                (".", false, 2),
                ("..", false, 2),
                ("keep.txt", false, 12),
                ("gone.bin", true, 13)
            ]
        );
        // Garbage in slack is ignored.
        let mut junk = block.clone();
        for b in &mut junk[40..60] {
            *b = 0xFF;
        }
        let entries = parse_block(&junk, 0, true, false, 100);
        assert!(entries.iter().all(|e| !e.deleted));
    }
}
