# Architectural Decision Records

Decisions that shape PHOINIX are recorded here so that they are not reopened
by accident. Each record states the context, the decision and its
consequences. A superseded record stays in place and links to its successor.

| ADR | Title |
|-----|-------|
| [0001](ADR-0001-rust-as-core-language.md) | Rust as core language |
| [0002](ADR-0002-read-only-blockreader.md) | Read-only `BlockReader` abstraction |
| [0003](ADR-0003-synchronous-filesystem-io.md) | Synchronous filesystem I/O |
| [0004](ADR-0004-native-ntfs-implementation.md) | Native NTFS implementation |
| [0005](ADR-0005-third-party-libraries-behind-adapters.md) | Third-party libraries behind adapters |
| [0006](ADR-0006-likelihood-vs-confidence.md) | Recovery likelihood is not assessment confidence |
| [0007](ADR-0007-no-source-writes-in-recovery-core.md) | No source writes in the recovery core |
| [0008](ADR-0008-candidate-addressing-before-sessions.md) | Candidate addressing before a session database exists |
| [0009](ADR-0009-carving-scope-and-dedup.md) | Carving scans free space by default and folds into metadata candidates |
