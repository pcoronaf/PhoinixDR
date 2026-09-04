//! Native NTFS reader and undelete engine (ADR-0004).
//!
//! Milestone M2 provides the boot-sector parser and the [`NtfsProbe`]; the
//! remaining reader (M3) and undelete (M4) modules build on them.

#![forbid(unsafe_code)]

pub mod boot;
mod error;
mod probe;

pub use boot::NtfsBootSector;
pub use error::NtfsError;
pub use probe::NtfsProbe;
