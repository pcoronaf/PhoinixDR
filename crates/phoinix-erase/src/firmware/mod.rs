//! Firmware sanitize (milestone M12.4): the device's own erase commands,
//! the only ones that reach the cells a solid-state drive has remapped
//! (NIST SP 800-88 *Purge*). ATA: SANITIZE DEVICE (crypto scramble, block
//! erase, overwrite) and SECURITY ERASE UNIT (normal and enhanced). NVMe:
//! Sanitize (crypto erase, block erase, overwrite) and Format NVM with a
//! secure-erase setting.
//!
//! The commands go through a [`Backend`] (ATA pass-through and NVMe admin
//! commands) that the platform module implements with `SG_IO` and the
//! NVMe ioctl on Linux, `IOCTL_ATA_PASS_THROUGH` and
//! `IOCTL_STORAGE_PROTOCOL_COMMAND` on Windows. The logic here is tested
//! against a mock backend; the platform calls are type-checked, not
//! exercised, in continuous integration. Treat the first use on real
//! hardware as a test on a disposable drive.

pub mod ata;
pub mod nvme;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How the device is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// ATA (SATA) through pass-through.
    Ata,
    /// NVMe admin commands.
    Nvme,
}

/// A firmware erase action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareAction {
    /// Cryptographic erase: the media encryption key is replaced and every
    /// sector reads as random data. Seconds. ATA CRYPTO SCRAMBLE EXT, NVMe
    /// Sanitize crypto erase.
    CryptoErase,
    /// Block erase: every NAND block is erased. Minutes. ATA BLOCK ERASE
    /// EXT, NVMe Sanitize block erase.
    BlockErase,
    /// Overwrite by the device itself, one pass of zeros. ATA OVERWRITE
    /// EXT, NVMe Sanitize overwrite.
    Overwrite,
    /// ATA SECURITY ERASE UNIT (normal): the device overwrites its user
    /// area; the older command, offered when SANITIZE is not.
    SecurityErase,
    /// ATA SECURITY ERASE UNIT (enhanced): also the reallocated sectors;
    /// on many SSDs a key change.
    EnhancedSecurityErase,
    /// NVMe Format NVM with user-data erase (SES=1).
    FormatUserData,
    /// NVMe Format NVM with cryptographic erase (SES=2).
    FormatCryptoErase,
}

impl FirmwareAction {
    /// Every action, best first.
    pub const ALL: [Self; 7] = [
        Self::CryptoErase,
        Self::BlockErase,
        Self::FormatCryptoErase,
        Self::EnhancedSecurityErase,
        Self::SecurityErase,
        Self::FormatUserData,
        Self::Overwrite,
    ];

    /// The identifier (`crypto`, `block`, `overwrite`, `security`,
    /// `enhanced`, `format`, `format-crypto`).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::CryptoErase => "crypto",
            Self::BlockErase => "block",
            Self::Overwrite => "overwrite",
            Self::SecurityErase => "security",
            Self::EnhancedSecurityErase => "enhanced",
            Self::FormatUserData => "format",
            Self::FormatCryptoErase => "format-crypto",
        }
    }

    /// Parses an identifier.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.id() == text.trim().to_ascii_lowercase())
    }

    /// Human-readable name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CryptoErase => "Firmware: cryptographic erase",
            Self::BlockErase => "Firmware: block erase",
            Self::Overwrite => "Firmware: overwrite by the device",
            Self::SecurityErase => "Firmware: ATA security erase",
            Self::EnhancedSecurityErase => "Firmware: ATA enhanced security erase",
            Self::FormatUserData => "Firmware: NVMe format with user-data erase",
            Self::FormatCryptoErase => "Firmware: NVMe format with cryptographic erase",
        }
    }

    /// What the user should know.
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Self::CryptoErase => {
                "The device replaces its media encryption key, so every sector, remapped cells included, reads as random data. Takes seconds. NIST 800-88 Purge for self-encrypting drives."
            }
            Self::BlockErase => {
                "The device erases every NAND block, remapped and over-provisioned ones included. Takes minutes. NIST 800-88 Purge."
            }
            Self::Overwrite => {
                "The device overwrites its own media, remapped cells included. Takes as long as a software overwrite or longer."
            }
            Self::SecurityErase => {
                "ATA SECURITY ERASE UNIT: the device erases its user area. The older command, for drives without the SANITIZE feature set; a temporary password is set and removed again."
            }
            Self::EnhancedSecurityErase => {
                "ATA SECURITY ERASE UNIT, enhanced: reallocated sectors included; on most SSDs a key change that takes seconds. A temporary password is set and removed again."
            }
            Self::FormatUserData => {
                "NVMe Format NVM with the user-data erase setting: the namespace is reformatted and its data erased by the controller."
            }
            Self::FormatCryptoErase => {
                "NVMe Format NVM with the cryptographic erase setting: the namespace key is replaced. Takes seconds."
            }
        }
    }

    /// Whether the action is a cryptographic erase (data reads as random
    /// afterwards rather than as zeros).
    #[must_use]
    pub const fn is_crypto(self) -> bool {
        matches!(
            self,
            Self::CryptoErase | Self::FormatCryptoErase | Self::EnhancedSecurityErase
        )
    }
}

