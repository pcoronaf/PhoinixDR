# Deep scan: signature carving (`phoinix-carve`)

Quick scan reads filesystem metadata. Deep scan adds *carving*: the
unallocated space of the volume is searched for file headers, each hit is
walked according to its format to find where the file ends, and the result
becomes a `RecoveryCandidate` with the same evidence and health model as
metadata candidates. Carving is what recovers a file after its directory
entry was reused, after a quick format, or on a source without a
recognisable filesystem.

```text
AllocationView (NTFS $Bitmap, FAT, exFAT bitmap, ext block bitmaps; or the whole source)
        │  free byte ranges
   find_headers            chunked (8 MiB), overlapping, parallel matching
        │  hits (offset, signature)
   Assembler               per format: where does the file end, how sound is it
        │  length, end_known, checks, refined type
   evidence + validators   allocation of the span, health validators, zero sampling
        │
   RecoveryCandidate       FileSystemObjectId::Carved { offset, type_id, extension }
        │
   deduplicate             folded into a metadata candidate with the same start
```

## What is scanned

| mode | ranges | when |
|---|---|---|
| default (`--deep`) | free clusters of the volume, merged into byte ranges | a supported filesystem is present |
| `--carve-all` | the whole volume | files hidden inside allocated space, damaged allocation maps |
| raw | the whole source | no supported filesystem (FAT/NTFS/exFAT/ext absent): carving is the only engine |

Only positions that are multiples of `--carve-align` (default 512) are
tested: files start on sector or cluster boundaries. Alignment 1 tests
every byte and finds embedded objects, at a cost.

The header search reads chunks sequentially on one thread (USB media do not
like parallel reads) and matches on `--carve-threads` worker threads
(default: all cores) using `std::thread::scope`; no thread-pool dependency.

## Signatures and assemblers

| id | header | end determined by | validity |
|---|---|---|---|
| `jpeg` | `FF D8 FF` | marker walk, then the entropy data up to `FF D9`; an impossible marker inside the scan data ends the file as damaged | SOI, segments, SOF dimensions, SOS, EOI |
| `png` | `89 PNG …` | chunk walk to `IEND`, every chunk CRC verified | IHDR, chunk sequence, CRCs, IEND |
| `gif` | `GIF87a` / `GIF89a` | screen descriptor, colour tables, block walk to the `3B` trailer | header, screen, blocks, trailer |
| `bmp` | `BM` | declared file size (header plausibility checked) | DIB header size, offsets |
| `pdf` | `%PDF-` | `/Linearized … /L n` when present; otherwise the chain of `%%EOF` markers, continuing while the bytes after one look like an incremental update | header, startxref, %%EOF |
| `zip` | `PK 03 04` | local headers (sizes, zip64 extra, data descriptors located by the next signature), central directory, end record | entries, end record; refined to `docx`/`xlsx`/`pptx`/`odf`/`jar` from the entry names |
| `sqlite` | `SQLite format 3\0` | page size × page count (flagged when the count may be stale) | header fields |
| `riff` | `RIFF` | declared size; form `WAVE`/`AVI `/`WEBP` refines the type | form, first chunk |
| `mp4` | `ftyp` at offset 4 | top-level box walk until a box type that is not an ISO media box | ftyp, moov, mdat |
| `7z` | `37 7A BC AF 27 1C` | start header (CRC verified) locates the end header | signature, CRCs |

Every assembler is bounded: loop counts, `max_size` per signature, and the
end of the scanned region. A file whose structure runs past the region is
carved up to the region end and marked damaged. When the end cannot be
found the assembler returns the last plausible structural boundary
(`endobj`, the last valid marker) with `end_known = false`, which the
health model reports as an upper bound.

### Declarative signatures

Extra definitions can be passed as JSON with `--carve-signatures FILE`.
An entry with an existing id replaces the built-in one.

```json
[
  {
    "id": "xcf", "name": "GIMP image", "extension": "xcf",
    "headers": [{ "offset": 0, "hex": "67 69 6D 70 20 78 63 66" }],
    "max_size": 536870912,
    "assembler": "header_only"
  },
  {
    "id": "html", "name": "HTML page", "extension": "html",
    "headers": [{ "hex": "3C21444F4354595045" }],
    "footer_hex": "3C2F68746D6C3E",
    "max_size": 16777216,
    "assembler": "footer"
  }
]
```

`assembler` is one of `jpeg`, `png`, `gif`, `bmp`, `pdf`, `zip`, `sqlite`,
`riff`, `mp4`, `seven_zip`, `footer` (header plus footer search) or
`header_only` (the file extends to `max_size`; end unknown).

## Nested hits

Once a hit assembles into a sound file (valid or mostly valid), hits inside
its span are skipped: a thumbnail inside a JPEG, a member inside a ZIP, a
picture inside a PDF. Hits inside a damaged assembly are not skipped,
because the damage may mean the outer file is a false start.

## Evidence and scoring

A carved candidate carries `CandidateSource::FileCarving` and:

- **metadata**: no record, name, path or timestamps; `logical_size` is the
  assembled length and `logical_size_available` is whether the end was
  determined by the structure;
- **extents**: one extent, contiguity assumed (`chain_known = false`);
- **allocation**: the engine's summary of the span (free / allocated /
  unknown clusters), so a carved file that runs into live data is capped
  exactly like a metadata candidate with reused clusters;
