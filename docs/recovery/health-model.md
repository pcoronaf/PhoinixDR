# Recovery health model (v0)

`phoinix-health` scores a `RecoveryEvidence` into a `RecoveryHealth` with two
independent numbers (ADR-0006):

- **likelihood** — estimated probability that recovery returns the original
  bytes;
- **confidence** — how much evidence the estimate rests on.

The model is deterministic: hard constraints cap the likelihood, weighted
evidence moves it within the cap, and every adjustment produces a reason.

## Likelihood (defaults in `ScoringModel`)

| evidence | effect |
|---|---|
| resident content in a valid record | likelihood ≤ 97 |
| every extent known, every cluster free | likelihood ≤ 92 |
| any required cluster allocated | cap 79 |
| ≥ 10 % allocated | cap 59 |
| ≥ 50 % allocated | cap 34 |
| 100 % allocated | cap 15 |
| extent map incomplete | cap 79 × (located share); 0 when nothing is located |
| allocation map unavailable | cap 74 |
| structure damaged / invalid | cap 59 / 34 |
| zeros contradict the format (`ZeroContentAssessment::ContradictsFormat`) | cap 20 |
| zeros suspicious for a recognised type (`Suspicious`) | cap 59 |
| empty file (logical size 0) | likelihood ≤ 97, validation not applicable |
| encrypted / compressed | cap 10 / 0 |
| heuristic layout (skipped clusters) or inferred start | cap 59, or 79 when the content validates completely |
| layout from an older journal copy that predates the last modification (`ExtentEvidence::stale`) | cap 59, or 79 when the content validates completely |
| layout recovered from a journal copy (`CandidateSource::Journal`), not stale | no cap; positive reason |
| carved file (`CandidateSource::FileCarving`), structure validated | cap 85 |
| carved file without a structural validator | cap 74 |
| carved file whose end could not be determined | cap 59 |
| valid structure | +3 |
| fragments | −1 per extra extent, at most −5 |

Fragmentation alone never makes a file *Poor* when every extent is known and
free.

## Zero-filled content

Zeros are not evidence of loss by themselves. Sampled zero blocks are
interpreted with the file's context:

```text
All-zero content detected
        │
        ├── stream is sparse                 → Expected, no penalty
        ├── recognised structured type
        │       ├── validation fails         → ContradictsFormat (cap 20)
        │       ├── validation passes        → Plausible, no penalty
        │       └── no validator             → Suspicious (cap 59)
        ├── name implies a structured type   → ContradictsFormat (cap 20)
        └── unknown / raw type               → Ambiguous: likelihood untouched,
                                               confidence −25, explicit warning
```

An empty file (logical size 0) has no content to recover; it is Excellent
when its metadata survives and validation is reported as not applicable.

## Confidence

Starts at 100 and loses points for what PHOINIX could not see: damaged record
(−30), unknown size (−10), incomplete extents (−25), no allocation map (−25,
or proportional to unknown clusters), no structural validator (−15), no
content sample (−5), ambiguous zero-filled content (−25), unknown medium
(−3), SSD without TRIM knowledge (−10), contiguity assumed (−10) or
heuristic layout (−30), start inferred (a further −20), stale journal
layout (−10), carved file (−15 for the missing metadata plus −10 for the
contiguity assumption).

## Carved candidates

A carved file has no metadata record, so the metadata rules are replaced by
a single reason ("Found by signature carving: …") and the size rule reads
whether the structure determined the end. Allocation caps, validation caps
and zero-content rules apply unchanged: a carved file over reused clusters
is still Very poor, and a damaged structure is still capped at 59. See
`docs/carving/deep-scan.md`.

## Journal candidates

On ext3/ext4 the kernel clears a deleted inode's size and extent tree, so
the layout usually comes from an older copy of the inode-table block in the
jbd2 journal (`docs/ext/reader.md`). Such a layout is complete and
trustworthy when it was the last state of the file before deletion: the
candidate carries `CandidateSource::Journal`, a diagnostic naming the
transaction and whether its checksum verified, and is scored like any
other known layout. When the on-disk inode shows a modification the copy
does not know about, the layout is marked stale and capped as above.

## Wording

`$Bitmap` proves that a cluster is *allocated to active data*. It does not
prove that every previous byte is gone. Reasons therefore say

> 42 % of the required clusters are currently allocated to active filesystem data

and never "overwritten". SSD sources produce a warning that TRIM may reduce
recoverability, affecting confidence and explanation rather than likelihood,
because PHOINIX has no evidence about NAND state.

## Categories (provisional)

| likelihood | category |
|---|---|
| 95–100 | Excellent |
| 80–94 | Very good |
| 60–79 | Good |
| 35–59 | Poor |
| 1–34 | Very poor |
| 0 | Unrecoverable |

All thresholds are development heuristics until calibrated against the
controlled corpus in `tests/fixtures/ntfs/undelete.manifest.json` and larger
successors.
