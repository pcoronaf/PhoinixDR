//! Checked arithmetic for media-derived values.
//!
//! Every calculation of the form `sector × sector_size`, `cluster ×
//! cluster_size`, `offset + length` or `extent_start + extent_length` must go
//! through these helpers (or the standard `checked_*` methods) so that a
//! malformed on-disk value cannot wrap silently.

use thiserror::Error;

/// An arithmetic operation on media-derived values overflowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("integer overflow in media-derived arithmetic")]
pub struct ArithmeticOverflow;

/// Checked `a + b`.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if the sum does not fit in `u64`.
#[inline]
pub const fn add(a: u64, b: u64) -> Result<u64, ArithmeticOverflow> {
    match a.checked_add(b) {
        Some(v) => Ok(v),
        None => Err(ArithmeticOverflow),
    }
}

/// Checked `a × b`.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if the product does not fit in `u64`.
#[inline]
pub const fn mul(a: u64, b: u64) -> Result<u64, ArithmeticOverflow> {
    match a.checked_mul(b) {
        Some(v) => Ok(v),
        None => Err(ArithmeticOverflow),
    }
}

/// Checked `a − b`.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if `b > a`.
#[inline]
pub const fn sub(a: u64, b: u64) -> Result<u64, ArithmeticOverflow> {
    match a.checked_sub(b) {
        Some(v) => Ok(v),
        None => Err(ArithmeticOverflow),
    }
}

/// Checked `a × b + c`, the common "base + index × size" pattern.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if any intermediate result overflows.
#[inline]
pub const fn mul_add(a: u64, b: u64, c: u64) -> Result<u64, ArithmeticOverflow> {
    match mul(a, b) {
        Ok(p) => add(p, c),
        Err(e) => Err(e),
    }
}

/// Checked conversion from `u64` to `usize`.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if the value does not fit in `usize` on
/// this platform.
#[inline]
pub fn to_usize(value: u64) -> Result<usize, ArithmeticOverflow> {
    usize::try_from(value).map_err(|_| ArithmeticOverflow)
}

/// Checked conversion from `usize` to `u64`.
///
/// This cannot fail on any supported platform but is kept explicit so that
/// parser code never contains an `as` cast.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] on platforms where `usize` is wider than
/// 64 bits.
#[inline]
pub fn from_usize(value: usize) -> Result<u64, ArithmeticOverflow> {
    u64::try_from(value).map_err(|_| ArithmeticOverflow)
}

/// Returns `true` if `value` is a power of two (and non-zero).
#[inline]
#[must_use]
pub const fn is_power_of_two(value: u64) -> bool {
    value != 0 && (value & (value - 1)) == 0
}

/// Integer ceiling division `⌈a / b⌉`.
///
/// # Errors
///
/// Returns [`ArithmeticOverflow`] if `b == 0` or the rounded result overflows.
#[inline]
pub const fn div_ceil(a: u64, b: u64) -> Result<u64, ArithmeticOverflow> {
    if b == 0 {
        return Err(ArithmeticOverflow);
    }
    let q = a / b;
    if a % b == 0 { Ok(q) } else { add(q, 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_overflows() {
        assert_eq!(add(u64::MAX, 1), Err(ArithmeticOverflow));
        assert_eq!(add(1, 2), Ok(3));
    }

    #[test]
    fn mul_overflows() {
        assert_eq!(mul(u64::MAX, 2), Err(ArithmeticOverflow));
        assert_eq!(mul(3, 4), Ok(12));
    }

    #[test]
    fn mul_add_checks_both_steps() {
        assert_eq!(mul_add(u64::MAX / 2, 2, 2), Err(ArithmeticOverflow));
        assert_eq!(mul_add(2, 512, 16), Ok(1040));
    }

    #[test]
    fn div_ceil_rounds_up() {
        assert_eq!(div_ceil(0, 4), Ok(0));
        assert_eq!(div_ceil(1, 4), Ok(1));
        assert_eq!(div_ceil(4, 4), Ok(1));
        assert_eq!(div_ceil(5, 4), Ok(2));
        assert_eq!(div_ceil(5, 0), Err(ArithmeticOverflow));
        assert_eq!(div_ceil(u64::MAX, 1), Ok(u64::MAX));
    }

    #[test]
    fn power_of_two() {
        assert!(is_power_of_two(512));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(513));
    }
}
