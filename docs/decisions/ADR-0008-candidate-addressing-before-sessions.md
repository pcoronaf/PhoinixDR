# ADR-0008 — Candidate addressing before a session database exists

**Status:** accepted · **Date:** 2026-09

## Context

The CLI needs `explain <candidate>` and `recover <candidate>` before the
SQLite session database (a later milestone) exists. `CandidateId` values are
random UUIDs generated per scan, so they cannot be typed back in.

## Decision

Until sessions exist, CLI commands address a candidate by source plus
filesystem object: `phoinix explain <source> <mft-record>` and
`phoinix recover <source> <mft-record> --output <dir>`. The commands rebuild
the candidate deterministically from the source. `--json` output still carries
the `CandidateId` for consumers that hold a scan result.

## Consequences

- No hidden state between commands; every command is reproducible.
- When the session database lands, session-relative addressing is added
  alongside, and this record is superseded.
