# ADR-0013: Image containers are read natively, not through libewf

## Status

Accepted (M11).

## Context

The technical specification proposed reading E01 images through libewf
behind an adapter. libewf is a C library (LGPL-3.0-or-later) whose own
project describes it as experimental; binding it means FFI (`unsafe`), a
C toolchain on every build platform including MSVC, and licence handling
for the desktop bundle. The EWF-E01 format itself is small: a chain of
sections, chunk tables, zlib chunks and adler-32 checksums, all of which
PhoinixDR already has the pieces for (flate2, checked arithmetic, bounded
parsing).

## Decision

1. Every container (EWF-E01/S01, split RAW, VHD, VHDX, VMDK) is read by
   native Rust code in `phoinix-image`, behind `BlockReader`, with
   `#![forbid(unsafe_code)]`. The crate depends on `flate2`, `md-5`,
   `sha1` and `sha2` only.
2. Formats are detected from content, never from the extension alone.
3. Unsupported variants (EWF2 `Ex01`, differencing and snapshot chains)
   are refused with a message naming the feature; partial support is
   never silently degraded to wrong bytes.
4. Corrupt container data is reported as a read error or a diagnostic,
   never returned as valid data. Stored hashes are verified on demand,
   not on open.
5. Should a format prove too large to maintain natively (EWF2 with its
   encryption and case data), a library adapter is still an option under
   ADR-0005, behind the same `BlockReader` contract.

## Consequences

- One toolchain, one licence, cross-platform builds unchanged.
- Fixtures are produced by the reference tools (ewfacquire, qemu-img) so
  the native readers are tested against real writers, not against
  themselves.
- Ex01 support is deferred; users get a clear refusal instead of a wrong
  result.
