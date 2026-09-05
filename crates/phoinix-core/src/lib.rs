//! Core types shared by every PHOINIX crate.
//!
//! This crate deliberately knows nothing about disks, partitions or
//! filesystems. It provides:
//!
//! - opaque domain identifiers ([`SourceId`], [`VolumeId`], [`CandidateId`]);
//! - validated ranges ([`ByteRange`], [`LbaRange`]);
//! - checked arithmetic helpers ([`arith`]) so that every media-derived
//!   calculation is overflow-safe;
//! - bounds-checked little-endian byte access ([`bytes`]) so that parsers
//!   never index a slice directly;
//! - the [`FileSystemType`] enumeration used by probes and candidates;
//! - small formatting helpers ([`fmt`]).

#![forbid(unsafe_code)]

pub mod arith;
pub mod bytes;
pub mod crc32c;
pub mod fmt;
mod fs_type;
mod ids;
mod range;

pub use arith::ArithmeticOverflow;
pub use fs_type::FileSystemType;
pub use ids::{CandidateId, SourceId, VolumeId};
pub use range::{ByteRange, LbaRange, RangeError};
