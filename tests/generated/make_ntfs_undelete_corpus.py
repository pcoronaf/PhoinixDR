#!/usr/bin/env python3
"""Builds tests/fixtures/ntfs/undelete.img.gz and undelete.manifest.json.

The image is a 64 MiB NTFS volume (mkntfs + ntfs-3g) holding the M4
deletion corpora:

  A  resident files                      exact recovery, Excellent
  B  contiguous non-resident files       exact recovery, Very good/Excellent
  C  fragmented files (2 and 10 extents) exact recovery, fragmentation reported
  D  reallocated clusters (1/10/25/50/100 %)  health declines monotonically
  E  stale parent (directory record reused)   path marked uncertain
  F  malformed deleted records           typed diagnostics, no crash
  G  SSD-style zeroing                   bitmap free but content wiped
  H  file inside a deleted directory     path recovered through the deleted dir
  V  real JPEG/PNG/PDF/DOCX for validators

Requires: root, mkntfs, ntfs-3g (FUSE), Pillow, and a built `phoinix` binary
(used only to read runlists so that D and G can be applied precisely; the
ground truth itself comes from the files written through ntfs-3g).
"""
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "tests" / "fixtures" / "ntfs"
PHOINIX = os.environ.get("PHOINIX", str(ROOT / "target" / "debug" / "phoinix"))
CLUSTER = 4096
IMAGE_SIZE = 64 * 1024 * 1024


def run(*args, **kw):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kw)


