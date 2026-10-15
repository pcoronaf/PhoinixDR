//! ATA: IDENTIFY DEVICE, the SANITIZE DEVICE feature set and the security
//! erase sequence (ACS-3 / ACS-4).

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::{
    AtaCommand, AtaResult, Backend, DataDir, FirmwareAction, FirmwareError, FirmwareInfo, MAX_WAIT,
    SanitizeProgress, Transport, poll_wait,
};

/// IDENTIFY DEVICE.
pub const CMD_IDENTIFY: u8 = 0xEC;
/// SANITIZE DEVICE.
pub const CMD_SANITIZE: u8 = 0xB4;
/// SECURITY SET PASSWORD.
pub const CMD_SECURITY_SET_PASSWORD: u8 = 0xF1;
/// SECURITY ERASE PREPARE.
pub const CMD_SECURITY_ERASE_PREPARE: u8 = 0xF3;
/// SECURITY ERASE UNIT.
pub const CMD_SECURITY_ERASE_UNIT: u8 = 0xF4;
/// SECURITY DISABLE PASSWORD.
pub const CMD_SECURITY_DISABLE_PASSWORD: u8 = 0xF6;

/// SANITIZE DEVICE features.
pub const SANITIZE_STATUS_EXT: u16 = 0x0000;
/// CRYPTO SCRAMBLE EXT.
pub const SANITIZE_CRYPTO_SCRAMBLE_EXT: u16 = 0x0011;
/// BLOCK ERASE EXT.
pub const SANITIZE_BLOCK_ERASE_EXT: u16 = 0x0012;
/// OVERWRITE EXT.
pub const SANITIZE_OVERWRITE_EXT: u16 = 0x0014;
/// Key in the LBA field of CRYPTO SCRAMBLE EXT ("Cryp").
pub const KEY_CRYPTO: u64 = 0x4372_7970;
/// Key in the LBA field of BLOCK ERASE EXT ("BkEr").
pub const KEY_BLOCK: u64 = 0x426B_4572;
/// Key in bits 47:32 of the LBA field of OVERWRITE EXT ("OW").
pub const KEY_OVERWRITE: u64 = 0x4F57;

/// The temporary password of the security erase sequence. It is set
/// right before the erase and removed right after; if the drive stays
/// locked after a failure, this is the password to unlock it with.
pub const TEMP_PASSWORD: &[u8; 9] = b"PhoinixDR";

/// IDENTIFY DEVICE words.
#[derive(Debug, Clone)]
pub struct Identify {
    words: Vec<u16>,
}

