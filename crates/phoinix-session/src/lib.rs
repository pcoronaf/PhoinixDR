//! Application service layer (milestone M6).
//!
//! Front-ends (the Tauri desktop, and any future API) talk to the recovery
//! core only through this crate:
//!
//! ```text
//! front-end ──typed DTOs──► Workspace
//!                              ├── inspect / devices      (phoinix-device, phoinix-volume, probes)
//!                              ├── start_scan → events    (engines, phoinix-carve, dedup)
//!                              ├── sessions (.phx JSON)   (metadata and evidence only)
//!                              ├── recover → events       (phoinix-recovery, destination safety)
//!                              └── preview                (reconstructed streams, no decoding)
//! ```
//!
//! No recovery logic lives here: the crate composes engines, turns their
//! results into plain data and reports progress. Everything is usable
//! without a GUI, which keeps the desktop testable and the core
//! independent from it.

#![forbid(unsafe_code)]

pub mod dto;
mod error;
pub mod preview;
pub mod recover;
pub mod scan;
pub mod session;
pub mod source;
mod workspace;

pub use error::SessionError;
pub use scan::ScanOutcome;
pub use session::ScanSession;
pub use workspace::{ScanHandle, SearchHandle, Workspace};
