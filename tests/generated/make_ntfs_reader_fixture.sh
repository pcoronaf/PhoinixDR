#!/usr/bin/env bash
# Builds tests/fixtures/ntfs/reader.img.gz: an NTFS volume with allocated
# files covering the M3 acceptance cases (resident, contiguous, fragmented,
# partial final cluster, empty, sparse, Unicode name, nested directories,
# alternate data stream), plus reader.manifest.json with SHA-256 ground truth.
#
# Requires: mkntfs and ntfs-3g (FUSE), python3, gzip. Run as root.
set -euo pipefail

out="${1:-$(dirname "$0")/../fixtures/ntfs}"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'fusermount -u "$work/mnt" 2>/dev/null || true; rm -rf "$work"' EXIT

img="$work/reader.img"
truncate -s 64M "$img"
mkntfs -F -q -s 512 -c 4096 -L PHXREADER "$img" >/dev/null 2>&1
mkdir -p "$work/mnt"
ntfs-3g "$img" "$work/mnt"
m="$work/mnt"

# Deterministic content generator: 32-byte records stamped with the file name
# and byte offset, so misordered or missing extents are detectable while the
# fixture stays compressible.
gen() { python3 - "$1" "$2" <<'PY'
import sys
name, size = sys.argv[1], int(sys.argv[2])
out = bytearray()
i = 0
while len(out) < size:
    out += f"{name[:8]:<8}{i*32:016x}-------\n".encode(); i += 1
sys.stdout.buffer.write(bytes(out[:size]))
PY
}

mkdir -p "$m/docs/nested/deeper"
gen resident 200            > "$m/resident.txt"
gen empty 0                 > "$m/empty.txt"
gen contiguous 1048576      > "$m/docs/contiguous_1mib.bin"
gen partial 5000            > "$m/docs/nested/partial_cluster.bin"
gen unicode 3000            > "$m/docs/nested/deeper/ünïcödé 文件 🚀.txt"
gen exact 8192              > "$m/docs/exact_two_clusters.bin"
# Sparse file: 2 MiB with data only in the middle.
truncate -s 2097152 "$m/sparse.bin"
gen sparse-mid 4096 | dd of="$m/sparse.bin" bs=4096 seek=100 conv=notrunc status=none
# Fragmented file: lay down adjacent fillers, exhaust the remaining free
# space, free every other filler, then write a file that can only be
# satisfied by the resulting holes.
for i in $(seq 1 40); do head -c 131072 /dev/zero | tr '\0' "$(printf '\\%03o' $((65 + i % 26)))" > "$m/filler_$i.bin"; done
dd if=/dev/zero of="$m/bigfill.bin" bs=1M status=none 2>/dev/null || true
sync
for i in $(seq 1 40 | awk 'NR % 2 == 1'); do rm "$m/filler_$i.bin"; done
gen fragmented 1310720 > "$m/docs/fragmented.bin"
rm "$m/bigfill.bin"
for i in $(seq 1 40 | awk 'NR % 2 == 0'); do rm "$m/filler_$i.bin"; done
# Alternate data stream via the ntfs-3g xattr interface.
gen ads 777 > "$work/ads.bin"
setfattr -n user.secret -v "$(gen ads 100 | base64 -w0)" "$m/docs/exact_two_clusters.bin" 2>/dev/null || true
sync
fusermount -u "$m"

python3 - "$img" "$out/reader.manifest.json" <<'PY'
import hashlib, json, os, sys
img, manifest = sys.argv[1], sys.argv[2]
def gen(name, size):
    out = bytearray(); i = 0
    while len(out) < size:
        out += f"{name[:8]:<8}{i*32:016x}-------\n".encode(); i += 1
    return bytes(out[:size])
def sparse():
    b = bytearray(2097152); b[409600:409600+4096] = gen('sparse-mid', 4096); return bytes(b)
files = {
    "\\resident.txt": gen('resident', 200),
    "\\empty.txt": b"",
    "\\docs\\contiguous_1mib.bin": gen('contiguous', 1048576),
    "\\docs\\nested\\partial_cluster.bin": gen('partial', 5000),
    "\\docs\\nested\\deeper\\ünïcödé 文件 🚀.txt": gen('unicode', 3000),
    "\\docs\\exact_two_clusters.bin": gen('exact', 8192),
    "\\sparse.bin": sparse(),
    "\\docs\\fragmented.bin": gen('fragmented', 1310720),
}
entries = [{"path": p, "size": len(d), "sha256": hashlib.sha256(d).hexdigest()} for p, d in files.items()]
json.dump({"image": "reader.img.gz", "label": "PHXREADER", "files": entries,
           "ads": {"path": "\\docs\\exact_two_clusters.bin", "stream": "secret"}}, open(manifest, 'w'), indent=2, ensure_ascii=False)
PY
gzip -9 -n -c "$img" > "$out/reader.img.gz"
echo "fixture written to $out"; ls -l "$out"
