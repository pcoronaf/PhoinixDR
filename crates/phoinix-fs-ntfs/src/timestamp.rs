//! NTFS timestamps: 100-nanosecond intervals since 1601-01-01 UTC.
//!
//! The raw integer is always retained; conversion to calendar time is a
//! presentation concern.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Offset between the NTFS epoch (1601-01-01) and the Unix epoch, in
/// 100-nanosecond units.
const EPOCH_DIFFERENCE_100NS: i128 = 116_444_736_000_000_000;

/// An NTFS timestamp.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct NtfsTimestamp {
    /// Raw on-disk value.
    pub raw: u64,
}

impl NtfsTimestamp {
    /// Wraps a raw value.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self { raw }
    }

    /// Whether the timestamp is zero (unset).
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.raw == 0
    }

    /// Nanoseconds since the Unix epoch (may be negative before 1970).
    #[must_use]
    pub fn unix_nanos(&self) -> i128 {
        (i128::from(self.raw) - EPOCH_DIFFERENCE_100NS) * 100
    }

    /// Whole seconds since the Unix epoch.
    #[must_use]
    pub fn unix_seconds(&self) -> i64 {
        let nanos = self.unix_nanos();
        i64::try_from(nanos.div_euclid(1_000_000_000)).unwrap_or(i64::MAX)
    }

    /// Formats as ISO-8601 UTC with microsecond precision, or `-` when zero.
    #[must_use]
    pub fn to_iso8601(&self) -> String {
        if self.is_zero() {
            return "-".to_owned();
        }
        let secs = self.unix_seconds();
        let sub_100ns = self.raw % 10_000_000;
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}Z",
            sod / 3600,
            (sod % 3600) / 60,
            sod % 60,
            sub_100ns / 10
        )
    }
}

impl fmt::Display for NtfsTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_iso8601())
    }
}

/// Howard Hinnant's algorithm: days since 1970-01-01 to (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_conversions() {
        // 1970-01-01T00:00:00Z
        let t = NtfsTimestamp::new(116_444_736_000_000_000);
        assert_eq!(t.unix_seconds(), 0);
        assert_eq!(t.to_iso8601(), "1970-01-01T00:00:00.000000Z");
        // 2026-09-04T12:34:56.5Z = 1788525296.5 s since Unix epoch
        let raw = 116_444_736_000_000_000 + 1_788_525_296 * 10_000_000 + 5_000_000;
        let t = NtfsTimestamp::new(raw);
        assert_eq!(t.to_iso8601(), "2026-09-04T12:34:56.500000Z");
        assert_eq!(NtfsTimestamp::default().to_iso8601(), "-");
        // Before 1970.
        let t = NtfsTimestamp::new(116_444_736_000_000_000 - 86_400 * 10_000_000);
        assert_eq!(t.to_iso8601(), "1969-12-31T00:00:00.000000Z");
    }
}
