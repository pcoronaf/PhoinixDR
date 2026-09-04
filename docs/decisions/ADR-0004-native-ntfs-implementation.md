# ADR-0004 — Native NTFS implementation

**Status:** accepted · **Date:** 2026-09

## Context

NTFS is the first filesystem PHOINIX must recover from. Undelete needs
evidence (fixup validity, runlist completeness, `$Bitmap` state, stale parent
sequence numbers) that generic filesystem libraries do not expose.

## Decision

`phoinix-fs-ntfs` implements NTFS natively: boot sector, update sequence
fixups, FILE records, attributes, runlists, `$FILE_NAME`,
`$STANDARD_INFORMATION`, resident and non-resident `$DATA`, `$Bitmap` and
parent-based path reconstruction. No TSK, TestDisk or ntfs-3g code is required
in the recovery path.

## Consequences

- PHOINIX controls the evidence model end to end.
- Compression and EFS are detected and reported as unsupported rather than
  silently mis-recovered until implemented.
- External tools remain valuable as comparison references in tests.
