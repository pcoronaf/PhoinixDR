#!/usr/bin/env python3
"""Builds FAT12/FAT16/FAT32 deletion fixtures with mtools (no mounting).

Outputs tests/fixtures/fat/<variant>.img.gz plus <variant>.manifest.json.
Scenarios per image:

  A  small and medium contiguous files          exact (contiguous assumption)
  C  fragmented file (holes technique)          heuristic reconstruction, lower confidence
  D  reallocated clusters                       clusters of the deleted file reused by a new file
  H  file inside a deleted directory            path recovered through the deleted directory
  V  real JPEG / PDF / DOCX                     validators
  E  empty file                                 Excellent, validation not applicable
  L  long name with spaces and Unicode          LFN reconstructed from deleted entries

The fat32w image reproduces a Windows deletion on a large FAT32 volume
(more than 65 536 clusters): Windows clears the high word of the first
cluster, so the surviving low word points into the region occupied by older
files. Scenario W checks that the start is inferred from free clusters
sharing the low word and their content.
"""
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "tests" / "fixtures" / "fat"


def run(*args, **kw):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kw)


def gen(name: str, size: int) -> bytes:
    out = bytearray(); i = 0
    while len(out) < size:
        out += f"{name[:8]:<8}{i * 32:016x}-------\n".encode(); i += 1
    return bytes(out[:size])


def make_pdf(payload_len: int) -> bytes:
    pdf = "%PDF-1.4\n"
    o1 = len(pdf); pdf += "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    o2 = len(pdf); pdf += "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
    o3 = len(pdf); pdf += f"3 0 obj\n<< /Length {payload_len} >>\nstream\n" + gen("pdfbody", payload_len).decode() + "\nendstream\nendobj\n"
    xref = len(pdf)
    pdf += "xref\n0 4\n0000000000 65535 f \n" + "".join(f"{o:010} 00000 n \n" for o in (o1, o2, o3))
    pdf += f"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    return pdf.encode()


def make_docx() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("[Content_Types].xml", "<Types/>", zipfile.ZIP_STORED)
        z.writestr("word/document.xml", "<w:document>" + gen("docxbody", 60000).decode() + "</w:document>", zipfile.ZIP_DEFLATED)
    return buf.getvalue()


def make_jpeg() -> bytes:
    from PIL import Image
    img = Image.new("RGB", (320, 240))
    px = img.load()
    for y in range(240):
        for x in range(320):
            px[x, y] = ((x * 7 + y) % 256, (x ^ y) % 256, (y * 3) % 256)
    b = io.BytesIO(); img.save(b, format="JPEG", quality=90); return b.getvalue()


