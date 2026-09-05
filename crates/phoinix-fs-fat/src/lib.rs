//! Native FAT12/FAT16/FAT32 reader and undelete engine.
//!
//! FAT keeps very little about a deleted file: the directory entry survives
//! with its first byte replaced by `0xE5`, still carrying the size, the
//! first cluster and the timestamps, but the cluster chain in the FAT is
//! cleared. PhoinixDR therefore distinguishes two reconstructions:
//!
//! - **contiguous** — the file is assumed to occupy consecutive clusters
//!   from its first cluster, all of which are still free; and
//! - **heuristic** — clusters in that span that are now allocated to other
//!   files are skipped, on the assumption that the file was fragmented
//!   around them.
//!
//! Both are reported in the evidence, with different confidence.

#![forbid(unsafe_code)]

pub mod boot;
pub mod dir;
mod error;
mod probe;
pub mod table;
pub mod undelete;
pub mod volume;

pub use boot::{FatBootSector, FatVariant};
pub use dir::{DirEntry, FatAttributes};
pub use error::FatError;
pub use probe::FatProbe;
pub use table::{FatEntry, FatTable};
pub use undelete::FatUndelete;
pub use volume::{FatVolume, Reconstruction, WalkedEntry};
