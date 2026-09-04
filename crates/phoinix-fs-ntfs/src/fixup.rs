//! Update Sequence Array (USA) handling for multi-sector records.
//!
//! NTFS stores the last two bytes of every sector of a FILE or INDX record in
//! the USA and overwrites them on disk with the update sequence number (USN).
//! Before any field beyond the header may be trusted, every sector tail must
//! match the USN and be restored from the array.

use phoinix_core::bytes::ByteView;

use crate::NtfsError;

/// Result of a successful fixup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixupInfo {
    /// The update sequence number that protected the record.
    pub usn: u16,
    /// Number of sectors that were verified.
    pub sectors: u32,
}

/// Verifies and applies the USA of `record` in place.
///
/// `usa_offset` and `usa_count` come from the record header (offsets 0x04
/// and 0x06 in FILE records). `stride` is the protected block size, normally
/// the volume's bytes-per-sector.
///
/// # Errors
///
/// Returns [`NtfsError::InvalidRecord`] if the array does not fit or the
/// count does not match the record size, and [`NtfsError::FixupMismatch`] if
/// any sector tail differs from the USN.
pub fn apply_fixup(
    record: &mut [u8],
    usa_offset: u16,
    usa_count: u16,
    stride: usize,
    record_number: u64,
) -> Result<FixupInfo, NtfsError> {
    let invalid = |reason: &str| NtfsError::InvalidRecord {
        record: record_number,
        reason: reason.to_owned(),
    };
    if stride < 4 || record.len() < stride || record.len() % stride != 0 {
        return Err(invalid(
            "record length is not a multiple of the sector size",
        ));
    }
    let sectors = record.len() / stride;
    let expected_count = sectors + 1;
    if usize::from(usa_count) != expected_count {
        return Err(invalid(&format!(
            "update sequence count {usa_count} does not match {sectors} sectors"
        )));
    }
    let usa_start = usize::from(usa_offset);
    let usa_len = usize::from(usa_count) * 2;
    let usa_end = usa_start
        .checked_add(usa_len)
        .ok_or_else(|| invalid("update sequence array overflows"))?;
    // The array must live inside the first sector and after the fixed header.
    if usa_start < 0x2A || usa_end > stride {
        return Err(invalid(
            "update sequence array lies outside the first sector",
        ));
    }
    let array: Vec<u8> = record
        .get(usa_start..usa_end)
        .ok_or_else(|| invalid("update sequence array truncated"))?
        .to_vec();
    let view = ByteView::new(&array);
    let usn = view
        .u16_le(0)
        .ok_or_else(|| invalid("update sequence number missing"))?;
    for i in 0..sectors {
        let tail = (i + 1) * stride - 2;
        let stored = record
            .get(tail..tail + 2)
            .ok_or_else(|| invalid("sector tail missing"))?;
        if stored != usn.to_le_bytes() {
            return Err(NtfsError::FixupMismatch {
                record: record_number,
                sector_index: u32::try_from(i).unwrap_or(u32::MAX),
            });
        }
        let original = view
            .array::<2>((i + 1) * 2)
            .ok_or_else(|| invalid("update sequence array truncated"))?;
        if let Some(slot) = record.get_mut(tail..tail + 2) {
            slot.copy_from_slice(&original);
        }
    }
    Ok(FixupInfo {
        usn,
        sectors: u32::try_from(sectors).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Helpers to protect synthetic records with a USA.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    /// Writes a USA at `usa_offset` for `record`, moving the sector tails into
    /// the array and stamping `usn` in their place.
    pub fn protect(record: &mut [u8], usa_offset: usize, stride: usize, usn: u16) {
        let sectors = record.len() / stride;
        record[4..6].copy_from_slice(&(usa_offset as u16).to_le_bytes());
        record[6..8].copy_from_slice(&((sectors + 1) as u16).to_le_bytes());
        record[usa_offset..usa_offset + 2].copy_from_slice(&usn.to_le_bytes());
        for i in 0..sectors {
            let tail = (i + 1) * stride - 2;
            let slot = usa_offset + 2 + i * 2;
            let (a, b) = (record[tail], record[tail + 1]);
            record[slot] = a;
            record[slot + 1] = b;
            record[tail..tail + 2].copy_from_slice(&usn.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]

    use super::testutil::protect;
    use super::*;

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn round_trips_1k_and_4k_records() {
        for (len, stride) in [(1024usize, 512usize), (4096, 512), (4096, 4096)] {
            let original = sample(len);
            let mut record = original.clone();
            protect(&mut record, 0x30, stride, 0x1234);
            assert_ne!(record, original);
            let info =
                apply_fixup(&mut record, 0x30, (len / stride + 1) as u16, stride, 7).unwrap();
            assert_eq!(info.usn, 0x1234);
            assert_eq!(info.sectors as usize, len / stride);
            // Everything except the USA fields and the array itself is restored.
            assert_eq!(&record[..4], &original[..4]);
            assert_eq!(&record[8..0x30], &original[8..0x30]);
            let usa_end = 0x30 + 2 * (len / stride + 1);
            assert_eq!(&record[usa_end..], &original[usa_end..]);
        }
    }

    #[test]
    fn detects_mismatch() {
        let mut record = sample(1024);
        protect(&mut record, 0x30, 512, 0x1234);
        record[1022] ^= 0xFF;
        assert!(matches!(
            apply_fixup(&mut record, 0x30, 3, 512, 9),
            Err(NtfsError::FixupMismatch {
                record: 9,
                sector_index: 1
            })
        ));
    }

    #[test]
    fn rejects_bad_geometry() {
        let mut record = sample(1024);
        protect(&mut record, 0x30, 512, 1);
        assert!(matches!(
            apply_fixup(&mut record.clone(), 0x30, 2, 512, 1),
            Err(NtfsError::InvalidRecord { .. })
        ));
        assert!(matches!(
            apply_fixup(&mut record.clone(), 0x1FC, 3, 512, 1),
            Err(NtfsError::InvalidRecord { .. })
        ));
        assert!(matches!(
            apply_fixup(&mut record.clone(), 0x10, 3, 512, 1),
            Err(NtfsError::InvalidRecord { .. })
        ));
        assert!(matches!(
            apply_fixup(&mut record[..1000], 0x30, 3, 512, 1),
            Err(NtfsError::InvalidRecord { .. })
        ));
    }
}
