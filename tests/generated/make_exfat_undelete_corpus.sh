#!/usr/bin/env bash
# Builds tests/fixtures/exfat/undelete.img.gz with mkfs.exfat and exfat-fuse
# over a loop device (root required), plus undelete.manifest.json.
set -euo pipefail
out="${1:-$(dirname "$0")/../fixtures/exfat}"
mkdir -p "$out"
work="$(mktemp -d)"
img="$work/undelete.img"
mnt="$work/mnt"
mkdir -p "$mnt"
cleanup() { umount "$mnt" 2>/dev/null || true; [ -n "${loop:-}" ] && losetup -d "$loop" 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT

truncate -s 64M "$img"
mkfs.exfat -L PHXEXFAT "$img" >/dev/null 2>&1
loop="$(losetup -f --show "$img")"
mount.exfat-fuse "$loop" "$mnt"

gen() { python3 - "$1" "$2" <<'PY'
import sys
name, size = sys.argv[1], int(sys.argv[2])
out = bytearray(); i = 0
while len(out) < size:
    out += f"{name[:8]:<8}{i*32:016x}-------\n".encode(); i += 1
sys.stdout.buffer.write(bytes(out[:size]))
PY
}

mkdir -p "$mnt/docs" "$mnt/gone" "$mnt/d" "$mnt/x"
# D first, so that it occupies the lowest clusters.
gen reused 65536 > "$mnt/d/reused.bin"
sync
python3 - "$mnt" <<'PY'
import io, sys
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
# Fragmented file via holes.
for i in 0 1 2 3 4 5 6 7; do head -c 16384 /dev/zero | tr '\0' "$(printf '\\%03o' $((65 + i)))" > "$mnt/filler$i.bin"; done
sync
for i in 0 2 4 6; do rm "$mnt/filler$i.bin"; done
sync
gen frag $((4 * 16384 - 100)) > "$mnt/docs/frag.bin"
sync
python3 - "$mnt" "$out/undelete.manifest.json" <<'PY'
import hashlib, json, os, sys
mnt, manifest = sys.argv[1], sys.argv[2]
def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()
files = [
    ("\\small.txt", "A", {"exact": True, "min": "very good"}),
    ("\\docs\\medium.bin", "A", {"exact": True, "min": "very good"}),
    ("\\empty.txt", "E", {"exact": True, "min": "excellent", "empty": True}),
    ("\\docs\\A long name with spaces ünï 文件.txt", "L", {"exact": True, "min": "very good"}),
    ("\\docs\\photo.jpg", "V", {"exact": True, "min": "very good", "type": "jpeg", "validation": "valid"}),
    ("\\gone\\keep.txt", "H", {"exact": True, "min": "very good", "via_deleted_dir": True}),
    ("\\d\\reused.bin", "D", {"exact": False, "max": "very poor", "reallocated": True}),
    ("\\docs\\frag.bin", "C", {"exact": True, "min": "poor"}),
]
entries = []
for path, scenario, expect in files:
    p = os.path.join(mnt, path.lstrip("\\").replace("\\", "/"))
    entries.append({"path": path, "size": os.path.getsize(p), "sha256": sha(p), "scenario": scenario, "expect": expect})
json.dump({"image": "undelete.img.gz", "label": "PHXEXFAT", "files": entries}, open(manifest, "w"), indent=2, ensure_ascii=False)
PY
rm "$mnt/d/reused.bin"
sync
gen intruder 65536 > "$mnt/x/intruder.bin"
sync
rm "$mnt/small.txt" "$mnt/docs/medium.bin" "$mnt/empty.txt" "$mnt/docs/A long name with spaces ünï 文件.txt" \
   "$mnt/docs/photo.jpg" "$mnt/docs/frag.bin"
rm -r "$mnt/gone"
sync
umount "$mnt"
losetup -d "$loop"; loop=""
gzip -9 -n -c "$img" > "$out/undelete.img.gz"
echo "fixture written to $out"; ls -l "$out"
