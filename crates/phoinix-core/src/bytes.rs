//! Bounds-checked little-endian byte access.
//!
//! Parsers must never index a slice with an offset taken from disk. Every
//! accessor here returns [`None`] when the requested field does not fit in
//! the buffer, and callers translate that into their own typed error.

/// A read-only view over a byte buffer with checked accessors.
#[derive(Debug, Clone, Copy)]
pub struct ByteView<'a> {
    data: &'a [u8],
}

impl<'a> ByteView<'a> {
    /// Wraps a byte slice.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Length of the underlying buffer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The underlying slice.
    #[must_use]
    pub const fn as_slice(&self) -> &'a [u8] {
        self.data
    }

    /// Returns `len` bytes starting at `offset`, if they fit.
    #[must_use]
    pub fn slice(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        let end = offset.checked_add(len)?;
        self.data.get(offset..end)
    }

    /// Returns a fixed-size array copied from `offset`, if it fits.
    #[must_use]
    pub fn array<const N: usize>(&self, offset: usize) -> Option<[u8; N]> {
        let bytes = self.slice(offset, N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Some(out)
    }

    /// Returns a sub-view starting at `offset` with `len` bytes.
    #[must_use]
    pub fn sub(&self, offset: usize, len: usize) -> Option<ByteView<'a>> {
        self.slice(offset, len).map(ByteView::new)
    }

    /// Returns the remainder of the buffer from `offset`.
    #[must_use]
    pub fn from(&self, offset: usize) -> Option<ByteView<'a>> {
        self.data.get(offset..).map(ByteView::new)
    }

    /// Reads a `u8`.
    #[must_use]
    pub fn u8(&self, offset: usize) -> Option<u8> {
        self.data.get(offset).copied()
    }

    /// Reads an `i8`.
    #[must_use]
    pub fn i8(&self, offset: usize) -> Option<i8> {
        self.u8(offset).map(|v| i8::from_le_bytes([v]))
    }

    /// Reads a little-endian `u16`.
    #[must_use]
    pub fn u16_le(&self, offset: usize) -> Option<u16> {
        self.array::<2>(offset).map(u16::from_le_bytes)
    }

    /// Reads a little-endian `u32`.
    #[must_use]
    pub fn u32_le(&self, offset: usize) -> Option<u32> {
        self.array::<4>(offset).map(u32::from_le_bytes)
    }

    /// Reads a little-endian `u64`.
    #[must_use]
    pub fn u64_le(&self, offset: usize) -> Option<u64> {
        self.array::<8>(offset).map(u64::from_le_bytes)
    }

    /// Reads a big-endian `u16`.
    #[must_use]
    pub fn u16_be(&self, offset: usize) -> Option<u16> {
        self.array::<2>(offset).map(u16::from_be_bytes)
    }

    /// Reads a big-endian `u32`.
    #[must_use]
    pub fn u32_be(&self, offset: usize) -> Option<u32> {
        self.array::<4>(offset).map(u32::from_be_bytes)
    }

    /// Reads a little-endian `i64`.
    #[must_use]
    pub fn i64_le(&self, offset: usize) -> Option<i64> {
        self.array::<8>(offset).map(i64::from_le_bytes)
    }

    /// Reads a little-endian unsigned integer of `width` bytes (0..=8).
    ///
    /// A width of zero yields zero.
    #[must_use]
    pub fn uint_le(&self, offset: usize, width: usize) -> Option<u64> {
        if width > 8 {
            return None;
        }
        let bytes = self.slice(offset, width)?;
        let mut value: u64 = 0;
        for (i, b) in bytes.iter().enumerate() {
            value |= u64::from(*b) << (8 * i);
        }
        Some(value)
    }

    /// Reads a little-endian *signed* integer of `width` bytes (0..=8),
    /// sign-extending from the most significant byte read.
    ///
    /// A width of zero yields zero.
    #[must_use]
    pub fn int_le(&self, offset: usize, width: usize) -> Option<i64> {
        if width > 8 {
            return None;
        }
        let bytes = self.slice(offset, width)?;
        if width == 0 {
            return Some(0);
        }
        let mut value: u64 = 0;
        for (i, b) in bytes.iter().enumerate() {
            value |= u64::from(*b) << (8 * i);
        }
        let shift = 64 - 8 * width;
        // Shift left then arithmetic-shift right to sign-extend.
        let widened = i64::from_le_bytes((value << shift).to_le_bytes());
        Some(widened >> shift)
    }
}

