# Security policy

## Threat model

PhoinixDR parses arbitrary, possibly corrupted or deliberately malicious storage.
Every source — physical disk, partition, or image file — is treated as
attacker-controlled input.

The engine therefore requires:

- strict bounds checking on every on-disk value;
- checked integer arithmetic for every sector, cluster, offset and length
  calculation derived from media;
- recursion and chain-depth limits (extended partitions, parent-directory
  chains, attribute lists);
- allocation limits (a single read request is capped, see
  `phoinix_block::MAX_SINGLE_READ`);
- no panics on malformed input: parsers return typed errors;
- `#![forbid(unsafe_code)]` in every crate except dedicated platform/FFI
  crates, where each `unsafe` block documents its invariants;
- no write primitive to source media anywhere in the recovery core.

Clippy is configured (see the workspace `Cargo.toml`) to flag `unwrap`,
`expect`, `panic!`, unchecked indexing and lossy casts so that the panic and
integer-safety policies are enforced mechanically, not by convention.

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's
"Report a vulnerability" feature on this repository rather than opening a
public issue. Include a minimal reproducing image or fixture whenever
possible; crash fixtures become regression tests.

We aim to acknowledge reports within seven days.