/// What the device said about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareInfo {
    /// The transport, when the device answered.
    pub transport: Option<Transport>,
    /// Model as the device reports it.
    pub model: Option<String>,
    /// Serial number as the device reports it.
    pub serial: Option<String>,
    /// Actions the device supports, best first.
    pub supports: Vec<FirmwareAction>,
    /// ATA: the security feature set is frozen (no SECURITY command is
    /// accepted until a power cycle).
    pub frozen: bool,
    /// ATA: a security password is set.
    pub security_enabled: bool,
    /// ATA: the device is locked.
    pub locked: bool,
    /// ATA: the device reports itself as non-rotating (SSD).
    pub solid_state: Option<bool>,
    /// Estimated time of a security erase, in minutes (ATA word 89/90).
    pub security_erase_minutes: Option<u32>,
    /// Estimated time of an enhanced security erase, in minutes.
    pub enhanced_erase_minutes: Option<u32>,
    /// Why nothing is offered, or what to know before using it.
    pub note: Option<String>,
}

impl FirmwareInfo {
    /// Whether `action` can be run now.
    ///
    /// # Errors
    ///
    /// Returns the reason it cannot.
    pub fn can_run(&self, action: FirmwareAction) -> Result<(), String> {
        if !self.supports.contains(&action) {
            return Err(format!("the device does not support {}", action.label()));
        }
        if self.locked {
            return Err("the device is security-locked; unlock it with its password first".into());
        }
        if self.frozen
            && matches!(
                action,
                FirmwareAction::SecurityErase | FirmwareAction::EnhancedSecurityErase
            )
        {
            return Err(FROZEN_ADVICE.into());
        }
        Ok(())
    }

    /// The best action the device supports, if any.
    #[must_use]
    pub fn best(&self) -> Option<FirmwareAction> {
        FirmwareAction::ALL
            .into_iter()
            .find(|a| self.can_run(*a).is_ok())
    }
}

/// What to tell the user when the ATA security feature set is frozen.
pub const FROZEN_ADVICE: &str = "the device is security-frozen: the BIOS or the operating system froze the ATA security feature set at boot. Put the computer to sleep and wake it up again, or unplug the drive's SATA power cable and plug it back in with the system running, then retry; a live Linux system is the usual way around a BIOS that always freezes";

/// Statement shown with a firmware action.
pub const FIRMWARE_NOTE: &str = "A firmware sanitize is run by the drive itself and reaches every cell, remapped and over-provisioned ones included; it is NIST SP 800-88 Purge where a software overwrite is only Clear. PhoinixDR can only ask the drive and read back a sample afterwards: it cannot see what the controller did inside, and a drive with buggy firmware may report success without erasing. The command cannot be cancelled once issued, and interrupting the power during it may leave the drive unusable. Verify with the post-erase deep scan, and keep the drive under your control until it is.";

/// Direction of a command's data phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDir {
    /// No data.
    None,
    /// Data from the device.
    In,
    /// Data to the device.
    Out,
}

/// An ATA command as task-file registers (48-bit form; 28-bit commands
/// use the low bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtaCommand {
    /// FEATURE (16-bit for EXT commands).
    pub feature: u16,
    /// COUNT.
    pub count: u16,
    /// LBA (48-bit).
    pub lba: u64,
    /// DEVICE register (0x40 for LBA mode).
    pub device: u8,
    /// COMMAND.
    pub command: u8,
    /// Whether the command is a 48-bit (EXT) command.
    pub ext: bool,
    /// Data direction.
    pub dir: DataDir,
}

