# PhoinixDR architecture overview

```text
┌─────────────────────────────────────────────────────────────┐
│                    PhoinixDR Desktop (future)                 │
│                    Tauri 2 + React/TS                       │
└───────────────────────────┬─────────────────────────────────┘
                            │ typed local IPC
┌───────────────────────────▼─────────────────────────────────┐
│                     phoinixd (future)                       │
└───────────────────────────┬─────────────────────────────────┘
                            │
┌───────────────────────────▼─────────────────────────────────┐
│                       PhoinixDR Core                          │
│  phoinix-core      identifiers, ranges, checked arithmetic  │
│  phoinix-block     read-only BlockReader, RAW, subranges    │
│  phoinix-device    device enumeration, read-only access     │
│  phoinix-volume    MBR / EBR / GPT, partition views         │
│  phoinix-fs        probes, recovery candidates, providers   │
│  phoinix-fs-ntfs   native NTFS reader + undelete            │
│  phoinix-fs-fat    native FAT12/16/32 reader + undelete     │
│  phoinix-fs-exfat  native exFAT reader + undelete           │
│  phoinix-health    evidence, scoring, explanations          │
│  phoinix-carve     deep scan: signature carving, assembly   │
│  phoinix-recovery  recovery writer, safety, SHA-256         │
└─────────────────────────────────────────────────────────────┘
```

## Crate dependency direction

```text
phoinix-core
   ▲
phoinix-block ◄── phoinix-device
   ▲
phoinix-volume        phoinix-health
   ▲                       ▲
phoinix-fs ◄───────────────┘
   ▲
phoinix-fs-ntfs / phoinix-fs-fat / phoinix-fs-exfat
phoinix-carve (phoinix-fs contracts + phoinix-health only)
   ▲
phoinix-recovery (via phoinix-fs contracts only)
   ▲
phoinix-cli
```

Generic crates never depend on filesystem-specific crates. `phoinix-recovery`
reads candidate content through the `DeletedFileProvider` contract in
`phoinix-fs`, so it works with any filesystem engine.

## Data flow of the first vertical slice

```text
RAW or physical NTFS source          phoinix-block / phoinix-device
        ↓
identify partition table             phoinix-volume
        ↓
locate NTFS volume                   phoinix-fs probes (NtfsProbe)
        ↓
parse $MFT                           phoinix-fs-ntfs
        ↓
identify deleted FILE records
recover filename and parent
resolve resident / non-resident data
resolve runlist, query $Bitmap
        ↓
build RecoveryEvidence               phoinix-fs-ntfs → phoinix-health
        ↓
calculate RecoveryHealth             phoinix-health
        ↓
recover to destination, SHA-256      phoinix-recovery
deep scan (carving of free space)    phoinix-carve, over AllocationView of the engine
```

## Policies

- [Read-only sources](../decisions/ADR-0002-read-only-blockreader.md)
- [Synchronous I/O](../decisions/ADR-0003-synchronous-filesystem-io.md)
- Typed errors (`thiserror`) in libraries; `anyhow` only in applications.
- Checked arithmetic for every media-derived value (`phoinix_core::arith`).
- Bounds-checked byte access (`phoinix_core::bytes`).
- `#![forbid(unsafe_code)]` except in `phoinix-device` platform modules.
- Structured `tracing`: INFO for process events, DEBUG may include filenames,
  TRACE for sector-level diagnostics. Recovered content is never logged.
