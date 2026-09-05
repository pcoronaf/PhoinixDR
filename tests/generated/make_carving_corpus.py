#!/usr/bin/env python3
"""Builds the deep-scan (carving) fixture: tests/fixtures/carve/corpus.img.gz
plus corpus.manifest.json.

A 32 MiB FAT32 volume (1 KiB clusters) holding one file of every built-in
carving signature, in three situations:

  V  deleted, directory entry intact     found by metadata and by carving; the
                                         carved hit is merged into the metadata
                                         candidate
  O  deleted, directory entry wiped      found by carving only ("orphans")
  F  fragmented PNG, entry wiped         the second half of the file was moved
                                         elsewhere and its original clusters
                                         hold other data: carving cannot follow
                                         the fragment, damaged, not exact
  L  still allocated                     not carved from unallocated space;
                                         found with --carve-all

Requires mtools, Pillow and the standard library only.
"""
import hashlib
import io
import json
import os
import sqlite3
import struct
import subprocess
import sys
import tempfile
import wave
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "tests" / "fixtures" / "carve"


def run(*args, **kw):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kw)


def gen(name: str, size: int) -> bytes:
    out = bytearray(); i = 0
    while len(out) < size:
        out += f"{name[:8]:<8}{i * 32:016x}-------\n".encode(); i += 1
    return bytes(out[:size])


def image(kind: str, size=(160, 120), fmt="JPEG", noise=False, **kw) -> bytes:
    """A deterministic test picture; `noise` makes it incompressible so that
    PNG output spans many clusters."""
    import random
    from PIL import Image
    rng = random.Random(sum(map(ord, kind)))
    img = Image.new("RGB", size)
    px = img.load()
    for y in range(size[1]):
        for x in range(size[0]):
            if noise:
                px[x, y] = (rng.randrange(256), rng.randrange(256), rng.randrange(256))
            else:
                px[x, y] = ((x * 7 + y) % 256, (x ^ y) % 256, (y * 3 + x) % 256)
    b = io.BytesIO(); img.save(b, format=fmt, **kw); return b.getvalue()


def make_pdf(payload_len: int, update: bool) -> bytes:
    pdf = "%PDF-1.4\n%âãÏÓ\n"
    o1 = len(pdf); pdf += "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    o2 = len(pdf); pdf += "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
    o3 = len(pdf); pdf += f"3 0 obj\n<< /Length {payload_len} >>\nstream\n" + gen("pdfbody", payload_len).decode() + "\nendstream\nendobj\n"
    xref = len(pdf)
    pdf += "xref\n0 4\n0000000000 65535 f \n" + "".join(f"{o:010} 00000 n \n" for o in (o1, o2, o3))
    pdf += f"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    if update:
        o4 = len(pdf); pdf += "4 0 obj\n<< /Type /Annot /Subtype /Text >>\nendobj\n"
        x2 = len(pdf)
        pdf += f"xref\n4 1\n{o4:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {xref} >>\nstartxref\n{x2}\n%%EOF\n"
    return pdf.encode("latin1")


class Unseekable(io.RawIOBase):
    """Forces zipfile to write data descriptors (streaming mode)."""
    def __init__(self):
        super().__init__(); self.buf = io.BytesIO()
    def writable(self): return True
    def seekable(self): return False
    def write(self, b): return self.buf.write(b)


