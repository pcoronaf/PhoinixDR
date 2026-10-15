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
pub mod files;
pub mod firmware;
pub mod method;
pub mod mounted;
pub mod pattern;
pub mod platform;
pub mod scan;
pub mod space;
pub mod target;
pub mod verify;
pub mod writer;

pub use certificate::{Certificate, CertificateFormat, Operator, VerificationRecord};
pub use engine::{
    EraseEvent, EraseOutcome, EraseRequest, EraseSummary, ResumePoint, run_erase, run_erase_with,
    run_erase_with_guards,
};
pub use error::{EraseError, Refusal};
pub use files::{FileEntry, FileFailure, FilesDescription, StreamEntry};
pub use firmware::{FIRMWARE_NOTE, FROZEN_ADVICE, FirmwareAction, FirmwareInfo, Transport};
pub use method::{EraseMethod, Pass, PassPattern, VerifyMode};
pub use platform::MountedVolumeInfo;
pub use scan::{PostScan, PostScanner, ScanProgress, ScanRecord};
pub use space::SpaceDescription;
pub use target::{EraseTarget, Guards, MountedVolume, TargetDescription, describe};

/// Every writable volume the operating system has mounted, as targets of
/// the filesystem-level free-space wipe.
#[must_use]
pub fn mounted_volumes() -> Vec<MountedVolumeInfo> {
    platform::mounted_volumes()
}

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

/// Statement shown with a raw free-space target: what the wipe reaches
/// and what it cannot.
pub const FREE_SPACE_NOTE: &str = "A free-space wipe overwrites every cluster the filesystem marks as free, and the slack at the end of each file when asked. It does not reach the content of small deleted files kept inside NTFS MFT records or ext inodes, copies in the journal, volume shadow copies or previous versions, nor the physical cells a solid-state drive has remapped. Deleted names, sizes and dates remain visible in the metadata.";

/// Statement shown with a mounted (filesystem-level) free-space target.
pub const MOUNTED_NOTE: &str = "Writing through the filesystem fills the space the volume reports as free; the filesystem keeps some clusters back, and small deleted files kept inside NTFS MFT records or ext inodes, journal copies, shadow copies, previous versions and remapped flash cells are not reached. The volume stays in use while it fills; it is full for a moment, then the fill files are removed.";

/// Statement shown with a files target: what overwriting a file by name
/// reaches and what it cannot.
pub const FILES_NOTE: &str = "Erasing files overwrites their current content through the filesystem (every named stream on NTFS, and the slack up to the end of the last cluster), then truncates, renames and deletes them. It cannot reach the copies the system keeps elsewhere: earlier versions in the journal, volume shadow copies and previous versions, temporary files, the swap or page file, backups and cloud copies, the old resident bytes an NTFS record may keep until it is reused, nor the physical cells a solid-state drive has remapped. The deleted name may survive in directory metadata. Follow with a free-space wipe of the volume to cover what the filesystem left behind.";

/// Statement shown when the target is solid-state.
pub const FLASH_WARNING: &str = "This is a solid-state device. Overwriting reaches the logical blocks, not the physical cells: the controller remaps writes, and over-provisioned or retired cells are never touched. An overwrite is a software-level clear (NIST 800-88 Clear); a purge needs the device's own sanitize command (ATA Secure Erase, NVMe Sanitize) or physical destruction.";
