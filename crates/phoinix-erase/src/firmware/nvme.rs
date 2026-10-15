//! NVMe: Identify Controller, Sanitize, Format NVM, the sanitize status
//! log (NVM Express base specification).

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::{
    Backend, DataDir, FirmwareAction, FirmwareError, FirmwareInfo, MAX_WAIT, NvmeCommand,
    SanitizeProgress, Transport, poll_wait,
};

/// Get Log Page.
pub const OP_GET_LOG_PAGE: u8 = 0x02;
/// Identify.
pub const OP_IDENTIFY: u8 = 0x06;
/// Format NVM.
pub const OP_FORMAT_NVM: u8 = 0x80;
/// Sanitize.
pub const OP_SANITIZE: u8 = 0x84;
/// Sanitize status log page identifier.
pub const LOG_SANITIZE_STATUS: u32 = 0x81;
/// Sanitize actions (SANACT).
pub const SANACT_BLOCK_ERASE: u32 = 2;
/// Overwrite.
pub const SANACT_OVERWRITE: u32 = 3;
/// Crypto erase.
pub const SANACT_CRYPTO_ERASE: u32 = 4;
/// Format NVM secure erase settings (SES).
pub const SES_USER_DATA: u32 = 1;
/// Crypto erase.
pub const SES_CRYPTO: u32 = 2;

/// Identify Controller data.
#[derive(Debug, Clone)]
pub struct IdentifyController {
    data: Vec<u8>,
}

impl IdentifyController {
    /// Parses the 4096-byte Identify Controller data.
    #[must_use]
    pub fn parse(data: &[u8]) -> Option<Self> {
        (data.len() >= 4096).then(|| Self {
            data: data.get(..4096).unwrap_or(data).to_vec(),
        })
    }

    fn byte(&self, i: usize) -> u8 {
        self.data.get(i).copied().unwrap_or(0)
    }

    fn u32_at(&self, i: usize) -> u32 {
        u32::from_le_bytes([
            self.byte(i),
            self.byte(i + 1),
            self.byte(i + 2),
            self.byte(i + 3),
        ])
    }

    fn text(&self, first: usize, count: usize) -> Option<String> {
        let s = String::from_utf8_lossy(self.data.get(first..first + count).unwrap_or(&[]))
            .trim()
            .to_owned();
        (!s.is_empty()).then_some(s)
    }

    /// Serial number (bytes 4–23).
    #[must_use]
    pub fn serial(&self) -> Option<String> {
        self.text(4, 20)
    }

    /// Model number (bytes 24–63).
    #[must_use]
    pub fn model(&self) -> Option<String> {
        self.text(24, 40)
    }

    /// Format NVM supported (OACS bit 1, bytes 256–257).
    #[must_use]
    pub fn format_supported(&self) -> bool {
        self.byte(256) & 0x02 != 0
    }

    /// Crypto erase supported by Format NVM (FNA bit 2, byte 524).
    #[must_use]
    pub fn format_crypto_supported(&self) -> bool {
        self.byte(524) & 0x04 != 0
    }

    /// SANICAP (bytes 328–331).
    #[must_use]
    pub fn sanicap(&self) -> u32 {
        self.u32_at(328)
    }

    /// Sanitize crypto erase supported (SANICAP bit 0).
    #[must_use]
    pub fn crypto_erase_supported(&self) -> bool {
        self.sanicap() & 0x1 != 0
    }

    /// Sanitize block erase supported (SANICAP bit 1).
    #[must_use]
    pub fn block_erase_supported(&self) -> bool {
        self.sanicap() & 0x2 != 0
    }

    /// Sanitize overwrite supported (SANICAP bit 2).
    #[must_use]
    pub fn overwrite_supported(&self) -> bool {
        self.sanicap() & 0x4 != 0
    }

    /// What the controller supports, best first.
    #[must_use]
    pub fn actions(&self) -> Vec<FirmwareAction> {
        let mut out = Vec::new();
        if self.crypto_erase_supported() {
            out.push(FirmwareAction::CryptoErase);
        }
        if self.block_erase_supported() {
            out.push(FirmwareAction::BlockErase);
        }
        if self.format_supported() && self.format_crypto_supported() {
            out.push(FirmwareAction::FormatCryptoErase);
        }
        if self.format_supported() {
            out.push(FirmwareAction::FormatUserData);
        }
        if self.overwrite_supported() {
            out.push(FirmwareAction::Overwrite);
        }
        out
    }
}