/// Decodes UTF-16LE bytes into a `String`.
///
/// Returns [`None`] if the byte length is odd or the data is not valid UTF-16
/// (for example an unpaired surrogate).
#[must_use]
pub fn utf16le_to_string(bytes: &[u8]) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            u16::from_le_bytes([
                c.first().copied().unwrap_or(0),
                c.get(1).copied().unwrap_or(0),
            ])
        })
        .collect();
    String::from_utf16(&units).ok()
}

/// Decodes UTF-16LE bytes into a `String`, replacing invalid sequences with
/// U+FFFD. An odd trailing byte is ignored.
#[must_use]
pub fn utf16le_to_string_lossy(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            u16::from_le_bytes([
                c.first().copied().unwrap_or(0),
                c.get(1).copied().unwrap_or(0),
            ])
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decodes a NUL-padded ASCII/Latin-1 field (such as an OEM ID) into a
/// `String`, mapping bytes outside printable ASCII to `?`.
#[must_use]
pub fn ascii_field(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b)
            } else {
                '?'
            }
        })
        .collect()
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
    fn checked_reads() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09];
        let v = ByteView::new(&data);
        assert_eq!(v.u8(0), Some(1));
        assert_eq!(v.u16_le(0), Some(0x0201));
        assert_eq!(v.u32_le(1), Some(0x0504_0302));
        assert_eq!(v.u64_le(1), Some(0x0908_0706_0504_0302));
        assert_eq!(v.u64_le(2), None);
        assert_eq!(v.u8(9), None);
        assert_eq!(v.slice(8, 1).map(<[u8]>::len), Some(1));
        assert_eq!(v.slice(8, 2), None);
        assert_eq!(v.slice(usize::MAX, 1), None);
    }

    #[test]
    fn variable_width_integers() {
        let data = [0xFF, 0xFF, 0x7F, 0x80];
        let v = ByteView::new(&data);
        assert_eq!(v.uint_le(0, 0), Some(0));
        assert_eq!(v.uint_le(0, 1), Some(0xFF));
        assert_eq!(v.uint_le(0, 3), Some(0x7F_FFFF));
        assert_eq!(v.int_le(0, 1), Some(-1));
        assert_eq!(v.int_le(0, 2), Some(-1));
        assert_eq!(v.int_le(0, 3), Some(0x7F_FFFF));
        assert_eq!(v.int_le(3, 1), Some(-128));
        assert_eq!(
            v.int_le(0, 4),
            Some(i32::from_le_bytes([0xFF, 0xFF, 0x7F, 0x80]).into())
        );
        assert_eq!(v.int_le(0, 9), None);
        assert_eq!(v.int_le(0, 5), None);
    }

    #[test]
    fn utf16() {
        let bytes = [b'h', 0, b'i', 0];
        assert_eq!(utf16le_to_string(&bytes).unwrap(), "hi");
        assert_eq!(utf16le_to_string(&bytes[..3]), None);
        let bad = [0x00, 0xD8, b'x', 0];
        assert_eq!(utf16le_to_string(&bad), None);
        assert_eq!(utf16le_to_string_lossy(&bad), "\u{FFFD}x");
    }

    #[test]
    fn ascii() {
        assert_eq!(ascii_field(b"NTFS    "), "NTFS    ");
        assert_eq!(ascii_field(b"AB\0CD"), "AB");
        assert_eq!(ascii_field(&[0x41, 0xFF]), "A?");
    }
}
