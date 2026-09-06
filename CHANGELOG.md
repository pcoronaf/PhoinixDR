# Changelog

All notable changes to PhoinixDR. The project follows the milestones of
its technical specification; versions are tagged `vX.Y.Z` and published as
GitHub Releases.

## Unreleased

- Deep scan, second stage: content examination of a carved file now reads
  at most 8 MiB and samples 8 blocks for zeros instead of re-reading up to
  256 MiB per file; a JPEG whose entropy data gives way to data without
  marker bytes, or a footer search that runs into a whole window of zeros,
  stops there instead of walking to the size limit. On a drive full of
  partly overwritten remnants this turns hours into minutes. The progress
  line shows the bytes read.
- Unreadable regions (I/O errors, driver timeouts such as Windows error
  121) no longer abort a deep scan: the chunk is retried in 64 KiB blocks,
  what still fails is skipped and treated as zeros, the scan reports the
  unreadable bytes and regions, and a carved file overlapping such a
  region carries a negative reason and a capped likelihood.
- Zero sampling of a candidate's content runs regardless of the *Examine
  content* option, in the carver and in the NTFS, FAT, exFAT and ext
  engines, so files discarded by TRIM or wiped are rated low instead of
  looking intact.
- Solid-state sources: the scan setup page (and the command line) warn that
  deleted data on an SSD is usually discarded by TRIM within seconds and
  explain when recovery is still possible.
- Sessions of a physical device are labelled with the drive's model and
  serial number in the recent-sessions list and the results page.

## 0.1.3

- Fixed: after the header search of a deep scan reached 100 %, the window
  stayed on the scanning page with no progress, sometimes for hours on
  large volumes, while the engine assembled and validated every hit; Cancel
  did nothing in that stage. The assembly stage now reports its own
  progress ("Examining carved files", hits examined), can be cancelled
  with partial results kept, and the command line prints it too.
- Desktop: the results table renders only the rows in view, so scans with
  hundreds of thousands of candidates no longer stall the window.
- Desktop, Advanced mode: the scanning page shows the equivalent
  `phoinix scan` command and a live engine log (what the command line
  prints with `-vv`), with copy buttons; the detail panel shows the
  `phoinix explain` and `phoinix recover` commands for the selected file.
  The log is forwarded from the engine only while Advanced is on. The
  session layer logs the scan lifecycle (request, volume, phases, counts).
- Documentation: the desktop guide (English and Spanish) now includes
  screenshots of every step, from the home page to the recovery result.
- Desktop: a **Restart as administrator** button appears when a device is
  not accessible (and on the home page when no device can be listed). It
  requests elevation through the system prompt (UAC on Windows, polkit on
  Linux) and starts an elevated copy of PhoinixDR, so users no longer need
  to find *Run as administrator* themselves.
- Desktop: the evidence panel of the results view scales with the window
  width instead of a fixed 380 px, long reasons wrap, and on narrow windows
  it moves below the file list.

## 0.1.2

- The Windows portable desktop executable is published as
  `PhoinixDR-<version>-windows-x64-portable.exe`, so the downloaded file
  says which release it is; the version also appears in the executable's
  file properties. The release workflow checks that the tag, the
  workspace version and the executable's version resource agree. The
  website resolves the current file name through the GitHub API; the
  command-line and Linux asset names are unchanged.

## 0.1.1

- Fixed: the Windows portable executable and the Linux desktop binary of
  0.1.0 were development builds that tried to reach a local dev server
  ("localhost refused to connect"). The release pipeline now builds the
  desktop application through the Tauri CLI in production mode and
  verifies that the executable embeds its front-end.
- Spanish documentation (docs/es) and website (`/es/`): guides, FAQ,
  about pages, disclaimer and release requirement, with a language switch
  and English fallback for untranslated pages.
- Windows portable release requirement (REL-001) and the release workflow
  that produces single-executable builds with SHA-256 sums.
- Disclaimer ("provided as is, used at your own risk") in the README,
  the website, the desktop application, the CLI help and output, and
  recovery reports.
- Attribution "by @pcoronaf" in the CLI (`--version`, `--help`), the
  desktop application and recovery reports.
- Official PhoinixDR logo ("Lost data lives again") in the README, the
  website, the desktop application and its icons.
- Documentation: getting started, desktop and command-line guides, FAQ,
  Development Declaration, "Yes, PHOINIX is vibecoded", "Where PHOINIX
  Came From"; the GitHub Pages site and its build.

## 0.1.0 (engineering preview)

Milestones M0 to M11 of the technical specification.

- **M0 Architecture:** Cargo workspace, decision records, contracts.
- **M1 Block I/O:** read-only `BlockReader`, RAW images, subranges,
  physical devices on Windows and Linux, fingerprints.
- **M2 MBR/GPT:** partition discovery (MBR, extended, GPT with backup),
  filesystem probes, `inspect`.
- **M3 NTFS reader:** native MFT, attributes, runlists, `ntfs` commands.
- **M4/M5 NTFS undelete and health:** deleted records, evidence model,
  likelihood and confidence scoring with printed reasons, zero-content
  assessment, verified recovery writer with destination safety.
- **M6 Desktop:** service layer with sessions and progress, Tauri 2 +
  React application with previews and recovery.
- **M7 FAT/exFAT:** FAT12/16/32 and exFAT engines; inference of the first
  cluster on large FAT32 volumes deleted by Windows.
- **M8 File carving:** deep scan of unallocated space with structural
  assemblers for JPEG, PNG, GIF, BMP, PDF, ZIP/Office, SQLite, RIFF, MP4
  and 7z; deduplication into metadata candidates.
- **M9 Partition recovery:** structure search for NTFS, FAT, exFAT and ext
  volumes, backup-sector repairs, virtual mounts.
- **M10 ext2/3/4:** native reader, jbd2 journal reader, undelete with
  journal-assisted names and layouts.
- **M11 Forensic images:** E01/split E01/SMART, split RAW, VHD, VHDX, VMDK
  containers; hash verification; case metadata; JSON/Markdown/HTML
  recovery reports.