/// What the device returned for an ATA command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AtaResult {
    /// STATUS register.
    pub status: u8,
    /// ERROR register.
    pub error: u8,
    /// COUNT field.
    pub count: u16,
    /// LBA field.
    pub lba: u64,
}

impl AtaResult {
    /// Whether the ERROR bit of STATUS is set.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.status & 0x01 != 0
    }
}

/// An NVMe admin command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NvmeCommand {
    /// Opcode.
    pub opcode: u8,
    /// Namespace identifier.
    pub nsid: u32,
    /// Command dwords 10 to 15.
    pub cdw10: u32,
    /// CDW11.
    pub cdw11: u32,
    /// CDW12.
    pub cdw12: u32,
    /// CDW13.
    pub cdw13: u32,
    /// CDW14.
    pub cdw14: u32,
    /// CDW15.
    pub cdw15: u32,
}

/// The pass-through the platform provides.
pub trait Backend: Send {
    /// The transport this backend speaks.
    fn transport(&self) -> Transport;
    /// Issues an ATA command; `data` receives (In) or supplies (Out) the
    /// data phase; `timeout` bounds the wait.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the pass-through itself; a command the
    /// device rejected comes back as a result with the ERROR bit set.
    fn ata(
        &mut self,
        cmd: &AtaCommand,
        data: &mut [u8],
        timeout: Duration,
    ) -> io::Result<AtaResult>;
    /// Issues an NVMe admin command; `data` receives (In) or supplies
    /// (Out) the data; returns the completion status (0 = success) and
    /// the completion dword 0.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the pass-through itself.
    fn nvme_admin(
        &mut self,
        cmd: &NvmeCommand,
        dir: DataDir,
        data: &mut [u8],
        timeout: Duration,
    ) -> io::Result<(u16, u32)>;
}

/// Progress of a running sanitize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SanitizeProgress {
    /// Percent done, when the device reports it.
    pub percent: Option<u8>,
    /// Seconds elapsed.
    pub elapsed: u64,
}

/// Why a sanitize did not complete.
#[derive(Debug, thiserror::Error)]
pub enum FirmwareError {
    /// The device cannot run the action now.
    #[error("{0}")]
    Refused(String),
    /// The pass-through failed.
    #[error("pass-through error: {0}")]
    Io(#[from] io::Error),
    /// The device rejected the command.
    #[error("the device rejected the command: {0}")]
    Rejected(String),
    /// The device reports the operation failed.
    #[error("the device reports the sanitize failed: {0}")]
    Failed(String),
    /// The operation was still running when the wait ended.
    #[error(
        "the sanitize is still running on the device after {0} s; it continues on its own and must finish before the drive is used"
    )]
    Timeout(u64),
}

/// Asks the device what it supports.
pub fn probe(backend: &mut dyn Backend) -> FirmwareInfo {
    match backend.transport() {
        Transport::Ata => ata::probe(backend),
        Transport::Nvme => nvme::probe(backend),
    }
}

/// Runs `action`; `progress` is called while the device works; `cancel`
/// stops the *waiting* (the device finishes on its own).
///
/// # Errors
///
/// Returns [`FirmwareError`] when the device refuses, rejects or fails.
pub fn sanitize(
    backend: &mut dyn Backend,
    info: &FirmwareInfo,
    action: FirmwareAction,
    progress: &mut dyn FnMut(SanitizeProgress),
    cancel: &AtomicBool,
) -> Result<(), FirmwareError> {
    info.can_run(action).map_err(FirmwareError::Refused)?;
    let started = Instant::now();
    match backend.transport() {
        Transport::Ata => ata::sanitize(backend, info, action, started, progress, cancel),
        Transport::Nvme => nvme::sanitize(backend, action, started, progress, cancel),
    }
}

