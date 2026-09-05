# Lost-partition recovery (`phoinix-partition-recovery`)

A partition table can be wiped, a partition deleted, or a disk quick-
formatted; the filesystems on it usually survive. The structure search
reads the whole source once and looks for filesystem structures wherever
they are, independently of the table, then mounts what it finds virtually
so that files can be browsed and recovered. Nothing is ever written to the
partition table or the volume (ADR-0011).

```text
source ──find_headers──► hits: NTFS/exFAT boot sectors, FAT boot sectors, EXT superblocks
             │
     interpret each hit   boundaries from the structure; primary or backup?
             │
     verify               probe + engine open on a virtual mount (repairs overlaid)
             │
     relate               listed / lost / inside a partition / nested / overlapping
             │
     PartitionCandidate   start, length, filesystem, label, serial, evidence, confidence
```

## Structures

| filesystem | primary | backup | end of volume from | primary vs backup discriminator |
|---|---|---|---|---|
| NTFS | boot sector (`NTFS    ` at +3) | last sector of the volume | total sectors × sector size | `FILE` at the `$MFT` position the boot sector declares |
| FAT12/16/32 | boot sector (`EB xx 90` / `E9`, `55 AA`) | FAT32: sector 6 | total sectors × sector size | media descriptor at the first FAT |
| exFAT | boot sector (`EXFAT   ` at +3) | sector 12 | volume length × sector size | `F8 FF FF FF` at the FAT |
| ext2/3/4 | superblock at +1024 (`EF53`) | first block of later block groups | blocks × block size (64-bit aware) | the superblock's own block-group number |

The discriminator matters: a backup boot sector is byte-identical to the
primary, so a hit alone does not say where the volume starts. When the
structure at the hit is not consistent with the hit being the volume start
but is consistent with the hit being the backup, the candidate starts
where the primary belongs and carries a **repair**: the backup bytes,
overlaid at the primary's position when the candidate is mounted
(`PartitionCandidate::open` → `PatchedReader`). The engine then opens the
volume as if the primary were intact.

## Evidence and confidence

| evidence | effect |
|---|---|
| primary structure valid / found through a backup | base 60 / 45 |
| backup structure matches / mismatches | +15 / −10 |
| filesystem probe on the mounted volume (0–100) | + up to 10 |
| engine opened the volume and read the root directory / could not | +15 / −30 |
| declared geometry does not fit the source or is not sector-aligned | −15, and capped at 60 when the length runs past the source |
| nested inside another candidate (an image file on that volume) | −30 |
| overlapping another candidate (a stale layout) | −15 |

Every candidate lists its evidence in words (`phoinix partitions`, the
desktop's lost-partition list).

## Relations

- **listed**: start and length match a table entry (NTFS may be one sector
  shorter than its partition: the backup boot sector lives there);
- **lost**: no table entry covers it, the case to recover;
- **inside partition N**: lies inside a table entry with different
  boundaries;
- **nested in #K**: lies inside another candidate; usually a disk image
  stored on that volume, not a partition;
- **overlaps #K**: partially overlaps another candidate; one of the two is
  stale.

## CLI

```bash
phoinix partitions disk.img                 # search, evidence, boundaries, status
phoinix partitions disk.img --json --no-verify
phoinix scan disk.img --lost 2              # mount candidate #2 virtually and scan it
phoinix recover disk.img --lost 2 64 --output /mnt/rescue
phoinix scan disk.img --at 1048576 --length 16356737024   # explicit byte range
```

`--lost` re-runs the search (one full read of the source), so on large
disks `--at` with the printed start and size is faster for repeated
commands; `--at` has no repairs, so it needs an intact primary structure.

## Desktop and service layer

`Workspace::start_partition_search` streams progress and returns the
candidates; a `ScanRequest.volume` range (offset, length, repairs) scans
a candidate, and the session records the range so recovery and previews
reopen the same virtual mount later. The Scan screen has a "Search for
lost partitions" step listing candidates with their status, confidence
and repair.

## Limitations

- HFS+/APFS structures are not searched yet (M10+).
- A volume whose primary and backup structures are both destroyed is not
  found; carving (`scan --deep`) still recovers its files by content.
- Length comes from the structure, so a partition that was shrunk after
  formatting reports its old length; the readable length is clamped to the
  source.

## Tests

`crates/phoinix-partition-recovery/tests/search.rs`: listed partitions of
the MBR and GPT fixtures with exact boundaries; the same after the tables
are wiped; NTFS found through its backup boot sector when the primary is
zeroed, and mounted; ext4 through its primary and its group-1 backup
superblock; nested and offset volumes; 25 corruption rounds. The service
layer and the CLI tests scan and recover files from a lost NTFS volume with
a destroyed boot sector.
