# PhoinixDR

**Open-source, evidence-driven data recovery.**

PhoinixDR (Phoinix Data Recovery) is a data-recovery engine and (future) desktop application that
reconstructs lost data from filesystems, raw media and damaged storage
structures while explaining how likely each recovered object is to be intact.

The fundamental workflow is:

```text
Select source → Scan → Find lost data → Assess recoverability → Preview → Recover
```

> **Status:** early engineering preview. The repository implements milestones
> M0–M8 of the technical specification: the read-only block layer, MBR/GPT
> discovery, native NTFS, FAT12/16/32 and exFAT readers with undelete, deep
> scan (signature carving of unallocated space), evidence-based recovery
> health, a verified recovery writer, the `phoinix` CLI and a desktop
> application (Tauri 2 + React) with sessions, previews and recovery.

## Principles

- **Read-only by default.** The block abstraction has no write primitive. No
  crate in the recovery core can modify source media.
- **Recovery methods stay independent.** Undelete, carving, filesystem
  reconstruction and partition reconstruction produce different evidence and
  different confidence.
- **Filesystem knowledge lives in filesystem crates.** NTFS rules never leak
  into generic recovery logic.
- **Every score is explainable.** Recovery likelihood and assessment
  confidence are separate numbers, and each is traceable to concrete evidence.
- **The core is independent of any GUI.** The same engine drives the CLI,
  tests, and future desktop and service front-ends.
- **Third-party libraries sit behind adapters.**

See [`docs/architecture/overview.md`](docs/architecture/overview.md), the
[architectural decision records](docs/decisions/), the
[NTFS reader notes](docs/ntfs/reader.md), the
[FAT/exFAT engine notes](docs/fat/reader.md), the
[deep scan / carving notes](docs/carving/deep-scan.md), the
[desktop architecture](docs/desktop/architecture.md), the
[health model](docs/recovery/health-model.md), the
[undelete corpora](docs/ntfs/undelete-corpus.md) and the
[real-hardware test procedure](docs/testing/real-hardware.md).

## Repository layout

```text
apps/phoinix-cli        command-line application
crates/phoinix-core     identifiers, byte ranges, checked arithmetic, byte parsing helpers
crates/phoinix-block    read-only BlockReader, RAW images, subrange views, fingerprints
crates/phoinix-device   physical device enumeration and read-only access (Linux, Windows)
crates/phoinix-volume   MBR / extended MBR / GPT discovery and partition views
crates/phoinix-fs       filesystem-neutral contracts: probes, recovery candidates
crates/phoinix-fs-ntfs  native NTFS reader and undelete engine
crates/phoinix-fs-fat   native FAT12/16/32 reader and undelete engine
crates/phoinix-fs-exfat native exFAT reader and undelete engine
crates/phoinix-health   recovery evidence model, scoring and explanations
crates/phoinix-carve    deep scan: signature carving with structural assembly
crates/phoinix-recovery recovery writer with destination safety and SHA-256 verification
crates/phoinix-session  application service layer: scans with progress, sessions, recovery, previews
apps/desktop            desktop application: Tauri 2 shell (src-tauri) + React/TypeScript front-end
tests/fixtures          compressed disk-image fixtures with ground-truth manifests
tests/generated         scripts that build the fixtures deterministically
tests/integration       end-to-end tests across crates
docs/                   architecture, NTFS notes, decision records
```

## Building

PhoinixDR is a standard Cargo workspace on stable Rust (edition 2024).

```bash
cargo build --release
cargo test --workspace
```

## Desktop application

```bash
cd apps/desktop
npm ci
npm run tauri dev      # development window
npm run tauri build    # installers under src-tauri/target/release/bundle
```

Linux needs the WebKitGTK development packages first
(`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`).
Scanning physical disks requires an elevated process; disk images do not.
See [`docs/desktop/architecture.md`](docs/desktop/architecture.md).

## Using the CLI

```bash
# Enumerate physical block devices (may require elevated privileges).
phoinix devices

# Identify partition table and filesystems of an image or device.
phoinix inspect disk.img
phoinix inspect disk.img --json

# Native NTFS reader.
phoinix ntfs info volume.img
phoinix ntfs ls volume.img
phoinix ntfs record volume.img 5
phoinix ntfs extract volume.img --record 64 --output file.bin

# Undelete: list deleted candidates with recovery health.
phoinix scan disk.img --deleted

# Deep scan: also carve files by signature from the unallocated space
# (--carve-all for the whole volume; works on raw sources without a filesystem).
phoinix scan disk.img --deep
phoinix scan disk.img --deep --carve-types jpeg,pdf,docx

# Explain the evidence behind a candidate's score (carved files are c<offset>).
phoinix explain disk.img 64
phoinix explain disk.img c1048576

# Recover candidates to another filesystem and verify by SHA-256.
phoinix recover disk.img 64 65 c1048576 --output /mnt/recovery --preserve-tree
```

Example `scan` output on the test corpus:

```text
ID   NAME                 SIZE      RECOVERY         CONF  PATH
68   tiny.txt             20 B      Excellent 97     82    \a\tiny.txt
77   photo.jpg            61.6 KiB  Excellent 95     97    \docs\photo.jpg
83   realloc_25.bin       256 KiB   Poor 59          82    \d\realloc_25.bin
87   wiped.jpg            61.6 KiB  Very poor 20     82    \g\wiped.jpg
96   document.txt         2.0 KiB   Very good 92     82    \?\document.txt
122  frag10.bin           640 KiB   Very good 87     82    \c\frag10.bin
```

`explain` lists the evidence behind each figure, for example
"16 of 64 required clusters are currently allocated to active filesystem
data" or "The JPEG image structure validates successfully".

`scan`, `explain` and `recover` detect the volume's filesystem (NTFS, FAT12/16/32
or exFAT) and use the matching engine. When a source contains a partition
table they operate on the first supported partition by default; pass
`--partition N` to choose another, or point them at a bare volume image.

## Safety

PhoinixDR never writes to the source. Recovery always targets another
filesystem, and the recovery writer refuses destinations that appear to live
on the source device. See [SECURITY.md](SECURITY.md) for the threat model and
how to report vulnerabilities.

## License

SPDX-License-Identifier: `MIT OR Apache-2.0`

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The Apache licence supplies the patent grant; the MIT
licence keeps compatibility with projects that prefer a minimal permissive
licence. Optional adapters for third-party libraries (for example
The Sleuth Kit or libewf) may carry different terms and are reviewed
dependency by dependency before any binary that includes them is released.
