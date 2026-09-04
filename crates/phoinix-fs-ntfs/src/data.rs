//! `$DATA` stream descriptors.

use serde::{Deserialize, Serialize};

use crate::runlist::{self, NtfsRun};

/// Where the bytes of a stream live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataStorage {
    /// Stored inside the MFT record.
    Resident {
        /// The bytes.
        #[serde(skip)]
        value: Vec<u8>,
    },
    /// Stored in clusters.
    NonResident {
        /// Runs sorted by VCN.
        runs: Vec<NtfsRun>,
        /// Logical size.
        real_size: u64,
        /// Bytes actually written; the rest reads as zero.
        initialized_size: u64,
        /// Allocated bytes (cluster-rounded).
        allocated_size: u64,
        /// Whether the runs cover every allocated cluster without gaps.
        complete: bool,
    },
    /// NTFS-compressed; PHOINIX does not decompress yet.
    UnsupportedCompressed {
        /// Runs (may include sparse runs marking compressed units).
        runs: Vec<NtfsRun>,
        /// Logical size.
        real_size: u64,
        /// Compression unit exponent.
        compression_unit: u8,
    },
    /// EFS-encrypted; bytes are recoverable but unusable without keys.
    UnsupportedEncrypted {
        /// Runs.
        runs: Vec<NtfsRun>,
        /// Logical size.
        real_size: u64,
    },
}

impl DataStorage {
    /// Runs backing the stream, if any.
    #[must_use]
    pub fn runs(&self) -> &[NtfsRun] {
        match self {
            DataStorage::Resident { .. } => &[],
            DataStorage::NonResident { runs, .. }
            | DataStorage::UnsupportedCompressed { runs, .. }
            | DataStorage::UnsupportedEncrypted { runs, .. } => runs,
        }
    }

    /// Whether the content can be read by PHOINIX.
    #[must_use]
    pub const fn is_readable(&self) -> bool {
        matches!(
            self,
            DataStorage::Resident { .. } | DataStorage::NonResident { .. }
        )
    }

    /// Whether the stream is resident.
    #[must_use]
    pub const fn is_resident(&self) -> bool {
        matches!(self, DataStorage::Resident { .. })
    }
}

/// One `$DATA` stream of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStreamDescriptor {
    /// Stream name; `None` for the unnamed (default) stream.
    pub name: Option<String>,
    /// Logical size in bytes.
    pub logical_size: u64,
    /// Storage.
    pub storage: DataStorage,
    /// Raw attribute flags.
    pub flags: u16,
}

impl DataStreamDescriptor {
    /// Whether this is the unnamed stream.
    #[must_use]
    pub fn is_unnamed(&self) -> bool {
        self.name.as_deref().is_none_or(str::is_empty)
    }

    /// Number of physical extents.
    #[must_use]
    pub fn extent_count(&self) -> u32 {
        runlist::extent_count(self.storage.runs())
    }

    /// Clusters covered by data runs (excluding sparse runs).
    #[must_use]
    pub fn data_clusters(&self) -> u64 {
        self.storage
            .runs()
            .iter()
            .filter(|r| !r.is_sparse())
            .fold(0u64, |a, r| a.saturating_add(r.clusters()))
    }

    /// Whether the stream contains sparse runs.
    #[must_use]
    pub fn has_sparse_runs(&self) -> bool {
        self.storage.runs().iter().any(NtfsRun::is_sparse)
    }
}
