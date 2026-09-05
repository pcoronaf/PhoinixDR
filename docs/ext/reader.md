# ext2/3/4 reader and undelete (M10)

`phoinix-fs-ext` reads ext2, ext3 and ext4 volumes natively over a
`BlockReader`, enumerates deleted files and recovers their layouts through
the jbd2 journal where one exists. Nothing is ever written to the source
(ADR-0002).

## Structures read

| structure | module | notes |
|---|---|---|
| superblock (+1024, magic `EF53`) | `superblock` | 64-bit block counts, feature flags, `metadata_csum` seed, checksum verification, meta_bg descriptor placement |
| group descriptors (32 or 64 bytes) | `group` | block/inode bitmaps, inode table, `INODE_UNINIT`, `itable_unused` |
| inodes (128 or larger) | `inode` | mode, size, links, `dtime`, `crtime`, generation, flags, crc32c checksum with the volume seed |
| extent trees (`F30A`) | `extent` | depth-bounded, cycle-safe walk into logical runs, uninitialized extents flagged |
| block maps (ext2/3) | `blockmap` | direct, indirect, double and triple indirection, holes as explicit runs |
| directories | `dir` | live entries, deleted entries hidden in record slack, htree roots |
| block bitmaps | `bitmap` | lazily cached per group, `BLOCK_UNINIT` groups are free |
| jbd2 journal | `journal` | descriptor, commit and revoke blocks; 8/12/16-byte tags; per-tag crc32c (v2/v3); escaped blocks restored |

Inline data (`INLINE_DATA`) is read from the inode's `i_block` only; the
part of an inline file that lives in the in-inode extended attribute is
not followed yet. Encryption and compression features are not supported.

## What deletion leaves behind

| driver | inode | directory entry |
|---|---|---|
| ext4 (also mounting ext3) | `dtime` and `ctime` set, `mtime` rewritten by the truncate, **size, block count and extent tree cleared**, generation kept | folded into the previous record's `rec_len`; name and inode number survive in slack |
| ext2 | as above, block map cleared | name **and inode number zeroed** |

So the inode table alone yields deleted inodes with their deletion time and
nothing to locate. The journal changes that: jbd2 logs whole metadata
blocks, and the copy of the inode-table block written by the last
transaction that touched the file while it was alive still carries its
size and extent tree. Directory blocks are journaled too, so names that
were removed from a directory or whose slack was overwritten can be read
back from older copies.

## The undelete join

`ExtUndelete` joins three sources per inode number:

```text
inode table ──► deleted inodes (dtime ≠ 0 or links = 0)
journal     ──► older copies of inode-table blocks: size, extents, timestamps
                older copies of directory blocks: names, plus the range of
                transactions in which each entry was live
directories ──► slack entries: name → inode number
```

Rules:

- A deleted inode that still carries its block map (older ext2 drivers) is
  used as is (`LayoutSource::Inode`).
- Otherwise the newest journal copy in which the inode was alive, had a
  layout and **the same generation** is used (`LayoutSource::Journal`).
  The same generation means the same file: ext4 bumps the generation when
  an inode number is reused.
- A live inode named by a slack entry is a candidate only if the journal
  holds a copy with a *different* generation: that earlier file was deleted
  and its number reused (`reused`, path uncertain). A rename leaves a slack
  entry for a live inode with the same generation and is not a deletion.
- A deleted directory's layout comes from the journal too, so the files it
  contained keep their paths (`/gone/keep.txt`).
- When several names refer to one inode number, the entry that was live in
  the transaction the layout came from wins; entries with an unknown
  lifetime come next, older names last.
- A journal copy shows the file was empty when its size is zero and it has
  no layout: such a file is recovered as empty rather than reported as
  unknown.
- If the on-disk inode carries a modification time that is neither the
  journal copy's nor the deletion time, the layout is marked `stale` and
  capped (`docs/recovery/health-model.md`).

Candidates are identified by `FileSystemObjectId::Ext { inode, generation }`
and addressed on the command line by inode number (`phoinix recover img 25`).

## Streams

`ExtVolume::open_layout` serves a file as an `ExtentStream`. Holes (logical
gaps between extents, tails beyond the last extent, zero pointers in a
block map) and uninitialized extents read as zeros through a zero-tail
reader, so sparse files come back byte-exact.

## Allocation view

`ExtUndelete` implements `AllocationView` over the block bitmaps
(first data block onwards, block-sized units), so deep scan carves free
blocks only and folds hits into journal candidates with the same start.

## Limits

- ext2 without a journal: deleted inodes are reported with their deletion
  time only; names, sizes and layouts are gone. Use `scan --deep`.
- A journal that wrapped past the transaction that last wrote the inode
  loses the layout; the candidate then reports that no copy survives.
- External journals, `fast_commit` records and encrypted volumes are not
  read.
