#!/usr/bin/env bash
# Builds the partition-table fixtures under tests/fixtures/volume.
#
# Requires: sfdisk, sgdisk, mkntfs (ntfs-3g), mkfs.vfat (dosfstools), mke2fs, gzip.
# The images are mostly zeros and compress to a few dozen kilobytes.
#
# Usage: tests/generated/make_volume_fixtures.sh [output-dir]
set -euo pipefail

out="${1:-$(dirname "$0")/../fixtures/volume}"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mib() { echo $(( $1 * 1024 * 1024 )); }

# Writes a filesystem image into $1 at sector $2 (512-byte sectors), $3 sectors long, type $4.
put_fs() {
  local disk="$1" start="$2" sectors="$3" kind="$4" label="$5"
  local fs="$work/fs.img"
  rm -f "$fs"
  truncate -s $(( sectors * 512 )) "$fs"
  case "$kind" in
    ntfs)  mkntfs -F -q -s 512 -c 4096 -L "$label" "$fs" >/dev/null 2>&1 ;;
    fat16) mkfs.vfat -F 16 -s 2 -n "$label" "$fs" >/dev/null ;;
    fat12) mkfs.vfat -F 12 -s 8 -n "$label" "$fs" >/dev/null ;;
    fat32) mkfs.vfat -F 32 -s 1 -n "$label" "$fs" >/dev/null ;;
  esac
  dd if="$fs" of="$disk" bs=512 seek="$start" conv=notrunc status=none
}

# --- mbr-extended: two primaries, an extended container with two logicals ---
disk="$work/mbr-extended.img"
truncate -s "$(mib 48)" "$disk"
sfdisk -q "$disk" <<'TABLE'
label: dos
label-id: 0x1234abcd
unit: sectors
1 : start=2048,  size=16384, type=7, bootable
2 : start=18432, size=16384, type=6
3 : start=34816, size=63488, type=5
5 : start=36864, size=16384, type=1
6 : start=55296, size=16384, type=7
TABLE
put_fs "$disk" 2048  16384 ntfs  PRIMARY1
put_fs "$disk" 18432 16384 fat16 PRIMARY2
put_fs "$disk" 36864 16384 fat12 LOGICAL5
put_fs "$disk" 55296 16384 ntfs  LOGICAL6
gzip -9 -n -c "$disk" > "$out/mbr-extended.img.gz"

# --- gpt-basic: EFI (FAT16), Microsoft Reserved, Basic Data (NTFS) ---
disk="$work/gpt-basic.img"
truncate -s "$(mib 48)" "$disk"
sgdisk -o \
  -n 1:2048:18431  -t 1:ef00 -c 1:"EFI System" \
  -n 2:18432:20479 -t 2:0c01 -c 2:"Microsoft reserved" \
  -n 3:20480:96000 -t 3:0700 -c 3:"Données" \
  "$disk" >/dev/null
put_fs "$disk" 2048  16384 fat16 EFI
put_fs "$disk" 20480 75521 ntfs  DATA
gzip -9 -n -c "$disk" > "$out/gpt-basic.img.gz"

# --- ntfs-bare: a bare NTFS volume with no partition table ---
disk="$work/ntfs-bare.img"
truncate -s "$(mib 16)" "$disk"
mkntfs -F -q -s 512 -c 4096 -L BARE "$disk" >/dev/null 2>&1
gzip -9 -n -c "$disk" > "$out/ntfs-bare.img.gz"

# --- ext4-bare: an ext4 volume with 1 KiB blocks (two block groups, so a
# backup superblock exists at block 8193) for partition recovery tests ---
disk="$work/ext4-bare.img"
truncate -s "$(mib 16)" "$disk"
mke2fs -q -t ext4 -b 1024 -L PHXEXT4 -U 0b0b0b0b-1111-4222-8333-444444444444 "$disk"
gzip -9 -n -c "$disk" > "$out/ext4-bare.img.gz"

echo "fixtures written to $out"
ls -l "$out"
