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
    fn dump_layout() {
        let d = hex_dump(0, b"NTFS    \x00\x02");
        assert!(d.starts_with("00000000  4e 54 46 53 20 20 20 20  00 02"));
        assert!(d.ends_with("|NTFS    ..|\n"));
    }
}
