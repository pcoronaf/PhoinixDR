# ADR-0007 — No source writes in the recovery core

**Status:** accepted · **Date:** 2026-09

## Context

Any write to the source can destroy the data being recovered. Users under
stress make mistakes; software must not amplify them.

## Decision

No crate in the recovery core exposes a source write. Devices are opened with
read-only access and, on Windows, without `GENERIC_WRITE`. The recovery writer
targets a destination path and refuses, by default, destinations that resolve
to the same physical device as the source.

## Consequences

- Automatic partition-table repair is out of scope for the core; a future
  capability would be a separately privileged, explicitly invoked component.
- Tests assert that fixture images are byte-identical before and after every
  scan and recovery.
