//! M12.4: the firmware sanitize logic against a simulated device: what
//! IDENTIFY and Identify Controller say becomes the offered actions, a
//! frozen drive refuses the security erase with the usual advice, the
//! security erase sets and removes its temporary password even when the
//! erase fails, and the asynchronous sanitize is polled to completion.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

use std::io;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use phoinix_erase::firmware::ata::{
    CMD_IDENTIFY, CMD_SANITIZE, CMD_SECURITY_DISABLE_PASSWORD, CMD_SECURITY_ERASE_PREPARE,
    CMD_SECURITY_ERASE_UNIT, CMD_SECURITY_SET_PASSWORD, KEY_BLOCK, KEY_CRYPTO,
    SANITIZE_BLOCK_ERASE_EXT, SANITIZE_CRYPTO_SCRAMBLE_EXT, SANITIZE_STATUS_EXT, TEMP_PASSWORD,
};
use phoinix_erase::firmware::nvme::{
    OP_FORMAT_NVM, OP_GET_LOG_PAGE, OP_IDENTIFY, OP_SANITIZE, SANACT_CRYPTO_ERASE,
};
use phoinix_erase::firmware::{
    AtaCommand, AtaResult, Backend, DataDir, FirmwareAction, FirmwareError, NvmeCommand, Transport,
    probe, sanitize,
};

/// A simulated ATA drive.
struct AtaDrive {
    words: [u16; 256],
    /// Polls of SANITIZE STATUS EXT before the operation completes.
    polls_to_finish: u32,
    polls: u32,
    password_set: bool,
    erase_fails: bool,
    log: Vec<u8>,
}

impl AtaDrive {
    fn new() -> Self {
        let mut words = [0u16; 256];
        // Model "SIMULATED SSD", serial "SN123456".
        for (i, pair) in b"SIMULATED SSD                           "
            .chunks(2)
            .enumerate()
        {
            words[27 + i] = u16::from(pair[0]) << 8 | u16::from(pair[1]);
        }
        for (i, pair) in b"SN123456            ".chunks(2).enumerate() {
            words[10 + i] = u16::from(pair[0]) << 8 | u16::from(pair[1]);
        }
        words[82] = 1 << 1; // security supported
        words[128] = 1 | 1 << 5; // supported, enhanced erase supported
        words[89] = 2; // 4 minutes
        words[90] = 1; // 2 minutes
        words[59] = 1 << 12 | 1 << 13 | 1 << 15; // sanitize, crypto, block erase
        words[217] = 1; // SSD
        Self {
            words,
            polls_to_finish: 3,
            polls: 0,
            password_set: false,
            erase_fails: false,
            log: Vec::new(),
        }
    }
}

impl Backend for AtaDrive {
    fn transport(&self) -> Transport {
        Transport::Ata
    }

