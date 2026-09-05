//! Recovery writer: streams candidate content to another filesystem,
//! verifies it with SHA-256, and refuses destinations on the source disk.
//!
//! Recovery always targets a destination path; the source is never written
//! (ADR-0007). Missing bytes are never fabricated: if content cannot be read
//! completely the output is kept under a `.partial` name and the result says
//! so.

#![forbid(unsafe_code)]

mod destination;
mod names;
mod report;
mod writer;

pub use destination::{DestinationCheck, check_destination};
pub use names::{sanitize_component, sanitize_relative_path};
pub use report::{
    CaseMetadata, RecoveryReport, ReportFormat, ReportItem, ReportSource, ReportSummary,
    ReportVolume,
};
pub use writer::{RecoveryError, RecoveryRequest, RecoveryResult, RecoveryWriter};
