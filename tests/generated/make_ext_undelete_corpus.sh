#!/usr/bin/env bash
# Builds the ext deletion corpus (tests/fixtures/ext/{ext4,ext3,ext2}.img.gz
# and their manifests) with mke2fs and the kernel drivers over loop devices
# (root required). Each image records the inode numbers, sizes, hashes and
# extent counts of its files before they are deleted, so the tests can check
# what the journal (ext3/ext4) or the bare inode table (ext2) still yields.
set -euo pipefail
out="${1:-$(dirname "$0")/../fixtures/ext}"
mkdir -p "$out"
work="$(mktemp -d)"
mnt="$work/mnt"
mkdir -p "$mnt"
cleanup() { umount "$mnt" 2>/dev/null || true; [ -n "${loop:-}" ] && losetup -d "$loop" 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT

gen() { python3 - "$1" "$2" <<'PY'
import sys
name, size = sys.argv[1], int(sys.argv[2])
out = bytearray(); i = 0
while len(out) < size:
    out += f"{name[:8]:<8}{i*32:016x}-------\n".encode(); i += 1
sys.stdout.buffer.write(bytes(out[:size]))
PY
}

build() {
  local flavour="$1" blocksize="$2" label="$3"
  local img="$work/$flavour.img"
  truncate -s 48M "$img"
  mke2fs -q -t "$flavour" -b "$blocksize" -I 256 -L "$label" "$img"
  loop="$(losetup -f --show "$img")"
  mount -o loop "$loop" "$mnt"

  mkdir -p "$mnt/docs" "$mnt/gone" "$mnt/d" "$mnt/x"
  # D first, so that it occupies the lowest data blocks and the intruder
  # written after its deletion lands on them.
  gen reused 1048576 > "$mnt/d/reused.bin"
  sync
  python3 - "$mnt" <<'PY'
import sys
from PIL import Image
img = Image.new("RGB", (320, 240)); px = img.load()
for y in range(240):
    for x in range(320):
        px[x, y] = ((x * 7 + y) % 256, (x ^ y) % 256, (y * 3) % 256)
img.save(sys.argv[1] + "/docs/photo.jpg", format="JPEG", quality=90)
PY
  gen small 700 > "$mnt/small.txt"
  gen medium 200000 > "$mnt/docs/medium.bin"
  : > "$mnt/empty.txt"
  gen longname 3000 > "$mnt/docs/A long name with spaces ünï 文件.txt"
  gen keep 2500 > "$mnt/gone/keep.txt"
  gen old 5000 > "$mnt/docs/renamed-old.txt"
  # Sparse: a head, a hole, a tail.
  gen sparse 4096 > "$mnt/docs/sparse.bin"
  truncate -s 300000 "$mnt/docs/sparse.bin"
  printf 'the tail of the sparse file\n' | dd of="$mnt/docs/sparse.bin" bs=1 seek=299000 conv=notrunc status=none
  # Fragmented: fillers, holes, then a file larger than one hole.
  for i in 0 1 2 3 4 5 6 7; do head -c 65536 /dev/zero | tr '\0' "$(printf '\\%03o' $((65 + i)))" > "$mnt/filler$i.bin"; done
  sync
  for i in 0 2 4 6; do rm "$mnt/filler$i.bin"; done
  sync
  gen frag $((3 * 65536 + 1000)) > "$mnt/docs/frag.bin"
  sync
  # Edited: written, committed, then grown so that the journal holds an
  # older copy of the inode with a smaller size; the newest copy must win.
  gen edited 20000 > "$mnt/docs/edited.bin"
  sync
  gen edited2 30000 >> "$mnt/docs/edited.bin"
  sync
  mv "$mnt/docs/renamed-old.txt" "$mnt/docs/renamed-new.txt"
  sync

  python3 - "$mnt" "$out/$flavour.manifest.json" "$flavour" "$label" <<'PY'
import hashlib, json, os, subprocess, sys
mnt, manifest, flavour, label = sys.argv[1:5]
journaled = flavour != "ext2"
def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()
def extents(p):
    out = subprocess.run(["filefrag", "-v", p], capture_output=True, text=True).stdout
    n = 0
    for line in out.splitlines():
        parts = line.split(":")
        if parts and parts[0].strip().isdigit():
            n += 1
    return n
# Recoverable through the journal only: ext2 clears the block map and the
# size on deletion, so there the files are found but nothing can be located.
def journal_only(expect):
    if journaled:
        return expect
    return {"exact": False, "max": "unrecoverable", "size_unknown": True, "layout": False}
files = [
    ("/small.txt", "A", journal_only({"exact": True, "min": "very good"})),
    ("/docs/medium.bin", "B", journal_only({"exact": True, "min": "very good"})),
    ("/empty.txt", "E", journal_only({"exact": True, "min": "excellent", "empty": True})),
    ("/docs/A long name with spaces ünï 文件.txt", "L", journal_only({"exact": True, "min": "very good"})),
    ("/docs/photo.jpg", "V", journal_only({"exact": True, "min": "very good", "type": "jpeg", "validation": "valid"})),
    ("/gone/keep.txt", "H", journal_only({"exact": True, "min": "very good", "via_deleted_dir": True})),
    ("/d/reused.bin", "D", journal_only({"exact": False, "max": "very poor", "reallocated": True})),
    ("/docs/frag.bin", "C", journal_only({"exact": True, "min": "poor"})),
    ("/docs/sparse.bin", "S", journal_only({"exact": True, "min": "good", "sparse": True})),
    ("/docs/edited.bin", "J", journal_only({"exact": True, "min": "very good"})),
]
entries = []
for path, scenario, expect in files:
    p = os.path.join(mnt, path.lstrip("/"))
    st = os.stat(p)
    e = {"path": path, "inode": st.st_ino, "size": st.st_size, "sha256": sha(p), "scenario": scenario, "expect": expect}
    if journaled and st.st_size > 0:
        e["extents"] = extents(p)
    entries.append(e)
absent = [{"path": "/docs/renamed-old.txt", "reason": "renamed, still live"}]
json.dump({"image": f"{flavour}.img.gz", "flavour": flavour, "label": label, "journaled": journaled,
           "files": entries, "absent": absent}, open(manifest, "w"), indent=2, ensure_ascii=False)
PY
  rm "$mnt/d/reused.bin"
  sync
  gen intruder 1048576 > "$mnt/x/intruder.bin"
  sync
  rm "$mnt/small.txt" "$mnt/docs/medium.bin" "$mnt/empty.txt" "$mnt/docs/A long name with spaces ünï 文件.txt" \
     "$mnt/docs/photo.jpg" "$mnt/docs/frag.bin" "$mnt/docs/sparse.bin" "$mnt/docs/edited.bin"
  rm -r "$mnt/gone"
  sync
  umount "$mnt"
  losetup -d "$loop"; loop=""
  gzip -9 -n -c "$img" > "$out/$flavour.img.gz"
}

build ext4 4096 PHXEXT4
build ext3 4096 PHXEXT3
build ext2 1024 PHXEXT2
echo "fixtures written to $out"; ls -l "$out"