    fn ata(&mut self, cmd: &AtaCommand, data: &mut [u8], _t: Duration) -> io::Result<AtaResult> {
        self.log.push(cmd.command);
        match cmd.command {
            CMD_IDENTIFY => {
                for (i, w) in self.words.iter().enumerate() {
                    data[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
                }
                Ok(AtaResult::default())
            }
            CMD_SANITIZE => match cmd.feature {
                SANITIZE_CRYPTO_SCRAMBLE_EXT => {
                    assert_eq!(cmd.lba, KEY_CRYPTO);
                    assert!(cmd.ext);
                    self.polls = 0;
                    Ok(AtaResult::default())
                }
                SANITIZE_BLOCK_ERASE_EXT => {
                    assert_eq!(cmd.lba, KEY_BLOCK);
                    self.polls = 0;
                    Ok(AtaResult::default())
                }
                SANITIZE_STATUS_EXT => {
                    self.polls += 1;
                    if self.polls < self.polls_to_finish {
                        Ok(AtaResult {
                            count: 1 << 14,
                            lba: u64::from(self.polls) * 20_000,
                            ..Default::default()
                        })
                    } else {
                        Ok(AtaResult {
                            count: 1 << 15,
                            ..Default::default()
                        })
                    }
                }
                _ => Ok(AtaResult {
                    status: 0x51,
                    error: 0x04,
                    ..Default::default()
                }),
            },
            CMD_SECURITY_SET_PASSWORD => {
                assert_eq!(&data[2..2 + TEMP_PASSWORD.len()], TEMP_PASSWORD);
                self.password_set = true;
                Ok(AtaResult::default())
            }
            CMD_SECURITY_ERASE_PREPARE => Ok(AtaResult::default()),
            CMD_SECURITY_ERASE_UNIT => {
                assert!(self.password_set);
                if self.erase_fails {
                    Ok(AtaResult {
                        status: 0x51,
                        error: 0x04,
                        ..Default::default()
                    })
                } else {
                    // A successful erase clears the password.
                    self.password_set = false;
                    Ok(AtaResult::default())
                }
            }
            CMD_SECURITY_DISABLE_PASSWORD => {
                self.password_set = false;
                Ok(AtaResult::default())
            }
            _ => Ok(AtaResult {
                status: 0x51,
                error: 0x04,
                ..Default::default()
            }),
        }
    }

    fn nvme_admin(
        &mut self,
        _cmd: &NvmeCommand,
        _dir: DataDir,
        _data: &mut [u8],
        _t: Duration,
    ) -> io::Result<(u16, u32)> {
        Err(io::Error::other("not NVMe"))
    }
}

#[test]
fn ata_identify_yields_the_actions_and_the_sanitize_is_polled_to_completion() {
    let mut drive = AtaDrive::new();
    let info = probe(&mut drive);
    assert_eq!(info.transport, Some(Transport::Ata));
    assert_eq!(info.model.as_deref(), Some("SIMULATED SSD"));
    assert_eq!(info.serial.as_deref(), Some("SN123456"));
    assert_eq!(
        info.supports,
        vec![
            FirmwareAction::CryptoErase,
            FirmwareAction::BlockErase,
            FirmwareAction::EnhancedSecurityErase,
            FirmwareAction::SecurityErase,
        ]
    );
    assert_eq!(info.solid_state, Some(true));
    assert_eq!(info.security_erase_minutes, Some(4));
    assert_eq!(info.enhanced_erase_minutes, Some(2));
    assert!(!info.frozen);
    assert_eq!(info.best(), Some(FirmwareAction::CryptoErase));
    let mut percents = Vec::new();
    sanitize(
        &mut drive,
        &info,
        FirmwareAction::CryptoErase,
        &mut |p| percents.push(p.percent),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(percents.first(), Some(&Some(0)));
    assert_eq!(percents.last(), Some(&Some(100)));
    assert!(percents.len() >= 3);
    assert_eq!(
        drive.log.iter().filter(|c| **c == CMD_SANITIZE).count(),
        1 + 3
    );
}

#[test]
fn ata_frozen_drives_refuse_the_security_erase_with_advice() {
    let mut drive = AtaDrive::new();
    drive.words[128] |= 1 << 3; // frozen
    let info = probe(&mut drive);
    assert!(info.frozen);
    assert!(info.note.as_deref().unwrap_or("").contains("frozen"));
    let err = sanitize(
        &mut drive,
        &info,
        FirmwareAction::EnhancedSecurityErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(err, FirmwareError::Refused(ref m) if m.contains("sleep")),
        "{err}"
    );
    // Sanitize commands are not affected by the frozen state.
    assert_eq!(info.best(), Some(FirmwareAction::CryptoErase));
    assert!(info.can_run(FirmwareAction::BlockErase).is_ok());
    // No command past IDENTIFY reached the drive.
    assert_eq!(drive.log, vec![CMD_IDENTIFY]);
}

#[test]
fn ata_security_erase_sets_and_removes_the_temporary_password() {
    let mut drive = AtaDrive::new();
    let info = probe(&mut drive);
    sanitize(
        &mut drive,
        &info,
        FirmwareAction::EnhancedSecurityErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        drive.log,
        vec![
            CMD_IDENTIFY,
            CMD_SECURITY_SET_PASSWORD,
            CMD_SECURITY_ERASE_PREPARE,
            CMD_SECURITY_ERASE_UNIT,
            CMD_SECURITY_DISABLE_PASSWORD
        ]
    );
    assert!(!drive.password_set);

    // A failing erase still removes the password so the drive is not
    // left locked.
    let mut drive = AtaDrive::new();
    drive.erase_fails = true;
    let info = probe(&mut drive);
    let err = sanitize(
        &mut drive,
        &info,
        FirmwareAction::SecurityErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(err, FirmwareError::Failed(_)), "{err}");
    assert!(!drive.password_set);
    assert_eq!(drive.log.last(), Some(&CMD_SECURITY_DISABLE_PASSWORD));
}

#[test]
fn a_drive_without_the_feature_sets_offers_nothing() {
    let mut drive = AtaDrive::new();
    drive.words[59] = 0;
    drive.words[82] = 0;
    drive.words[128] = 0;
    let info = probe(&mut drive);
    assert!(info.supports.is_empty());
    assert!(info.best().is_none());
    assert!(info.note.as_deref().unwrap_or("").contains("neither"));
    let err = sanitize(
        &mut drive,
        &info,
        FirmwareAction::CryptoErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(err, FirmwareError::Refused(_)));
}

/// A simulated NVMe controller.
struct NvmeDrive {
    sanicap: u32,
    oacs_format: bool,
    fna_crypto: bool,
    polls: u32,
    formatted: Option<(u32, u32)>,
    log: Vec<u8>,
}

impl Backend for NvmeDrive {
    fn transport(&self) -> Transport {
        Transport::Nvme
    }

    fn ata(&mut self, _c: &AtaCommand, _d: &mut [u8], _t: Duration) -> io::Result<AtaResult> {
        Err(io::Error::other("not ATA"))
    }

    fn nvme_admin(
        &mut self,
        cmd: &NvmeCommand,
        dir: DataDir,
        data: &mut [u8],
        _t: Duration,
    ) -> io::Result<(u16, u32)> {
        self.log.push(cmd.opcode);
        match cmd.opcode {
            OP_IDENTIFY if cmd.cdw10 == 1 => {
                assert_eq!(dir, DataDir::In);
                data[4..24].copy_from_slice(b"NVME-SERIAL-0001    ");
                data[24..64].copy_from_slice(b"Simulated NVMe Controller               ");
                data[256] = if self.oacs_format { 0x02 } else { 0 };
                data[524] = if self.fna_crypto { 0x04 } else { 0 };
                data[328..332].copy_from_slice(&self.sanicap.to_le_bytes());
                Ok((0, 0))
            }
            OP_IDENTIFY => {
                data[26] = 0x02; // FLBAS: format 2
                Ok((0, 0))
            }
            OP_SANITIZE => {
                assert_eq!(cmd.cdw10 & 0x7, SANACT_CRYPTO_ERASE);
                self.polls = 0;
                Ok((0, 0))
            }
            OP_GET_LOG_PAGE => {
                assert_eq!(cmd.cdw10 & 0xFF, 0x81);
                self.polls += 1;
                let (sprog, sstat): (u16, u16) = if self.polls < 3 { (32_768, 2) } else { (0, 1) };
                data[0..2].copy_from_slice(&sprog.to_le_bytes());
                data[2..4].copy_from_slice(&sstat.to_le_bytes());
                Ok((0, 0))
            }
            OP_FORMAT_NVM => {
                self.formatted = Some((cmd.nsid, cmd.cdw10));
                Ok((0, 0))
            }
            _ => Ok((0x0001, 0)),
        }
    }
}

#[test]
fn nvme_identify_yields_the_actions_and_sanitize_and_format_run() {
    let mut drive = NvmeDrive {
        sanicap: 0b011,
        oacs_format: true,
        fna_crypto: true,
        polls: 0,
        formatted: None,
        log: Vec::new(),
    };
    let info = probe(&mut drive);
    assert_eq!(info.transport, Some(Transport::Nvme));
    assert_eq!(info.model.as_deref(), Some("Simulated NVMe Controller"));
    assert_eq!(info.serial.as_deref(), Some("NVME-SERIAL-0001"));
    assert_eq!(
        info.supports,
        vec![
            FirmwareAction::CryptoErase,
            FirmwareAction::BlockErase,
            FirmwareAction::FormatCryptoErase,
            FirmwareAction::FormatUserData,
        ]
    );
    let mut percents = Vec::new();
    sanitize(
        &mut drive,
        &info,
        FirmwareAction::CryptoErase,
        &mut |p| percents.push(p.percent),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(percents.contains(&Some(50)));
    assert_eq!(percents.last(), Some(&Some(100)));
    sanitize(
        &mut drive,
        &info,
        FirmwareAction::FormatCryptoErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    // Namespace 1, LBA format 2 kept, SES = 2 (crypto erase).
    assert_eq!(drive.formatted, Some((1, 2 | 2 << 9)));
    let err = sanitize(
        &mut drive,
        &info,
        FirmwareAction::SecurityErase,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(err, FirmwareError::Refused(_)));
}

#[test]
fn action_ids_round_trip() {
    for a in FirmwareAction::ALL {
        assert_eq!(FirmwareAction::parse(a.id()), Some(a));
        assert!(!a.label().is_empty() && !a.note().is_empty());
    }
    assert_eq!(FirmwareAction::parse("nope"), None);
}