def make_docx() -> bytes:
    sink = Unseekable()
    with zipfile.ZipFile(sink, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", "<Types/>")
        z.writestr("_rels/.rels", "<Relationships/>")
        z.writestr("word/document.xml", "<w:document>" + gen("docxbody", 30000).decode() + "</w:document>")
    return sink.buf.getvalue()


def make_zip() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        z.writestr("readme.txt", gen("readme", 3000))
        z.writestr("data/table.csv", gen("csv", 9000))
    return buf.getvalue()


def make_sqlite() -> bytes:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "notes.sqlite"
        con = sqlite3.connect(path)
        con.execute("create table notes(id integer primary key, body text)")
        con.executemany("insert into notes(body) values (?)", [(f"note {i} " * 40,) for i in range(300)])
        con.commit(); con.execute("vacuum"); con.close()
        return path.read_bytes()


def make_wav() -> bytes:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
        w.writeframes(bytes((i * 37) % 256 for i in range(16000)))
    return buf.getvalue()


def box(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def make_mp4() -> bytes:
    return box(b"ftyp", b"isom\0\0\x02\0isomiso2mp41") + box(b"moov", gen("moov", 400)) + box(b"mdat", gen("mdat", 20000))


def make_7z(packed: int, header: int) -> bytes:
    fields = struct.pack("<QQI", packed, header, 0xDEADBEEF)
    return b"7z\xbc\xaf\x27\x1c" + b"\x00\x04" + struct.pack("<I", zlib.crc32(fields)) + fields + gen("7zdata", packed + header)


class Image_:
    def __init__(self, path: Path):
        self.path = path
        self.files = {}

    def m(self, tool, *args):
        return run(tool, "-i", str(self.path), *args)

    def put(self, target: str, data: bytes, scenario: str, kind: str, expect: dict):
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.write(data); tmp = f.name
        try:
            self.m("mcopy", "-o", tmp, "::" + target)
        finally:
            os.unlink(tmp)
        self.files[target] = {"path": target, "type": kind, "size": len(data),
                              "sha256": hashlib.sha256(data).hexdigest(), "scenario": scenario, "expect": expect}

    def delete(self, target: str):
        self.m("mdel", "::" + target)

    def mkdir(self, target: str):
        self.m("mmd", "::" + target)


class Layout:
    """Reads FAT32 geometry and directory entries straight from the image."""
    def __init__(self, img: Path):
        self.img = img
        d = self.d = bytearray(img.read_bytes())
        self.bps = struct.unpack_from("<H", d, 11)[0]; spc = d[13]; rsv = struct.unpack_from("<H", d, 14)[0]
        nf = d[16]; spf = struct.unpack_from("<I", d, 36)[0]
        self.fat_off = rsv * self.bps; self.fat_bytes = spf * self.bps; self.nf = nf
        self.data_off = self.fat_off + nf * self.fat_bytes
        self.cs = self.bps * spc
        self.root_cluster = struct.unpack_from("<I", d, 44)[0]

    def cluster_off(self, c): return self.data_off + (c - 2) * self.cs

    def entries(self, cluster):
        """Yields (offset, short entry, long name) for a single-cluster directory."""
        base = self.cluster_off(cluster)
        pieces = []
        for i in range(0, self.cs, 32):
            e = self.d[base + i:base + i + 32]
            if e[0] == 0: break
            if e[11] == 0x0F:
                if e[0] != 0xE5:
                    chars = e[1:11] + e[14:26] + e[28:32]
                    text = chars.decode("utf-16-le", errors="replace").split("\0", 1)[0].rstrip("\uffff")
                    pieces.insert(0, text)
                continue
            long_name = "".join(pieces) if pieces else None
            pieces = []
            yield base + i, e, long_name

    def subdir_cluster(self, name: str):
        for _, e, _ in self.entries(self.root_cluster):
            if e[:11].decode("latin1").strip() == name and e[11] & 0x10 and e[0] != 0xE5:
                return (struct.unpack_from("<H", e, 20)[0] << 16) | struct.unpack_from("<H", e, 26)[0]
        raise KeyError(name)

    def file_offsets(self, dirname: str):
        """{lower-case name: content offset} for the files of a directory."""
        out = {}
        for _, e, long_name in self.entries(self.subdir_cluster(dirname)):
            if e[11] & 0x10: continue
            first = (struct.unpack_from("<H", e, 20)[0] << 16) | struct.unpack_from("<H", e, 26)[0]
            stem, ext = e[:8].decode("latin1").strip(), e[8:11].decode("latin1").strip()
            name = long_name or (f"{stem}.{ext}" if ext else stem)
            out[name.lower()] = self.cluster_off(first)
        return out

    def fragment(self, offset: int, size: int, split: int, dest: int):
        """Moves bytes [offset+split, offset+size) to `dest` and overwrites the
        vacated span with foreign data, so that the file is no longer
        contiguous on disk."""
        tail = bytes(self.d[offset + split:offset + size])
        self.d[dest:dest + len(tail)] = tail
        self.d[offset + split:offset + size] = (b"XFOREIGN" * (len(tail) // 8 + 1))[:len(tail)]

    def wipe_directory(self, dirname: str):
        c = self.subdir_cluster(dirname)
        base = self.cluster_off(c)
        self.d[base:base + self.cs] = bytes(self.cs)

    def save(self):
        self.img.write_bytes(self.d)


def build(work: Path):
    img = work / "corpus.img"
    with open(img, "wb") as f:
        f.truncate(32 * 1024 * 1024)
    run("mkfs.vfat", "-F", "32", "-s", "2", "-n", "CARVE", str(img))
    im = Image_(img)
    im.mkdir("/v"); im.mkdir("/o"); im.mkdir("/l")
    valid = {"exact": True, "status": "valid"}
    # V — every type, deleted with the entry intact.
    im.put("/v/photo.jpg", image("jpg"), "V", "jpeg", {**valid, "merged": True})
    im.put("/v/diagram.png", image("png", fmt="PNG"), "V", "png", {**valid, "merged": True})
    im.put("/v/anim.gif", image("gif", size=(64, 48), fmt="GIF"), "V", "gif", {**valid, "merged": True})
    im.put("/v/bitmap.bmp", image("bmp", size=(40, 30), fmt="BMP"), "V", "bmp", {**valid, "merged": True})
    im.put("/v/report.pdf", make_pdf(30000, update=True), "V", "pdf", {**valid, "merged": True})
    im.put("/v/proposal.docx", make_docx(), "V", "docx", {**valid, "merged": True})
    im.put("/v/archive.zip", make_zip(), "V", "zip", {**valid, "merged": True})
    im.put("/v/notes.sqlite", make_sqlite(), "V", "sqlite", {**valid, "merged": True})
    im.put("/v/sound.wav", make_wav(), "V", "wav", {**valid, "merged": True})
    im.put("/v/clip.mp4", make_mp4(), "V", "mp4", {**valid, "merged": True})
    im.put("/v/bundle.7z", make_7z(5000, 60), "V", "7z", {**valid, "merged": True})
    # O — orphans: the directory holding them is wiped after deletion.
    im.put("/o/orphan.jpg", image("orphan", size=(200, 150)), "O", "jpeg", {**valid, "orphan": True})
    im.put("/o/orphan.pdf", make_pdf(12000, update=False), "O", "pdf", {**valid, "orphan": True})
    # F — a PNG that will be split in two after deletion, then orphaned.
    png = image("frag", size=(120, 90), fmt="PNG", noise=True)
    assert len(png) > 16 * 1024, len(png)
    im.put("/o/frag.png", png, "F", "png", {"exact": False, "status": "damaged", "orphan": True, "fragmented": True})
    # L — still allocated.
    im.put("/l/live.jpg", image("live", size=(96, 64)), "L", "jpeg", {**valid, "live": True})

    layout = Layout(img)
    offsets = {}
    for d in ("V", "O", "L"):
        offsets.update({f"/{d.lower()}/{k}": v for k, v in layout.file_offsets(d).items()})
    for path, meta in im.files.items():
        meta["offset"] = offsets[path.lower()]

    for path in list(im.files):
        if im.files[path]["scenario"] != "L":
            im.delete(path)
    layout = Layout(img)
    layout.wipe_directory("O")
    frag = im.files["/o/frag.png"]
    # Split at a cluster boundary in the middle; park the tail 8 MiB in.
    split = (frag["size"] // 2 // layout.cs) * layout.cs
    layout.fragment(frag["offset"], frag["size"], split, 8 * 1024 * 1024)
    frag["fragment_split"] = split
    layout.save()

    manifest = {"image": "corpus.img.gz", "filesystem": "FAT32", "cluster_size": layout.cs,
                "files": sorted(im.files.values(), key=lambda f: f["offset"])}
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "corpus.manifest.json").write_text(json.dumps(manifest, indent=2))
    subprocess.run(f"gzip -9 -n -c '{img}' > '{OUT / 'corpus.img.gz'}'", shell=True, check=True)


def main():
    work = Path(tempfile.mkdtemp())
    try:
        build(work)
    finally:
        for p in work.iterdir():
            p.unlink()
        work.rmdir()
    for p in sorted(OUT.iterdir()):
        print(f"  {p.name}  {p.stat().st_size}")


if __name__ == "__main__":
    main()
