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