impl Identify {
    /// Parses the 512-byte IDENTIFY DEVICE data.
    #[must_use]
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 512 {
            return None;
        }
        let words: Vec<u16> = data
            .chunks_exact(2)
            .take(256)
            .map(|c| {
                u16::from_le_bytes([
                    c.first().copied().unwrap_or(0),
                    c.get(1).copied().unwrap_or(0),
                ])
            })
            .collect();
        if words.first().copied().unwrap_or(0) & 0x8000 != 0 {
            // Bit 15 set: not an ATA device (ATAPI).
            return None;
        }
        Some(Self { words })
    }

    fn word(&self, n: usize) -> u16 {
        self.words.get(n).copied().unwrap_or(0)
    }

    fn bit(&self, word: usize, bit: u8) -> bool {
        self.word(word) & (1 << bit) != 0
    }

    /// An ATA string (byte-swapped words), trimmed.
    fn text(&self, first: usize, count: usize) -> Option<String> {
        let bytes: Vec<u8> = (first..first + count)
            .flat_map(|w| {
                let v = self.word(w);
                [(v >> 8) as u8, (v & 0xFF) as u8]
            })
            .collect();
        let s = String::from_utf8_lossy(&bytes).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// Model number (words 27–46).
    #[must_use]
    pub fn model(&self) -> Option<String> {
        self.text(27, 20)
    }

    /// Serial number (words 10–19).
    #[must_use]
    pub fn serial(&self) -> Option<String> {
        self.text(10, 10)
    }

    /// Security feature set supported (word 82 bit 1 / word 128 bit 0).
    #[must_use]
    pub fn security_supported(&self) -> bool {
        self.bit(82, 1) || self.bit(128, 0)
    }

    /// Security enabled (word 128 bit 1).
    #[must_use]
    pub fn security_enabled(&self) -> bool {
        self.bit(128, 1)
    }

    /// Security locked (word 128 bit 2).
    #[must_use]
    pub fn security_locked(&self) -> bool {
        self.bit(128, 2)
    }

    /// Security frozen (word 128 bit 3).
    #[must_use]
    pub fn security_frozen(&self) -> bool {
        self.bit(128, 3)
    }

    /// Enhanced security erase supported (word 128 bit 5).
    #[must_use]
    pub fn enhanced_erase_supported(&self) -> bool {
        self.bit(128, 5)
    }

    /// Minutes of a (normal or enhanced) security erase, from word 89 /
    /// 90: units of 2 minutes; 0 = not specified; bit 15 set means the
    /// extended format (bits 14:0), otherwise bits 7:0.
    fn erase_minutes(&self, word: usize) -> Option<u32> {
        let v = self.word(word);
        let units = if v & 0x8000 != 0 {
            u32::from(v & 0x7FFF)
        } else {
            u32::from(v & 0x00FF)
        };
        (units > 0).then_some(units * 2)
    }

    /// Minutes of a normal security erase.
    #[must_use]
    pub fn security_erase_minutes(&self) -> Option<u32> {
        self.erase_minutes(89)
    }

    /// Minutes of an enhanced security erase.
    #[must_use]
    pub fn enhanced_erase_minutes(&self) -> Option<u32> {
        self.erase_minutes(90)
    }

    /// SANITIZE feature set supported (word 59 bit 12).
    #[must_use]
    pub fn sanitize_supported(&self) -> bool {
        self.bit(59, 12)
    }

    /// CRYPTO SCRAMBLE EXT supported (word 59 bit 13).
    #[must_use]
    pub fn crypto_scramble_supported(&self) -> bool {
        self.sanitize_supported() && self.bit(59, 13)
    }

    /// OVERWRITE EXT supported (word 59 bit 14).
    #[must_use]
    pub fn overwrite_supported(&self) -> bool {
        self.sanitize_supported() && self.bit(59, 14)
    }

    /// BLOCK ERASE EXT supported (word 59 bit 15).
    #[must_use]
    pub fn block_erase_supported(&self) -> bool {
        self.sanitize_supported() && self.bit(59, 15)
    }

    /// Non-rotating medium (word 217 == 1).
    #[must_use]
    pub fn solid_state(&self) -> Option<bool> {
        match self.word(217) {
            0 | 0xFFFF => None,
            1 => Some(true),
            _ => Some(false),
        }
    }

    /// What the device supports, best first.
    #[must_use]
    pub fn actions(&self) -> Vec<FirmwareAction> {
        let mut out = Vec::new();
        if self.crypto_scramble_supported() {
            out.push(FirmwareAction::CryptoErase);
        }
        if self.block_erase_supported() {
            out.push(FirmwareAction::BlockErase);
        }
        if self.security_supported() && self.enhanced_erase_supported() {
            out.push(FirmwareAction::EnhancedSecurityErase);
        }
        if self.security_supported() {
            out.push(FirmwareAction::SecurityErase);
        }
        if self.overwrite_supported() {
            out.push(FirmwareAction::Overwrite);
        }
        out
    }
}

/// The IDENTIFY DEVICE command.
#[must_use]
pub const fn identify_command() -> AtaCommand {
    AtaCommand {
        feature: 0,
        count: 0,
        lba: 0,
        device: 0x40,
        command: CMD_IDENTIFY,
        ext: false,
        dir: DataDir::In,
    }
}

/// A SANITIZE DEVICE command for `feature`.
#[must_use]
pub const fn sanitize_command(feature: u16, count: u16, lba: u64) -> AtaCommand {
    AtaCommand {
        feature,
        count,
        lba,
        device: 0x40,
        command: CMD_SANITIZE,
        ext: true,
        dir: DataDir::None,
    }
}

/// The 512-byte data block of SECURITY SET PASSWORD / ERASE UNIT /
/// DISABLE PASSWORD: word 0 flags, words 1–16 the password.
#[must_use]
pub fn security_block(enhanced: bool, maximum_level: bool) -> Vec<u8> {
    let mut block = vec![0u8; 512];
    let mut word0: u16 = 0; // bit 0: user password
    if enhanced {
        word0 |= 1 << 1;
    }
    if maximum_level {
        word0 |= 1 << 8;
    }
    if let Some(slot) = block.get_mut(0..2) {
        slot.copy_from_slice(&word0.to_le_bytes());
    }
    if let Some(slot) = block.get_mut(2..2 + TEMP_PASSWORD.len()) {
        slot.copy_from_slice(TEMP_PASSWORD);
    }
    block
}