- **content**: the assembler's checks merged with the health validators
  (the worse status wins), type detection, zero sampling.

Scoring (see `docs/recovery/health-model.md`): a validated carved file is
capped at 85 (Very good), one without a validator at 74, one whose end is
unknown at 59; confidence loses 15 for the missing metadata and 10 for the
contiguity assumption. Reused clusters, damaged structure and zero-filled
content apply their usual caps.

## Cost, early exit and unreadable regions

A deep scan has two stages, both reported through `ScanProgress`
(`CarveStage::Search`, then `Assemble`):

1. **Header search**: one sequential pass over the eligible ranges in
   8 MiB chunks. Its cost is the free space at the drive's streaming speed.
2. **Assembly**: for every hit, the assembler walks the structure from the
   hit through a cached 256 KiB probe window, then the content is examined.
   This stage reads the source again, one hit at a time, and its cost is
   what the hits point at, not the free space. Progress reports hits
   examined, candidates produced and bytes read.

Three rules keep the second stage proportional to the real files it finds
rather than to the size limits of the signatures:

- **Early exit.** A JPEG's entropy scan stops when a whole probe window
  contains no `FF` byte at all (real entropy data carries a stuffed `FF`
  every few hundred bytes), and a footer search stops when a whole window
  is zero-filled. Both report the file as damaged and end it there instead
  of walking on to the size limit (128 MiB for a JPEG). A header sitting on
  overwritten, discarded or wiped data therefore costs one window, not a
  hundred megabytes.
- **Bounded examination.** For carved files the validators read at most
  `CarveOptions::byte_budget` (8 MiB by default: the assembler has already
  walked the structure, the validator only needs the head) and zero
  sampling takes `zero_samples` blocks (8 by default; each is a seek on a
  rotational device). The filesystem engines keep the 256 MiB budget and
  64 samples for metadata candidates, which are far fewer.
- **Zero sampling always runs.** Turning *Examine content* off skips the
  validators but not the zero sampling, because a file whose clusters were
  discarded by TRIM or wiped looks intact in every other respect.

**Unreadable regions.** A read that fails with an I/O error (a bad sector,
a driver timeout such as Windows error 121) does not abort the scan. The
failed chunk is re-read in 64 KiB blocks aligned to 64 KiB boundaries, a
failed block in 4 KiB pieces, and what still fails is zero-filled and
recorded as an unreadable range; after four consecutive failing pieces the
rest of the enclosing piece is written off without further attempts, since
every failure can cost a timeout of many seconds. The probe used by the
assemblers applies the same rule, so a file next to a bad sector is still
assembled with the bad piece zeroed. Ranges found by both stages are merged
and counted once in `CarveReport::unreadable_bytes` /
`unreadable_ranges`; a carved candidate that overlaps one carries
`extents.unreadable_bytes` and a diagnostic, and the scoring model turns
that into a negative reason and a cap. Errors other than I/O errors
(permission denied, out of bounds) still propagate.

## Deduplication

A metadata candidate and a carved hit whose content starts at the same
volume offset describe the same file. The carved hit is folded into the
metadata candidate, which keeps its name, path and timestamps and gains a
diagnostic:

> Signature carving found the same content at this offset (PDF document,
> 40,308 bytes, structure valid, carved likelihood 85%)

This corroborates inferred layouts (for example the FAT32 start inference)
without double counting. Carved hits that no metadata candidate claims are
listed as `carved-<offset>.<ext>` with reference `c<offset>`.

## CLI

```bash
phoinix scan disk.img --deep                    # metadata + carving of free space
phoinix scan disk.img --deep --carve-only       # carving only
phoinix scan disk.img --deep --carve-all        # whole volume
phoinix scan disk.img --deep --carve-types jpeg,pdf --carve-min-size 4096
phoinix scan raw.bin --deep                     # no filesystem: raw carving
phoinix explain disk.img c1048576               # evidence of a carved file
phoinix recover disk.img c1048576 64 --output /mnt/rescue
```

Progress is printed on stderr when it is a terminal. `--json` adds a
`carving` object with the run statistics (bytes scanned, hits, nested hits
skipped, rejected, merged).

## Limitations

- Fragmented files are carved as their first fragment plus whatever
  follows; the structure (CRCs, markers) usually reveals it and the
  candidate is marked damaged, but a fragmented JPEG whose fragments are
  separated by data without markers can still assemble to its real EOI
  with foreign bytes inside. Fragment reassembly is future work.
- Formats without a determinable end (gzip, raw audio, OLE compound files,
  TIFF) are not built in; a `header_only` definition carves them up to a
  size limit with the end reported as unknown.
- Names, paths and timestamps do not exist for carved files. When the
  filesystem still records the file, the metadata candidate is the one to
  recover; deduplication makes sure it is the one shown.

## Corpus

`tests/generated/make_carving_corpus.py` builds `tests/fixtures/carve/`:
a 32 MiB FAT32 volume with one file per built-in signature deleted with
its entry intact (merged into metadata candidates), two orphans whose
directory was wiped (found by carving only), a PNG whose second half was
moved elsewhere (damaged, not exact) and a live file (found only with
`--carve-all`, capped for allocated clusters).
`tests/integration/tests/carving.rs` asserts every row, whole-volume and
raw carving, and 40 corruption rounds without panics.
