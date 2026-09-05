//! The ext volume facade: superblock, groups, inodes, layouts, streams,
//! directories, the journal.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use phoinix_block::{BlockReader, BlockReaderExt, MemoryReader};
use phoinix_fs::{Extent, ExtentStream};
use serde::{Deserialize, Serialize};

use crate::bitmap::{BlockBitmaps, BlockState};
use crate::dir::{DirEntry, parse_block};
use crate::extent::Run;
use crate::group::GroupDescriptor;
use crate::inode::{Inode, InodeKind, flags as iflags};
use crate::journal::{Journal, LoggedBlock, extents_from_runs};
use crate::superblock::{SUPERBLOCK_OFFSET, Superblock, incompat};
use crate::{ExtError, blockmap, extent};

/// Root directory inode.
pub const ROOT_INODE: u32 = 2;
/// Default journal inode.
pub const JOURNAL_INODE: u32 = 8;
/// Deepest directory nesting walked.
pub const MAX_DEPTH: usize = 128;
/// Largest directory read.
pub const MAX_DIRECTORY_BYTES: u64 = 256 * 1024 * 1024;

/// One entry found while walking the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkedEntry {
    /// POSIX path (`/docs/photo.jpg`).
    pub path: String,
    /// The directory entry.
    pub entry: DirEntry,
    /// Inode of the directory holding the entry.
    pub parent: u32,
    /// The entry lies in a directory that is itself deleted.
    pub via_deleted_directory: bool,
}

/// Where a layout came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    /// The inode on disk.
    Inode,
    /// The data lives in the inode.
    Inline,
    /// An older copy of the inode from the journal.
    Journal {
        /// Transaction sequence.
        sequence: u32,
        /// Checksum of the logged block verified (None when untagged).
        checksum_ok: Option<bool>,
    },
}

/// The layout of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// Runs in logical order (holes included).
    pub runs: Vec<Run>,
    /// Size in bytes.
    pub size: u64,
    /// Where it came from.
    pub source: LayoutSource,
    /// Inline bytes, for inline data.
    pub inline: Option<Vec<u8>>,
    /// The inode version the layout was read from.
    pub inode: Inode,
}

/// An opened ext volume.
pub struct ExtVolume {
    reader: Arc<dyn BlockReader>,
    sb: Superblock,
    groups: Vec<GroupDescriptor>,
    bitmaps: BlockBitmaps,
    journal: OnceLock<Option<Journal>>,
}

impl std::fmt::Debug for ExtVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtVolume")
            .field("source", &self.reader.describe())
            .field("flavour", &self.sb.flavour())
            .field("blocks", &self.sb.blocks_count)
            .finish()
    }
}

impl ExtVolume {
    /// Opens the volume at offset 0 of `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] if the superblock or the descriptors cannot be
    /// read.
    pub fn open(reader: Arc<dyn BlockReader>) -> Result<Self, ExtError> {
        let bytes = reader.read_vec(SUPERBLOCK_OFFSET, 1024)?;
        let sb = Superblock::parse(&bytes)?;
        let group_count = sb.group_count();
        if group_count > 1 << 24 {
            return Err(ExtError::InvalidSuperblock(format!(
                "{group_count} block groups"
            )));
        }
        let mut groups = Vec::with_capacity(usize::try_from(group_count).unwrap_or(0));
        let desc_size = usize::from(sb.desc_size);
        for g in 0..group_count {
            let (block, within) = sb.descriptor_location(g);
            let off = sb
                .block_offset(block)?
                .checked_add(within)
                .ok_or(ExtError::Overflow)?;
            let raw = reader.read_vec(off, desc_size)?;
            let desc =
                GroupDescriptor::parse(&raw, sb.desc_size).ok_or_else(|| ExtError::Malformed {
                    structure: "group descriptor",
                    detail: format!("group {g} unreadable"),
                })?;
            if desc.inode_table >= sb.blocks_count {
                return Err(ExtError::Malformed {
                    structure: "group descriptor",
                    detail: format!(
                        "group {g}: inode table at block {} beyond the volume",
                        desc.inode_table
                    ),
                });
            }
            groups.push(desc);
        }
        tracing::info!(
            flavour = sb.flavour(),
            blocks = sb.blocks_count,
            block_size = sb.block_size,
            groups = groups.len(),
            "ext volume opened"
        );
        Ok(Self {
            reader,
            sb,
            bitmaps: BlockBitmaps::new(groups.clone()),
            groups,
            journal: OnceLock::new(),
        })
    }

