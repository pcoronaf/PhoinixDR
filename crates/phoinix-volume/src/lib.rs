//! MBR, extended MBR and GPT partition discovery.
//!
//! Given any [`BlockReader`](phoinix_block::BlockReader), [`read_partition_table`] identifies the
//! partitioning scheme, validates the structures it finds, and exposes each
//! partition as a [`Partition`] that can be opened as its own
//! [`SubrangeReader`](phoinix_block::SubrangeReader) so that filesystem
//! parsers only ever see their own volume.
//!
//! Damaged tables do not simply fail: partial evidence is returned together
//! with [`VolumeDiagnostic`]s, which later feeds partition recovery.
//! Automatic repair is out of scope (ADR-0007).

#![forbid(unsafe_code)]

mod detect;
mod diagnostic;
mod error;
pub mod gpt;
pub mod mbr;
mod model;

pub use detect::{looks_like_filesystem_boot_sector, read_partition_table};
pub use diagnostic::VolumeDiagnostic;
pub use error::VolumeError;
pub use model::{
    Partition, PartitionConfidence, PartitionFlags, PartitionScheme, PartitionTable, PartitionType,
};
