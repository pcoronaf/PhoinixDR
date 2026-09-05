//! Lost-partition recovery (milestone M9).
//!
//! A partition search reads the whole source and looks for filesystem
//! structures independently of the partition table: NTFS, FAT and exFAT
//! boot sectors (primary and backup), and EXT superblocks (primary and
//! backup). Every structure yields a [`PartitionCandidate`] with its
//! boundaries, evidence and a confidence, related to the existing table
//! and to the other candidates.
//!
//! Candidates are *virtually mounted*: [`PartitionCandidate::open`] returns
//! a read-only view of the range, which the filesystem engines open like
//! any volume, so files can be browsed and recovered from a lost partition
//! without touching the partition table.

#![forbid(unsafe_code)]

mod candidate;
mod error;
mod search;

pub use candidate::{FoundVia, PartitionCandidate, Relation, Repair, open_range};
pub use error::PartitionRecoveryError;
pub use search::{SearchOptions, find_partitions, structure_signatures};
