//! Small formatting helpers shared by user-facing front-ends.

/// Formats a byte count using decimal SI units (`1.0 TB`, `260 MB`) the way
/// storage vendors label devices.
#[must_use]
pub fn bytes_si(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("?");
    if value >= 100.0 {
        format!("{value:.0} {name}")
    } else {
        format!("{value:.1} {name}")
    }
}

/// Formats a byte count using binary units (`4.0 KiB`, `1.5 MiB`).
#[must_use]
pub fn bytes_iec(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("?");
    if value >= 100.0 {
        format!("{value:.0} {name}")
    } else {
        format!("{value:.1} {name}")
    }
}

/// Formats an integer with thousands separators (`1,234,567`).
#[must_use]
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Formats seconds since the Unix epoch plus microseconds as ISO-8601 UTC.
#[must_use]
pub fn iso8601_utc(unix_seconds: i64, micros: u32) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let sod = unix_seconds.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        micros % 1_000_000
    )
}

/// Days since 1970-01-01 for a civil date (inverse of the civil-from-days
/// conversion used by [`iso8601_utc`]).
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Converts a DOS date/time pair (as used by FAT and exFAT) into seconds
/// since the Unix epoch, treating the value as UTC. `tenths` adds 10 ms
/// units (0–199) as on-disk. Returns `None` for an all-zero or invalid date.
#[must_use]
pub fn dos_datetime_to_unix(date: u16, time: u16, tenths: u8) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let year = 1980 + i64::from(date >> 9);
    let month = u32::from((date >> 5) & 0x0F);
    let day = u32::from(date & 0x1F);
    let hour = i64::from(time >> 11);
    let minute = i64::from((time >> 5) & 0x3F);
    let second = i64::from((time & 0x1F) * 2);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second + i64::from(tenths / 100))
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

/// Renders bytes as a classic hex dump (offset, 16 hex bytes, ASCII).
#[must_use]
pub fn hex_dump(base_offset: u64, data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = base_offset.saturating_add((i * 16) as u64);
        out.push_str(&format!("{offset:08x}  "));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            out.push(if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b)
            } else {
                '.'
            });
        }
        out.push_str("|\n");
    }
    out
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
    use super::*;

    #[test]
    fn si_units() {
        assert_eq!(bytes_si(0), "0 B");
        assert_eq!(bytes_si(999), "999 B");
        assert_eq!(bytes_si(1000), "1.0 kB");
        assert_eq!(bytes_si(1_000_204_886_016), "1.0 TB");
        assert_eq!(bytes_si(272_629_760), "273 MB");
    }

    #[test]
    fn iec_units() {
        assert_eq!(bytes_iec(1024), "1.0 KiB");
        assert_eq!(bytes_iec(1536), "1.5 KiB");
        assert_eq!(bytes_iec(64 * 1024 * 1024), "64.0 MiB");
    }

    #[test]
    fn grouping() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn iso8601() {
        assert_eq!(iso8601_utc(0, 0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            iso8601_utc(1_788_525_296, 500_000),
            "2026-09-04T12:34:56.500000Z"
        );
        assert_eq!(iso8601_utc(-86_400, 0), "1969-12-31T00:00:00.000000Z");
    }

    #[test]
    fn dos_datetime() {
        // 2026-09-05 01:38:00 → date: (46<<9)|(9<<5)|5, time: (1<<11)|(38<<5)|0
        let date = (46 << 9) | (9 << 5) | 5;
        let time = (1 << 11) | (38 << 5);
        let secs = dos_datetime_to_unix(date, time, 0).unwrap();
        assert_eq!(iso8601_utc(secs, 0), "2026-09-05T01:38:00.000000Z");
        assert_eq!(dos_datetime_to_unix(0, 0, 0), None);
        assert_eq!(dos_datetime_to_unix((46 << 9) | (13 << 5) | 5, 0, 0), None);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn dump_layout() {
        let d = hex_dump(0, b"NTFS    \x00\x02");
        assert!(d.starts_with("00000000  4e 54 46 53 20 20 20 20  00 02"));
        assert!(d.ends_with("|NTFS    ..|\n"));
    }
}
