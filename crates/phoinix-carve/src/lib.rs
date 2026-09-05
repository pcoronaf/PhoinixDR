//! Deep scan: signature carving of unallocated space (milestone M8).
//!
//! ```text
//! AllocationView (engine)  ──►  free byte ranges
//!                                     │
//!                              find_headers (chunked, parallel)
//!                                     │  hits
//!                              Assembler per signature
//!                                     │  length, end_known, checks
//!                              evidence + validators + scoring
//!                                     │
//!                              RecoveryCandidate (Carved)
//!                                     │
//!                              deduplicate against metadata candidates
//! ```
//!
//! Carved files have no name, path or timestamps; their health rests on
//! the structure of the content, the allocation state of the span they
//! occupy and the assumption that they were stored contiguously. The
//! [`CarveEngine`] implements the same
//! [`DeletedFileProvider`](phoinix_fs::DeletedFileProvider) contract as the
//! metadata engines, so scan, explain and recover treat carved candidates
//! like any other.

#![forbid(unsafe_code)]

pub mod assemble;
mod engine;
mod error;
pub mod probe;
pub mod scanner;
pub mod signature;

pub use engine::{CarveEngine, CarveOptions, CarveReport};
pub use error::CarveError;
pub use scanner::{Hit, ScanOptions, ScanProgress};
pub use signature::{AssemblerKind, CarveSignature, SignatureSet};
