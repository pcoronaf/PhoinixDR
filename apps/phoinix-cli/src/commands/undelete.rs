//! Shared setup for the undelete commands: open a source, pick the volume,
//! detect its filesystem and build the matching engines (metadata engine
//! and, on demand, the carving engine over the same volume).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use phoinix_block::BlockReader;
use phoinix_carve::{CarveEngine, CarveOptions, SignatureSet};
use phoinix_core::FileSystemType;
use phoinix_device::platform_enumerator;
use phoinix_fs::{AllocationView, DeletedFileProvider, FileSystemObjectId, WholeSource};
use phoinix_fs_exfat::{ExfatUndelete, ExfatVolume};
use phoinix_fs_ext::{ExtUndelete, ExtVolume};
use phoinix_fs_fat::{FatUndelete, FatVolume};
use phoinix_fs_ntfs::{NtfsUndelete, NtfsVolume};
use phoinix_health::{DeviceKind, StorageEvidence};

use crate::source::{self, standard_probes};

/// Common source-selection arguments for scan/explain/recover.
#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Device path or image file.
    pub source: PathBuf,
    /// Partition index to use (default: the first partition with a
    /// supported filesystem, or the whole source when it has no partition
    /// table).
    #[arg(long, short = 'p')]
    pub partition: Option<u32>,
    /// Skip content examination (faster; lowers assessment confidence).
    #[arg(long)]
    pub no_content: bool,
    /// Use the volume starting at this byte offset of the source (a lost
    /// partition found by `phoinix partitions`) instead of a table entry.
    #[arg(long = "at", conflicts_with_all = ["partition", "lost"])]
    pub volume_offset: Option<u64>,
    /// Length in bytes of the volume selected with --at (default: to the end
    /// of the source).
    #[arg(long = "length", requires = "volume_offset")]
    pub volume_length: Option<u64>,
    /// Use candidate number N of `phoinix partitions` (runs the structure search
    /// again and mounts the candidate virtually, repairs included).
    #[arg(long, conflicts_with = "partition")]
    pub lost: Option<usize>,
}

/// Carving arguments shared by scan/explain/recover (`explain` and
/// `recover` need them to rebuild carved candidates the same way `scan`
/// found them).
#[derive(Debug, Clone, clap::Args)]
pub struct CarveArgs {
    /// Carve the whole volume instead of only its unallocated space.
    #[arg(long)]
    pub carve_all: bool,
    /// Comma-separated signature ids to carve (default: all built-in).
    #[arg(long, value_delimiter = ',')]
    pub carve_types: Vec<String>,
    /// Extra signature definitions (JSON array, see docs/carving).
    #[arg(long)]
    pub carve_signatures: Option<PathBuf>,
    /// Test only offsets that are multiples of this (default 512; 1 tests
    /// every byte and is slow).
    #[arg(long, default_value_t = 512)]
    pub carve_align: u64,
    /// Drop carved files shorter than this many bytes.
    #[arg(long, default_value_t = 0)]
    pub carve_min_size: u64,
    /// Worker threads for the header search (0 = all cores).
    #[arg(long, default_value_t = 0)]
    pub carve_threads: usize,
}

impl Default for CarveArgs {
    fn default() -> Self {
        Self {
            carve_all: false,
            carve_types: Vec::new(),
            carve_signatures: None,
            carve_align: 512,
            carve_min_size: 0,
            carve_threads: 0,
        }
    }
}

/// An opened volume with its engines.
pub struct Session {
    /// Detected filesystem.
    pub filesystem: FileSystemType,
    /// The metadata engine, when the filesystem has one.
    pub engine: Option<Arc<dyn DeletedFileProvider>>,
    /// What is known about allocation (the engine's view, or nothing).
    pub space: Arc<dyn AllocationView>,
    /// The volume.
    pub reader: Arc<dyn BlockReader>,
    /// The medium.
    pub storage: StorageEvidence,
    no_content: bool,
}

impl Session {
    /// Opens the source and builds the engine for its filesystem; fails
    /// for filesystems without an engine.
    pub fn open(args: &SourceArgs) -> anyhow::Result<Self> {
        let session = Self::open_any(args)?;
        if session.engine.is_none() {
            anyhow::bail!(
                "no undelete engine for {}; supported: NTFS, FAT12/16/32, exFAT, ext2/3/4 (use --partition to pick another volume, or `scan --deep` to carve the raw volume)",
                session.filesystem
            );
        }
        Ok(session)
    }

