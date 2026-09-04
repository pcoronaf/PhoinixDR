# NTFS reader notes

How `phoinix-fs-ntfs` bootstraps and what it trusts.

## Bootstrap

```text
boot sector (offset 0 of the volume)
    ↓ $MFT LCN, cluster size, record size
FILE record #0 read directly at $MFT LCN
    ↓ (on failure: the copy in $MFTMirr)
update sequence fixup
    ↓
unnamed $DATA runlist of record 0
    ↓ (+ $DATA pieces named in $ATTRIBUTE_LIST extension records)
logical $MFT stream
```

After bootstrap every record is read through the `$MFT` stream, so a
fragmented MFT is transparent.

## Records and attributes

- A record must start with `FILE`; `BAAD` and empty records are typed errors.
- Fixups are verified for every sector; a mismatch is
  `NtfsError::FixupMismatch` and the record is not used. There is no silent
  salvage mode yet.
- Attribute iteration validates each length and stops at the first damaged
  attribute, keeping what was parsed before it (`NtfsDiagnostic::AttributeError`).
- Unknown attribute types are skipped, never fatal.
- `$ATTRIBUTE_LIST` is followed into extension records; each extension must
  reference the base record with a matching sequence number.

## Runlists

LCN deltas are signed and relative to the previous run. The decoder rejects
zero-length runs, invalid field widths, truncated pairs, VCN overflow, LCN
underflow and LCNs beyond the volume. Runs are kept in VCN order; a stream
whose runs leave a VCN uncovered reports `MissingExtent` when read rather
than zero-filling.

## Streams

`file offset → VCN → run → LCN → volume offset`. Sparse runs and bytes beyond
the initialised size read as zero; bytes beyond the logical size are never
exposed. Compressed and encrypted streams are recognised and refused.

## Paths

Paths are reconstructed from `$FILE_NAME.parent`, which also works for
deleted files. The resolver keeps a visited set and a depth limit (1024).
Parent sequence numbers are compared with the reference:

| parent record state | sequence relation | interpretation |
|---|---|---|
| in use | equal | original directory |
| not in use | reference + 1 | directory deleted, name still valid (`ParentDeleted`) |
| any other | differs | record reused: path uncertain, shown as `\?\name` |
