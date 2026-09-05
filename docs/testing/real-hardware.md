# Testing PhoinixDR on real hardware

The automated suite runs against disk images. This procedure exercises the
paths that only real devices reach: device enumeration, raw device reads,
sector geometry, and destination safety. It is designed so that the
evidence disk is never written to.

## What you need

- A spare USB stick or SD card (any size from 1 GB up). It will be
  formatted, so use one with nothing on it.
- A build of the CLI: `cargo build --release`, binary at
  `target/release/phoinix` (`phoinix.exe` on Windows).
- Administrator (Windows) or root (Linux) rights for raw device access.

## Step 1 — enumerate devices

Windows (elevated PowerShell):

```powershell
.\phoinix.exe devices
.\phoinix.exe devices --json
```

Linux:

```bash
sudo ./phoinix devices
sudo ./phoinix devices --partitions
```

Expect: every physical disk listed with bus, size, sector size and model.
Check that the reported size matches Disk Management / `lsblk`, and that the
sector size is `512/4096` or `4096` on Advanced Format / 4Kn drives.

Without elevation, expect `access denied` entries and a hint to elevate.

## Step 2 — prepare the USB stick (the only write, done by the OS)

1. Format the stick as NTFS using the operating system.
2. Copy a few files onto it: a photo (JPEG), a PDF, a DOCX, and one large
   file (> 100 MB) so that fragmentation is possible.
3. Compute and save their SHA-256 digests (`Get-FileHash` on Windows,
   `sha256sum` on Linux).
4. Delete two of the files normally (bypass the Recycle Bin: Shift+Delete on
   Windows, `rm` on Linux).
5. Safely eject and re-insert the stick. Note its device path from step 1
   (`\\.\PhysicalDrive2`, `/dev/sdb`).

## Step 3 — read-only inspection

```bash
phoinix inspect \\.\PhysicalDrive2          # Windows
sudo ./phoinix inspect /dev/sdb              # Linux
```

Expect: the partition table (MBR or GPT), one NTFS volume with confidence
95 %, and a size consistent with step 1. Run `phoinix read <device>
--length 512 --hex` and confirm the first bytes are a partition table or
the NTFS boot sector.

## Step 4 — scan and explain

```bash
sudo ./phoinix scan /dev/sdb --deleted
sudo ./phoinix explain /dev/sdb <ID>
```

Expect: the two deleted files listed with their original paths; validated
types (JPEG, PDF, DOCX) reported as Excellent with a passing structure
check; `explain` shows "All N required clusters are currently free".

If the stick is flash media, `explain` should also print the SSD/TRIM
caution when the OS reports the medium as non-rotational.

## Step 4b — deep scan

```bash
sudo ./phoinix scan /dev/sdb --deep
sudo ./phoinix scan /dev/sdb --deep --json > deep.json
```

Expect the metadata candidates from step 4 plus carved rows (`c<offset>`)
only for content the filesystem no longer describes; the summary line
reports how many carved hits were merged into metadata candidates. On a
16 GB stick the header search reads the whole free space once (a few
minutes over USB 2.0); progress is shown on stderr. `explain` on a merged
metadata candidate shows the carving corroboration as an informational
diagnostic.

## Step 5 — recovery and verification

Recover to a directory on a *different* disk (for example your home
directory):

```bash
sudo ./phoinix recover /dev/sdb <ID> <ID> --output ~/phoinix-out --preserve-tree
```

Expect: SHA-256 printed for each file, identical to the digests saved in
step 2.

Then attempt the unsafe case and confirm it is refused:

```bash
sudo ./phoinix recover /dev/sdb <ID> --output /media/<stick-mountpoint>/out
```

Expect: `error: The selected recovery destination is located on the disk
being recovered…` and nothing written. `--allow-source-destination` is the
only way past this and should not be used for the test.

## Step 6 — immutability check

Image the stick before step 3 and after step 5 and compare:

```bash
sudo dd if=/dev/sdb of=before.img bs=4M status=progress
# … steps 3–5 …
sudo dd if=/dev/sdb of=after.img bs=4M status=progress
cmp before.img after.img && echo "source unchanged"
```

On Windows, use any imaging tool that produces a raw image and compare with
`fc /b`.

## Step 7 — the image behaves like the device

Run steps 3–5 again against `before.img`. Results (candidates, health,
digests) must be identical to the device run. This is the cross-platform
equivalence check: an image made on one OS can be scanned on the other.

## What to report

- OS, PhoinixDR version, device model and sector size.
- Output of `phoinix devices --json` and `phoinix inspect --json`.
- Any candidate whose health or path looks wrong, with
  `phoinix explain … --json`.
- Whether step 6 reported the source unchanged.