/// Waits between status polls, honouring cancellation of the wait.
pub(crate) fn poll_wait(cancel: &AtomicBool) -> bool {
    for _ in 0..20 {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

/// How long the wait for an asynchronous sanitize may last.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(24 * 3600);

/// What the engine's firmware path produced.
#[derive(Debug)]
pub struct FirmwareOutcome {
    /// The sample comparison, when the device could be read.
    pub verification: Option<crate::certificate::VerificationRecord>,
    /// `Ok` when the device reports the sanitize completed.
    pub result: Result<(), crate::error::EraseError>,
}

/// Blocks sampled before and after the sanitize.
const SAMPLE_COUNT: u64 = 256;

/// Reads `SAMPLE_COUNT` blocks of 64 KiB spread over the device: (offset,
/// SHA-256, all zero).
fn sample_device(path: &std::path::Path, len: u64) -> Vec<(u64, [u8; 32], bool)> {
    use phoinix_block::BlockReaderExt;
    use sha2::{Digest, Sha256};
    let Ok(reader) = phoinix_device::open_source(path) else {
        return Vec::new();
    };
    let block = crate::verify::SAMPLE_BLOCK as u64;
    let blocks = len / block;
    if blocks == 0 {
        return Vec::new();
    }
    let samples = blocks.min(SAMPLE_COUNT);
    let mut buf = vec![0u8; crate::verify::SAMPLE_BLOCK];
    let mut out = Vec::with_capacity(usize::try_from(samples).unwrap_or(0));
    for i in 0..samples {
        let offset = (i * blocks / samples) * block;
        if reader.read_exact_at(offset, &mut buf).is_err() {
            continue;
        }
        let digest: [u8; 32] = Sha256::digest(&buf).into();
        out.push((offset, digest, buf.iter().all(|b| *b == 0)));
    }
    out
}

/// Runs a firmware sanitize on the described device: samples before,
/// issues the command, waits, samples after and compares. A block that
/// held data before and reads the same afterwards is a mismatch.
pub fn run(
    desc: &crate::target::TargetDescription,
    info: &FirmwareInfo,
    action: FirmwareAction,
    verify: crate::method::VerifyMode,
    sink: &mut dyn FnMut(crate::engine::EraseEvent),
    cancel: &AtomicBool,
) -> FirmwareOutcome {
    use crate::certificate::VerificationRecord;
    use crate::engine::EraseEvent;
    use crate::error::{EraseError, Refusal};
    let path = desc.target.path();
    sink(EraseEvent::Pass {
        pass: 1,
        passes: 1,
        pattern: action.label().to_owned(),
    });
    let before = if verify == crate::method::VerifyMode::None {
        Vec::new()
    } else {
        sample_device(path, desc.length)
    };
    let bus = desc.device.as_ref().and_then(|d| d.bus);
    let Some(mut backend) = crate::platform::open_firmware(path, bus) else {
        return FirmwareOutcome {
            verification: None,
            result: Err(EraseError::Refused(Refusal::Unsupported {
                reason: "the device offers no firmware pass-through on this platform".into(),
            })),
        };
    };
    tracing::info!(action = action.id(), model = ?info.model, "firmware sanitize starting");
    let started = Instant::now();
    let mut progress = |p: SanitizeProgress| {
        sink(EraseEvent::Progress {
            pass: 1,
            passes: 1,
            done: u64::from(p.percent.unwrap_or(0)),
            total: 100,
            bytes_per_sec: 0,
            unwritable: 0,
        });
    };
    let result = sanitize(&mut *backend, info, action, &mut progress, cancel);
    drop(backend);
    let result = result.map_err(|e| match e {
        FirmwareError::Refused(reason) => EraseError::Refused(Refusal::Unsupported { reason }),
        other => EraseError::Invalid(other.to_string()),
    });
    let elapsed = started.elapsed().as_secs();
    tracing::info!(elapsed, ok = result.is_ok(), "firmware sanitize finished");
    let verification = if result.is_ok() && !before.is_empty() {
        let mut progress_v = |done, total| sink(EraseEvent::Verifying { done, total });
        progress_v(0, before.len() as u64);
        let after = sample_device(path, desc.length);
        let mut record = VerificationRecord {
            mode: verify,
            ..Default::default()
        };
        for (offset, digest, was_zero) in &before {
            let Some((_, now, _)) = after.iter().find(|(o, _, _)| o == offset) else {
                continue;
            };
            record.blocks_checked += 1;
            record.bytes_checked += crate::verify::SAMPLE_BLOCK as u64;
            if !*was_zero && now == digest {
                record.blocks_mismatched += 1;
                if record.first_mismatch.is_none() {
                    record.first_mismatch = Some(*offset);
                }
            }
        }
        progress_v(before.len() as u64, before.len() as u64);
        record.passed = record.blocks_checked > 0 && record.blocks_mismatched == 0;
        Some(record)
    } else {
        None
    };
    FirmwareOutcome {
        verification,
        result,
    }
}