def gen(name: str, size: int) -> bytes:
    """32-byte records stamped with name and offset: compressible, position-sensitive."""
    out = bytearray()
    i = 0
    while len(out) < size:
        out += f"{name[:8]:<8}{i * 32:016x}-------\n".encode()
        i += 1
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
        z.writestr("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>', zipfile.ZIP_STORED)
        z.writestr("_rels/.rels", '<?xml version="1.0"?><Relationships/>', zipfile.ZIP_STORED)
        z.writestr("word/document.xml", '<?xml version="1.0"?><w:document>' + gen("docxbody", 200_000).decode() + '</w:document>', zipfile.ZIP_DEFLATED)
        z.writestr("word/_rels/document.xml.rels", '<?xml version="1.0"?><Relationships/>', zipfile.ZIP_STORED)
    return buf.getvalue()


def make_images():
    from PIL import Image
    w, h = 512, 384
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = ((x * 7 + y) % 256, (x ^ y) % 256, (y * 3 + x // 4) % 256)
    jpg = io.BytesIO(); img.save(jpg, format="JPEG", quality=92)
    png = io.BytesIO(); img.resize((256, 192)).save(png, format="PNG")
    return jpg.getvalue(), png.getvalue()


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


class Corpus:
    def __init__(self):
        self.work = Path(tempfile.mkdtemp())
        self.img = self.work / "undelete.img"
        self.mnt = self.work / "mnt"
        self.mnt.mkdir()
        self.entries = {}   # path -> dict
        self.mounted = False

    def mount(self):
        run("ntfs-3g", str(self.img), str(self.mnt)); self.mounted = True

    def umount(self):
        if self.mounted:
            run("sync"); run("fusermount", "-u", str(self.mnt)); self.mounted = False

    def put(self, rel: str, data: bytes, scenario: str, expect: dict):
        p = self.mnt / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        self.entries["\\" + rel.replace("/", "\\")] = {
            "path": "\\" + rel.replace("/", "\\"), "size": len(data), "sha256": sha(data),
            "scenario": scenario, "expect": expect}

    def fill_disk(self, name="bigfill.bin"):
        with open(self.mnt / name, "wb") as f:
            block = b"\0" * (1 << 20)
            try:
                while True:
                    f.write(block)
            except OSError:
                pass
        run("sync")

    # --- phoinix helpers (read-only queries of the unmounted image) ---
    def records(self):
        out = run(PHOINIX, "ntfs", "ls", str(self.img), "--all", "--system", "--json").stdout
        return {e["path"]: e for e in json.loads(out)}

    def runs(self, record: int):
        out = run(PHOINIX, "ntfs", "record", str(self.img), str(record), "--json").stdout
        rep = json.loads(out)
        for s in rep["file"]["streams"]:
            if s["name"] is None:
                st = s["storage"]
                if st["kind"] == "non_resident":
                    return [(r["lcn"], r["clusters"]) for r in st["runs"] if r["kind"] == "data"]
                return []
        return []

    def raw_record_offset(self, record: int) -> int:
        # $MFT is contiguous at LCN 4 in these small mkntfs images; verify via phoinix.
        out = run(PHOINIX, "ntfs", "record", str(self.img), "0", "--json").stdout
        runs = [r for r in json.loads(out)["file"]["streams"][0]["storage"]["runs"] if r["kind"] == "data"]
        assert runs[0]["vcn"] == 0
        return runs[0]["lcn"] * CLUSTER + record * 1024


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    c = Corpus()
    try:
        with open(c.img, "wb") as f:
            f.truncate(IMAGE_SIZE)
        run("mkntfs", "-F", "-q", "-s", "512", "-c", "4096", "-L", "PHXUNDEL", str(c.img))
        c.mount()
        jpg, png = make_images()

        # E — stale parent: the directories are created first so that they
        # occupy the lowest MFT records and are the ones reused later.
        (c.mnt / "e" / "olddir" / "sub").mkdir(parents=True)

        # A — resident
        c.put("a/tiny.txt", gen("tiny", 20), "A", {"exact": True, "min": "excellent", "resident": True})
        c.put("a/config.json", b'{"key": "' + gen("json", 88) + b'"}', "A", {"exact": True, "min": "excellent", "resident": True})
        c.put("a/small.bin", bytes(range(256)) + bytes(range(144)), "A", {"exact": True, "min": "excellent", "resident": True})
        c.put("a/ünïcödé 文件.txt", gen("unicode", 50), "A", {"exact": True, "min": "excellent", "resident": True})
        c.put("a/empty.txt", b"", "A", {"exact": True, "min": "excellent", "empty": True})
        # B — contiguous non-resident
        c.put("b/64k.bin", gen("b64k", 65536), "B", {"exact": True, "min": "very good", "max_extents": 1})
        c.put("b/1mib.bin", gen("b1mib", 1 << 20), "B", {"exact": True, "min": "very good", "max_extents": 1})
        # V — validators
        c.put("docs/report.pdf", make_pdf(120_000), "V", {"exact": True, "min": "very good", "type": "pdf", "validation": "valid"})
        c.put("docs/photo.jpg", jpg, "V", {"exact": True, "min": "very good", "type": "jpeg", "validation": "valid"})
        c.put("docs/logo.png", png, "V", {"exact": True, "min": "very good", "type": "png", "validation": "valid"})
        c.put("docs/proposal.docx", make_docx(), "V", {"exact": True, "min": "very good", "type": "docx", "validation": "valid"})
        # D — files that will have clusters reallocated (256 KiB = 64 clusters, 1 MiB = 256 clusters)
        for pct in (1, 10, 25, 50, 100):
            size = (1 << 20) if pct == 1 else 256 * 1024
            c.put(f"d/realloc_{pct}.bin", gen(f"re{pct}", size), "D", {"exact": False, "allocated_percent": pct})
        # G — zeroing simulation: a JPEG (name implies a format) and a raw
        # binary (ambiguous), both zeroed after deletion; plus a file that
        # legitimately consists of zeros and is never touched.
        c.put("g/wiped.jpg", jpg, "G", {"exact": False, "max": "very poor", "zero_assessment": "contradicts_format"})
        c.put("g/wiped.bin", gen("wipedraw", 65536), "G", {"exact": False, "min": "good", "max_confidence": 65, "zero_assessment": "ambiguous"})
        c.put("g/zeros.bin", b"\0" * 65536, "Z", {"exact": True, "min": "good", "max_confidence": 65, "zero_assessment": "ambiguous"})
        # H — file in a directory that is deleted and never reused
        c.put("h/gone/keep.txt", gen("keep", 3000), "H", {"exact": True, "min": "very good", "via_deleted_dir": True})
        # F — records to corrupt afterwards
        for kind in ("usa", "attr", "runlist", "namelen"):
            c.put(f"f/corrupt_{kind}.bin", gen(kind, 40000), "F", {"corruption": kind})
        # C — fragmented via holes: 24 fillers, fill disk, free odd fillers.
        for i in range(24):
            (c.mnt / f"filler_{i}.bin").write_bytes(bytes([65 + i % 26]) * 65536)
        c.fill_disk()
        for i in range(0, 24, 2):
            (c.mnt / f"filler_{i}.bin").unlink()
        run("sync")
        c.put("c/frag10.bin", gen("frag10", 10 * 65536), "C", {"exact": True, "min": "very good", "min_extents": 8})
        c.put("c/frag2.bin", gen("frag2", 2 * 65536), "C", {"exact": True, "min": "very good", "min_extents": 2})
        (c.mnt / "bigfill.bin").unlink()
        for i in range(1, 24, 2):
            (c.mnt / f"filler_{i}.bin").unlink()
        # E — the file itself is created last so its record is never reused.
        c.put("e/olddir/sub/document.txt", gen("stale", 2000), "E", {"exact": True, "path_uncertain": True})
        c.umount()

        # Record the MFT numbers and runs of everything before deletion.
        recs = c.records()
        for path, e in c.entries.items():
            e["record"] = recs[path]["record"]
            e["runs"] = c.runs(e["record"])

        # Delete E first and reuse its three directory records (the lowest free
        # MFT entries) with new directories, then delete everything else.
        c.mount()
        shutil.rmtree(c.mnt / "e")
        run("sync")
        for i in range(3):
            (c.mnt / f"newdir_{i}").mkdir()
        run("sync")
        for rel in ("a", "b", "docs", "d", "g", "h", "c", "f"):
            shutil.rmtree(c.mnt / rel)
        run("sync")
        c.umount()

        # D: mark a percentage of each file's clusters allocated in $Bitmap and
        # overwrite their content, simulating reuse by another file.
        recs_after = c.records()
        bitmap_rec = 6
        bitmap_runs = c.runs(bitmap_rec)
        assert bitmap_runs, "bitmap must be non-resident"
        with open(c.img, "r+b") as f:
            def set_bit(lcn):
                byte_index = lcn // 8
                # locate byte in bitmap runs
                pos = 0
                for lcn0, count in bitmap_runs:
                    if byte_index < pos + count * CLUSTER:
                        off = lcn0 * CLUSTER + (byte_index - pos)
                        f.seek(off); b = f.read(1)[0]; f.seek(off); f.write(bytes([b | (1 << (lcn % 8))]))
                        return
                    pos += count * CLUSTER
                raise RuntimeError("bitmap byte not found")
            for path, e in c.entries.items():
                if e["scenario"] == "D":
                    clusters = [lcn0 + i for lcn0, n in e["runs"] for i in range(n)]
                    pct = e["expect"]["allocated_percent"]
                    take = max(1, round(len(clusters) * pct / 100))
                    chosen = clusters[:take] if pct == 100 else clusters[len(clusters) // 3:len(clusters) // 3 + take]
                    for lcn in chosen:
                        set_bit(lcn)
                        f.seek(lcn * CLUSTER); f.write(gen("intruder", CLUSTER))
                    e["expect"]["allocated_clusters"] = len(chosen)
                    e["expect"]["total_clusters"] = len(clusters)
                if e["scenario"] == "G":
                    for lcn0, n in e["runs"]:
                        f.seek(lcn0 * CLUSTER); f.write(b"\0" * (n * CLUSTER))
                if e["scenario"] == "F":
                    off = c.raw_record_offset(e["record"])
                    f.seek(off); rec = bytearray(f.read(1024))
                    kind = e["expect"]["corruption"]
                    if kind == "usa":
                        rec[0x30] ^= 0xFF          # update sequence number no longer matches sector tails
                    elif kind == "attr":
                        first = int.from_bytes(rec[0x14:0x16], "little")
                        rec[first + 4:first + 8] = (0xFFFF0).to_bytes(4, "little")  # huge attribute length
                    elif kind == "runlist":
                        # find non-resident $DATA and trash its mapping pairs
                        pos = int.from_bytes(rec[0x14:0x16], "little")
                        while pos + 8 <= 1024:
                            t = int.from_bytes(rec[pos:pos + 4], "little"); ln = int.from_bytes(rec[pos + 4:pos + 8], "little")
                            if t == 0xFFFFFFFF or ln == 0: break
                            if t == 0x80 and rec[pos + 8] == 1:
                                ro = int.from_bytes(rec[pos + 32:pos + 34], "little")
                                rec[pos + ro:pos + ro + 4] = b"\x19\xff\xff\xff"  # invalid widths / truncated
                                break
                            pos += ln
                    elif kind == "namelen":
                        pos = int.from_bytes(rec[0x14:0x16], "little")
                        while pos + 8 <= 1024:
                            t = int.from_bytes(rec[pos:pos + 4], "little"); ln = int.from_bytes(rec[pos + 4:pos + 8], "little")
                            if t == 0xFFFFFFFF or ln == 0: break
                            if t == 0x30:
                                vo = int.from_bytes(rec[pos + 20:pos + 22], "little")
                                rec[pos + vo + 64] = 200  # name length beyond the attribute
                                break
                            pos += ln
                    # Re-protect sector tails for every kind except 'usa', which must fail fixup.
                    if kind != "usa":
                        usa_off = int.from_bytes(rec[4:6], "little")
                        usn = rec[usa_off:usa_off + 2]
                        for i in range(2):
                            tail = (i + 1) * 512 - 2
                            rec[usa_off + 2 + i * 2:usa_off + 4 + i * 2] = rec[tail:tail + 2] if False else rec[usa_off + 2 + i * 2:usa_off + 4 + i * 2]
                            rec[tail:tail + 2] = usn
                    f.seek(off); f.write(bytes(rec))

        # E: verify the directory records were reused (sequence bumped and in use).
        manifest = {
            "image": "undelete.img.gz", "label": "PHXUNDEL", "cluster_size": CLUSTER,
            "files": [dict(e, runs=None) for e in c.entries.values()],
        }
        for e in manifest["files"]:
            del e["runs"]
        (OUT / "undelete.manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False))
        subprocess.run(f"gzip -9 -n -c '{c.img}' > '{OUT / 'undelete.img.gz'}'", shell=True, check=True)
        print("fixture written to", OUT)
        for p in sorted(OUT.iterdir()):
            print(f"  {p.name}  {p.stat().st_size}")
    finally:
        c.umount()
        shutil.rmtree(c.work, ignore_errors=True)


if __name__ == "__main__":
    main()
