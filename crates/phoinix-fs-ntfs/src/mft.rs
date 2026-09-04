//! `$MFT` access: bootstrap and record reading.

use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::arith;

use crate::NtfsError;
use crate::attribute::AttributeType;
use crate::attribute_list::parse_attribute_list;
use crate::boot::NtfsBootSector;
use crate::record::FileRecord;
use crate::runlist::{NtfsRun, decode_runlist};
use crate::stream::NtfsDataStream;

/// Record number of `$MFT`.
pub const MFT_RECORD: u64 = 0;
/// Record number of `$MFTMirr`.
pub const MFT_MIRROR_RECORD: u64 = 1;
/// Record number of `$Volume`.
pub const VOLUME_RECORD: u64 = 3;
/// Record number of the root directory.
pub const ROOT_RECORD: u64 = 5;
/// Record number of `$Bitmap`.
pub const BITMAP_RECORD: u64 = 6;
/// Upper bound on the size of an attribute list PHOINIX will read.
const MAX_ATTRIBUTE_LIST_BYTES: u64 = 16 * 1024 * 1024;

/// The master file table as a logical stream of fixed-size records.
///
/// After [`bootstrap`](Self::bootstrap) records are read through the `$MFT`
/// data stream, so a fragmented MFT is handled transparently.
#[derive(Debug, Clone)]
pub struct Mft {
    stream: NtfsDataStream,
    record_size: u32,
    stride: usize,
    record_count: u64,
    /// Whether record 0 had to be taken from `$MFTMirr`.
    pub used_mirror: bool,
}

impl Mft {
    /// Locates `$MFT` from the boot sector and builds its data stream.
    ///
    /// Record 0 is read directly from the `$MFT` LCN; if it is damaged, the
    /// copy in `$MFTMirr` is tried. The unnamed `$DATA` runlist of record 0
    /// describes the MFT itself; when record 0 carries an `$ATTRIBUTE_LIST`,
    /// the `$DATA` pieces held in extension records are appended so that
    /// very large MFTs are covered completely.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError`] if record 0 cannot be read from either location
    /// or its runlist is invalid.
    pub fn bootstrap(
        reader: Arc<dyn BlockReader>,
        boot: &NtfsBootSector,
    ) -> Result<Self, NtfsError> {
        let record_size = boot.mft_record_size;
        let stride = usize::from(boot.bytes_per_sector);
        let record_len = usize::try_from(record_size).map_err(|_| NtfsError::Overflow)?;
        let total_clusters = boot.total_clusters();

        let primary = reader.read_vec(boot.mft_offset()?, record_len)?;
        let (record0, used_mirror) = match FileRecord::parse(MFT_RECORD, primary, stride) {
            Ok(r) => (r, false),
            Err(primary_err) => {
                tracing::warn!(error = %primary_err, "$MFT record 0 unusable; trying $MFTMirr");
                let mirror =
                    reader.read_vec(boot.lcn_to_offset(boot.mft_mirror_lcn)?, record_len)?;
                match FileRecord::parse(MFT_RECORD, mirror, stride) {
                    Ok(r) => (r, true),
                    Err(mirror_err) => {
                        return Err(NtfsError::InvalidRecord {
                            record: 0,
                            reason: format!("$MFT: {primary_err}; $MFTMirr: {mirror_err}"),
                        });
                    }
                }
            }
        };

        let mut runs: Vec<NtfsRun> = Vec::new();
        let mut real_size = 0u64;
        let mut initialized_size = 0u64;
        let mut attribute_list: Option<Vec<u8>> = None;
        let mut attribute_list_runs: Option<(Vec<NtfsRun>, u64)> = None;
        for attr in record0.attributes() {
            let attr = attr?;
            match attr.header.attribute_type {
                AttributeType::Data if attr.is_unnamed() => {
                    let (nr, pairs) = match &attr.body {
                        crate::attribute::AttributeBody::NonResident {
                            header,
                            mapping_pairs,
                        } => (header, *mapping_pairs),
                        crate::attribute::AttributeBody::Resident { .. } => {
                            return Err(NtfsError::InvalidRecord {
                                record: 0,
                                reason: "$MFT $DATA is resident".into(),
                            });
                        }
                    };
                    runs.extend(decode_runlist(pairs, nr.starting_vcn, total_clusters)?);
                    if nr.starting_vcn == 0 {
                        real_size = nr.real_size;
                        initialized_size = nr.initialized_size;
                    }
                }
                AttributeType::AttributeList => match &attr.body {
                    crate::attribute::AttributeBody::Resident { value, .. } => {
                        attribute_list = Some(value.to_vec())
                    }
                    crate::attribute::AttributeBody::NonResident {
                        header,
                        mapping_pairs,
                    } => {
                        let list_runs =
                            decode_runlist(mapping_pairs, header.starting_vcn, total_clusters)?;
                        attribute_list_runs = Some((list_runs, header.real_size));
                    }
                },
                _ => {}
            }
        }
        if runs.is_empty() {
            return Err(NtfsError::InvalidRecord {
                record: 0,
                reason: "$MFT has no unnamed $DATA runs".into(),
            });
        }

        let partial = NtfsDataStream::non_resident(
            reader.clone(),
            boot.cluster_size,
            runs.clone(),
            real_size,
            initialized_size,
        );

        // Resolve the attribute list, if any, to pick up further $DATA pieces.
        let list_bytes = match (attribute_list, attribute_list_runs) {
            (Some(bytes), _) => Some(bytes),
            (None, Some((list_runs, len))) => {
                if len > MAX_ATTRIBUTE_LIST_BYTES {
                    return Err(NtfsError::Unsupported(
                        "$MFT attribute list is unreasonably large".into(),
                    ));
                }
                let s = NtfsDataStream::non_resident(
                    reader.clone(),
                    boot.cluster_size,
                    list_runs,
                    len,
                    len,
                );
                Some(s.read_all(MAX_ATTRIBUTE_LIST_BYTES)?)
            }
            (None, None) => None,
        };
        if let Some(bytes) = list_bytes {
            let entries = parse_attribute_list(0, &bytes)?;
            let mut partial_mft = Self {
                stream: partial.clone(),
                record_size,
                stride,
                record_count: real_size / u64::from(record_size),
                used_mirror,
            };
            for entry in entries.iter().filter(|e| {
                e.attribute_type == AttributeType::Data
                    && e.name.is_empty()
                    && e.reference.record != 0
            }) {
                let ext = partial_mft.record(entry.reference.record).map_err(|e| {
                    NtfsError::InvalidRecord {
                        record: entry.reference.record,
                        reason: format!("$MFT extension record unreadable: {e}"),
                    }
                })?;
                if ext.header().base_reference.record != MFT_RECORD {
                    return Err(NtfsError::InvalidRecord {
                        record: entry.reference.record,
                        reason: "$MFT extension record does not reference record 0".into(),
                    });
                }
                for attr in ext.attributes() {
                    let attr = attr?;
                    if attr.header.attribute_type == AttributeType::Data
                        && attr.is_unnamed()
                        && let crate::attribute::AttributeBody::NonResident {
                            header,
                            mapping_pairs,
                        } = &attr.body
                        && header.starting_vcn == entry.starting_vcn
                    {
                        runs.extend(decode_runlist(
                            mapping_pairs,
                            header.starting_vcn,
                            total_clusters,
                        )?);
                    }
                }
                partial_mft.stream = NtfsDataStream::non_resident(
                    reader.clone(),
                    boot.cluster_size,
                    runs.clone(),
                    real_size,
                    initialized_size,
                );
            }
        }

        let stream = NtfsDataStream::non_resident(
            reader,
            boot.cluster_size,
            runs,
            real_size,
            initialized_size,
        );
        let record_count = real_size / u64::from(record_size);
        tracing::info!(
            record_count,
            record_size,
            used_mirror,
            extents = stream.runs().len(),
            "$MFT bootstrapped"
        );
        Ok(Self {
            stream,
            record_size,
            stride,
            record_count,
            used_mirror,
        })
    }