fn security_command(command: u8, dir: DataDir) -> AtaCommand {
    AtaCommand {
        feature: 0,
        count: 0,
        lba: 0,
        device: 0x40,
        command,
        ext: false,
        dir,
    }
}

/// Probes an ATA device.
pub fn probe(backend: &mut dyn Backend) -> FirmwareInfo {
    let mut data = vec![0u8; 512];
    let mut info = FirmwareInfo {
        transport: Some(Transport::Ata),
        ..Default::default()
    };
    let result = match backend.ata(&identify_command(), &mut data, Duration::from_secs(10)) {
        Ok(r) => r,
        Err(e) => {
            info.note = Some(format!(
                "the device did not answer IDENTIFY DEVICE through the pass-through ({e}); a USB bridge usually does not forward ATA commands"
            ));
            return info;
        }
    };
    if result.failed() {
        info.note = Some("the device rejected IDENTIFY DEVICE".into());
        return info;
    }
    let Some(id) = Identify::parse(&data) else {
        info.note = Some("not an ATA device (ATAPI or malformed IDENTIFY data)".into());
        return info;
    };
    info.model = id.model();
    info.serial = id.serial();
    info.supports = id.actions();
    info.frozen = id.security_frozen();
    info.security_enabled = id.security_enabled();
    info.locked = id.security_locked();
    info.solid_state = id.solid_state();
    info.security_erase_minutes = id.security_erase_minutes();
    info.enhanced_erase_minutes = id.enhanced_erase_minutes();
    if info.supports.is_empty() {
        info.note =
            Some("the device reports neither the SANITIZE nor the security feature set".into());
    } else if info.frozen {
        info.note = Some(format!("security is frozen; {}", super::FROZEN_ADVICE));
    } else if info.locked {
        info.note = Some("the device is security-locked".into());
    }
    info
}

/// Reads SANITIZE STATUS EXT: (in progress, completed, percent).
fn sanitize_status(backend: &mut dyn Backend) -> Result<(bool, bool, Option<u8>), FirmwareError> {
    let mut none = [];
    let r = backend.ata(
        &sanitize_command(SANITIZE_STATUS_EXT, 0, 0),
        &mut none,
        Duration::from_secs(10),
    )?;
    if r.failed() {
        return Err(FirmwareError::Failed(format!(
            "SANITIZE STATUS EXT returned error {:#04x}",
            r.error
        )));
    }
    let in_progress = r.count & (1 << 14) != 0;
    let completed = r.count & (1 << 15) != 0;
    let percent = if in_progress {
        let fraction = r.lba & 0xFFFF;
        Some(u8::try_from(fraction * 100 / 65_536).unwrap_or(100))
    } else {
        None
    };
    Ok((in_progress, completed, percent))
}

