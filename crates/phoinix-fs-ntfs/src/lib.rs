//! Native NTFS reader and undelete engine (ADR-0004).
//!
//! Module map (in the order the parser bootstraps itself):
//!
//! - [`boot`] — boot sector and geometry;
//! - [`fixup`] — update sequence arrays;
//! - [`record`] — FILE record headers;
//! - [`attribute`] — attribute headers, resident and non-resident bodies;
//! - [`runlist`] — mapping-pair decoding with signed LCN deltas;
//! - [`filename`], [`standard_information`], [`timestamp`],
//!   [`attribute_list`] — attribute values;
//! - [`mft`] — `$MFT` bootstrap (with `$MFTMirr` fallback) and record access;
//! - [`stream`] — logical data streams over runs;
//! - [`file`](mod@file) — assembly of a file from base and extension records;
//! - [`tree`] — path reconstruction with stale-parent detection;
//! - [`bitmap`] — `$Bitmap` cluster allocation;
//! - [`volume`] — the [`NtfsVolume`] facade;
//! - [`probe`](NtfsProbe) — filesystem detection.

#![forbid(unsafe_code)]

pub mod attribute;
pub mod attribute_list;
pub mod bitmap;
pub mod boot;
pub mod data;
pub mod diagnostic;
mod error;
pub mod file;
pub mod filename;
pub mod fixup;
pub mod mft;
mod probe;
pub mod record;
pub mod runlist;
pub mod standard_information;
pub mod stream;
pub mod timestamp;
pub mod tree;
pub mod volume;

pub use bitmap::{ClusterAllocationMap, ClusterBitmap, ClusterState, RangeAllocation};
pub use boot::NtfsBootSector;
pub use data::{DataStorage, DataStreamDescriptor};
pub use diagnostic::NtfsDiagnostic;
pub use error::NtfsError;
pub use file::NtfsFile;
pub use filename::{FileNameAttribute, FileNameNamespace};
pub use mft::Mft;
pub use probe::NtfsProbe;
pub use record::{FileRecord, FileRecordHeader, FileReference};
pub use runlist::NtfsRun;
pub use standard_information::StandardInformation;
pub use stream::{NtfsDataStream, StreamCursor};
pub use timestamp::NtfsTimestamp;
pub use tree::{PathResolver, ResolvedPath};
pub use volume::{NtfsVolume, VolumeInformation};
