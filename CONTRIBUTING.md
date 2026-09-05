# Contributing to PhoinixDR

Thank you for helping build an open, evidence-driven recovery platform.

## Ground rules

1. **Source media is read-only.** Do not add any API that can write to a
   `BlockReader` source. Partition-table repair, if it ever exists, lives in a
   separately privileged component.
2. **Typed errors in libraries.** Library crates use `thiserror`; `anyhow` is
   allowed only at application boundaries (`apps/`).
3. **No panics on media-controlled values.** Do not use `unwrap`, `expect`,
   `unreachable!` or direct slice indexing on values read from disk. Use the
   bounds-checked readers in `phoinix_core::bytes` and checked arithmetic in
   `phoinix_core::arith`. Test code may opt out with
   `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::cast_possible_truncation)]`.
4. **No `unsafe` outside platform crates.** If unsafe is unavoidable, document
   why, the assumptions, ownership, bounds and lifetimes next to the block.
5. **Filesystem knowledge stays in filesystem crates.** Generic crates must
   not learn NTFS/FAT/EXT specifics.
6. **Recovery likelihood and assessment confidence are different numbers.**
   Never conflate them.
7. **Conservative dependencies.** Prefer the standard library. New
   dependencies need a short justification in the pull request.

## Before you push

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

CI runs the same commands on Linux and Windows.

## Architectural decisions

Significant decisions are recorded in `docs/decisions/` as ADRs. If you want to
reopen one, open an issue that references the ADR number and explains what has
changed since it was written.

## Test fixtures

Disk-image fixtures live compressed under `tests/fixtures/` together with a
`manifest.json` containing ground truth (file names, sizes, SHA-256 digests,
fragmentation and deletion state). Fixtures are produced by the scripts in
`tests/generated/`; never edit a fixture by hand — change the generator and
regenerate.

## Commit style

Use clear, imperative commit subjects prefixed with the affected area when it
helps, e.g. `ntfs: reject runlists with zero-length runs`.
