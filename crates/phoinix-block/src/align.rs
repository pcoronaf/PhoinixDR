//! Helper for sources that require sector-aligned I/O.
//!
//! Windows physical drives (and `O_DIRECT` handles) reject reads whose offset
//! or length is not a multiple of the logical sector size. This helper turns
//! an arbitrary request into an aligned superset read plus a copy.

use phoinix_core::arith;

use crate::BlockError;

/// Performs an unaligned read on top of an aligned read primitive.
///
/// `aligned_read(offset, buffer)` is called with a sector-aligned offset and a
/// buffer whose length is a multiple of `sector_size`; it returns the number
/// of bytes it produced. The requested bytes are copied into `buffer`.
///
/// If the request is already aligned it is forwarded without copying.
///
/// # Errors
///
/// Propagates errors from `aligned_read`; returns
/// [`BlockError::IntegerOverflow`] if the aligned range overflows.
pub fn read_via_aligned<F>(
    sector_size: u32,
    offset: u64,
    buffer: &mut [u8],
    mut aligned_read: F,
) -> Result<usize, BlockError>
where
    F: FnMut(u64, &mut [u8]) -> Result<usize, BlockError>,
{
    let sector = u64::from(sector_size.max(1));
    let len = arith::from_usize(buffer.len())?;
    if buffer.is_empty() {
        return Ok(0);
    }
    if offset % sector == 0 && len % sector == 0 {
        return aligned_read(offset, buffer);
    }

    let start = offset - offset % sector;
    let end = arith::add(offset, len)?;
    let end_aligned = arith::mul(arith::div_ceil(end, sector)?, sector)?;
    let span = arith::to_usize(arith::sub(end_aligned, start)?)?;
    let mut scratch = vec![0u8; span];
    let produced = aligned_read(start, &mut scratch)?;

    let head = arith::to_usize(offset - start)?;
    // Bytes of the request actually covered by what the aligned read produced.
    let available = produced.saturating_sub(head).min(buffer.len());
    let src = scratch
        .get(head..head + available)
        .ok_or(BlockError::IntegerOverflow)?;
    let dst = buffer
        .get_mut(..available)
        .ok_or(BlockError::IntegerOverflow)?;
    dst.copy_from_slice(src);
    Ok(available)
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

    fn backing() -> Vec<u8> {
        (0..4096u32).map(|i| (i % 251) as u8).collect()
    }

    fn aligned_source<'a>(
        data: &'a [u8],
    ) -> impl FnMut(u64, &mut [u8]) -> Result<usize, BlockError> + 'a {
        move |offset, buf| {
            assert_eq!(offset % 512, 0, "offset must be aligned");
            assert_eq!(buf.len() % 512, 0, "length must be aligned");
            let start = offset as usize;
            let end = (start + buf.len()).min(data.len());
            let n = end.saturating_sub(start);
            buf[..n].copy_from_slice(&data[start..end]);
            Ok(n)
        }
    }

    #[test]
    fn unaligned_request_is_translated() {
        let data = backing();
        let mut out = vec![0u8; 700];
        let n = read_via_aligned(512, 300, &mut out, aligned_source(&data)).unwrap();
        assert_eq!(n, 700);
        assert_eq!(&out[..], &data[300..1000]);
    }

    #[test]
    fn aligned_request_passes_through() {
        let data = backing();
        let mut out = vec![0u8; 1024];
        let n = read_via_aligned(512, 512, &mut out, aligned_source(&data)).unwrap();
        assert_eq!(n, 1024);
        assert_eq!(&out[..], &data[512..1536]);
    }

    #[test]
    fn short_aligned_read_is_reported() {
        let data = backing();
        let mut out = vec![0u8; 100];
        // The aligned read covers 3584..4096 fully, so this is complete.
        let n = read_via_aligned(512, 4000, &mut out, aligned_source(&data)).unwrap();
        assert_eq!(n, 96);
    }

    #[test]
    fn empty_request() {
        let data = backing();
        let mut out = [];
        assert_eq!(
            read_via_aligned(512, 3, &mut out, aligned_source(&data)).unwrap(),
            0
        );
    }
}
