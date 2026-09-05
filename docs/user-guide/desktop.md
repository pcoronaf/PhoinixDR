# Desktop guide

PhoinixDR's desktop application walks through the same steps as the
command line: choose a source, scan, understand what was found, preview,
recover. It never writes to the source.

## Starting

- **Windows:** run `PhoinixDR-<version>-windows-x64-portable.exe`. Nothing is
  installed (see [Windows portable release](../release/windows-portable.md)).
  To scan a physical disk or a USB stick, right-click the executable and
  choose *Run as administrator*; disk images do not need that.
- **Linux:** run `phoinix-desktop`; physical devices need `sudo` or a
  udev rule that grants read access to the disks.

The home page shows the version and author in the top bar and lists recent
scan sessions.

## Two ways to recover

PhoinixDR can work on the device itself or on an image of it. Pick one
before you start; the desktop guide below applies to both:

1. **Directly from the device, in one step.** Start PhoinixDR *as
   administrator* (Windows: right-click, *Run as administrator*; Linux:
   `sudo`), choose *Physical disk* or *Removable device*, scan and
   recover. Fastest; PhoinixDR only ever reads from the device.
2. **From a disk image.** First make an image of the device with an
   imaging tool (FTK Imager or Arsenal Image Mounter on Windows, `dd`
   or `ewfacquire` on Linux) and then open the image file in PhoinixDR
   with *Disk image*. PhoinixDR itself needs no elevation for this; the
   imaging tool does, since it reads the same raw device. This is the
   recommended path for a failing drive, for anything you may need to
   examine again, and for forensic work (E01 hashes are verified and
   reported).

Both paths give the same results on a healthy device. In both, recover to
a different disk than the one you are recovering from.

## 1. Choose a source

| choice | what it lists |
|---|---|
| **Physical disk** | internal drives and SSDs |
| **Removable device** | USB sticks, SD cards, external disks |
| **Disk image** | a file: RAW/dd, split RAW, E01 (and split E01), SMART, VHD, VHDX, VMDK |

A device that cannot be opened is shown greyed as *Not accessible from
this process*, and the page explains the fix: close PhoinixDR and start it
again with *Run as administrator* (Windows) or `sudo` (Linux). Disk
images never need elevation.

## 2. Scan setup

The setup page shows the source, its partition table and every volume with
its filesystem and detection confidence.

- **Image container** (image files only): format, variant, segment count,
  compression, the acquisition header of an E01 (case number, examiner,
  dates, software), stored hashes, and a **Verify hashes** button that
  reads the whole image and compares its MD5/SHA-1 with the stored ones.
- **Volume:** pick a partition when the source has several.
- **Lost partitions:** *Search for lost partitions* scans the whole source
  for filesystem structures independently of the table. A found volume can
  be selected and is mounted virtually; when its primary boot sector is
  destroyed the backup is used in memory. The partition table is never
  modified.
- **Mode:** *Quick Scan* reads filesystem metadata (deleted files and
  records). *Deep Scan* also carves the unallocated space for files by
  signature; it reads the free space once. Volumes without a recognised
  filesystem can only be deep-scanned.
- **Deep scan options:** carve the whole volume instead of only the free
  space; restrict the file types.
- **Assessment:** *Examine content* validates file structures (JPEG, PNG,
  PDF, ZIP/DOCX, …) and raises assessment confidence; turning it off is
  faster.

## 3. Scanning

Progress shows the phase (metadata, carving), counts and throughput. The
scan can be cancelled; partial results are kept.

## 4. Results

Every row is a recovery candidate:

| column | meaning |
|---|---|
| name | original name, or a synthetic `carved-…` name for carved files |
| health | **likelihood** that recovery returns the original bytes, with its category (Excellent … Unrecoverable), and **confidence** in that estimate |
| size | logical size when known |
| type | detected or expected file type |
| path | original path; `(uncertain)` when the directory record was reused; `journal` and `carved` tags say how the file was found |

Filters narrow the list by text, health category, type and origin
(metadata or carved). Selecting a row opens the detail panel:

- the **evidence**: every positive and negative reason behind the two
  numbers, and diagnostics such as reused clusters, journal transactions
  or inferred starts;
- a **preview**: images are rendered, text is shown, other content is
  hex-dumped; previews read the source, never write it.

Read the evidence before trusting a number. "Allocated to active
filesystem data" means the clusters have been reused, not that every
byte is gone; "Unrecoverable" means no extent of the content could be
located, in which case a deep scan may still carve it.

## 5. Recovery

Select rows and press **Recover**:

- **Destination:** a directory on another disk. A destination on the
  source disk is refused; the expert override is for people who know they
  are risking the data they are recovering. A destination that is the
  image file itself is always refused.
- **Options:** recreate the original folder structure, apply original
  timestamps, verify every file with SHA-256.
- **Report and case (optional):** choose a report file (`.html`, `.md` or `.json`),
  optionally hash the whole source for it, and fill in case number,
  evidence number, examiner and notes. Fields are prefilled from the E01
  acquisition header when the source has one.

The result table shows every file with bytes written, `verified` or
`PARTIAL`, the SHA-256 prefix and the output path. The report path is
shown when one was written.

## Sessions

Every scan is saved as a session (`.phx`) in the application data
directory and listed on the home page. Reopening a session reopens the
source at the same volume (repairs included) so previews and recovery
work later without rescanning. Sessions can be opened from any path with
*Browse…*.

## Privacy

The application does not contact the network. Logs (`-v` on the command
line, none in the desktop) never contain recovered content.
