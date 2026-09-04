# ADR-0006 — Recovery likelihood is not assessment confidence

**Status:** accepted · **Date:** 2026-09

## Context

Consumer tools show a single colour. That conflates "how likely is this file
intact?" with "how much does the tool actually know?".

## Decision

`RecoveryHealth` carries two independent values in the range 0–100:

- **likelihood** — the estimated probability that recovery yields the original
  content;
- **confidence** — the quality of the evidence behind that estimate.

Both are produced by a deterministic model of hard constraints plus weighted
evidence, and every score is accompanied by concrete, machine-generated
reasons. Cluster reallocation is described as "allocated to active data",
never as "definitely overwritten", because `$Bitmap` proves the former only.

## Consequences

- "Likelihood 38 / confidence 96" and "likelihood 75 / confidence 31" are
  distinct, meaningful outcomes.
- Thresholds and weights are provisional and must be calibrated empirically
  against a controlled corpus; the UI labels them as estimates until then.
