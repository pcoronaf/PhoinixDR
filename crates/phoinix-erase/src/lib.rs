//! Secure erase (milestone M12): the only crate in PhoinixDR that opens a
//! medium for writing.
//!
//! The recovery stack is read-only by construction ([`phoinix_block`] has
//! no write primitive). This crate owns its own writer, its own device
//! opening code and its own refusal rules, and reads back through the
//! read-only stack for verification. See ADR-0014 and `docs/erase/plan.md`.
//!
//! ```text
//! EraseRequest ──► target::describe (device, range, volumes, guards)
//!              ──► engine::run_erase: passes × chunks → platform writer
//!              ──► verify::read_back (phoinix-device, read-only)
//!              ──► certificate (JSON / Markdown / HTML)
//! ```
//!
//! What the methods guarantee and what they cannot (flash remapping, file
//! copies the filesystem keeps) is part of the public API: every method
//! carries its note and every multi-pass method its warning, with the
//! author's articles linked.

#![deny(unsafe_code)]

pub mod certificate;
pub mod engine;
mod error;
pub mod method;
pub mod pattern;
pub mod platform;
pub mod target;
pub mod verify;
pub mod writer;

pub use certificate::{Certificate, CertificateFormat, Operator, VerificationRecord};
pub use engine::{EraseEvent, EraseOutcome, EraseRequest, EraseSummary, ResumePoint, run_erase};
pub use error::{EraseError, Refusal};
pub use method::{EraseMethod, Pass, PassPattern, VerifyMode};
pub use target::{EraseTarget, Guards, MountedVolume, TargetDescription, describe};

/// The warning shown wherever an erase can be started. Distinct from the
/// recovery disclaimer: this one is about irreversibility.
pub const ERASE_WARNING: &str = "Erasing is irreversible. PhoinixDR will overwrite the target you confirm; nothing on it can be recovered afterwards, by PhoinixDR or by anyone. Check the model, serial number and size before confirming, and unplug every disk you do not intend to erase.";

/// Article on why one pass is enough (pcoronaf, 2014).
pub const ARTICLE_GUTMANN_MYTH: &str = "https://www.pcoronaf.com/2014/07/el-mito-de-gutmann.html";
/// Article on secure deletion of files and what it cannot reach (pcoronaf, 2020).
pub const ARTICLE_SECURE_DELETION: &str =
    "https://www.pcoronaf.com/2020/12/borrado-seguro-de-archivos.html";

/// Statement shown when a multi-pass method is chosen.
pub const MULTI_PASS_WARNING: &str = "More than one pass adds time and wear, not security. Multi-pass schemes come from 1990s drive encodings; Gutmann's own epilogue calls the full 35-pass method \u{201c}pointless for any drive\u{201d}, and NIST SP 800-88 states that one overwrite protects drives made after 2001 against both keyboard and laboratory attack. The method is offered because some policies still require it.";

/// Statement shown when the target is solid-state.
pub const FLASH_WARNING: &str = "This is a solid-state device. Overwriting reaches the logical blocks, not the physical cells: the controller remaps writes, and over-provisioned or retired cells are never touched. An overwrite is a software-level clear (NIST 800-88 Clear); a purge needs the device's own sanitize command (ATA Secure Erase, NVMe Sanitize) or physical destruction.";
