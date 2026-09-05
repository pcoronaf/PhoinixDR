//! Filesystem-neutral contracts.
//!
//! Filesystem engines (`phoinix-fs-ntfs`, later FAT/EXT/…) implement the
//! traits defined here; generic code (CLI, scan coordinator, recovery writer)
//! depends only on this crate. Filesystem-specific knowledge never leaks
//! upward.
//!
//! - [`FileSystemProbe`] recognises a filesystem on a
//!   [`BlockReader`](phoinix_block::BlockReader) and reports its evidence.
//! - [`ProbeRegistry`] runs a set of probes and picks the best match.
//! - [`signature`] provides cheap, signature-only probes for filesystems
//!   PHOINIX does not yet parse natively, so that `inspect` can still name
//!   them.
//! - [`RecoveryCandidate`] is the filesystem-neutral description of a
//!   recoverable object, and [`DeletedFileProvider`] is how an engine hands
//!   candidates and their content to generic code such as the recovery
//!   writer.

#![forbid(unsafe_code)]

mod candidate;
mod error;
mod probe;
pub mod signature;
pub mod stream;

pub use candidate::{
    CandidateContent, CandidateTimestamps, DeletedFileProvider, FileSystemObjectId,
    RecoveryCandidate,
};
pub use error::FsError;
pub use probe::{
    Detection, FileSystemProbe, POSITIVE_THRESHOLD, ProbeEvidence, ProbeRegistry, ProbeResult,
};
pub use stream::{Extent, ExtentStream, ExtentStreamCursor};