/// Runs an ATA action.
pub fn sanitize(
    backend: &mut dyn Backend,
    info: &FirmwareInfo,
    action: FirmwareAction,
    started: Instant,
    progress: &mut dyn FnMut(SanitizeProgress),
    cancel: &AtomicBool,
) -> Result<(), FirmwareError> {
    match action {
        FirmwareAction::CryptoErase | FirmwareAction::BlockErase | FirmwareAction::Overwrite => {
            let cmd = match action {
                FirmwareAction::CryptoErase => {
                    sanitize_command(SANITIZE_CRYPTO_SCRAMBLE_EXT, 0, KEY_CRYPTO)
                }
                FirmwareAction::BlockErase => {
                    sanitize_command(SANITIZE_BLOCK_ERASE_EXT, 0, KEY_BLOCK)
                }
                // One pass of zeros: COUNT bits 3:0 = pass count, LBA bits
                // 31:0 = pattern, bits 47:32 = key.
                _ => sanitize_command(SANITIZE_OVERWRITE_EXT, 1, KEY_OVERWRITE << 32),
            };
            let mut none = [];
            let r = backend.ata(&cmd, &mut none, Duration::from_secs(30))?;
            if r.failed() {
                return Err(FirmwareError::Rejected(format!(
                    "SANITIZE DEVICE error {:#04x} (status {:#04x})",
                    r.error, r.status
                )));
            }
            progress(SanitizeProgress {
                percent: Some(0),
                elapsed: 0,
            });
            loop {
                if started.elapsed() > MAX_WAIT {
                    return Err(FirmwareError::Timeout(started.elapsed().as_secs()));
                }
                if !poll_wait(cancel) {
                    return Err(FirmwareError::Timeout(started.elapsed().as_secs()));
                }
                let (in_progress, completed, percent) = sanitize_status(backend)?;
                progress(SanitizeProgress {
                    percent: if in_progress { percent } else { Some(100) },
                    elapsed: started.elapsed().as_secs(),
                });
                if !in_progress {
                    if completed {
                        return Ok(());
                    }
                    return Err(FirmwareError::Failed(
                        "the device reports the sanitize is neither in progress nor completed"
                            .into(),
                    ));
                }
            }
        }
        FirmwareAction::SecurityErase | FirmwareAction::EnhancedSecurityErase => {
            let enhanced = action == FirmwareAction::EnhancedSecurityErase;
            let minutes = if enhanced {
                info.enhanced_erase_minutes
            } else {
                info.security_erase_minutes
            }
            .unwrap_or(120);
            // The erase blocks until done: wait the device's estimate
            // twice, at least two hours.
            let timeout = Duration::from_secs(u64::from(minutes.max(60)) * 60 * 2);
            // 1. Set the temporary user password (high security level).
            let mut block = security_block(false, false);
            let r = backend.ata(
                &security_command(CMD_SECURITY_SET_PASSWORD, DataDir::Out),
                &mut block,
                Duration::from_secs(30),
            )?;
            if r.failed() {
                return Err(FirmwareError::Rejected(format!(
                    "SECURITY SET PASSWORD error {:#04x}",
                    r.error
                )));
            }
            // 2. Prepare, 3. erase. On any failure past this point, try to
            // remove the password so the drive is not left locked.
            let outcome = (|| -> Result<(), FirmwareError> {
                let mut none = [];
                let r = backend.ata(
                    &security_command(CMD_SECURITY_ERASE_PREPARE, DataDir::None),
                    &mut none,
                    Duration::from_secs(30),
                )?;
                if r.failed() {
                    return Err(FirmwareError::Rejected(format!(
                        "SECURITY ERASE PREPARE error {:#04x}",
                        r.error
                    )));
                }
                progress(SanitizeProgress {
                    percent: None,
                    elapsed: started.elapsed().as_secs(),
                });
                let mut block = security_block(enhanced, false);
                let r = backend.ata(
                    &security_command(CMD_SECURITY_ERASE_UNIT, DataDir::Out),
                    &mut block,
                    timeout,
                )?;
                if r.failed() {
                    return Err(FirmwareError::Failed(format!(
                        "SECURITY ERASE UNIT error {:#04x}",
                        r.error
                    )));
                }
                Ok(())
            })();
            if outcome.is_err() {
                let mut block = security_block(false, false);
                let _ = backend.ata(
                    &security_command(CMD_SECURITY_DISABLE_PASSWORD, DataDir::Out),
                    &mut block,
                    Duration::from_secs(30),
                );
            }
            outcome?;
            progress(SanitizeProgress {
                percent: Some(100),
                elapsed: started.elapsed().as_secs(),
            });
            // A successful erase clears the password on most drives; make
            // sure by disabling it (a rejection here is harmless).
            let mut block = security_block(false, false);
            let _ = backend.ata(
                &security_command(CMD_SECURITY_DISABLE_PASSWORD, DataDir::Out),
                &mut block,
                Duration::from_secs(30),
            );
            Ok(())
        }
        FirmwareAction::FormatUserData | FirmwareAction::FormatCryptoErase => Err(
            FirmwareError::Refused("NVMe Format NVM does not apply to an ATA device".into()),
        ),
    }
}

/// Decodes the result registers of a 48-bit pass-through reply.
#[must_use]
pub fn result_from_taskfile(
    status: u8,
    error: u8,
    count_lo: u8,
    count_hi: u8,
    lba: [u8; 6],
) -> AtaResult {
    let lba_v = u64::from(lba[0])
        | u64::from(lba[1]) << 8
        | u64::from(lba[2]) << 16
        | u64::from(lba[3]) << 24
        | u64::from(lba[4]) << 32
        | u64::from(lba[5]) << 40;
    AtaResult {
        status,
        error,
        count: u16::from(count_lo) | u16::from(count_hi) << 8,
        lba: lba_v,
    }
}
