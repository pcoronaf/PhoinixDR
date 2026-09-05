# Changelog

All notable changes to PhoinixDR. The project follows the milestones of
its technical specification; versions are tagged `vX.Y.Z` and published as
GitHub Releases.

## Unreleased

- Windows portable release requirement (REL-001) and the release workflow
  that produces single-executable builds with SHA-256 sums.
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
