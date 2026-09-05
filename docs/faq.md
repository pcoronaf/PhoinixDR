# Frequently asked questions

### Does PhoinixDR write to my disk?

No. The block layer has no write primitive, so no component of the
recovery engine can modify a source (ADR-0002, ADR-0007). The only writes
are recovered files, reports and sessions, and the recovery writer refuses
a destination that lives on the source disk.

### Why does a file show 0 % / "Unrecoverable"?

No extent of its content could be located: the filesystem cleared the
file's layout on deletion and no journal copy survived (ext2, an ext4
journal that wrapped), or the record is damaged. The name and timestamps
are still shown because they are evidence of the file's existence. A
**deep scan** may still find the content by signature; carved hits that
start where a metadata candidate expects its data are merged into it.

### The scan of my USB stick shows "Unrecoverable" for everything but one file.

On large FAT32 volumes the Windows driver clears the high half of the
first-cluster number when deleting. PhoinixDR infers the real start from
the free clusters and their content (`explain` prints "start was
inferred"). Make sure you are running a current build; the fix ships since
the M7 follow-up.

### What does "allocated to active filesystem data" mean?

The clusters a deleted file used are now marked in use by other files. The
old bytes may or may not still be there; PhoinixDR reports allocation as
evidence of reuse and lowers the likelihood accordingly, but never claims
"overwritten" without proof.

### Likelihood versus confidence?

**Likelihood** estimates the probability that recovery returns the
original bytes. **Confidence** says how much evidence that estimate rests
on: a scan with content examination off, a file without a structural
validator or an unknown medium lowers confidence without changing the
likelihood. See the [health model](recovery/health-model.md).

### Why do I need administrator rights?

Reading `\\.\PhysicalDriveN` (Windows) or `/dev/sdX` (Linux) is a
privileged operation on both systems. Disk images do not need elevation.

### The desktop application does not start on Windows.

It needs the WebView2 runtime, which is part of Windows 10 21H2+ and
Windows 11. On an older or trimmed installation, install Microsoft's
WebView2 Evergreen Bootstrapper. The command-line `phoinix.exe` has no
such dependency.

### SmartScreen or an antivirus warns about the executable.

The executables read raw disks, which some heuristics flag, and the
publisher is new. Compare the SHA-256 with `SHA256SUMS.txt` from the
release, or build from source.

### Can I recover from an SSD?

Yes, with a caveat: after deletion the drive may have discarded (TRIM) the
blocks, in which case they read as zeros. PhoinixDR has no evidence about
NAND state, so it warns and lowers confidence rather than pretending to
know. Zero-filled content of a recognised type is reported as
contradicting its format.

### Which filesystems and images are supported?

NTFS, FAT12/16/32, exFAT and ext2/3/4 with undelete; any other volume can
be deep-scanned (carved). Sources: physical disks, RAW/dd, split RAW,
E01/split E01/SMART, VHD, VHDX and VMDK images. HFS+, APFS, XFS, Btrfs
and RAID/LVM are on the roadmap.

### Is the recovered file identical to the original?

When the layout is known and every cluster is free, yes, and the SHA-256
in the recovery output lets you check it against any hash you have. When
some clusters were reused the file is written as it is now, marked with
its allocation evidence, so that you can judge it.

### Where are sessions stored?

In the user's local application-data directory
(`%LOCALAPPDATA%\org.phoinixdr.desktop` on Windows,
`~/.local/share/org.phoinixdr.desktop` on Linux). They contain the scan
results and evidence, never file content. Delete the directory to remove
them.

### Can PhoinixDR repair a partition table or a filesystem?

No. Lost partitions are mounted virtually and recovered from; the table is
never written (ADR-0011). Repair tools change the disk you are trying to
recover from and are out of scope by design.

### How was PhoinixDR built?

With extensive AI assistance and deliberate engineering. Read the
[Development Declaration](about/development-declaration.md),
[Yes, PHOINIX is vibecoded](about/vibecoded.md) and
[Where PHOINIX Came From](about/origin.md).