class Image_:
    def __init__(self, path: Path, variant: str):
        self.path = path
        self.variant = variant
        self.entries = {}

    def m(self, tool, *args):
        return run(tool, "-i", str(self.path), *args)

    def put(self, target: str, data: bytes, scenario: str, expect: dict):
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.write(data); tmp = f.name
        try:
            self.m("mcopy", "-o", tmp, "::" + target)
        finally:
            os.unlink(tmp)
        path = target.replace("/", "\\")
        self.entries[path] = {"path": path, "size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "scenario": scenario, "expect": expect}

    def delete(self, target: str):
        self.m("mdel", "::" + target)

    def deltree(self, target: str):
        self.m("mdeltree", "::" + target)

    def mkdir(self, target: str):
        self.m("mmd", "::" + target)


def reallocate(img: Path, dirname: str, short_name_tail: str):
    """Marks the clusters of a deleted file (found by its 8.3 name without the
    lost first character) as allocated and overwrites them."""
    import struct
    d = bytearray(open(img, "rb").read())
    bps = struct.unpack_from("<H", d, 11)[0]; spc = d[13]; rsv = struct.unpack_from("<H", d, 14)[0]
    nf = d[16]; root_entries = struct.unpack_from("<H", d, 17)[0]
    spf = struct.unpack_from("<H", d, 22)[0] or struct.unpack_from("<I", d, 36)[0]
    total = struct.unpack_from("<H", d, 19)[0] or struct.unpack_from("<I", d, 32)[0]
    fat_off = rsv * bps; fat_bytes = spf * bps
    root_off = fat_off + nf * fat_bytes; root_bytes = root_entries * 32
    data_off = root_off + root_bytes
    cs = bps * spc
    clusters = (total * bps - data_off) // cs
    # A FAT32 layout (no 16-bit sectors-per-FAT) is FAT32 whatever the count.
    variant = 32 if struct.unpack_from("<H", d, 22)[0] == 0 else (12 if clusters < 4085 else 16)
    def cluster_off(c): return data_off + (c - 2) * cs
    def entries(buf):
        for i in range(0, len(buf), 32):
            e = buf[i:i + 32]
            if e[0] == 0: break
            if e[11] == 0x0F: continue
            yield e
    if variant == 32:
        root_cluster = struct.unpack_from("<I", d, 44)[0]
        root = d[cluster_off(root_cluster):cluster_off(root_cluster) + cs]
    else:
        root = d[root_off:root_off + root_bytes]
    sub = next(e for e in entries(root) if e[:11].decode("latin1").strip() == dirname and e[11] & 0x10)
    sub_cluster = (struct.unpack_from("<H", sub, 20)[0] << 16) | struct.unpack_from("<H", sub, 26)[0]
    subdir = d[cluster_off(sub_cluster):cluster_off(sub_cluster) + cs]
    target = next(e for e in entries(subdir) if e[0] == 0xE5 and e[1:11].decode("latin1") == short_name_tail)
    first = (struct.unpack_from("<H", target, 20)[0] << 16) | struct.unpack_from("<H", target, 26)[0]
    size = struct.unpack_from("<I", target, 28)[0]
    n = -(-size // cs)
    chain = list(range(first, first + n))
    def set_entry(c, value):
        for f in range(nf):
            base = fat_off + f * fat_bytes
            if variant == 12:
                off = base + c + c // 2
                pair = struct.unpack_from("<H", d, off)[0]
                pair = (pair & 0x000F) | (value << 4) if c & 1 else (pair & 0xF000) | (value & 0x0FFF)
                struct.pack_into("<H", d, off, pair)
            elif variant == 16:
                struct.pack_into("<H", d, base + c * 2, value)
            else:
                struct.pack_into("<I", d, base + c * 4, value)
    eoc = {12: 0xFFF, 16: 0xFFFF, 32: 0x0FFFFFFF}[variant]
    for i, c in enumerate(chain):
        set_entry(c, chain[i + 1] if i + 1 < len(chain) else eoc)
        d[cluster_off(c):cluster_off(c) + cs] = gen("intruder", cs)
    open(img, "wb").write(d)


def clear_high_words(img: Path, dirname: str) -> int:
    """Simulates the Windows FAT32 driver, which zeroes the high word of the
    first cluster of every entry it deletes. Returns the number of entries
    patched; every one of them must have had a non-zero high word, or the
    scenario would not exercise the inference."""
    import struct
    d = bytearray(open(img, "rb").read())
    bps = struct.unpack_from("<H", d, 11)[0]; spc = d[13]; rsv = struct.unpack_from("<H", d, 14)[0]
    nf = d[16]; spf = struct.unpack_from("<I", d, 36)[0]
    fat_off = rsv * bps; data_off = fat_off + nf * spf * bps
    cs = bps * spc
    def cluster_off(c): return data_off + (c - 2) * cs
    root_cluster = struct.unpack_from("<I", d, 44)[0]
    root = d[cluster_off(root_cluster):cluster_off(root_cluster) + cs]
    sub = None
    for i in range(0, len(root), 32):
        e = root[i:i + 32]
        if e[0] == 0: break
        if e[11] != 0x0F and e[:11].decode("latin1").strip() == dirname and e[11] & 0x10:
            sub = e; break
    assert sub is not None, dirname
    sub_cluster = (struct.unpack_from("<H", sub, 20)[0] << 16) | struct.unpack_from("<H", sub, 26)[0]
    base = cluster_off(sub_cluster)
    patched = 0
    for i in range(0, cs, 32):
        e = d[base + i:base + i + 32]
        if e[0] == 0: break
        if e[0] != 0xE5 or e[11] == 0x0F or e[11] & 0x10: continue
        high = struct.unpack_from("<H", e, 20)[0]
        assert high != 0, "deleted entry must start above cluster 65535"
        struct.pack_into("<H", d, base + i + 20, 0)
        patched += 1
    open(img, "wb").write(d)
    return patched


def build_windows_deleted(work: Path):
    """fat32w: 40 MiB, 512-byte clusters (about 81 000 clusters)."""
    img = work / "fat32w.img"
    with open(img, "wb") as f:
        f.truncate(40 * 1024 * 1024)
    run("mkfs.vfat", "-F", "32", "-s", "1", "-n", "FAT32W", str(img))
    im = Image_(img, "fat32")
    im.mkdir("/x"); im.mkdir("/docs")
    # Older data filling the first 36 MiB: every low-word cluster of the
    # files deleted later lands inside this region.
    for i in range(9):
        im.put(f"/x/older{i}.bin", (f"older{i} ".encode() * (600 * 1024))[:4 * 1024 * 1024], "filler", {})
        im.entries.pop(f"\\x\\older{i}.bin")
    im.put("/docs/photo.jpg", make_jpeg(), "W", {"exact": True, "min": "good", "type": "jpeg", "validation": "valid", "inferred_start": True})
    im.put("/docs/report.pdf", make_pdf(40_000), "W", {"exact": True, "min": "good", "type": "pdf", "validation": "valid", "inferred_start": True})
    im.put("/docs/proposal.docx", make_docx(), "W", {"exact": True, "min": "good", "type": "docx", "validation": "valid", "inferred_start": True})
    im.put("/docs/notes.txt", gen("notes", 9_000), "W", {"exact": True, "min": "poor", "max": "poor", "inferred_start": True, "max_confidence": 60})
    for target in ("/docs/photo.jpg", "/docs/report.pdf", "/docs/proposal.docx", "/docs/notes.txt"):
        im.delete(target)
    patched = clear_high_words(img, "DOCS")
    assert patched == 4, patched
    manifest = {"image": "fat32w.img.gz", "variant": "fat32", "files": list(im.entries.values())}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "fat32w.manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False))
    subprocess.run(f"gzip -9 -n -c '{img}' > '{OUT / 'fat32w.img.gz'}'", shell=True, check=True)


def build(variant: str, size_mb: int, mkfs_args: list, work: Path):
    img = work / f"{variant}.img"
    with open(img, "wb") as f:
        f.truncate(size_mb * 1024 * 1024)
    run("mkfs.vfat", *mkfs_args, "-n", variant.upper(), str(img))
    im = Image_(img, variant)
    im.mkdir("/docs"); im.mkdir("/gone"); im.mkdir("/d"); im.mkdir("/x")
    jpg = make_jpeg()
    # D — created first so that it occupies the lowest clusters; after the
    # deletions a new file of the same size takes exactly those clusters.
    im.put("/d/reused.bin", gen("reused", 65_536), "D", {"exact": False, "max": "very poor", "reallocated": True})
    # A — contiguous
    im.put("/small.txt", gen("small", 700), "A", {"exact": True, "min": "very good", "max_confidence": 90})
    im.put("/docs/medium.bin", gen("medium", 200_000), "A", {"exact": True, "min": "very good"})
    # E — empty
    im.put("/empty.txt", b"", "E", {"exact": True, "min": "excellent", "empty": True})
    # L — long names
    im.put("/docs/A long name with spaces ünï.txt", gen("longname", 3000), "L", {"exact": True, "min": "very good", "long_name": True})
    # V — validators
    im.put("/docs/photo.jpg", jpg, "V", {"exact": True, "min": "very good", "type": "jpeg", "validation": "valid"})
    im.put("/docs/report.pdf", make_pdf(50_000), "V", {"exact": True, "min": "very good", "type": "pdf", "validation": "valid"})
    im.put("/docs/proposal.docx", make_docx(), "V", {"exact": True, "min": "very good", "type": "docx", "validation": "valid"})
    # H — file inside a directory that will be deleted
    im.put("/gone/keep.txt", gen("keep", 2500), "H", {"exact": True, "min": "very good", "via_deleted_dir": True})
    # C — fragmented: fillers, delete alternate ones, then a file that must
    # fragment into the holes (mtools allocates first-fit).
    for i in range(8):
        im.put(f"/filler{i}.bin", bytes([65 + i]) * 16_384, "filler", {})
    for i in range(8):
        im.entries.pop(f"\\filler{i}.bin")
    for i in range(0, 8, 2):
        im.delete(f"/filler{i}.bin")
    im.put("/docs/frag.bin", gen("frag", 4 * 16_384 - 100), "C", {"exact": True, "min": "poor"})
    im.delete("/d/reused.bin")
    for target in ("/small.txt", "/docs/medium.bin", "/empty.txt", "/docs/A long name with spaces ünï.txt",
                   "/docs/photo.jpg", "/docs/report.pdf", "/docs/proposal.docx", "/docs/frag.bin"):
        im.delete(target)
    im.deltree("/gone")
    # D: mark every cluster of the deleted reused.bin allocated in both FATs
    # and overwrite its content, exactly as a new file taking the space would.
    reallocate(img, "D", "EUSED  BIN")
    manifest = {"image": f"{variant}.img.gz", "variant": variant, "files": list(im.entries.values())}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{variant}.manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False))
    subprocess.run(f"gzip -9 -n -c '{img}' > '{OUT / (variant + '.img.gz')}'", shell=True, check=True)


def main():
    work = Path(tempfile.mkdtemp())
    try:
        build("fat12", 4, ["-F", "12", "-s", "2"], work)      # 4 MiB, 1 KiB clusters → FAT12
        build("fat16", 16, ["-F", "16", "-s", "4"], work)     # 16 MiB, 2 KiB clusters
        build("fat32", 48, ["-F", "32", "-s", "8"], work)     # 48 MiB, 4 KiB clusters
        build_windows_deleted(work)
    finally:
        for p in work.iterdir():
            p.unlink()
        work.rmdir()
    for p in sorted(OUT.iterdir()):
        print(f"  {p.name}  {p.stat().st_size}")


if __name__ == "__main__":
    main()