    /// Number of records in the MFT.
    #[must_use]
    pub const fn record_count(&self) -> u64 {
        self.record_count
    }

    /// Size of one record in bytes.
    #[must_use]
    pub const fn record_size(&self) -> u32 {
        self.record_size
    }

    /// The MFT data stream.
    #[must_use]
    pub const fn stream(&self) -> &NtfsDataStream {
        &self.stream
    }

    /// Reads the raw (not fixed-up) bytes of a record.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::NoSuchRecord`] beyond the end of the MFT, or a
    /// read error.
    pub fn raw_record(&self, number: u64) -> Result<Vec<u8>, NtfsError> {
        if number >= self.record_count {
            return Err(NtfsError::NoSuchRecord(number));
        }
        let offset = arith::mul(number, u64::from(self.record_size))?;
        let mut buf =
            vec![0u8; usize::try_from(self.record_size).map_err(|_| NtfsError::Overflow)?];
        self.stream.read_exact_at(offset, &mut buf)?;
        Ok(buf)
    }

    /// Reads and fixes up a record.
    ///
    /// # Errors
    ///
    /// As [`raw_record`](Self::raw_record), plus [`NtfsError::InvalidRecord`]
    /// and [`NtfsError::FixupMismatch`].
    pub fn record(&self, number: u64) -> Result<FileRecord, NtfsError> {
        tracing::trace!(mft_record = number, "reading MFT record");
        FileRecord::parse(number, self.raw_record(number)?, self.stride)
    }

    /// Iterates every record. Corrupt records yield an error item and
    /// iteration continues with the next record.
    pub fn records(&self) -> impl Iterator<Item = (u64, Result<FileRecord, NtfsError>)> + '_ {
        (0..self.record_count).map(move |n| (n, self.record(n)))
    }
}
