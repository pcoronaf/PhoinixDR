//! M2 integration tests: real partition tables produced by sfdisk/sgdisk.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::sync::Arc;

use std::sync::Mutex;

use phoinix_block::{BlockError, BlockGeometry, BlockReader, MemoryReader, check_request};
use phoinix_core::FileSystemType;
use phoinix_core::SourceId;
use phoinix_fs::{ProbeRegistry, signature};
use phoinix_fs_ntfs::NtfsProbe;
use phoinix_integration_tests::{Rng, fixture_reader, load_gz, manifest};
use phoinix_volume::{PartitionFlags, PartitionScheme, PartitionType, read_partition_table};

fn probes() -> ProbeRegistry {
    signature::register_all(ProbeRegistry::new().with(Box::new(NtfsProbe)))
}

fn fs_from_name(name: &str) -> FileSystemType {
    match name {
        "ntfs" => FileSystemType::Ntfs,
        "fat12" => FileSystemType::Fat12,
        "fat16" => FileSystemType::Fat16,
        "fat32" => FileSystemType::Fat32,
        other => panic!("unknown filesystem name {other}"),
    }
}

#[test]
fn fixtures_match_manifest() {
    let m = manifest("volume/manifest.json");
    let images = m["images"].as_object().unwrap();
    assert_eq!(images.len(), 3);
    for (name, expected) in images {
        let reader: Arc<dyn BlockReader> = Arc::new(fixture_reader(&format!("volume/{name}")));
        assert_eq!(
            reader.len(),
            expected["size"].as_u64().unwrap(),
            "{name} size"
        );
        let table = read_partition_table(&*reader).unwrap();
        let scheme = match expected["scheme"].as_str().unwrap() {
            "mbr" => PartitionScheme::Mbr,
            "gpt" => PartitionScheme::Gpt,
            "none" => PartitionScheme::None,
            s => panic!("{s}"),
        };
        assert_eq!(
            table.scheme, scheme,
            "{name} scheme; diagnostics {:?}",
            table.diagnostics
        );
        if scheme == PartitionScheme::Gpt {
            assert!(
                table.diagnostics.is_empty(),
                "{name}: {:?}",
                table.diagnostics
            );
            assert!(table.disk_guid.is_some());
        }
        if let Some(sig) = expected.get("mbr_disk_signature") {
            assert_eq!(table.mbr_disk_signature, Some(sig.as_u64().unwrap() as u32));
        }

        let parts = expected["partitions"].as_array().unwrap();
        assert_eq!(
            table.partitions.len(),
            parts.len(),
            "{name} partition count: {:?}",
            table.partitions
        );
        let registry = probes();
        for (got, want) in table.partitions.iter().zip(parts) {
            assert_eq!(
                got.index,
                want["index"].as_u64().unwrap() as u32,
                "{name} index"
            );
            assert_eq!(
                got.start_lba,
                want["start_lba"].as_u64().unwrap(),
                "{name} p{} start",
                got.index
            );
            assert_eq!(
                got.end_lba,
                want["end_lba"].as_u64().unwrap(),
                "{name} p{} end",
                got.index
            );
            assert_eq!(got.start_offset, got.start_lba * 512);
            assert_eq!(got.length, (got.end_lba - got.start_lba + 1) * 512);
            match got.partition_type {
                PartitionType::Mbr(t) => {
                    assert_eq!(u64::from(t), want["type"]["value"].as_u64().unwrap())
                }
                PartitionType::Gpt(g) => {
                    assert_eq!(g.to_string(), want["type"]["value"].as_str().unwrap())
                }
            }
            if let Some(b) = want.get("bootable") {
                assert_eq!(
                    got.flags.contains(PartitionFlags::BOOTABLE),
                    b.as_bool().unwrap(),
                    "{name} p{} bootable",
                    got.index
                );
            }
            if let Some(l) = want.get("logical") {
                assert_eq!(
                    got.flags.contains(PartitionFlags::LOGICAL),
                    l.as_bool().unwrap(),
                    "{name} p{} logical",
                    got.index
                );
            }
            if let Some(n) = want.get("name") {
                assert_eq!(
                    got.name.as_deref(),
                    n.as_str(),
                    "{name} p{} name",
                    got.index
                );
            }
            let view = got.open(reader.clone()).unwrap();
            let detection = registry.detect(&view);
            match want["filesystem"].as_str() {
                Some(fs) => assert_eq!(
                    detection.filesystem(),
                    fs_from_name(fs),
                    "{name} p{}: {detection:?}",
                    got.index
                ),
                None => assert_eq!(
                    detection.filesystem(),
                    FileSystemType::Unknown,
                    "{name} p{}",
                    got.index
                ),
            }
        }
        if let Some(fs) = expected.get("whole_source_filesystem") {
            assert_eq!(
                probes().detect(&*reader).filesystem(),
                fs_from_name(fs.as_str().unwrap())
            );
        }
    }
}

