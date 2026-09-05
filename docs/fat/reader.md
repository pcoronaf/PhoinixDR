# FAT and exFAT engine notes

## FAT12/16/32 (`phoinix-fs-fat`)

- The variant is decided from the data-cluster count (FAT12 < 4085,
  FAT16 < 65525, otherwise FAT32), never from the type label in the BPB.
- The active FAT is loaded into memory (up to 256 MiB) and compared with its
  mirror; `mirror_consistent` is reported.
- Directory entries are parsed with long-name assembly and checksum
  verification. A deleted short entry (`0xE5`) has lost its first character
  (shown as `?`); its deleted LFN entries are still assembled but flagged
  `long_name_unverified` because the checksum covered the lost byte.
- Deleted directories are walked through their first cluster only (the chain
  is gone); files found there report "via deleted directory".

### Reconstruction of deleted files

```text
first cluster + size
        │
        ├── FAT chain still intact (driver did not clear it)
        │       → chain_known, exact
        │
        └── chain gone
                ├── every cluster in first..first+n free   → contiguous assumption
                │                                             (chain_known = false, confidence −10)
                └── some clusters allocated to other files → heuristic: skip them
                                                              (heuristic = true, cap 59, confidence −30)
```

Allocation evidence counts free vs allocated FAT entries over the assumed
contiguous span. When the contiguous search skips 65 536 allocated clusters
without locating anything it gives up (`search_exhausted`), and the
candidate says so: the recorded start is probably wrong.

### Windows deletions on large FAT32 volumes

The Windows FAT32 driver zeroes the high 16 bits of the first cluster when
it deletes a file. On volumes with more than 65 536 clusters (any USB stick
above a few gigabytes) the surviving low word points into the first part of
the volume, which is exactly where the older files live, so a plain
contiguous reconstruction locates nothing and reports 0 %.

For a deleted FAT32 entry whose high word is zero on such a volume, the
engine infers the start (`FatVolume::infer_start`):

```text
candidates = { (high << 16) | low : high in 0..=max_high }, free clusters only
each candidate: read the first 4 KiB, rank the content
    3  carries the signature of the type expected from the name (.pdf → %PDF)
    2  carries some recognisable signature (no type expected)
    1  is not zero-filled                       (weak evidence)
    0  zero-filled, or contradicts the expected type
the recorded cluster keeps ties; among alternatives the highest cluster wins
(new files land after existing data)
```

The choice is reported as `InferredStart { recorded, recorded_allocated,
chosen, candidates, evidence }`, sets `ExtentEvidence::start_inferred`, and
the diagnostic names the recorded cluster, the chosen one, how many free
candidates shared the low word and why the chosen one won. The health
model caps such files at 59 (79 when the reconstructed content validates
completely) and lowers confidence by a further 20; weak evidence is spelled
out as such. The `fat32w` fixture reproduces the situation.

An exFAT stream entry whose first cluster is zero although its length is
not is reported as unlocatable (nothing located, 0 %) instead of as an
empty file.

## exFAT (`phoinix-fs-exfat`)

- The boot region checksum (sectors 0–10 vs sector 11) is verified and used
  as probe evidence.
- Directory entry sets (File + Stream Extension + File Name) are parsed with
  their set checksum; for deleted sets (in-use bit cleared on every entry)
  the checksum is verified with the bits restored, so damage is still
  detectable.
- The allocation bitmap is loaded from its root-directory entry and provides
  per-cluster allocation evidence.
- Files with `NoFatChain` are contiguous by definition, so a deleted
  contiguous file keeps a fully known layout. Files with a FAT chain follow
  the FAT; if the chain was cleared, contiguity is assumed and reported.
