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
| ≥ 50 % of sampled blocks zero | cap 20 |
| encrypted / compressed | cap 10 / 0 |
| valid structure | +3 |
| fragments | −1 per extra extent, at most −5 |

Fragmentation alone never makes a file *Poor* when every extent is known and
free.

## Confidence

Starts at 100 and loses points for what PHOINIX could not see: damaged record
(−30), unknown size (−10), incomplete extents (−25), no allocation map (−25,
or proportional to unknown clusters), no structural validator (−15), no
content sample (−5), unknown medium (−3), SSD without TRIM knowledge (−10).

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
