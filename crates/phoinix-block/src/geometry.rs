//! Sector geometry of a block source.

use phoinix_core::arith;
use serde::{Deserialize, Serialize};

use crate::BlockError;

/// Smallest logical sector size PHOINIX accepts.
pub const MIN_SECTOR_SIZE: u32 = 128;
/// Largest logical sector size PHOINIX accepts.
pub const MAX_SECTOR_SIZE: u32 = 65_536;

/// Sector geometry of a block source.
///
/// Typical values are 512/4096 (Advanced Format) and 4096/4096 (4Kn).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockGeometry {
    /// Size of one logical sector (the addressing unit) in bytes.
    pub logical_sector_size: u32,
    /// Size of one physical sector in bytes, if known.
    pub physical_sector_size: Option<u32>,
    /// Preferred I/O alignment in bytes, if known.
    pub alignment: Option<u32>,
}

impl BlockGeometry {
    /// Conventional 512-byte logical sectors with unknown physical size.
    pub const SECTOR_512: BlockGeometry = BlockGeometry {
        logical_sector_size: 512,
        physical_sector_size: None,
        alignment: None,
    };

    /// 4096-byte logical and physical sectors (4Kn).
    pub const SECTOR_4K: BlockGeometry = BlockGeometry {
        logical_sector_size: 4096,
        physical_sector_size: Some(4096),
        alignment: Some(4096),
    };

    /// Creates a geometry with the given logical sector size after validating
    /// that it is a power of two within the supported range.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::InvalidGeometry`] for unsupported sizes.
    pub fn new(logical_sector_size: u32) -> Result<Self, BlockError> {
        Self::validate_sector_size(logical_sector_size)?;
        Ok(Self {
            logical_sector_size,
            physical_sector_size: None,
            alignment: None,
        })
    }

    /// Sets the physical sector size.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::InvalidGeometry`] if the size is unsupported or
    /// smaller than the logical sector size.
    pub fn with_physical(mut self, physical_sector_size: u32) -> Result<Self, BlockError> {
        Self::validate_sector_size(physical_sector_size)?;
        if physical_sector_size < self.logical_sector_size {
            return Err(BlockError::InvalidGeometry(format!(
                "physical sector size {physical_sector_size} smaller than logical {}",
                self.logical_sector_size
            )));
        }
        self.physical_sector_size = Some(physical_sector_size);
        Ok(self)
    }

    /// Sets the preferred alignment.
    #[must_use]
    pub const fn with_alignment(mut self, alignment: u32) -> Self {
        self.alignment = Some(alignment);
        self
    }

    fn validate_sector_size(size: u32) -> Result<(), BlockError> {
        if !arith::is_power_of_two(u64::from(size))
            || !(MIN_SECTOR_SIZE..=MAX_SECTOR_SIZE).contains(&size)
        {
            return Err(BlockError::InvalidGeometry(format!(
                "sector size {size} must be a power of two between {MIN_SECTOR_SIZE} and {MAX_SECTOR_SIZE}"
            )));
        }
        Ok(())
    }

    /// Converts a logical block address to a byte offset with overflow
    /// checking.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::IntegerOverflow`] if the product overflows.
    pub fn lba_to_offset(&self, lba: u64) -> Result<u64, BlockError> {
        Ok(arith::mul(lba, u64::from(self.logical_sector_size))?)
    }

    /// Converts a byte offset to the logical block address containing it.
    #[must_use]
    pub const fn offset_to_lba(&self, offset: u64) -> u64 {
        offset / (self.logical_sector_size as u64)
    }

    /// Whether `value` is a multiple of the logical sector size.
    #[must_use]
    pub const fn is_aligned(&self, value: u64) -> bool {
        value % (self.logical_sector_size as u64) == 0
    }
}

impl Default for BlockGeometry {
    fn default() -> Self {
        Self::SECTOR_512
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

    use super::*;

    #[test]
    fn validates_sizes() {
        assert!(BlockGeometry::new(512).is_ok());
        assert!(BlockGeometry::new(4096).is_ok());
        assert!(BlockGeometry::new(513).is_err());
        assert!(BlockGeometry::new(0).is_err());
        assert!(BlockGeometry::new(64).is_err());
        assert!(BlockGeometry::new(131_072).is_err());
        assert!(
            BlockGeometry::new(4096)
                .unwrap()
                .with_physical(512)
                .is_err()
        );
        assert_eq!(
            BlockGeometry::new(512)
                .unwrap()
                .with_physical(4096)
                .unwrap()
                .physical_sector_size,
            Some(4096)
        );
    }

    #[test]
    fn conversions() {
        let g = BlockGeometry::SECTOR_4K;
        assert_eq!(g.lba_to_offset(3).unwrap(), 12_288);
        assert_eq!(g.offset_to_lba(12_289), 3);
        assert!(g.lba_to_offset(u64::MAX).is_err());
        assert!(g.is_aligned(8192));
        assert!(!g.is_aligned(8193));
    }
}
