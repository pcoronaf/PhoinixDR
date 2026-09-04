# ADR-0001 — Rust as core language

**Status:** accepted · **Date:** 2026-09

## Context

PHOINIX parses attacker-controlled binary structures from arbitrary media,
must run natively on Windows and Linux, and needs predictable performance on
multi-terabyte sources.

## Decision

The recovery engine, CLI and privileged service are written in stable Rust
(edition 2024). Filesystem parsers prefer explicit parsing over heavy
abstraction. Async runtimes are not used in parsing crates.

## Consequences

- Memory and bounds safety by default; `unsafe` is confined to platform/FFI
  crates and must be documented.
- C libraries (TSK, libewf) are reachable through FFI adapters when needed.
- Contributors need Rust; the desktop front-end (Tauri + React) speaks to the
  engine over typed IPC rather than sharing a language.
