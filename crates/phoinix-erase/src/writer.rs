//! The range writer and write tolerance.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use phoinix_core::ByteRange;

/// Writes bytes at offsets of a target. The only write path in PhoinixDR.
pub trait RangeWriter: Send {
    /// Writes all of `buf` at `offset`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()>;
    /// Flushes to the device.
    ///
    /// # Errors
    ///
    /// Returns the I/O error.
    fn flush(&mut self) -> io::Result<()>;
    /// Reads `buf.len()` bytes at `offset` (for the read-modify-write of a
    /// partial sector).
    ///
    /// # Errors
    ///
    /// Returns the I/O error.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

/// A writer over a `std::fs::File` (regular file or block device). On
/// Windows it also holds the volume lock handles for the erase's duration.
pub struct FileWriter {
    file: File,
    path: PathBuf,
    /// Handles kept open to hold volume locks (Windows); unused elsewhere.
    #[allow(dead_code)]
    locks: Vec<File>,
}

impl FileWriter {
    /// Wraps an open file.
    #[must_use]
    pub fn new(file: File, path: &Path, locks: Vec<File>) -> Self {
        Self {
            file,
            path: path.to_path_buf(),
            locks,
        }
    }

    /// The path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RangeWriter for FileWriter {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let Some(rest) = buf.get(done..) else { break };
            let n = write_once(&self.file, offset.saturating_add(done as u64), rest)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the device accepted no bytes",
                ));
            }
            done = done.saturating_add(n);
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let Some(rest) = buf.get_mut(done..) else {
                break;
            };
            let n = read_once(&self.file, offset.saturating_add(done as u64), rest)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the device returned no bytes",
                ));
            }
            done = done.saturating_add(n);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn write_once(file: &File, offset: u64, buf: &[u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

#[cfg(unix)]
fn read_once(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(windows)]
fn write_once(file: &File, offset: u64, buf: &[u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(buf, offset)
}

#[cfg(windows)]
fn read_once(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(not(any(unix, windows)))]
fn write_once(_file: &File, _offset: u64, _buf: &[u8]) -> io::Result<usize> {
    Err(io::Error::other(
        "writing is not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn read_once(_file: &File, _offset: u64, _buf: &mut [u8]) -> io::Result<usize> {
    Err(io::Error::other(
        "reading is not supported on this platform",
    ))
}

/// Writes `buf` at `offset` on a device whose writes must be aligned to
/// `sector_size`: the partial head and tail sectors are read, patched and
/// written back; the aligned middle goes through [`write_tolerant`].
///
/// # Errors
///
/// Propagates fatal errors.
pub fn write_aligned(
    writer: &mut dyn RangeWriter,
    offset: u64,
    buf: &[u8],
    sector_size: u64,
    failed: &mut Vec<ByteRange>,
) -> io::Result<()> {
    let ss = sector_size.max(1);
    if ss == 1 || (offset % ss == 0 && buf.len() as u64 % ss == 0) {
        return write_tolerant(writer, offset, buf, failed);
    }
    let end = offset.saturating_add(buf.len() as u64);
    let first_sector = offset / ss * ss;
    let last_sector = end.div_ceil(ss) * ss;
    let mut pos = offset;
    let mut sector = vec![0u8; usize::try_from(ss).unwrap_or(512)];
    // Head: a partial first sector.
    if offset % ss != 0 {
        let head_end = (first_sector + ss).min(end);
        let n = usize::try_from(head_end - offset).unwrap_or(0);
        patch_sector(
            writer,
            first_sector,
            offset,
            buf.get(..n).unwrap_or(&[]),
            &mut sector,
            failed,
        )?;
        pos = head_end;
    }
    // Middle: whole sectors.
    let middle_end = if end % ss == 0 { end } else { last_sector - ss };
    if middle_end > pos {
        let from = usize::try_from(pos - offset).unwrap_or(0);
        let to = usize::try_from(middle_end - offset).unwrap_or(0);
        write_tolerant(writer, pos, buf.get(from..to).unwrap_or(&[]), failed)?;
        pos = middle_end;
    }
    // Tail: a partial last sector.
    if pos < end {
        let from = usize::try_from(pos - offset).unwrap_or(0);
        patch_sector(
            writer,
            last_sector - ss,
            pos,
            buf.get(from..).unwrap_or(&[]),
            &mut sector,
            failed,
        )?;
    }
    Ok(())
}

fn patch_sector(
    writer: &mut dyn RangeWriter,
    sector_offset: u64,
    at: u64,
    data: &[u8],
    sector: &mut [u8],
    failed: &mut Vec<ByteRange>,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if let Err(e) = writer.read_at(sector_offset, sector) {
        if is_fatal(&e) {
            return Err(e);
        }
        tracing::warn!(offset = sector_offset, error = %e, "sector unreadable; its slack is left as is");
        push_failed(failed, at, data.len() as u64);
        return Ok(());
    }
    let within = usize::try_from(at - sector_offset).unwrap_or(0);
    if let Some(slot) = sector.get_mut(within..within + data.len()) {
        slot.copy_from_slice(data);
    }
    write_tolerant(writer, sector_offset, sector, failed)
}

/// Piece size when a chunk write fails.
pub const RETRY_BLOCK: usize = 64 * 1024;
/// Consecutive failing pieces after which the rest of the chunk is written
/// off (every failure can cost a driver timeout).
pub const MAX_CONSECUTIVE_FAILURES: usize = 4;

/// Writes a chunk, tolerating I/O errors: a failed chunk is retried in
/// [`RETRY_BLOCK`] pieces and what still fails is recorded in `failed`
/// (those bytes are *not* erased; the certificate says so). Errors that
/// mean the target is gone (not found, permission denied, write zero)
/// propagate.
///
/// # Errors
///
/// Propagates fatal errors.
pub fn write_tolerant(
    writer: &mut dyn RangeWriter,
    offset: u64,
    buf: &[u8],
    failed: &mut Vec<ByteRange>,
) -> io::Result<()> {
    match writer.write_at(offset, buf) {
        Ok(()) => return Ok(()),
        Err(e) if is_fatal(&e) => return Err(e),
        Err(e) => {
            tracing::warn!(offset, length = buf.len(), error = %e, "write failed; retrying in smaller blocks")
        }
    }
    let mut done = 0usize;
    let mut failures = 0usize;
    while done < buf.len() {
        let step = RETRY_BLOCK.min(buf.len() - done);
        let at = offset.saturating_add(done as u64);
        if failures >= MAX_CONSECUTIVE_FAILURES {
            push_failed(failed, at, (buf.len() - done) as u64);
            tracing::warn!(
                offset = at,
                length = buf.len() - done,
                "unwritable region written off"
            );
            return Ok(());
        }
        let Some(piece) = buf.get(done..done + step) else {
            break;
        };
        match writer.write_at(at, piece) {
            Ok(()) => failures = 0,
            Err(e) if is_fatal(&e) => return Err(e),
            Err(e) => {
                tracing::debug!(offset = at, length = step, error = %e, "unwritable block");
                push_failed(failed, at, step as u64);
                failures += 1;
            }
        }
        done += step;
    }
    Ok(())
}

fn is_fatal(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::WriteZero
    )
}

fn push_failed(failed: &mut Vec<ByteRange>, offset: u64, length: u64) {
    if let Some(last) = failed.last_mut()
        && last.end() == offset
    {
        last.length = last.length.saturating_add(length);
        return;
    }
    failed.push(ByteRange { offset, length });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    /// A writer that fails inside a range.
    struct Flaky {
        inner: FileWriter,
        bad: ByteRange,
    }
    impl RangeWriter for Flaky {
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
            let end = offset + buf.len() as u64;
            if offset < self.bad.end() && end > self.bad.offset {
                return Err(io::Error::other(
                    "The request could not be performed because of an I/O device error.",
                ));
            }
            self.inner.write_at(offset, buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            self.inner.read_at(offset, buf)
        }
    }

    #[test]
    fn aligned_writes_patch_partial_sectors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.img");
        std::fs::write(&path, vec![0xEEu8; 4096]).unwrap();
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut w = FileWriter::new(file, &path, Vec::new());
        let mut failed = Vec::new();
        // 100 bytes at 500: head sector 0 (500..512), middle none, tail
        // sector 1 (512..600).
        write_aligned(&mut w, 500, &[0u8; 100], 512, &mut failed).unwrap();
        // 1100 bytes at 1000: head (1000..1024), middle 1024..2048, tail 2048..2100.
        write_aligned(&mut w, 1000, &[0u8; 1100], 512, &mut failed).unwrap();
        assert!(failed.is_empty());
        let d = std::fs::read(&path).unwrap();
        assert!(d[..500].iter().all(|b| *b == 0xEE));
        assert!(d[500..600].iter().all(|b| *b == 0));
        assert!(d[600..1000].iter().all(|b| *b == 0xEE));
        assert!(d[1000..2100].iter().all(|b| *b == 0));
        assert!(d[2100..].iter().all(|b| *b == 0xEE));
    }

    #[test]
    fn tolerant_writes_record_the_bad_pieces_and_write_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.img");
        std::fs::write(&path, vec![0xEEu8; 512 * 1024]).unwrap();
        let file = File::options().write(true).open(&path).unwrap();
        let bad = ByteRange {
            offset: 200 * 1024,
            length: 10,
        };
        let mut w = Flaky {
            inner: FileWriter::new(file, &path, Vec::new()),
            bad,
        };
        let mut failed = Vec::new();
        write_tolerant(&mut w, 0, &vec![0u8; 512 * 1024], &mut failed).unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].offset, 192 * 1024);
        assert_eq!(failed[0].length, 64 * 1024);
        let data = std::fs::read(&path).unwrap();
        assert!(data[..192 * 1024].iter().all(|b| *b == 0));
        assert!(data[192 * 1024..256 * 1024].iter().all(|b| *b == 0xEE));
        assert!(data[256 * 1024..].iter().all(|b| *b == 0));
    }
}
