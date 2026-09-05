//! Native exFAT reader and undelete engine.
//!
//! exFAT is not forced through FAT abstractions: it has its own boot region,
//! an allocation bitmap, directory *entry sets* (File + Stream Extension +
//! File Name entries) and a `NoFatChain` flag that makes most files
//! contiguous without any FAT chain. Deletion clears bit 7 of every entry's
//! type byte and the bitmap bits, so the layout of a contiguous file
//! survives intact.

#![forbid(unsafe_code)]

pub mod bitmap;
pub mod boot;
pub mod dir;
mod error;
mod probe;
pub mod table;
pub mod undelete;
pub mod volume;

pub use bitmap::AllocationBitmap;
pub use boot::ExfatBootSector;
pub use dir::{EntrySet, ExfatAttributes, StreamFlags};
pub use error::ExfatError;
pub use probe::ExFatProbe;
pub use table::ExfatTable;
pub use undelete::ExfatUndelete;
pub use volume::{ExfatVolume, Reconstruction, WalkedEntry};
