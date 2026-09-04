//! Logical data streams over resident bytes or cluster runs.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::arith;

use crate::NtfsError;
use crate::runlist::NtfsRun;

#[derive(Debug, Clone)]
enum Backing {
    Resident(Arc<Vec<u8>>),
    Runs {
        reader: Arc<dyn BlockReader>,
        cluster_size: u64,
        runs: Arc<Vec<NtfsRun>>,
        initialized_size: u64,
    },
}

/// A stream mapping file offsets to volume bytes.
///
/// ```text
/// file byte offset → VCN → run → LCN → volume byte offset
/// ```
///
/// Sparse runs and bytes beyond the initialised size read as zero. Bytes
/// beyond the logical size are never exposed, so cluster padding in the
/// final run is not file content. A VCN with no run is an error
/// ([`NtfsError::MissingExtent`]) rather than silently zero-filled.
#[derive(Debug, Clone)]
pub struct NtfsDataStream {
    backing: Backing,
    len: u64,
}

impl NtfsDataStream {
    /// A stream over resident bytes.
    #[must_use]
    pub fn resident(value: Vec<u8>) -> Self {
        let len = value.len() as u64;
        Self {
            backing: Backing::Resident(Arc::new(value)),
            len,
        }
    }

    /// A stream over cluster runs.
    ///
    /// Runs are sorted by VCN. `len` is the logical size and
    /// `initialized_size` the number of bytes actually written.
    #[must_use]
    pub fn non_resident(
        reader: Arc<dyn BlockReader>,
        cluster_size: u32,
        mut runs: Vec<NtfsRun>,
        len: u64,
        initialized_size: u64,
    ) -> Self {
        runs.sort_by_key(NtfsRun::vcn);
        Self {
            backing: Backing::Runs {
                reader,
                cluster_size: u64::from(cluster_size.max(1)),
                runs: Arc::new(runs),
                initialized_size: initialized_size.min(len),
            },
            len,
        }
    }

    /// Logical length in bytes.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Whether the stream is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the stream is resident.
    #[must_use]
    pub const fn is_resident(&self) -> bool {
        matches!(self.backing, Backing::Resident(_))
    }

    /// The volume reader behind a non-resident stream.
    #[must_use]
    pub fn volume_reader(&self) -> Option<Arc<dyn BlockReader>> {
        match &self.backing {
            Backing::Resident(_) => None,
            Backing::Runs { reader, .. } => Some(reader.clone()),
        }
    }

    /// The runs backing a non-resident stream (empty when resident).
    #[must_use]
    pub fn runs(&self) -> &[NtfsRun] {
        match &self.backing {
            Backing::Resident(_) => &[],
            Backing::Runs { runs, .. } => runs,
        }
    }

    /// Reads from `offset`, returning the number of bytes produced; zero at
    /// or beyond the end of the stream.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::MissingExtent`] for gaps in the runlist and
    /// propagates block errors.
    pub fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, NtfsError> {
        if offset >= self.len || buffer.is_empty() {
            return Ok(0);
        }
        let want = usize::try_from((self.len - offset).min(buffer.len() as u64))
            .map_err(|_| NtfsError::Overflow)?;
        let buffer = buffer.get_mut(..want).ok_or(NtfsError::Overflow)?;
        match &self.backing {
            Backing::Resident(value) => {
                let start = usize::try_from(offset).map_err(|_| NtfsError::Overflow)?;
                let src = value.get(start..start + want).ok_or(NtfsError::Overflow)?;
                buffer.copy_from_slice(src);
                Ok(want)
            }
            Backing::Runs {
                reader,
                cluster_size,
                runs,
                initialized_size,
            } => {
                let mut done = 0usize;
                while done < want {
                    let pos = arith::add(offset, arith::from_usize(done)?)?;
                    let remaining = want - done;
                    let vcn = pos / cluster_size;
                    let run = find_run(runs, vcn).ok_or(NtfsError::MissingExtent { vcn })?;
                    let run_start_byte = arith::mul(run.vcn(), *cluster_size)?;
                    let run_len_bytes = arith::mul(run.clusters(), *cluster_size)?;
                    let within = pos - run_start_byte;
                    let chunk = usize::try_from((run_len_bytes - within).min(remaining as u64))
                        .map_err(|_| NtfsError::Overflow)?;
                    let dst = buffer
                        .get_mut(done..done + chunk)
                        .ok_or(NtfsError::Overflow)?;
                    match run {
                        NtfsRun::Sparse { .. } => dst.fill(0),
                        NtfsRun::Data { lcn, .. } => {
                            if pos >= *initialized_size {
                                dst.fill(0);
                            } else {
                                // Read only the initialised part; zero the rest.
                                let readable =
                                    usize::try_from((*initialized_size - pos).min(chunk as u64))
                                        .map_err(|_| NtfsError::Overflow)?;
                                let physical =
                                    arith::add(arith::mul(*lcn, *cluster_size)?, within)?;
                                let (head, tail) = dst.split_at_mut(readable);
                                reader.read_exact_at(physical, head)?;
                                tail.fill(0);
                            }
                        }
                    }
                    done += chunk;
                }
                Ok(done)
            }
        }
    }