    /// The superblock.
    #[must_use]
    pub const fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// The group descriptors.
    #[must_use]
    pub fn groups(&self) -> &[GroupDescriptor] {
        &self.groups
    }

    /// The reader.
    #[must_use]
    pub fn reader(&self) -> &Arc<dyn BlockReader> {
        &self.reader
    }

    /// Block size in bytes.
    #[must_use]
    pub const fn block_size(&self) -> u32 {
        self.sb.block_size
    }

    /// State of a block in the block bitmap.
    #[must_use]
    pub fn block_state(&self, block: u64) -> BlockState {
        self.bitmaps.state(&*self.reader, &self.sb, block)
    }

    /// Byte offset of `inode` inside the volume, and the block holding it.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError::InodeOutOfRange`].
    pub fn inode_location(&self, inode: u32) -> Result<(u64, u64), ExtError> {
        if inode == 0 || inode > self.sb.inodes_count {
            return Err(ExtError::InodeOutOfRange(inode));
        }
        let index = u64::from(inode - 1);
        let group = index / u64::from(self.sb.inodes_per_group);
        let within = index % u64::from(self.sb.inodes_per_group);
        let desc = self
            .groups
            .get(usize::try_from(group).map_err(|_| ExtError::Overflow)?)
            .ok_or(ExtError::InodeOutOfRange(inode))?;
        let byte = within
            .checked_mul(u64::from(self.sb.inode_size))
            .ok_or(ExtError::Overflow)?;
        let block = desc
            .inode_table
            .checked_add(byte / u64::from(self.sb.block_size))
            .ok_or(ExtError::Overflow)?;
        let offset = self
            .sb
            .block_offset(desc.inode_table)?
            .checked_add(byte)
            .ok_or(ExtError::Overflow)?;
        Ok((offset, block))
    }

    /// Reads inode `number` from disk.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] if the inode is out of range or unreadable.
    pub fn inode(&self, number: u32) -> Result<Inode, ExtError> {
        let (offset, _) = self.inode_location(number)?;
        let raw = self
            .reader
            .read_vec(offset, usize::from(self.sb.inode_size))?;
        Inode::parse(number, &raw, &self.sb).ok_or_else(|| ExtError::Malformed {
            structure: "inode",
            detail: format!("inode {number} unreadable"),
        })
    }

    /// Parses inode `number` out of a copy of its inode-table block (from
    /// the journal).
    #[must_use]
    pub fn inode_from_block(&self, number: u32, block_copy: &[u8]) -> Option<Inode> {
        let index = u64::from(number.checked_sub(1)?) % u64::from(self.sb.inodes_per_group);
        let byte = index.checked_mul(u64::from(self.sb.inode_size))?;
        let within = usize::try_from(byte % u64::from(self.sb.block_size)).ok()?;
        let raw = block_copy.get(within..within.checked_add(usize::from(self.sb.inode_size))?)?;
        Inode::parse(number, raw, &self.sb)
    }

