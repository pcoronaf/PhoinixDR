//! Validated byte and sector ranges.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::arith::{self, ArithmeticOverflow};

/// A range could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RangeError {
    /// `offset + length` overflowed.
    #[error("range end overflows: offset={offset}, length={length}")]
    Overflow {
        /// Requested start.
        offset: u64,
        /// Requested length.
        length: u64,
    },
    /// The range does not fit inside its container.
    #[error("range [{offset}, +{length}) exceeds bound {bound}")]
    OutOfBounds {
        /// Requested start.
        offset: u64,
        /// Requested length.
        length: u64,
        /// Exclusive upper bound of the container.
        bound: u64,
    },
}

impl From<ArithmeticOverflow> for RangeError {
    fn from(_: ArithmeticOverflow) -> Self {
        RangeError::Overflow {
            offset: u64::MAX,
            length: u64::MAX,
        }
    }
}

/// A half-open byte range `[offset, offset + length)` whose end is guaranteed
/// not to overflow `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    /// First byte of the range.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

impl ByteRange {
    /// Constructs a range, validating that `offset + length` fits in `u64`.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] if the end overflows.
    pub const fn new(offset: u64, length: u64) -> Result<Self, RangeError> {
        match offset.checked_add(length) {
            Some(_) => Ok(Self { offset, length }),
            None => Err(RangeError::Overflow { offset, length }),
        }
    }

    /// Constructs a range and additionally checks that it lies within
    /// `[0, bound)`.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] or [`RangeError::OutOfBounds`].
    pub const fn bounded(offset: u64, length: u64, bound: u64) -> Result<Self, RangeError> {
        let range = match Self::new(offset, length) {
            Ok(r) => r,
            Err(e) => return Err(e),
        };
        if range.end() > bound {
            return Err(RangeError::OutOfBounds {
                offset,
                length,
                bound,
            });
        }
        Ok(range)
    }

    /// Exclusive end of the range.
    #[must_use]
    pub const fn end(&self) -> u64 {
        // Cannot overflow: checked in the constructor.
        self.offset.saturating_add(self.length)
    }

    /// Whether the range is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Whether `position` lies inside the range.
    #[must_use]
    pub const fn contains(&self, position: u64) -> bool {
        position >= self.offset && position < self.end()
    }

    /// Whether `other` lies entirely inside `self`.
    #[must_use]
    pub const fn contains_range(&self, other: &ByteRange) -> bool {
        other.offset >= self.offset && other.end() <= self.end()
    }

    /// Whether the two non-empty ranges share at least one byte.
    #[must_use]
    pub const fn overlaps(&self, other: &ByteRange) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.offset < other.end()
            && other.offset < self.end()
    }

    /// Translates this range by `base`, e.g. to map a partition-relative
    /// range onto the parent disk.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] if the translated end overflows.
    pub fn translate(&self, base: u64) -> Result<Self, RangeError> {
        let offset = arith::add(self.offset, base).map_err(|_| RangeError::Overflow {
            offset: self.offset,
            length: self.length,
        })?;
        Self::new(offset, self.length)
    }
}

/// A range of logical sectors `[start, start + sectors)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LbaRange {
    /// First logical block address.
    pub start: u64,
    /// Number of sectors.
    pub sectors: u64,
}

impl LbaRange {
    /// Constructs a range, validating that `start + sectors` fits in `u64`.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] if the end overflows.
    pub const fn new(start: u64, sectors: u64) -> Result<Self, RangeError> {
        match start.checked_add(sectors) {
            Some(_) => Ok(Self { start, sectors }),
            None => Err(RangeError::Overflow {
                offset: start,
                length: sectors,
            }),
        }
    }

    /// Constructs a range from an inclusive first and last LBA.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] if `last < first`.
    pub const fn from_inclusive(first: u64, last: u64) -> Result<Self, RangeError> {
        if last < first {
            return Err(RangeError::Overflow {
                offset: first,
                length: 0,
            });
        }
        // last - first + 1 cannot overflow because last >= first and last <= u64::MAX
        // only when first == 0 gives u64::MAX + 1: guard that case.
        match (last - first).checked_add(1) {
            Some(sectors) => Self::new(first, sectors),
            None => Err(RangeError::Overflow {
                offset: first,
                length: u64::MAX,
            }),
        }
    }

    /// Exclusive end LBA.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.sectors)
    }

    /// Converts to a byte range using `sector_size` bytes per sector.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Overflow`] if the byte offset or length overflow.
    pub fn to_byte_range(&self, sector_size: u32) -> Result<ByteRange, RangeError> {
        let size = u64::from(sector_size);
        let offset = arith::mul(self.start, size).map_err(|_| RangeError::Overflow {
            offset: self.start,
            length: self.sectors,
        })?;
        let length = arith::mul(self.sectors, size).map_err(|_| RangeError::Overflow {
            offset: self.start,
            length: self.sectors,
        })?;
        ByteRange::new(offset, length)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn byte_range_validation() {
        assert!(ByteRange::new(u64::MAX, 1).is_err());
        assert!(ByteRange::new(u64::MAX, 0).is_ok());
        let r = ByteRange::new(10, 5).unwrap();
        assert_eq!(r.end(), 15);
        assert!(r.contains(10));
        assert!(r.contains(14));
        assert!(!r.contains(15));
        assert!(ByteRange::bounded(10, 5, 15).is_ok());
        assert!(matches!(
            ByteRange::bounded(10, 6, 15),
            Err(RangeError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn overlap_semantics() {
        let a = ByteRange::new(0, 10).unwrap();
        let b = ByteRange::new(10, 10).unwrap();
        let c = ByteRange::new(5, 10).unwrap();
        let empty = ByteRange::new(5, 0).unwrap();
        assert!(!a.overlaps(&b));
        assert!(a.overlaps(&c));
        assert!(c.overlaps(&b));
        assert!(!a.overlaps(&empty));
        assert!(a.contains_range(&ByteRange::new(2, 3).unwrap()));
        assert!(!a.contains_range(&c));
    }

    #[test]
    fn translate_checks_overflow() {
        let r = ByteRange::new(10, 5).unwrap();
        assert_eq!(r.translate(100).unwrap(), ByteRange::new(110, 5).unwrap());
        assert!(r.translate(u64::MAX).is_err());
    }

    #[test]
    fn lba_conversion() {
        let l = LbaRange::from_inclusive(2048, 4095).unwrap();
        assert_eq!(l.sectors, 2048);
        let b = l.to_byte_range(512).unwrap();
        assert_eq!(b.offset, 1_048_576);
        assert_eq!(b.length, 1_048_576);
        assert!(LbaRange::from_inclusive(5, 4).is_err());
        assert!(LbaRange::from_inclusive(0, u64::MAX).is_err());
        assert!(LbaRange::new(u64::MAX, 1).is_err());
        assert!(
            LbaRange::new(u64::MAX / 2, 1)
                .unwrap()
                .to_byte_range(4096)
                .is_err()
        );
    }
}
