# M12: Secure erase

Status: M12.1 delivered (devices, partitions and files; every method; refusal rules; read-back verification; certificate; CLI and desktop). M12.2 delivered (free space of NTFS/FAT/exFAT/ext volumes from the engines' allocation maps, file slack, the filesystem-level wipe of a mounted volume, post-erase quick and deep scans in the certificate). M12.3 delivered (files and folders through the filesystem: named streams, slack padding, read-back, truncate, rename, delete; the post-erase scan counts the erased names). M12.4 delivered (ATA SANITIZE and SECURITY ERASE, NVMe Sanitize and Format NVM through SG_IO, the NVMe ioctl and the Windows pass-through IOCTLs; frozen-state detection and advice; the firmware action preselected on solid-state drives; before-and-after sample comparison). M12 complete. Decision record: [ADR-0014](../decisions/ADR-0014-secure-erase-as-a-separate-write-crate.md).

PhoinixDR gains an erase mode: overwrite a whole device, a partition, the
free space of a volume, or individual files and folders, with a chosen
method and number of passes, then verify with the recovery engine and
produce a sanitization certificate. The recovery stack stays read-only;
all writing lives in a new crate, `phoinix-erase`.

## What it must say before it does anything

The feature is only honest if the interface states what each operation
guarantees. These statements are part of the requirement, not of the
documentation alone:

- **One pass is enough on any drive made this century.** Multi-pass
  schemes (Gutmann 35, DoD 5220.22-M 3/7, Schneier 7, VSITR) come from
  1990s drive encodings; Gutmann's own epilogue says the full method is
  "pointless for any drive" and that "a good scrubbing with random data
  will do about as well as can be expected", and NIST SP 800-88 states
  that for ATA drives made after 2001 a single overwrite protects against
  both keyboard and laboratory attack. When a user chooses any method
  with more than one pass, the interface shows a warning that links
  [El mito de Gutmann](https://www.pcoronaf.com/2014/07/el-mito-de-gutmann.html)
  (pcoronaf, 2014): the extra passes are offered because some guidelines
  still ask for them, not because they add measurable security, and they
  multiply the time and the wear.
- **DoD 5220.22-M is a historical label.** It left the NISPOM in 2007 and
  is no requirement anywhere; it is kept because policies still name it.
- **Overwriting does not sanitize flash.** SSDs, USB sticks and cards
  remap writes across cells; an overwrite reaches the logical blocks, not
  the physical ones, and never the over-provisioned or retired cells.
  NIST SP 800-88 *Purge* for flash is the device's own sanitize command
  (ATA Secure Erase / Sanitize, NVMe Format with secure-erase setting or
  Sanitize) or cryptographic erase; an overwrite is *Clear* at best,
  enough against software recovery. On a solid-state target the default
  offer is the firmware command when the platform exposes it, and the
  overwrite option is labelled "Clear (software-level) only".
- **Overwriting a file is not erasing a file.** Resident data in the MFT
  record, slack in the last cluster, journal copies, volume shadow
  copies and previous versions, copy-on-write filesystems and flash
  remapping all keep copies the file-level overwrite cannot reach. The
  file and folder operation is labelled "overwrite current content", and
  the interface recommends following it with a free-space wipe of the
  volume, as the author's
  [Borrado seguro de archivos](https://www.pcoronaf.com/2020/12/borrado-seguro-de-archivos.html)
  (pcoronaf, 2020) recommends, including the temporary files, swap and
  page file that may hold copies.
- **When a disk must leave your control and the data must not, destroy
  it.** The documentation says so for failing drives (an unreadable
  region is an unerased region) and for high-sensitivity cases.

## Scope

| target | operation | verification |
|---|---|---|
| whole device | overwrite every sector, partition table included; or firmware sanitize on SSD/NVMe | read-back sample of written blocks; `phoinix inspect` shows no partition table; deep scan finds no hits |
| partition | overwrite the partition's byte range; table entry left in place | read-back sample; quick scan finds no filesystem or no records; deep scan finds no hits |
| free space of a volume | overwrite every unallocated cluster of an unmounted NTFS/FAT/exFAT/ext volume (raw, from the engine's allocation map), plus the slack of allocated files optionally; or fill a mounted volume through the filesystem until it is full, then remove the fill files | deep scan of the volume finds no carved candidates; quick scan finds no deleted records with content (resident records are reported apart, since no free-space wipe reaches them) |
| files and folders | overwrite the content of the named files (every stream on NTFS), rename to a random name, truncate, delete | the deleted record, if still visible, has zero-filled content; a diagnostic lists what this operation cannot reach |

Out of scope for M12: erasing the system disk from inside the running
system, RAID/LVM members, encrypted volumes' keys (cryptographic erase is
offered only through the device's own command), and remote or cloud
copies.

## Methods

| id | passes | pattern | note shown to the user |
|---|---|---|---|
| `zero` | 1 | 0x00 | NIST 800-88 Clear; fastest; the default for magnetic disks |
| `random` | 1 | CSPRNG | NIST 800-88 Clear; indistinguishable from encrypted data; the default when the user wants no readable pattern |
| `nist` | 1 | random, then verify sample | the documented Clear procedure with the verification 800-88 asks for |
| `dod-3` | 3 | 0x00, 0xFF, random + verify | DoD 5220.22-M (E), obsolete since 2007; multi-pass warning |
| `dod-7` | 7 | 5220.22-M (ECE) sequence | obsolete; multi-pass warning |
| `schneier` | 7 | 0xFF, 0x00, 5 × random | multi-pass warning |
| `vsitr` | 7 | 0x00/0xFF × 3, 0xAA | BSI 1999; multi-pass warning |
| `gutmann` | 35 | the 1996 sequence | multi-pass warning with the article link; "pointless for any drive" quoted from the author's epilogue |
| `custom` | n | user pattern or random per pass | multi-pass warning when n > 1 |
| `firmware` | – | ATA Secure Erase / Sanitize, NVMe Format/Sanitize | solid-state Purge; the only method that reaches remapped cells; requires the device to be unfrozen and not the boot disk |

Every method ends with the same optional read-back verification: a
sample of blocks spread over the target (every block for targets under
1 GiB) is read through the read-only stack and compared with the last
pattern.

## Safety

- The target is shown with model, serial number, size and the volumes it
  holds, and the user types the serial number (or the last six
  characters of it) to confirm. The command line requires
  `--i-understand-this-destroys-data <serial>`.
- Refused without override: the OS disk, the executable's disk, the
  sessions disk, a source currently open for recovery. No override exists
  for the OS disk.
- Volumes on the target are locked and dismounted (Windows) or must be
  unmounted (Linux) before the first write; a volume that cannot be
  dismounted aborts the operation before any write.
- A five-second countdown after confirmation during which Cancel is the
  only control.
- Cancellation stops after the current block; the interface states that
  the target is partially erased and offers to resume from the last
  written block, with the resume point stored in the session directory.
- The erase mode is reachable only from the home page under its own
  heading, never from a results page, and is coloured and labelled apart
  from recovery.
- Every erase writes an entry to the engine log and a certificate file;
  the desktop refuses to close during an erase without a confirmation.

## Certificate

A sanitization record per NIST 800-88 §4.8, in HTML, Markdown and JSON
like the recovery report, carrying: device model, serial number and
capacity; target (whole device, partition offset and length, volume,
file list); method, passes and patterns; start and end time; software
version; operator fields (name, organisation, case or asset number,
notes); verification performed and its result (blocks sampled, mismatches,
scan summary); unreadable regions encountered (these are *not* erased and
the certificate says so); the disclaimer. The certificate is written
before the first pass as "in progress" and rewritten at the end.

## Architecture

```
phoinix-erase            write-capable: targets, methods, writer, verification, certificate
  ├─ target.rs           WholeDevice | Partition | FreeSpace | Files; resolution and refusal rules
  ├─ method.rs           the table above; pass generator; CSPRNG (getrandom + ChaCha)
  ├─ writer/{windows,linux}.rs   open for writing, lock/dismount, aligned writes, flush, resume
  ├─ freespace.rs        unallocated clusters from the read-only engines' AllocationView
  ├─ files.rs            per-filesystem file overwrite (streams, rename, truncate, delete)
  ├─ firmware/{ata,nvme}.rs      sanitize commands (Windows IOCTLs, Linux ioctls); frozen-state handling
  ├─ verify.rs           read-back sampling + recovery-engine scans through phoinix-session
  └─ certificate.rs      the record and its renderings
phoinix-session          EraseRequest / EraseEvent / erase job with progress, cancel, resume
apps/phoinix-cli         phoinix erase {device|partition|free-space|files} …
apps/desktop             Erase page: target picker, method picker with warnings, confirmation, progress, certificate
```

`phoinix-erase` depends on `phoinix-core`, `phoinix-block` and
`phoinix-device` (read-only, for verification and enumeration) and
`phoinix-volume` (partition tables); from M12.2 also on the filesystem
engines for allocation views and file lookup. No recovery crate depends on
it. The session layer composes it for the front-ends and runs the
verification scans of M12.2, so there is no cycle. `#![forbid(unsafe_code)]`; the Windows
IOCTLs go through the `windows-sys` crate like the readers do.

## Delivery

1. **M12.1 Devices and partitions.** Crate, writer for both platforms,
   `zero`/`random`/`nist` and the multi-pass table, confirmation and
   refusal rules, progress and cancellation, read-back verification,
   certificate, CLI subcommand, desktop Erase page, documentation with
   the guarantee statements and the article links. Tests on loop
   devices (Linux CI) and image files; a Windows test writes to a VHD
   attached in the runner.
2. **M12.2 Free space and verification scans** (delivered). Free-space
   wipe for NTFS/FAT/exFAT/ext from the engines' allocation views
   (`AllocationView::free_ranges`), slack overwrite as an option
   (`AllocationView::slack_ranges`, implemented by every engine for live
   files with plain, complete layouts), the filesystem-level fill of a
   mounted volume (no raw access, no administrator rights), and post-erase
   quick and deep scans run by the session layer through a `PostScanner`
   hook and summarised in the certificate. Raw targets are written as
   lists of ranges with read-modify-write of partial sectors, so slack
   can be erased on devices that need aligned writes.
3. **M12.3 Files and folders** (delivered). File overwrite through the
   filesystem with the stated limits and the "follow with a free-space
   wipe" recommendation; NTFS alternate streams (enumerated with the
   stream API and overwritten by name); slack through padding to the
   allocation unit; resident data rewritten in place through the
   filesystem API (the record itself is never written raw), then
   truncate, random rename and delete; folders removed deepest first;
   read-back before deletion; symbolic links skipped and the running
   executable refused.
4. **M12.4 Firmware sanitize** (delivered). ATA IDENTIFY DEVICE, SANITIZE
   DEVICE (crypto scramble, block erase, overwrite) and the SECURITY ERASE
   UNIT sequence with a temporary password that is removed even on
   failure; NVMe Identify Controller, Sanitize (crypto, block, overwrite)
   with the status log polled, and Format NVM with a secure-erase setting
   keeping the namespace's LBA format. Linux through `SG_IO` (ATA
   PASS-THROUGH 16) and `NVME_IOCTL_ADMIN_CMD`, Windows through
   `IOCTL_ATA_PASS_THROUGH` and `IOCTL_STORAGE_PROTOCOL_COMMAND`.
   Frozen-state detection with the sleep/replug advice; the action
   preselected on solid-state drives; a sample of blocks hashed before
   and compared after. The logic is tested against simulated drives; the
   platform calls are type-checked on both targets and not exercised in
   continuous integration.

Each part ships behind the same safety rules; none is released without
its tests, its documentation in English and Spanish, and a changelog
entry. The Gutmann warning, the SSD statement and the file-overwrite
limits are present from M12.1 onwards, since they are text and policy
rather than engine work.
