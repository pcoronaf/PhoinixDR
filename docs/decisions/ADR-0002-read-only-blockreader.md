# ADR-0002 — Read-only `BlockReader` abstraction

**Status:** accepted · **Date:** 2026-09

## Context

The source device is evidence. Every recovery method (undelete, carving,
partition reconstruction) needs uniform random access to physical disks,
partitions, RAW images and forensic containers.

## Decision

All sources implement `phoinix_block::BlockReader`:

```rust
pub trait BlockReader: Send + Sync {
    fn id(&self) -> SourceId;
    fn len(&self) -> u64;
    fn geometry(&self) -> &BlockGeometry;
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError>;
}
```

There is intentionally no `write_at`. Partitions are exposed as
`SubrangeReader` views over a parent reader so that filesystem code never sees
the whole disk.

## Consequences

- Filesystem engines behave identically on `/dev/sdb`, `\\.\PhysicalDrive2`,
  `disk.dd` or a virtually mounted lost partition.
- Reads are bounded: a request beyond the end of the source is an error, not a
  short read, and single requests are capped by `MAX_SINGLE_READ`.
- Any future partition-table write capability must live in a separate,
  explicitly privileged component (see ADR-0007).
