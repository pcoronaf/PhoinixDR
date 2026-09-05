# ADR-0012: The filesystem journal is a metadata source, matched by generation and transaction

## Status

Accepted (M10).

## Context

Modern ext3/ext4 drivers clear a deleted inode's size and extent tree, so
the classic inode-table undelete finds files with nothing to locate. jbd2
logs whole metadata blocks, and older copies of inode-table and directory
blocks usually survive in the journal until it wraps. Reading them raises
two risks: attributing a copy to the wrong file (inode numbers are reused)
and presenting a stale layout as current.

## Decision

1. Journal copies are read only for layouts and names; the current on-disk
   structures remain the primary source and the journal never overrides a
   live inode.
2. An inode-table copy is attributed to a deleted inode only when its
   generation matches the on-disk inode; for an inode that is alive again,
   only copies with a different generation describe the earlier file.
3. Directory entries record the range of transactions in which they were
   live. When several names refer to one inode number, the name live in
   the transaction the layout came from wins.
4. A candidate built from a journal copy carries
   `CandidateSource::Journal`, a diagnostic with the transaction and its
   checksum status, and is scored like a known layout. A copy that the
   on-disk inode proves out of date is marked `stale` and capped.
5. The same rules apply to any future journaling filesystem (NTFS `$LogFile`
   would follow the same contract).

## Consequences

- ext3/ext4 files deleted after a normal commit are recovered byte-exact
  with their names and paths; ext2 (no journal) yields deletion times only.
- Recovery depends on the journal not having wrapped; the candidate says
  when no copy survives.
- Tests must assert generation and transaction matching explicitly
  (`tests/integration/tests/ext_undelete.rs`, scenario D).
