//! Native ext2/ext3/ext4 reader and undelete engine (milestone M10).
//!
//! The reader parses superblocks, block-group descriptors (32 and 64-byte),
//! inodes (with crc32c checksums), extent trees, ext2/3 block maps, inline
//! data, linear and htree directories, and the block bitmaps.
//!
//! Deleting a file on a modern kernel zeroes the inode's size and layout
//! and only sets its deletion time, so the inode alone no longer says
//! where the data was. The engine therefore combines three sources:
//!
//! - directory slack: a removed entry's bytes survive inside the previous
//!   entry's record, giving the name and the inode number;
//! - the deleted inode: mode, owner, timestamps, deletion time;
//! - the jbd2 journal: older copies of the inode-table block still hold
//!   the live inode with its size and its extents or block map.
//!
//! See `docs/ext/reader.md`.

#![forbid(unsafe_code)]

pub mod bitmap;
pub mod blockmap;
pub mod dir;
mod error;
pub mod extent;
pub mod group;
pub mod inode;
pub mod journal;
pub mod probe;
pub mod superblock;
pub mod undelete;
pub mod volume;

pub use error::ExtError;
pub use probe::ExtProbe;
pub use superblock::Superblock;
pub use undelete::ExtUndelete;
pub use volume::{ExtVolume, Layout, LayoutSource, WalkedEntry};
