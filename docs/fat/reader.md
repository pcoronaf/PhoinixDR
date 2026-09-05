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
contiguous span. On FAT32 volumes with more than 65535 clusters a zero
first-cluster high word is flagged, because some drivers clear it on
deletion.

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
