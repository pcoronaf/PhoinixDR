# ADR-0009: Carving scans free space by default and folds into metadata candidates

## Status

Accepted (M8).

## Context

Signature carving can find files that no metadata describes, but it also
finds every file the filesystem still describes, every embedded object,
and every stale copy. A naive deep scan floods the result table with
duplicates and offers unnamed carved copies next to named metadata
candidates that recover the same bytes better.

## Decision

1. **Scope.** Deep scan carves the *unallocated* ranges reported by the
   filesystem engine (`AllocationView`). The whole volume is scanned only
   on request (`--carve-all`) or when no supported filesystem exists.
2. **Alignment.** Only sector-aligned (512) positions are tested by
   default. Files start on cluster boundaries; unaligned matching is an
   option for embedded objects.
3. **Nested hits** inside a sound assembly are skipped.
4. **Deduplication.** A carved hit whose start equals the first extent of
   a metadata candidate is folded into that candidate as a diagnostic. The
   metadata candidate is always the survivor: it has the name, path and
   timestamps, and its allocation evidence is at least as good.
5. **Same contract.** The carving engine implements `DeletedFileProvider`
   and `AllocationView`-based evidence, so scan, explain and recover treat
   carved candidates like any other; references are `c<offset>` and are
   re-derived deterministically (ADR-0008).
6. **No new runtime dependency.** Parallel matching uses scoped standard
   threads; I/O stays sequential.

## Consequences

- Deep scan on a healthy volume produces few new rows: mostly orphans.
- A carved file that outscores its metadata twin is not shown separately;
  its assessment is recorded on the metadata candidate instead.
- Whole-volume carving reports live files with reused-cluster caps, which
  is correct evidence (the clusters are allocated) even though the content
  is intact; the filesystem is the right tool for allocated files.