/// The Identify Controller command (CNS 1).
#[must_use]
pub const fn identify_controller() -> NvmeCommand {
    NvmeCommand {
        opcode: OP_IDENTIFY,
        nsid: 0,
        cdw10: 1,
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    }
}

/// The Identify Namespace command (CNS 0) for `nsid`.
#[must_use]
pub const fn identify_namespace(nsid: u32) -> NvmeCommand {
    NvmeCommand {
        opcode: OP_IDENTIFY,
        nsid,
        cdw10: 0,
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    }
}

/// The Sanitize command for `sanact` (one overwrite pass of zeros when
/// overwriting).
#[must_use]
pub const fn sanitize_command(sanact: u32) -> NvmeCommand {
    let owpass = if sanact == SANACT_OVERWRITE {
        1 << 4
    } else {
        0
    };
    NvmeCommand {
        opcode: OP_SANITIZE,
        nsid: 0,
        cdw10: sanact | owpass,
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    }
}

/// The Get Log Page command for the 512-byte sanitize status log.
#[must_use]
pub const fn sanitize_status_log() -> NvmeCommand {
    // NUMD (dwords - 1) in bits 27:16, LID in bits 7:0.
    NvmeCommand {
        opcode: OP_GET_LOG_PAGE,
        nsid: 0xFFFF_FFFF,
        cdw10: (127 << 16) | LOG_SANITIZE_STATUS,
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    }
}

/// The Format NVM command for `nsid` keeping LBA format `lbaf`, with
/// secure-erase setting `ses`.
#[must_use]
pub const fn format_command(nsid: u32, lbaf: u8, ses: u32) -> NvmeCommand {
    NvmeCommand {
        opcode: OP_FORMAT_NVM,
        nsid,
        cdw10: (lbaf as u32 & 0xF) | (ses << 9),
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    }
}

/// Sanitize status from the log page: (status code, percent).
/// Status: 0 never sanitized, 1 completed, 2 in progress, 3 failed, 4
/// completed with deallocation.
#[must_use]
pub fn parse_status_log(data: &[u8]) -> (u8, u8) {
    let sprog = u16::from_le_bytes([
        data.first().copied().unwrap_or(0),
        data.get(1).copied().unwrap_or(0),
    ]);
    let sstat = u16::from_le_bytes([
        data.get(2).copied().unwrap_or(0),
        data.get(3).copied().unwrap_or(0),
    ]);
    (
        u8::try_from(sstat & 0x7).unwrap_or(0),
        u8::try_from(u32::from(sprog) * 100 / 65_536).unwrap_or(100),
    )
}

/// Probes an NVMe controller.
pub fn probe(backend: &mut dyn Backend) -> FirmwareInfo {
    let mut data = vec![0u8; 4096];
    let mut info = FirmwareInfo {
        transport: Some(Transport::Nvme),
        solid_state: Some(true),
        ..Default::default()
    };
    match backend.nvme_admin(
        &identify_controller(),
        DataDir::In,
        &mut data,
        Duration::from_secs(10),
    ) {
        Ok((0, _)) => {}
        Ok((status, _)) => {
            info.note = Some(format!(
                "the controller rejected Identify Controller (status {status:#06x}); the Windows inbox NVMe driver only forwards vendor-specific commands, so a live Linux system is the way to run firmware sanitize there"
            ));
            return info;
        }
        Err(e) => {
            info.note = Some(format!(
                "the controller did not answer Identify Controller through the pass-through ({e}); the Windows inbox NVMe driver only forwards vendor-specific commands, so a live Linux system is the way to run firmware sanitize there"
            ));
            return info;
        }
    }
    let Some(id) = IdentifyController::parse(&data) else {
        info.note = Some("malformed Identify Controller data".into());
        return info;
    };
    info.model = id.model();
    info.serial = id.serial();
    info.supports = id.actions();
    if info.supports.is_empty() {
        info.note = Some("the controller reports neither Sanitize nor Format NVM".into());
    }
    info
}

