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
