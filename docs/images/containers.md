# Image containers, hash verification and reports (M11)

`phoinix-image` opens forensic and virtual-disk image files as read-only
block sources. A container reader implements `BlockReader`, so partition
discovery, every filesystem engine, the carver and the partition search
work on an E01 exactly as on a RAW image or a device. Nothing here needs a
native library (ADR-0013).

## Formats

| format | files | reader | what is read |
|---|---|---|---|
| RAW / dd | one file | `RawImage` | as is |
| split RAW | `disk.001`, `disk.002` … (or `.000` first, or `disk.aa`, `disk.ab` …) | `SplitRawImage` | siblings found by name, concatenated |
| EWF-E01 (EnCase 5/6/7, FTK), SMART (`.s01`) | `case.E01` … `E99`, `EAA` … `ZZZ` | `EwfImage` | section chains, chunk tables, zlib chunks with adler-32 checks, `header`/`header2` acquisition text, `hash`/`digest` sections, `error2` count |
| VHD | one file | `VhdImage` | footer, fixed data or dynamic BAT with sector bitmaps |
| VHDX | one file | `VhdxImage` | CRC-32C headers, region table, metadata items, 64-bit BAT |
| VMDK | descriptor and extents | `VmdkImage` | sparse (grain directory/tables), flat, ZERO extents, stream-optimized deflate grains, 2 GiB split extents, embedded or standalone descriptor |

Detection reads the content (EWF/VHDX/VMDK signatures at the start, the
VHD footer at the end), never the extension alone; a name that merely ends
in `.E01` but holds RAW bytes opens as RAW. Any segment of a multi-file
image can be given: the sequence is walked from its first file.

### Not read

- EWF2 (`Ex01`, `Lx01`) and logical evidence (`L01`) files: refused with a
  message naming the format.
- Differencing VHD/VHDX disks and VMDK snapshot chains: refused; merge the
  chain with the hypervisor's tools first.
- The VHDX log is not replayed. An image closed uncleanly carries a
  diagnostic saying that recent writes may be missing.
- AFF4 and QCOW2.

## Caching and memory

Each reader keeps a small least-recently-used cache of decoded units
(16–32 MiB) so sequential scans decompress every chunk once. Reads never
allocate more than one unit plus the request; tables and directories are
bounded (16 M chunk entries, 64 M BAT entries) so a malformed header cannot
drive an allocation.

## Container information

`open_image` returns a `ContainerInfo` beside the reader: format and
variant, the segment files, media size and sector size, chunk/block size,
compression, identifier, media type, the stored MD5/SHA-1, the acquisition
header (case number, evidence number, description, examiner, notes,
acquisition and system dates, software, operating system, model, serial)
and diagnostics (bad section checksums, wrong segment numbers, a missing
last segment, unflushed logs). `phoinix inspect` prints it as an
**Image container** section and includes it in `--json`; the desktop
shows it on the scan-setup page.

## Hash verification

`phoinix verify <image>` (and the desktop's *Verify hashes* button, the
service layer's `Workspace::verify_source`) reads the whole media through
the container and computes MD5, SHA-1 and SHA-256. When the container
stores hashes they are compared; a mismatch makes the command exit with an
error. A container without stored hashes (VHD, VMDK, RAW) still yields the
computed hashes, which document the source as it was read.

Corrupt EWF data surfaces as read errors (a chunk that fails to inflate or
comes back with the wrong length) or as checksum diagnostics for
uncompressed chunks; it is never returned silently as valid data.

## Case metadata and reports

`phoinix recover … --report <path>` writes a recovery report after the
files. The extension selects the rendering: `.json` (the report
structure), `.md` (Markdown) or `.html` (a self-contained page). The
report records:

- the tool and version, the generation time;
- the case: `--case-number`, `--evidence-number`, `--examiner`,
  `--case-notes`; fields not given are taken from the image's acquisition
  header, so an E01 acquired under a case number reports that number;
- the source: path, size, kind, the container description, the stored
  hashes, and (with `--verify-source`) the computed hashes and whether
  they match;
- the volume: filesystem, offset, length, partition;
- every requested file: reference, name, original path, size, health at
  recovery time, how it was found, where it was written, bytes written,
  SHA-256, completeness, or the failure;
- totals.

The desktop's recovery dialog has the same case fields (prefilled from the
acquisition header) and a report chooser. The service layer emits
`RecoverEvent::Verifying` while hashing and reports the written path in
`RecoverEvent::Finished`.

## Tests

`tests/generated/make_image_fixtures.sh` wraps the FAT12 undelete corpus
in every container (ewfacquire for E01 best-compressed, uncompressed split
into five 1 MiB segments, and SMART; qemu-img for VHD dynamic/fixed, VHDX,
VMDK sparse/stream-optimized/2 GiB extents; `split` for RAW pieces).
`tests/integration/tests/images.rs` opens each, compares the whole media
and 200 random unaligned reads with the RAW bytes, verifies the stored
hashes and the acquisition header, recovers a JPEG through five
containers with the FAT engine, refuses Ex01 and differencing disks,
survives 60 corruption rounds of an E01 without panicking or passing
damaged data as verified, and writes HTML and JSON reports through the
service layer. `apps/phoinix-cli/tests/cli.rs` covers `inspect`,
`verify` (including the non-zero exit on damage), `scan` and
`recover --report --verify-source` on the same images.
