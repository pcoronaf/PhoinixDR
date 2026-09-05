# Command-line guide

`phoinix` is the command-line face of PhoinixDR. Every command is read-only
towards the source: the only things it writes are recovered files, reports
and JSON output you ask for.

```text
phoinix [OPTIONS] <COMMAND>

Commands:
  devices     List block devices visible to this process
  inspect     Identify the partition table and filesystems of a device or image
  verify      Hash a source and compare with the hashes stored in its image container (E01)
  partitions  Find volumes by their filesystem structures, independently of the partition table
  scan        Scan a source for recoverable files and assess their health
  explain     Explain the evidence behind a candidate's recovery health
  recover     Recover candidates to another filesystem and verify them
  ntfs        Native NTFS reader commands (info, ls, record, extract)
  read        Read raw bytes from a source (developer/debug command)

Options:
  -v, --verbose...   -v info, -vv debug, -vvv trace (to stderr)
  -V, --version      Print version
```

Print `--version` to see the build: `phoinix 0.1.0 by @pcoronaf`.

## Sources

A *source* is a device path or an image file:

| source | example |
|---|---|
| physical disk (Windows) | `\\.\PhysicalDrive1` (run as administrator) |
| block device (Linux) | `/dev/sdb` (run with `sudo`) |
| RAW / dd image | `disk.img`, `stick.dd` |
| split RAW | `disk.001` (any segment; siblings are found by name) |
| EWF / E01 | `case.E01` (split `E01`…`E99`, `EAA`… are followed; SMART `.s01` too) |
| VHD, VHDX, VMDK | `disk.vhd`, `disk.vhdx`, `disk.vmdk` |

Containers are recognised from their content, so an `.img` that is really
an E01 opens as an E01. See [image containers](../images/containers.md).

### Choosing the volume

`scan`, `explain`, `recover` and the `ntfs` commands work on one volume:

| option | meaning |
|---|---|
| (none) | the first partition with a supported filesystem, or the whole source when it has no partition table |
| `--partition N` | partition `N` of the table (1-based, as `inspect` prints) |
| `--lost N` | candidate `N` of `phoinix partitions`, mounted virtually with its repairs |
| `--at OFFSET [--length BYTES]` | an explicit byte range |

## Workflow

### 1. Find the source

```bash
phoinix devices              # disks visible to this process (elevated on Windows)
phoinix devices --partitions # include partition nodes
phoinix devices --json
```

### 2. Inspect it

```bash
phoinix inspect disk.img
phoinix inspect case.E01           # adds an "Image container" section
phoinix inspect disk.img --json
phoinix inspect disk.img --fingerprint   # SHA-256 of the first and last MiB
```

The output lists the partition table, its diagnostics, and for every
volume the detected filesystem with the evidence behind the detection.

### 3. Verify an image (optional)

```bash
phoinix verify case.E01        # MD5, SHA-1, SHA-256; compares with the stored hashes
phoinix verify disk.vmdk       # no stored hash: the computed hashes document the source
phoinix verify case.E01 --json
```

The exit code is non-zero when a stored hash does not match.

### 4. Scan

```bash
phoinix scan disk.img                   # deleted files through filesystem metadata
phoinix scan disk.img --deep            # also carve the unallocated space by signature
phoinix scan disk.img --deep --carve-types jpeg,pdf,docx
phoinix scan disk.img --carve-all       # carve the whole volume (allocated space too)
phoinix scan disk.img --carve-only      # skip the metadata scan
phoinix scan disk.img --min-health good --name invoice
phoinix scan disk.img --no-content      # faster; lowers assessment confidence
phoinix scan disk.img --json
```

Each row shows the candidate reference (`ID`), name, size, **recovery
likelihood** with its category, **assessment confidence** and the original
path. Carved files are referenced as `c<offset>`. Categories:

| likelihood | category |
|---|---|
| 95–100 | Excellent |
| 80–94 | Very good |
| 60–79 | Good |
| 35–59 | Poor |
| 1–34 | Very poor |
| 0 | Unrecoverable |

Deep-scan options: `--carve-align` (default 512; `1` tests every byte and
is slow), `--carve-min-size`, `--carve-threads`, `--carve-signatures
file.json` for your own signatures (see [deep scan](../carving/deep-scan.md)).

### 5. Understand a candidate

```bash
phoinix explain disk.img 64
phoinix explain disk.img c1048576
phoinix explain disk.img 64 --json
```

`explain` prints every reason behind the two numbers: metadata validity,
whether the layout is known, how many clusters are allocated to other
files, what the content examination found, and diagnostics such as
"Layout recovered from journal transaction 9". Read it before trusting a
figure. The wording is deliberate: "allocated to active filesystem data"
means reuse is proven, not that every byte is gone.

### 6. Lost partitions

```bash
phoinix partitions disk.img              # search the whole source for volume structures
phoinix partitions disk.img --no-verify  # faster: do not open candidates with their engines
phoinix scan disk.img --lost 2
phoinix recover disk.img --lost 2 64 --output /mnt/recovery
```

Nothing is written to the partition table. A candidate whose primary boot
sector is destroyed is mounted with its backup overlaid in memory.

### 7. Recover

```bash
phoinix recover disk.img 64 65 c1048576 --output /mnt/recovery
phoinix recover disk.img 64 --output /mnt/recovery --preserve-tree
phoinix recover disk.img 64 --output /mnt/recovery --no-timestamps --no-hash --overwrite
```

Rules the writer enforces:

- the destination must not be on the source disk (`--allow-source-destination`
  is an expert override that can destroy the data you are recovering);
- existing files are never overwritten unless `--overwrite` is given;
- every written file is hashed with SHA-256 and reported as complete or
  `PARTIAL`; a partial recovery makes the command exit non-zero.

### 8. Report

```bash
phoinix recover case.E01 64 65 --output D:\recovered --report D:\recovered\report.html \
    --case-number 2026-017 --evidence-number HDD-3 --examiner "J. Doe" --verify-source
```

The report (`.html`, `.md` or `.json` by extension) records the tool
version, case metadata (fields not given are taken from the E01 acquisition
header), the source and its container, the stored and computed hashes with
`--verify-source`, and every file with its health at recovery time, output
path and SHA-256. See [reports](../images/containers.md#case-metadata-and-reports).

## NTFS developer commands

```bash
phoinix ntfs info volume.img                 # geometry, MFT location, flags
phoinix ntfs ls volume.img --all --system    # every MFT record, deleted and system files included
phoinix ntfs record volume.img 5 --hex       # one record with attributes and a hex dump
phoinix ntfs extract volume.img --record 64 --stream Zone.Identifier --output out.bin
phoinix read disk.img --offset 512 --length 512 --hex
```

## Exit codes

| code | meaning |
|---|---|
| 0 | success (for `recover`: every file complete; for `verify`: hashes match or none stored) |
| 1 | an error, a failed or partial recovery, or a hash mismatch; the message names the cause |

## JSON

Every command has `--json`. Objects use `snake_case` keys, sizes are byte
counts, times are ISO-8601 UTC, and filesystem names are kebab-case
(`ntfs`, `fat32`, `ex-fat`, `ext`). The `scan` output is
`{ "filesystem": …, "candidates": [ … ], "carving": … }`; each candidate
carries its `filesystem_object` (the stable reference used by `explain` and
`recover`), `evidence` and `health`.
