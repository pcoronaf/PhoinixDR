# ADR-0014: Secure erase lives in a separate write-capable crate; the recovery stack stays read-only

## Status

Proposed (M12).

## Context

PhoinixDR's recovery stack is read-only by construction: `BlockReader` has
no write primitive (ADR-0004), partition repairs are virtual (ADR-0011),
and every user-facing text says that nothing is ever written to the
source. A secure-erase feature is the opposite operation. It is wanted
because the same people who recover data also have to dispose of media,
and because PhoinixDR has something no eraser has: an engine that can
verify, after the erase, that nothing is recoverable.

Adding a write primitive to the block layer would silently put every
engine one bug away from writing to evidence. Putting erase in the same
code path as recovery would also blur the two in the interface.

## Decision

1. **A separate crate, `phoinix-erase`, is the only code in the workspace
   that opens a source for writing.** It does not depend on
   `phoinix-block` for its writer; it owns its own device-opening code
   (`O_WRONLY`/`GENERIC_WRITE`, volume lock and dismount on Windows,
   unmount check on Linux) and its own writer type. `BlockReader` keeps no
   write method. The recovery crates do not depend on `phoinix-erase`.
2. **Reading for verification goes through the normal read-only stack.**
   After an erase, `phoinix-erase` asks `phoinix-device` and the engines
   to open the medium read-only and scan it, exactly as a user would.
3. **The desktop application and the command line expose erase as a
   distinct mode**: a separate top-level choice and page in the desktop
   ("Erase", its own colour, never on the results page), and a separate
   `phoinix erase` subcommand that requires an explicit long flag to run
   without an interactive confirmation. The recovery views never show an
   erase action.
4. **Targets that are refused without an override**: the disk that hosts
   the running operating system, the disk the executable runs from, the
   disk that holds the sessions directory, and any disk image file that
   is open as a recovery source.
5. **Honesty about guarantees is part of the design.** Every method is
   labelled with what it achieves on magnetic disks and on flash, and
   the interface links the author's articles on the subject when a user
   chooses a multi-pass method.

## Consequences

- The "nothing is written to the source" claim stays true for every
  recovery operation and is restated as "the recovery engine never
  writes; the erase mode writes only to the target you confirm".
- One more crate with `#![forbid(unsafe_code)]`; the platform device
  code for writing is the largest new surface and needs the same
  fixture-based tests as the readers, run against loop devices and
  image files, never against real media in CI.
- The release pipeline and documentation gain an "Erase" section, and
  the disclaimer is extended to cover irreversible erasure.

## Alternatives rejected

- A `write_at` on `BlockReader` behind a feature flag: the flag would be
  on in the desktop build, which is where the risk lives.
- A separate executable: cleaner isolation, but it breaks the single
  portable executable requirement (REL-001) and doubles the download.
  The crate boundary plus the interface separation gives the isolation
  that matters.