    /// Opens the source; a filesystem without an engine yields a session
    /// that can only carve.
    pub fn open_any(args: &SourceArgs) -> anyhow::Result<Self> {
        let opened = source::open(&args.source)?;
        let reader: Arc<dyn BlockReader> = if let Some(index) = args.lost {
            let candidates = crate::commands::partitions::search_with_progress(
                &opened.reader,
                Some(&opened.table),
                &phoinix_partition_recovery::SearchOptions::default(),
            )?;
            let candidate = index
                .checked_sub(1)
                .and_then(|i| candidates.get(i))
                .with_context(|| {
                    format!(
                        "lost partition candidate #{index} does not exist ({} found)",
                        candidates.len()
                    )
                })?;
            tracing::info!(
                start = candidate.start,
                length = candidate.length,
                "lost partition mounted"
            );
            candidate
                .open(opened.reader.clone())
                .context("mounting the candidate")?
        } else if let Some(offset) = args.volume_offset {
            let length = args
                .volume_length
                .unwrap_or_else(|| opened.reader.len().saturating_sub(offset));
            let range = phoinix_core::ByteRange { offset, length };
            Arc::new(
                phoinix_block::SubrangeReader::new(opened.reader.clone(), range)
                    .with_context(|| format!("opening {length} bytes at offset {offset}"))?,
            )
        } else {
            let selected = source::select_volume(&opened, args.partition, None)?;
            tracing::info!(partition = ?selected.partition, offset = selected.offset, "volume selected");
            selected.reader
        };
        let detection = standard_probes().detect(&*reader);
        let filesystem = detection.filesystem();
        let storage = storage_evidence(&args.source);
        let (engine, space): (
            Option<Arc<dyn DeletedFileProvider>>,
            Arc<dyn AllocationView>,
        ) = match filesystem {
            FileSystemType::Ntfs => {
                let volume =
                    Arc::new(NtfsVolume::open(reader.clone()).context("opening NTFS volume")?);
                let e = NtfsUndelete::new(volume, storage.clone());
                let e = Arc::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                });
                (Some(e.clone()), e)
            }
            FileSystemType::Fat12 | FileSystemType::Fat16 | FileSystemType::Fat32 => {
                let volume =
                    Arc::new(FatVolume::open(reader.clone()).context("opening FAT volume")?);
                let e = FatUndelete::new(volume, storage.clone());
                let e = Arc::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                });
                (Some(e.clone()), e)
            }
            FileSystemType::ExFat => {
                let volume =
                    Arc::new(ExfatVolume::open(reader.clone()).context("opening exFAT volume")?);
                let e = ExfatUndelete::new(volume, storage.clone());
                let e = Arc::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                });
                (Some(e.clone()), e)
            }
            FileSystemType::Ext => {
                let volume =
                    Arc::new(ExtVolume::open(reader.clone()).context("opening ext volume")?);
                let e = ExtUndelete::new(volume, storage.clone());
                let e = Arc::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                });
                (Some(e.clone()), e)
            }
            _ => (
                None,
                Arc::new(WholeSource::new(
                    reader.len(),
                    u64::from(reader.geometry().logical_sector_size.max(1)),
                )),
            ),
        };
        Ok(Self {
            filesystem,
            engine,
            space,
            reader,
            storage,
            no_content: args.no_content,
        })
    }

    /// The metadata engine.
    pub fn engine(&self) -> anyhow::Result<&dyn DeletedFileProvider> {
        self.engine
            .as_deref()
            .with_context(|| format!("no undelete engine for {}", self.filesystem))
    }

    /// Builds the carving engine for this volume.
    pub fn carve_engine(&self, args: &CarveArgs) -> anyhow::Result<CarveEngine> {
        let mut signatures = SignatureSet::builtin();
        if let Some(path) = &args.carve_signatures {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            signatures = signatures
                .with_json(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
        }
        if !args.carve_types.is_empty() {
            signatures = signatures.only(&args.carve_types)?;
        }
        let mut options = CarveOptions {
            whole_volume: args.carve_all || self.engine.is_none(),
            min_size: args.carve_min_size,
            examine_content: !self.no_content,
            ..Default::default()
        };
        options.scan.alignment = args.carve_align.max(1);
        options.scan.threads = args.carve_threads;
        Ok(CarveEngine::new(
            self.reader.clone(),
            self.space.clone(),
            self.filesystem,
            self.storage.clone(),
        )
        .with_signatures(signatures)
        .with_options(options))
    }

    /// Whether `reference` names a carved candidate (`c<offset>`).
    #[must_use]
    pub fn is_carved_reference(reference: &str) -> bool {
        FileSystemObjectId::parse_carved_reference(reference).is_some()
    }
}

/// Gathers what is known about the medium behind `source`.
pub fn storage_evidence(source: &Path) -> StorageEvidence {
    let enumerator = platform_enumerator();
    if !enumerator.is_device_path(source) {
        return StorageEvidence {
            device_kind: DeviceKind::Image,
            rotational: None,
            trim_supported: None,
            trim_state_known: false,
        };
    }
    let wanted = source.to_string_lossy();
    let info = enumerator.enumerate().ok().and_then(|devices| {
        devices.into_iter().find(|d| {
            d.path.as_str() == wanted || d.parent.as_ref().is_some_and(|p| p.as_str() == wanted)
        })
    });
    StorageEvidence {
        device_kind: DeviceKind::BlockDevice,
        rotational: info.and_then(|d| d.rotational),
        trim_supported: None,
        trim_state_known: false,
    }
}