    /// Every inode number that can exist, group by group.
    pub fn inode_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        (1..=self.sb.inodes_count).filter(move |n| {
            let group = (n - 1) / self.sb.inodes_per_group;
            self.groups
                .get(usize::try_from(group).unwrap_or(usize::MAX))
                .is_some_and(|d| !d.inode_uninit())
        })
    }

    /// The layout described by `inode` itself.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] for malformed trees or maps.
    pub fn layout_of(&self, inode: &Inode) -> Result<Layout, ExtError> {
        if inode.inline_data() {
            let len = usize::try_from(inode.size.min(60)).unwrap_or(60);
            return Ok(Layout {
                runs: Vec::new(),
                size: inode.size,
                source: LayoutSource::Inline,
                inline: Some(inode.i_block.get(..len).unwrap_or(&[]).to_vec()),
                inode: inode.clone(),
            });
        }
        let runs = if inode.uses_extents() {
            extent::walk(&*self.reader, &self.sb, &inode.i_block)?
        } else {
            blockmap::walk(&*self.reader, &self.sb, &inode.i_block, inode.size)?
        };
        Ok(Layout {
            runs,
            size: inode.size,
            source: LayoutSource::Inode,
            inline: None,
            inode: inode.clone(),
        })
    }

    /// Byte extents of a layout, holes as gaps (the stream reads gaps as
    /// zeros), trimmed to the size.
    #[must_use]
    pub fn extents_of(&self, layout: &Layout) -> Vec<Extent> {
        let bs = u64::from(self.sb.block_size);
        let mut out: Vec<Extent> = Vec::new();
        for r in &layout.runs {
            let Some(p) = r.physical else {
                continue;
            };
            let start = r.logical.saturating_mul(bs);
            if start >= layout.size {
                break;
            }
            let length = r.count.saturating_mul(bs).min(layout.size - start);
            out.push(Extent {
                offset: p.saturating_mul(bs),
                length,
            });
        }
        out
    }

    /// The runs of a layout with every logical gap (a hole) made explicit,
    /// up to the number of blocks the size needs.
    #[must_use]
    pub fn dense_runs(&self, layout: &Layout) -> Vec<Run> {
        let bs = u64::from(self.sb.block_size.max(1));
        let needed = layout.size.div_ceil(bs);
        let mut out = Vec::new();
        let mut next = 0u64;
        for r in &layout.runs {
            if r.logical > next {
                out.push(Run {
                    logical: next,
                    physical: None,
                    count: r.logical - next,
                    uninitialized: false,
                });
            }
            out.push(*r);
            next = r.logical.saturating_add(r.count).max(next);
        }
        if next < needed {
            out.push(Run {
                logical: next,
                physical: None,
                count: needed - next,
                uninitialized: false,
            });
        }
        out
    }

    /// Whether the layout covers the whole size. Extent trees describe
    /// holes implicitly (a logical gap is a hole), so a walked tree is
    /// complete; block maps must cover every block, holes being explicit
    /// zero pointers.
    #[must_use]
    pub fn layout_complete(&self, layout: &Layout) -> bool {
        if layout.inline.is_some() || layout.inode.uses_extents() {
            return true;
        }
        let bs = u64::from(self.sb.block_size);
        let needed = layout.size.div_ceil(bs.max(1));
        let mut next = 0u64;
        for r in &layout.runs {
            if r.logical != next {
                return false;
            }
            next = next.saturating_add(r.count);
            if next >= needed {
                return true;
            }
        }
        next >= needed
    }

    /// Opens a stream over a layout.
    #[must_use]
    pub fn open_layout(&self, layout: &Layout) -> ExtentStream {
        if let Some(inline) = &layout.inline {
            let mem: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(inline.clone()));
            let len = inline.len() as u64;
            return ExtentStream::new(
                mem,
                vec![Extent {
                    offset: 0,
                    length: len,
                }],
                len,
            );
        }
        let bs = u64::from(self.sb.block_size);
        // Holes must read as zeros: map them onto a zero reader would need a
        // second source, so holes are represented by extents of a zero
        // buffer sized to the largest hole.
        let mut extents = Vec::new();
        let mut zero_needed = 0u64;
        for r in &self.dense_runs(layout) {
            let start = r.logical.saturating_mul(bs);
            if start >= layout.size {
                break;
            }
            let length = r.count.saturating_mul(bs).min(layout.size - start);
            match r.physical {
                Some(p) if !r.uninitialized => extents.push((false, p.saturating_mul(bs), length)),
                _ => {
                    zero_needed = zero_needed.max(length);
                    extents.push((true, 0, length));
                }
            }
        }
        if zero_needed == 0 {
            let plain = extents
                .iter()
                .map(|(_, o, l)| Extent {
                    offset: *o,
                    length: *l,
                })
                .collect();
            return ExtentStream::new(self.reader.clone(), plain, layout.size);
        }
        // Compose: a reader that returns zeros beyond the volume end.
        let zero_base = self.reader.len();
        let composed: Arc<dyn BlockReader> = Arc::new(ZeroTail {
            inner: self.reader.clone(),
            zeros: zero_needed,
        });
        let plain = extents
            .iter()
            .map(|(hole, o, l)| Extent {
                offset: if *hole { zero_base } else { *o },
                length: *l,
            })
            .collect();
        ExtentStream::new(composed, plain, layout.size)
    }

    /// Reads a directory's blocks and parses its entries, live and slack.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] on read failures.
    pub fn read_directory(&self, layout: &Layout) -> Result<Vec<DirEntry>, ExtError> {
        let bs = u64::from(self.sb.block_size);
        let filetype = self.sb.has_incompat(incompat::FILETYPE);
        let indexed = layout.inode.flags & iflags::INDEX != 0;
        let mut out = Vec::new();
        if let Some(inline) = &layout.inline {
            // Inline directories: 4-byte parent, then entries.
            let body = inline.get(4..).unwrap_or(&[]);
            out.extend(parse_block(body, 0, filetype, false, self.sb.inodes_count));
            return Ok(out);
        }
        let mut read = 0u64;
        let mut first = true;
        for r in &layout.runs {
            let Some(p) = r.physical else {
                first = false;
                continue;
            };
            for i in 0..r.count {
                if read >= layout.size.min(MAX_DIRECTORY_BYTES) {
                    return Ok(out);
                }
                let block_no = p.saturating_add(i);
                let off = self.sb.block_offset(block_no)?;
                let block = self
                    .reader
                    .read_vec(off, usize::try_from(bs).map_err(|_| ExtError::Overflow)?)?;
                let current = parse_block(
                    &block,
                    off,
                    filetype,
                    indexed && first,
                    self.sb.inodes_count,
                );
                out.extend(self.merge_directory_versions(
                    block_no,
                    off,
                    filetype,
                    indexed && first,
                    current,
                ));
                first = false;
                read += bs;
            }
        }
        Ok(out)
    }

    /// Joins the current entries of directory block `block_no` with those
    /// found in its journal copies: entries only an older copy still holds
    /// are added as deleted, and every entry learns the range of
    /// transactions in which it was live.
    fn merge_directory_versions(
        &self,
        block_no: u64,
        off: u64,
        filetype: bool,
        indexed_root: bool,
        mut current: Vec<DirEntry>,
    ) -> Vec<DirEntry> {
        let Some(journal) = self.journal() else {
            return current;
        };
        let copies = journal.copies(block_no);
        if copies.is_empty() {
            return current;
        }
        let mut seen: HashSet<(u32, String)> =
            current.iter().map(|e| (e.inode, e.name.clone())).collect();
        let mut alive: HashMap<(u32, String), (u32, u32)> = HashMap::new();
        let mut extra = Vec::new();
        for copy in copies {
            let Ok(data) = journal.read_copy(copy) else {
                continue;
            };
            for mut entry in parse_block(&data, off, filetype, indexed_root, self.sb.inodes_count) {
                if entry.is_dot() {
                    continue;
                }
                let key = (entry.inode, entry.name.clone());
                if !entry.deleted {
                    alive
                        .entry(key.clone())
                        .and_modify(|(a, b)| {
                            *a = (*a).min(copy.sequence);
                            *b = (*b).max(copy.sequence);
                        })
                        .or_insert((copy.sequence, copy.sequence));
                }
                if seen.insert(key) {
                    entry.deleted = true;
                    extra.push(entry);
                }
            }
        }
        current.extend(extra);
        for entry in &mut current {
            entry.alive_in = alive.get(&(entry.inode, entry.name.clone())).copied();
        }
        current
    }

    /// Walks the tree from the root, including deleted entries found in
    /// directory slack and the contents of deleted directories whose
    /// layout can still be obtained through `resolve_layout`.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] if the root directory cannot be read.
    pub fn walk(
        &self,
        resolve_layout: &dyn Fn(&Inode) -> Option<Layout>,
    ) -> Result<Vec<WalkedEntry>, ExtError> {
        let root = self.inode(ROOT_INODE)?;
        let root_layout = self.layout_of(&root)?;
        let mut out = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        visited.insert(ROOT_INODE);
        let mut stack: Vec<(String, Layout, u32, bool, usize)> =
            vec![(String::new(), root_layout, ROOT_INODE, false, 0)];
        while let Some((prefix, layout, parent, via_deleted, depth)) = stack.pop() {
            let entries = match self.read_directory(&layout) {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!(parent, error = %e, "directory unreadable");
                    continue;
                }
            };
            for entry in entries {
                if entry.is_dot() {
                    continue;
                }
                let path = format!("{prefix}/{}", entry.name);
                let deleted_here = via_deleted || entry.deleted;
                let is_dir = entry.is_directory()
                    || (entry.file_type.is_none()
                        && entry.inode != 0
                        && self
                            .inode(entry.inode)
                            .is_ok_and(|i| i.kind() == InodeKind::Directory));
                if is_dir && depth < MAX_DEPTH && entry.inode != 0 && visited.insert(entry.inode) {
                    if let Ok(child) = self.inode(entry.inode) {
                        let child_layout =
                            if child.kind() == InodeKind::Directory && !child.is_deleted() {
                                self.layout_of(&child).ok()
                            } else if child.kind() == InodeKind::Directory || child.is_deleted() {
                                resolve_layout(&child)
                                    .filter(|l| l.inode.kind() == InodeKind::Directory)
                            } else {
                                None
                            };
                        if let Some(l) = child_layout {
                            stack.push((path.clone(), l, entry.inode, deleted_here, depth + 1));
                        }
                    }
                }
                out.push(WalkedEntry {
                    path,
                    entry,
                    parent,
                    via_deleted_directory: via_deleted,
                });
            }
        }
        Ok(out)
    }

    /// The journal, parsed on first use (None without a journal).
    #[must_use]
    pub fn journal(&self) -> Option<&Journal> {
        self.journal
            .get_or_init(|| {
                let inum = if self.sb.journal_inum != 0 {
                    self.sb.journal_inum
                } else {
                    JOURNAL_INODE
                };
                if !self.sb.has_compat(crate::superblock::compat::HAS_JOURNAL) {
                    return None;
                }
                let inode = self.inode(inum).ok()?;
                let layout = self.layout_of(&inode).ok()?;
                let extents = extents_from_runs(&layout.runs, u64::from(self.sb.block_size));
                let len: u64 = extents.iter().map(|e| e.length).sum();
                let stream = ExtentStream::new(self.reader.clone(), extents, len.min(layout.size));
                match Journal::parse(stream, &self.sb.uuid) {
                    Ok(j) => {
                        tracing::info!(
                            descriptors = j.info().descriptors,
                            logged = j.info().logged_blocks,
                            "journal parsed"
                        );
                        Some(j)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "journal unusable");
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Every journal copy of `inode`, newest transaction first, paired
    /// with the inode as it was in that copy. Copies whose block cannot be
    /// read or parsed are skipped.
    #[must_use]
    pub fn journal_versions(&self, inode: u32) -> Vec<(LoggedBlock, Inode)> {
        let Some(journal) = self.journal() else {
            return Vec::new();
        };
        let Ok((_, block)) = self.inode_location(inode) else {
            return Vec::new();
        };
        journal
            .copies(block)
            .iter()
            .filter_map(|copy| {
                let data = journal.read_copy(copy).ok()?;
                let old = self.inode_from_block(inode, &data)?;
                Some((*copy, old))
            })
            .collect()
    }

    /// The most recent journal copy of `inode` that still describes data
    /// (alive, with a size and a layout) and satisfies `accept`, if any.
    #[must_use]
    pub fn journal_layout_where(
        &self,
        inode: u32,
        accept: &dyn Fn(&Inode) -> bool,
    ) -> Option<Layout> {
        for (copy, old) in self.journal_versions(inode) {
            if old.dtime != 0 || old.links == 0 || old.mode == 0 || !old.has_layout() {
                continue;
            }
            if old.size == 0 && old.kind() != InodeKind::Directory {
                continue;
            }
            if !accept(&old) {
                continue;
            }
            let Ok(mut layout) = self.layout_of(&old) else {
                continue;
            };
            layout.source = LayoutSource::Journal {
                sequence: copy.sequence,
                checksum_ok: copy.checksum_ok,
            };
            return Some(layout);
        }
        None
    }

    /// The most recent journal copy of `inode` that still describes data
    /// (alive, with a size and a layout), if any.
    #[must_use]
    pub fn journal_layout(&self, inode: u32) -> Option<Layout> {
        self.journal_layout_where(inode, &|_| true)
    }
}

/// A reader that serves zeros for the range past its inner reader's end.
struct ZeroTail {
    inner: Arc<dyn BlockReader>,
    zeros: u64,
}

impl std::fmt::Debug for ZeroTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZeroTail")
            .field("zeros", &self.zeros)
            .finish()
    }
}

impl BlockReader for ZeroTail {
    fn id(&self) -> phoinix_core::SourceId {
        self.inner.id()
    }

    fn len(&self) -> u64 {
        self.inner.len().saturating_add(self.zeros)
    }

    fn geometry(&self) -> &phoinix_block::BlockGeometry {
        self.inner.geometry()
    }

    fn describe(&self) -> String {
        format!("{} (+zero tail)", self.inner.describe())
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, phoinix_block::BlockError> {
        let inner_len = self.inner.len();
        if offset >= inner_len {
            let available = self.len().saturating_sub(offset);
            let n = usize::try_from(available.min(buffer.len() as u64)).unwrap_or(buffer.len());
            if let Some(b) = buffer.get_mut(..n) {
                b.fill(0);
            }
            return Ok(n);
        }
        let n =
            usize::try_from((inner_len - offset).min(buffer.len() as u64)).unwrap_or(buffer.len());
        let Some(head) = buffer.get_mut(..n) else {
            return Ok(0);
        };
        self.inner.read_at(offset, head)
    }
}