fn status_text(status: u16) -> String {
    match status {
        0x0001 => "invalid command opcode".into(),
        0x0002 => "invalid field in command".into(),
        0x000B => "namespace not ready".into(),
        0x0012 => "command sequence error".into(),
        0x010A => "invalid format".into(),
        0x0115 => "sanitize in progress".into(),
        0x0116 => "sanitize failed".into(),
        0x0117 => "sanitize prohibited while persistent memory region is enabled".into(),
        other => format!("status {other:#06x}"),
    }
}

/// Runs an NVMe action.
pub fn sanitize(
    backend: &mut dyn Backend,
    action: FirmwareAction,
    started: Instant,
    progress: &mut dyn FnMut(SanitizeProgress),
    cancel: &AtomicBool,
) -> Result<(), FirmwareError> {
    match action {
        FirmwareAction::CryptoErase | FirmwareAction::BlockErase | FirmwareAction::Overwrite => {
            let sanact = match action {
                FirmwareAction::CryptoErase => SANACT_CRYPTO_ERASE,
                FirmwareAction::BlockErase => SANACT_BLOCK_ERASE,
                _ => SANACT_OVERWRITE,
            };
            let mut none = [];
            let (status, _) = backend.nvme_admin(
                &sanitize_command(sanact),
                DataDir::None,
                &mut none,
                Duration::from_secs(30),
            )?;
            if status != 0 {
                return Err(FirmwareError::Rejected(status_text(status)));
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
                let mut log = vec![0u8; 512];
                let (status, _) = backend.nvme_admin(
                    &sanitize_status_log(),
                    DataDir::In,
                    &mut log,
                    Duration::from_secs(10),
                )?;
                if status != 0 {
                    return Err(FirmwareError::Failed(format!(
                        "sanitize status log unavailable: {}",
                        status_text(status)
                    )));
                }
                let (code, percent) = parse_status_log(&log);
                match code {
                    2 => progress(SanitizeProgress {
                        percent: Some(percent),
                        elapsed: started.elapsed().as_secs(),
                    }),
                    1 | 4 => {
                        progress(SanitizeProgress {
                            percent: Some(100),
                            elapsed: started.elapsed().as_secs(),
                        });
                        return Ok(());
                    }
                    3 => return Err(FirmwareError::Failed("sanitize status: failed".into())),
                    _ => {
                        return Err(FirmwareError::Failed(
                            "sanitize status: the device reports no sanitize operation".into(),
                        ));
                    }
                }
            }
        }
        FirmwareAction::FormatUserData | FirmwareAction::FormatCryptoErase => {
            let ses = if action == FirmwareAction::FormatCryptoErase {
                SES_CRYPTO
            } else {
                SES_USER_DATA
            };
            // Keep the namespace's current LBA format (FLBAS bits 3:0 of
            // Identify Namespace, byte 26).
            let nsid = 1;
            let mut ns = vec![0u8; 4096];
            let (status, _) = backend.nvme_admin(
                &identify_namespace(nsid),
                DataDir::In,
                &mut ns,
                Duration::from_secs(10),
            )?;
            if status != 0 {
                return Err(FirmwareError::Rejected(format!(
                    "Identify Namespace: {}",
                    status_text(status)
                )));
            }
            let lbaf = ns.get(26).copied().unwrap_or(0) & 0x0F;
            progress(SanitizeProgress {
                percent: None,
                elapsed: 0,
            });
            let mut none = [];
            let (status, _) = backend.nvme_admin(
                &format_command(nsid, lbaf, ses),
                DataDir::None,
                &mut none,
                Duration::from_secs(2 * 3600),
            )?;
            if status != 0 {
                return Err(FirmwareError::Failed(format!(
                    "Format NVM: {}",
                    status_text(status)
                )));
            }
            progress(SanitizeProgress {
                percent: Some(100),
                elapsed: started.elapsed().as_secs(),
            });
            Ok(())
        }
        FirmwareAction::SecurityErase | FirmwareAction::EnhancedSecurityErase => Err(
            FirmwareError::Refused("ATA security erase does not apply to an NVMe device".into()),
        ),
    }
}