#[test]
fn source_is_not_modified_by_discovery() {
    for name in ["volume/mbr-extended.img.gz", "volume/gpt-basic.img.gz"] {
        let data = load_gz(name);
        let reader = MemoryReader::new(data.clone());
        let _ = read_partition_table(&reader).unwrap();
        let _ = probes().detect(&reader);
        assert_eq!(reader.data(), &data[..], "{name} changed");
    }
}

/// A reader over shared, mutable bytes so mutation rounds need no copies.
struct SharedReader {
    id: SourceId,
    data: Arc<Mutex<Vec<u8>>>,
    len: u64,
    geometry: BlockGeometry,
}

impl BlockReader for SharedReader {
    fn id(&self) -> SourceId {
        self.id
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.len, offset, buffer.len())?;
        let data = self.data.lock().unwrap();
        let start = offset as usize;
        buffer.copy_from_slice(&data[start..start + buffer.len()]);
        Ok(buffer.len())
    }
}

/// Corrupts partition-table bytes in many deterministic ways and requires
/// that discovery never panics or reads outside the source.
#[test]
fn mutated_tables_never_panic() {
    for name in ["volume/mbr-extended.img.gz", "volume/gpt-basic.img.gz"] {
        let shared = Arc::new(Mutex::new(load_gz(name)));
        let full_len = shared.lock().unwrap().len();
        let mut rng = Rng::new(0xC0FFEE);
        for round in 0..300u32 {
            let mut data = shared.lock().unwrap();
            // Focus on LBA 0..=33 (MBR, GPT header, entry array) and the
            // backup structures at the end. Mutations are undone after the
            // round so the image is not copied.
            let region_len = 34 * 512;
            let flips = 1 + rng.below(8) as usize;
            let mut undo: Vec<(usize, u8)> = Vec::with_capacity(flips);
            for _ in 0..flips {
                let in_tail = rng.below(4) == 0;
                let base = if in_tail { full_len - region_len } else { 0 };
                let pos = base + rng.below(region_len as u64) as usize;
                undo.push((pos, data[pos]));
                match rng.below(3) {
                    0 => data[pos] ^= 1 << rng.below(8),
                    1 => data[pos] = 0xFF,
                    _ => data[pos] = rng.next_u64() as u8,
                }
            }
            // Occasionally view a truncated prefix of the image as well.
            let keep = if round % 7 == 0 {
                512 + rng.below((full_len - 512) as u64) as usize
            } else {
                full_len
            };
            drop(data);
            let reader: Arc<dyn BlockReader> = Arc::new(SharedReader {
                id: SourceId::new(),
                data: shared.clone(),
                len: keep as u64,
                geometry: BlockGeometry::SECTOR_512,
            });
            let table = read_partition_table(&*reader)
                .unwrap_or_else(|e| panic!("{name} round {round}: {e}"));
            let registry = probes();
            for p in &table.partitions {
                // Opening must either succeed (possibly clamped) or fail cleanly.
                if let Ok(view) = p.open(reader.clone()) {
                    let _ = registry.detect(&view);
                }
            }
            let mut data = shared.lock().unwrap();
            for (pos, byte) in undo.into_iter().rev() {
                data[pos] = byte;
            }
        }
    }
}
