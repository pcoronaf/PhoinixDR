# Disclaimer

PhoinixDR is provided “as is” and is used entirely at your own risk. Data recovery is inherently uncertain, and improper use may result in permanent data loss or damage. Always work from a copy or disk image when possible and recover files to a different storage device.

## What this means in practice

- PhoinixDR reads sources only; it never writes to the media it scans
  (ADR-0002, ADR-0007). The risk lies in what happens around it: using
  the failing disk while recovering, writing recovered files back onto it,
  or trusting a recovered file without checking it.
- Work from a disk image when the medium is failing. PhoinixDR opens RAW,
  E01, VHD, VHDX and VMDK images directly, so imaging first costs nothing
  in capability.
- Recover to a different storage device. The recovery writer refuses a
  destination on the source disk; the expert override exists for people
  who accept the consequences.
- Recovery likelihood and confidence are estimates backed by evidence,
  not guarantees. Read `phoinix explain` or the evidence panel before
  relying on a file, and verify recovered files with the SHA-256 digests
  PhoinixDR prints.
- The software is released under MIT OR Apache-2.0, both of which
  disclaim warranties; see [LICENSE-MIT](../LICENSE-MIT) and
  [LICENSE-APACHE](../LICENSE-APACHE).
