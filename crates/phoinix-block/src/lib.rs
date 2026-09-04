//! Read-only block-level access to storage sources.
//!
//! Everything PHOINIX reads — physical disks, RAW images, partitions,
//! virtually mounted lost partitions — implements [`BlockReader`]. The trait
//! is deliberately read-only (see ADR-0002 and ADR-0007): there is no
//! `write_at`, and none will be added here.
//!
//! Reads are synchronous and positional (ADR-0003). Implementations must be
//! safe for concurrent callers: they never share a mutable seek cursor.
//!
//! # Contract of [`BlockReader::read_at`]
//!
//! - A request whose byte range does not lie entirely inside the source is an
//!   [`BlockError::OutOfBounds`] error, never a short read.
//! - A single request may not exceed [`MAX_SINGLE_READ`] bytes
//!   ([`BlockError::RequestTooLarge`]); [`BlockReaderExt::read_exact_at`]
//!   splits larger requests.
//! - A zero-length read at any offset `<= len()` succeeds with `Ok(0)`.
//! - A short read (fewer bytes than requested) is permitted only when the
//!   operating system returned fewer bytes; callers that need every byte use
//!   [`BlockReaderExt::read_exact_at`].

#![forbid(unsafe_code)]

pub mod align;
mod error;
mod fingerprint;
mod geometry;
mod memory;
mod raw;
mod reader;
mod subrange;

pub use error::BlockError;
pub use fingerprint::{SourceFingerprint, to_hex};
pub use geometry::BlockGeometry;
pub use memory::MemoryReader;
pub use raw::RawImage;
pub use reader::{BlockReader, BlockReaderExt, MAX_SINGLE_READ, check_request};
pub use subrange::SubrangeReader;
