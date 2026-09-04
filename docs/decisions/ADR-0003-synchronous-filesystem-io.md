# ADR-0003 — Synchronous filesystem I/O

**Status:** accepted · **Date:** 2026-09

## Context

Filesystem parsers perform deterministic random reads that depend on previous
results (boot sector → MFT → runlist → clusters). Async I/O at that layer adds
complexity without improving throughput for dependent reads.

## Decision

`BlockReader::read_at` is synchronous and positional. Parallelism belongs
above the abstraction: a scan coordinator may run filesystem, validator and
carving workers concurrently, each using synchronous reads.

## Consequences

- Filesystem code is deterministic and easy to test with in-memory readers.
- Implementations must be safe for concurrent callers: positional reads must
  not share a mutable seek cursor.
- Tokio, if adopted for orchestration, stays out of parsing crates.
