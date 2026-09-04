# ADR-0005 — Third-party libraries behind adapters

**Status:** accepted · **Date:** 2026-09

## Context

Mature libraries such as The Sleuth Kit and libewf cover formats PHOINIX does
not yet implement, but they carry their own licences, stability caveats and
data models.

## Decision

External libraries are integrated only through adapter crates
(`adapters/phoinix-tsk`, `adapters/phoinix-libewf`, …) that translate library
output into PHOINIX types. Core crates never depend on an adapter, and PHOINIX
must remain functional when an adapter is removed. TestDisk/PhotoRec are
treated as prior art and comparison tools, never linked in.

## Consequences

- Licensing of the PHOINIX-owned crates stays `MIT OR Apache-2.0`.
- A dependency-by-dependency licence review is required before shipping any
  binary that contains an adapter.
- Adapter crates are the only place besides `phoinix-device` where `unsafe`
  FFI is permitted.
