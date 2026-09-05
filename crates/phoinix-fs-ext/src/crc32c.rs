//! CRC-32C (Castagnoli) as ext4 and jbd2 use it: a raw register update
//! without pre- or post-inversion, seeded by the caller.

#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0x82F6_3B78 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const TABLE: [u32; 256] = make_table();

/// Updates `crc` with `data` (raw register, as the kernel's `crc32c`).
#[must_use]
pub fn update(mut crc: u32, data: &[u8]) -> u32 {
    for b in data {
        let idx = ((crc ^ u32::from(*b)) & 0xFF) as usize;
        crc = TABLE.get(idx).copied().unwrap_or(0) ^ (crc >> 8);
    }
    crc
}

/// The standard CRC-32C of `data` (pre- and post-inverted), for tests.
#[must_use]
pub fn checksum(data: &[u8]) -> u32 {
    !update(!0, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_value() {
        assert_eq!(checksum(b"123456789"), 0xE306_9283);
        assert_eq!(checksum(b""), 0);
    }
}
