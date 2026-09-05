#!/usr/bin/env bash
# Builds tests/fixtures/images: the FAT12 undelete corpus wrapped in every
# container PhoinixDR reads, each file gzipped separately (multi-file images
# keep their segment names, minus the .gz). Requires ewf-tools (ewfacquire)
# and qemu-utils (qemu-img).
set -euo pipefail
out="${1:-$(dirname "$0")/../fixtures/images}"
src_gz="$(dirname "$0")/../fixtures/fat/fat12.img.gz"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gunzip -c "$src_gz" > "$work/fat12.img"
cd "$work"

# EWF: EnCase 6 with best compression and a full acquisition header.
ewfacquire -u -t e01 -C "PHX-011" -D "FAT12 corpus" -E "EV-11" -e "Examiner Name" \
  -N "acquired for the M11 fixtures" -c best -f encase6 fat12.img >/dev/null
# EWF: uncompressed, split into 1 MiB segments (five files).
ewfacquire -u -t split -C "PHX-012" -c none -f encase6 -S 1MiB fat12.img >/dev/null
# EWF: SMART (s01) layout.
ewfacquire -u -t smart -c fast -f smart fat12.img >/dev/null
# VHD dynamic and fixed, VHDX, VMDK sparse / stream-optimized / 2 GiB extents.
qemu-img convert -f raw -O vpc -o subformat=dynamic fat12.img dyn.vhd
qemu-img convert -f raw -O vpc -o subformat=fixed fat12.img fixed.vhd
qemu-img convert -f raw -O vhdx -o subformat=dynamic fat12.img disk.vhdx
qemu-img convert -f raw -O vmdk -o subformat=monolithicSparse fat12.img sparse.vmdk
qemu-img convert -f raw -O vmdk -o subformat=streamOptimized fat12.img stream.vmdk
qemu-img convert -f raw -O vmdk -o subformat=twoGbMaxExtentSparse fat12.img twogb.vmdk
# Split RAW: four 1 MiB pieces.
split -b 1M -d -a 3 --additional-suffix= fat12.img raw. 
for f in $(ls raw.0* | sort -r); do n="${f#raw.}"; mv "$f" "raw.$(printf '%03d' $((10#$n + 1)))"; done

python3 - "$out" <<'PY'
import glob, gzip, hashlib, json, os, sys, shutil
out = sys.argv[1]
raw = open("fat12.img", "rb").read()
files = sorted(f for f in os.listdir(".") if f != "fat12.img")
for f in files:
    with open(f, "rb") as i, gzip.open(os.path.join(out, f + ".gz"), "wb", compresslevel=9) as o:
        shutil.copyfileobj(i, o)
manifest = {
    "raw_size": len(raw),
    "raw_sha256": hashlib.sha256(raw).hexdigest(),
    "raw_md5": hashlib.md5(raw).hexdigest(),
    "raw_sha1": hashlib.sha1(raw).hexdigest(),
    "files": files,
    "images": [
        {"open": "e01.E01", "format": "ewf", "variant": "E01 (EnCase/FTK)", "segments": 1, "stored_md5": True,
         "acquisition": {"case_number": "PHX-011", "evidence_number": "EV-11", "examiner": "Examiner Name",
                          "description": "FAT12 corpus", "notes": "acquired for the M11 fixtures"}},
        {"open": "split.E03", "format": "ewf", "variant": "E01 (EnCase/FTK)", "segments": 5, "stored_md5": True,
         "acquisition": {"case_number": "PHX-012"}},
        {"open": "smart.s01", "format": "ewf", "variant": "S01 (SMART)", "segments": 1, "stored_md5": True},
        {"open": "dyn.vhd", "format": "vhd", "variant": "dynamic", "segments": 1, "padded": True},
        {"open": "fixed.vhd", "format": "vhd", "variant": "fixed", "segments": 1, "padded": True},
        {"open": "disk.vhdx", "format": "vhdx", "variant": "dynamic", "segments": 1},
        {"open": "sparse.vmdk", "format": "vmdk", "variant": "monolithicSparse", "segments": 1},
        {"open": "stream.vmdk", "format": "vmdk", "variant": "streamOptimized", "segments": 1},
        {"open": "twogb.vmdk", "format": "vmdk", "variant": "twoGbMaxExtentSparse", "segments": 2},
        {"open": "raw.002", "format": "split-raw", "variant": "4 files", "segments": 4},
    ],
}
json.dump(manifest, open(os.path.join(out, "manifest.json"), "w"), indent=2)
PY
echo "fixtures written to $out"; ls -l "$out"
