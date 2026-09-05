# NTFS undelete corpus

`tests/generated/make_ntfs_undelete_corpus.py` builds
`tests/fixtures/ntfs/undelete.img.gz` with mkntfs and ntfs-3g and records the
ground truth in `undelete.manifest.json`. Each entry carries the original
path, size, SHA-256, MFT record number and expectations.

| scenario | content | expectation |
|---|---|---|
| A | resident files (20 B, ~100 B JSON, 400 B binary, Unicode name, empty file) | exact, Excellent |
| B | contiguous 64 KiB and 1 MiB | exact, ≥ Very good, one extent |
| C | 2- and 10-extent fragmented files | exact, ≥ Very good, fragmentation reported |
| D | 1/10/25/50/100 % of clusters marked allocated in `$Bitmap` and overwritten | allocation counts match, likelihood declines monotonically, wording never says "overwritten" |
| E | file whose directory records were reused by new directories | path uncertain (`\?\document.txt`), data still exact |
| F | deleted records with corrupted USA, attribute length, runlist, name length | typed diagnostics, scan continues; a lost runlist is Unrecoverable |
| G | JPEG and raw binary whose clusters were zeroed after deletion (TRIM simulation) | `$Bitmap` still free; the JPEG contradicts its format (≤ Very poor), the raw file is ambiguous (likelihood kept, confidence ≤ 65) |
| Z | file that legitimately consists of zeros | exact, ambiguous zero assessment, confidence ≤ 65 |
| H | file inside a deleted (not reused) directory | exact, path recovered through the deleted directory |
| V | real JPEG, PNG, PDF, DOCX | exact, structure validates, Excellent |

The integration test `tests/integration/tests/ntfs_undelete.rs` asserts every
row and additionally checks that the image is never written and that a
recovery destination equal to the image is refused.


# FAT and exFAT corpora

`tests/generated/make_fat_undelete_corpus.py` builds FAT12, FAT16 and FAT32
images with mtools (no mounting), and
`tests/generated/make_exfat_undelete_corpus.sh` builds an exFAT image with
mkfs.exfat and exfat-fuse over a loop device. Each has a manifest with paths,
sizes, SHA-256 digests and expectations:

| scenario | content | expectation |
|---|---|---|
| A | small and 200 KB contiguous files | exact under the contiguous assumption |
| C | file written into holes between fillers | FAT: heuristic reconstruction, exact but capped; exFAT: FAT chain intact, ≥ 2 extents |
| D | file whose clusters were reused by a new file | reallocated clusters reported, ≤ Very poor, not exact |
| E | empty file | Excellent, validation not applicable |
| H | file inside a deleted directory | exact, path through the deleted directory |
| L | long name with spaces and Unicode | name reconstructed from deleted LFN / name entries |
| V | JPEG, PDF, DOCX | validators pass, exact |
| W (`fat32w` only) | JPEG, PDF, DOCX, TXT deleted the way Windows does on a large FAT32 volume: FAT chain cleared **and** first-cluster high word zeroed, low word pointing into 36 MiB of older files | start inferred, exact; Good for validated types, Poor with confidence ≤ 60 for the unknown type |

The `fat32w` image (40 MiB, 512-byte clusters, about 80 000 clusters) is the
regression fixture for the 0 % / 0-byte results first seen on a 16 GB USB
stick.

`tests/integration/tests/fat_exfat_undelete.rs` asserts every row for all
five images and runs 240 corruption rounds without panics.


# ext2/3/4 corpora

`tests/generated/make_ext_undelete_corpus.sh` builds an ext4 image (4 KiB
blocks, extents, checksummed 64-bit journal tags), an ext3 image (4 KiB
blocks, block maps, legacy journal tags) and an ext2 image (1 KiB blocks,
six block groups, no journal) with mke2fs and the kernel drivers over loop
devices. The manifests record each file's inode number, size, SHA-256 and
extent count (`filefrag`) *before* deletion, so the tests assert exactly
what the journal still yields:

| scenario | content | ext3/ext4 expectation | ext2 expectation |
|---|---|---|---|
| A, B | 700 B and 200 KB contiguous files | exact, ≥ Very good, layout from the journal | found by inode, no name, no size, Unrecoverable |
| E | empty file | Excellent, validation not applicable (the journal shows it was empty) | as above |
| L | long name with spaces and Unicode | name and path from directory slack | — |
| V | JPEG | validator passes, exact | as above |
| H | file inside a directory removed with `rm -r` | exact; the directory's layout comes from the journal, its entries from its (still readable) block | as above |
| D | 1 MiB file whose blocks **and inode** were reused by a new file | reallocated blocks reported, ≤ Very poor, not exact; the name is chosen by the transaction in which it was live | absent (the inode is alive again) |
| C | file written into holes between fillers | exact, extent count as recorded | as above |
| S | sparse file (head, 290 KB hole, tail) | exact, sparse reported, holes read as zeros | as above |
| J | file grown after its first commit | exact from the newest journal copy | as above |
| absent | file renamed while alive | never a candidate | — |

The ext2 rows document what the kernel's ext2 driver leaves behind: it
clears the size, the block map and even the directory entries on deletion,
so without a journal PhoinixDR reports deleted inodes with their deletion
time only and leaves the content to carving (`scan --deep`).

`tests/integration/tests/ext_undelete.rs` asserts every row for the three
images, checks journal tag checksums, the allocation view against the
superblock's free count, and runs 120 corruption rounds without panics.