    /// Fills `buffer` completely from `offset`.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::Block`] with a short-read error if the stream
    /// ends first, or any error from [`read_at`](Self::read_at).
    pub fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), NtfsError> {
        let n = self.read_at(offset, buffer)?;
        if n != buffer.len() {
            return Err(NtfsError::Block(phoinix_block::BlockError::ShortRead {
                expected: buffer.len(),
                actual: n,
            }));
        }
        Ok(())
    }

    /// Reads the whole stream into memory, refusing streams larger than
    /// `limit` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::Unsupported`] if the stream exceeds `limit`, or
    /// any read error.
    pub fn read_all(&self, limit: u64) -> Result<Vec<u8>, NtfsError> {
        if self.len > limit {
            return Err(NtfsError::Unsupported(format!(
                "stream of {} bytes exceeds the {limit}-byte in-memory limit",
                self.len
            )));
        }
        let mut out = vec![0u8; usize::try_from(self.len).map_err(|_| NtfsError::Overflow)?];
        self.read_exact_at(0, &mut out)?;
        Ok(out)
    }

    /// A seekable [`Read`] cursor over the stream.
    #[must_use]
    pub fn cursor(&self) -> StreamCursor {
        StreamCursor {
            stream: self.clone(),
            pos: 0,
        }
    }
}

fn find_run(runs: &[NtfsRun], vcn: u64) -> Option<&NtfsRun> {
    let idx = runs.partition_point(|r| r.vcn() <= vcn);
    let run = runs.get(idx.checked_sub(1)?)?;
    (vcn < run.end_vcn()).then_some(run)
}

/// `Read + Seek` adapter over an [`NtfsDataStream`].
#[derive(Debug, Clone)]
pub struct StreamCursor {
    stream: NtfsDataStream,
    pos: u64,
}

impl StreamCursor {
    /// The underlying stream.
    #[must_use]
    pub const fn stream(&self) -> &NtfsDataStream {
        &self.stream
    }

    /// Current position.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.pos
    }
}

impl Read for StreamCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self
            .stream
            .read_at(self.pos, buf)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.pos = self.pos.saturating_add(n as u64);
        Ok(n)
    }
}

impl Seek for StreamCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.stream.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let new =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        self.pos = new;
        Ok(new)
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
    use phoinix_block::MemoryReader;

    fn volume() -> (Arc<dyn BlockReader>, Vec<u8>) {
        let data: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 253) as u8).collect();
        (Arc::new(MemoryReader::new(data.clone())), data)
    }

    #[test]
    fn resident_stream() {
        let s = NtfsDataStream::resident(b"hello world".to_vec());
        let mut buf = [0u8; 5];
        assert_eq!(s.read_at(6, &mut buf).unwrap(), 5);
        assert_eq!(&buf, b"world");
        assert_eq!(s.read_at(11, &mut buf).unwrap(), 0);
        assert_eq!(s.read_all(1024).unwrap(), b"hello world");
        assert!(s.read_all(4).is_err());
    }

    #[test]
    fn fragmented_stream_follows_logical_order() {
        let (reader, data) = volume();
        // cluster 1024 bytes: VCN 0..2 at LCN 10, VCN 2..3 at LCN 3 (backwards), sparse VCN 3..4, VCN 4..5 at LCN 20
        let runs = vec![
            NtfsRun::Data {
                vcn: 0,
                lcn: 10,
                clusters: 2,
            },
            NtfsRun::Data {
                vcn: 2,
                lcn: 3,
                clusters: 1,
            },
            NtfsRun::Sparse {
                vcn: 3,
                clusters: 1,
            },
            NtfsRun::Data {
                vcn: 4,
                lcn: 20,
                clusters: 1,
            },
        ];
        let s = NtfsDataStream::non_resident(reader, 1024, runs, 4600, 4600);
        let all = s.read_all(1 << 20).unwrap();
        assert_eq!(all.len(), 4600);
        assert_eq!(&all[..2048], &data[10 * 1024..12 * 1024]);
        assert_eq!(&all[2048..3072], &data[3 * 1024..4 * 1024]);
        assert!(all[3072..4096].iter().all(|b| *b == 0));
        assert_eq!(&all[4096..4600], &data[20 * 1024..20 * 1024 + 504]);
        // Reads that straddle run boundaries.
        let mut buf = [0u8; 100];
        assert_eq!(s.read_at(2000, &mut buf).unwrap(), 100);
        assert_eq!(&buf[..48], &data[10 * 1024 + 2000..12 * 1024]);
        assert_eq!(&buf[48..], &data[3 * 1024..3 * 1024 + 52]);
        // Padding beyond the logical size is never exposed.
        assert_eq!(s.read_at(4599, &mut buf).unwrap(), 1);
    }

    #[test]
    fn initialized_size_and_gaps() {
        let (reader, data) = volume();
        let runs = vec![
            NtfsRun::Data {
                vcn: 0,
                lcn: 5,
                clusters: 2,
            },
            NtfsRun::Data {
                vcn: 3,
                lcn: 9,
                clusters: 1,
            },
        ];
        let s = NtfsDataStream::non_resident(reader, 1024, runs, 4096, 1500);
        let mut buf = vec![0u8; 2048];
        s.read_exact_at(0, &mut buf).unwrap();
        assert_eq!(&buf[..1500], &data[5 * 1024..5 * 1024 + 1500]);
        assert!(buf[1500..].iter().all(|b| *b == 0));
        assert!(matches!(
            s.read_at(2048, &mut buf),
            Err(NtfsError::MissingExtent { vcn: 2 })
        ));
    }

    #[test]
    fn cursor_read_and_seek() {
        let s = NtfsDataStream::resident((0..=255u8).collect());
        let mut c = s.cursor();
        c.seek(SeekFrom::End(-4)).unwrap();
        let mut buf = Vec::new();
        c.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, vec![252, 253, 254, 255]);
        assert_eq!(c.seek(SeekFrom::Start(10)).unwrap(), 10);
        let mut two = [0u8; 2];
        c.read_exact(&mut two).unwrap();
        assert_eq!(two, [10, 11]);
        assert!(c.seek(SeekFrom::Current(-100)).is_err());
    }
}
