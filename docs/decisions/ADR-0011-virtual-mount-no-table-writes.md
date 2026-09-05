# ADR-0011: Lost partitions are mounted virtually; the partition table is never written

## Status

Accepted (M9).

## Context

Traditional partition-recovery tools rewrite the partition table so that
the operating system mounts the volume again. That is the most
destructive step of a recovery: a wrong boundary or a wrong scheme can
turn a recoverable disk into a damaged one, and it happens before the user
has seen a single file.

## Decision

1. The structure search produces candidates with boundaries and evidence;
   it changes nothing.
2. A candidate is opened as a read-only view of its byte range
   (`SubrangeReader`). A destroyed primary structure is compensated with an
   in-memory overlay of its backup (`PatchedReader`), recorded on the
   candidate as a *repair* and persisted in sessions, so the same virtual
   mount is reproduced later for recovery and previews.
3. The filesystem engines and the carver work on the mounted candidate
   exactly as on a listed partition: browse, assess, recover.
4. The CLI addresses candidates by search index (`--lost N`) or by byte
   range (`--at`, `--length`); the desktop lists them in the scan setup.
5. Writing a repaired partition table remains out of scope. If it is ever
   added, it will be a separate, explicit, opt-in command with a dry run.

## Consequences

- Recovery never depends on the operating system mounting anything.
- Sessions of lost volumes are self-contained: offset, length and repairs
  are in the file.
- The whole source is read once per search; `--at` avoids repeating it.
