//! M9 acceptance: partitions are found from their structures with the
//! right boundaries, with and without a partition table, from backups
//! when the primary is gone, and can be virtually mounted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::sync::Arc;

use phoinix_block::{BlockReader, MemoryReader};
use phoinix_core::FileSystemType;
use phoinix_fs::DeletedFileProvider;
use phoinix_fs_ntfs::{NtfsUndelete, NtfsVolume};
use phoinix_health::StorageEvidence;
use phoinix_integration_tests::{Rng, load_gz, manifest};
use phoinix_partition_recovery::{
    FoundVia, PartitionCandidate, Relation, SearchOptions, find_partitions,
};
use phoinix_volume::read_partition_table;

fn search(image: Vec<u8>, with_table: bool) -> Vec<PartitionCandidate> {
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image));
    let table = with_table.then(|| read_partition_table(&*reader).unwrap());
    let mut progress = 0;
    let found = find_partitions(
        &reader,
        table.as_ref(),
        &SearchOptions::default(),
        &mut |_| progress += 1,
    )
    .unwrap();
    assert!(progress >= 1);
    found
}

fn expected(name: &str) -> Vec<(u32, u64, u64, FileSystemType)> {
    let m = manifest("volume/manifest.json");
    m["images"][name]["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| !p["filesystem"].is_null())
        .map(|p| {
            let fs = match p["filesystem"].as_str().unwrap() {
                "ntfs" => FileSystemType::Ntfs,
                "fat16" => FileSystemType::Fat16,
                "fat12" => FileSystemType::Fat12,
                other => panic!("{other}"),
            };
            let start = p["start_lba"].as_u64().unwrap() * 512;
            let len = (p["end_lba"].as_u64().unwrap() - p["start_lba"].as_u64().unwrap() + 1) * 512;
            (p["index"].as_u64().unwrap() as u32, start, len, fs)
        })
        .collect()
}

#[test]
fn finds_listed_partitions_with_their_boundaries() {
    for (name, key) in [
        ("volume/gpt-basic.img.gz", "gpt-basic.img.gz"),
        ("volume/mbr-extended.img.gz", "mbr-extended.img.gz"),
    ] {
        let found = search(load_gz(name), true);
        let want = expected(key);
        assert_eq!(found.len(), want.len(), "{name}: {found:#?}");
        for (index, start, len, fs) in want {
            let c = found
                .iter()
                .find(|c| c.start == start)
                .unwrap_or_else(|| panic!("{name}: no candidate at {start}: {found:#?}"));
            assert_eq!(c.filesystem, fs, "{name}");
            assert_eq!(c.relation, Relation::Listed { index }, "{name}: {c:?}");
            // Declared length equals the partition (NTFS keeps one sector for the backup boot sector).
            assert!(
                c.length == len || c.length + u64::from(c.sector_size) == len,
                "{name}: {} vs {len}",
                c.length
            );
            assert!(c.primary_structure_valid && c.geometry_consistent);
            assert_eq!(c.engine_verified, Some(true), "{name}: {c:?}");
            assert!(c.confidence >= 80, "{name}: {c:?}");
            if fs == FileSystemType::Ntfs {
                assert_eq!(c.backup_structure_valid, Some(true), "{c:?}");
                assert!(c.serial.is_some());
            }
        }
    }
}

#[test]
fn finds_lost_partitions_after_the_table_is_wiped() {
    let mut image = load_gz("volume/gpt-basic.img.gz");
    // Wipe the protective MBR, the GPT header and entries, and the backup GPT.
    for b in &mut image[..34 * 512] {
        *b = 0;
    }
    let len = image.len();
    for b in &mut image[len - 33 * 512..] {
        *b = 0;
    }
    let table = read_partition_table(&MemoryReader::new(image.clone())).unwrap();
    assert!(table.volumes().count() == 0, "{table:?}");
    let found = search(image.clone(), true);
    let want = expected("gpt-basic.img.gz");
    assert_eq!(found.len(), want.len(), "{found:#?}");
    for (_, start, _, fs) in &want {
        let c = found.iter().find(|c| c.start == *start).unwrap();
        assert_eq!(c.filesystem, *fs);
        assert_eq!(c.relation, Relation::Lost, "{c:?}");
        assert!(c.confidence >= 80, "{c:?}");
    }
    // Virtual mount: the lost NTFS volume opens with the engine.
    let ntfs = found
        .iter()
        .find(|c| c.filesystem == FileSystemType::Ntfs)
        .unwrap();
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image));
    let view: Arc<dyn BlockReader> = Arc::new(ntfs.open(reader).unwrap());
    let volume = Arc::new(NtfsVolume::open(view).unwrap());
    let engine = NtfsUndelete::new(volume, StorageEvidence::default());
    let _ = engine.deleted_files().count();
}

#[test]
fn recovers_from_backup_structures_when_primaries_are_destroyed() {
    // NTFS: zero the primary boot sector of partition 3 of gpt-basic.
    let mut image = load_gz("volume/gpt-basic.img.gz");
    let ntfs_start = 20480 * 512;
    for b in &mut image[ntfs_start..ntfs_start + 512] {
        *b = 0;
    }
    let found = search(image.clone(), true);
    let c = found
        .iter()
        .find(|c| c.filesystem == FileSystemType::Ntfs)
        .unwrap();
    assert_eq!(c.start, ntfs_start as u64, "{c:?}");
    assert_eq!(c.found_via, FoundVia::BackupBootSector);
    assert!(!c.primary_structure_valid && c.backup_structure_valid == Some(true));
    assert_eq!(c.relation, Relation::Listed { index: 3 });
    assert!(
        c.evidence
            .iter()
            .any(|e| e.description.contains("backup boot sector"))
    );
    // Mounting substitutes the backup for the destroyed primary, so the
    // engine opens the volume.
    assert_eq!(c.repairs.len(), 1);
    assert_eq!(c.engine_verified, Some(true), "{c:?}");
    assert!(c.confidence >= 80, "{c:?}");
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image.clone()));
    let view = c.open(reader).unwrap();
    assert!(NtfsVolume::open(view).is_ok());

    // EXT: primary and backup superblocks, then the primary destroyed.
    let image = load_gz("volume/ext4-bare.img.gz");
    let found = search(image.clone(), false);
    assert_eq!(found.len(), 1, "{found:#?}");
    let e = &found[0];
    assert_eq!(
        (e.start, e.length, e.filesystem),
        (0, 16 * 1024 * 1024, FileSystemType::Ext)
    );
    assert_eq!(e.found_via, FoundVia::Superblock);
    assert_eq!(e.backup_structure_valid, Some(true), "{e:?}");
    assert_eq!(e.label.as_deref(), Some("PHXEXT4"));
    assert_eq!(
        e.serial.as_deref(),
        Some("0b0b0b0b-1111-4222-8333-444444444444")
    );
    assert_eq!(e.cluster_size, Some(1024));
    assert!(e.confidence >= 70, "{e:?}");
    let mut damaged = image;
    for b in &mut damaged[1024..2048] {
        *b = 0;
    }
    let found = search(damaged, false);
    assert_eq!(found.len(), 1, "{found:#?}");
    let e = &found[0];
    assert_eq!(e.start, 0);
    assert_eq!(e.found_via, FoundVia::BackupSuperblock);
    assert!(!e.primary_structure_valid);
    assert_eq!(e.length, 16 * 1024 * 1024);
    assert_eq!(e.repairs.len(), 1);
    assert_eq!(e.repairs[0].offset, 1024);
}

#[test]
fn nested_and_offset_volumes_are_related() {
    // An NTFS volume image stored 3 MiB inside a bigger raw area, next to a
    // FAT32 volume: both are lost, and a copy of the FAT boot sector placed
    // inside the NTFS data area is reported as nested.
    let ntfs = load_gz("volume/ntfs-bare.img.gz");
    let fat = load_gz("fat/fat32.img.gz");
    let mut image = vec![0u8; 3 * 1024 * 1024];
    image.extend_from_slice(&ntfs);
    image.extend(std::iter::repeat_n(0, 1024 * 1024));
    let fat_start = image.len();
    image.extend_from_slice(&fat);
    image.extend(std::iter::repeat_n(0, 1024 * 1024));
    let found = search(image.clone(), false);
    let starts: Vec<(u64, FileSystemType, Relation)> = found
        .iter()
        .map(|c| (c.start, c.filesystem, c.relation.clone()))
        .collect();
    assert!(
        starts.contains(&(3 * 1024 * 1024, FileSystemType::Ntfs, Relation::Lost)),
        "{starts:?}"
    );
    assert!(
        starts.contains(&(fat_start as u64, FileSystemType::Fat32, Relation::Lost)),
        "{starts:?}"
    );
    // Plant a FAT12 boot sector (a 4 MiB volume) inside the NTFS data area,
    // 8 MiB in: it lies entirely inside the NTFS candidate.
    let small = load_gz("fat/fat12.img.gz");
    let planted = 3 * 1024 * 1024 + 8 * 1024 * 1024;
    image[planted..planted + 512].copy_from_slice(&small[..512]);
    let found = search(image, false);
    let nested = found
        .iter()
        .find(|c| c.start == planted as u64)
        .expect("planted boot sector found");
    assert!(
        matches!(nested.relation, Relation::Nested { .. }),
        "{nested:?}"
    );
    assert!(nested.confidence < 60, "{nested:?}");
    let ntfs_c = found
        .iter()
        .find(|c| c.filesystem == FileSystemType::Ntfs)
        .unwrap();
    assert_eq!(ntfs_c.relation, Relation::Lost);
}

#[test]
fn corrupted_sources_never_panic() {
    let original = load_gz("volume/mbr-extended.img.gz");
    let mut rng = Rng::new(0x9A11);
    for round in 0..25u32 {
        let mut data = original.clone();
        for _ in 0..(1 + rng.below(40)) {
            let pos = rng.below(data.len() as u64) as usize;
            data[pos] = rng.next_u64() as u8;
        }
        // Hit boot sectors specifically half of the time.
        if round % 2 == 0 {
            for lba in [2048u64, 18432, 36864, 55296] {
                let pos = (lba * 512) as usize + rng.below(512) as usize;
                data[pos] ^= 0xFF;
            }
        }
        if round % 5 == 0 {
            let keep = 4096 + rng.below((data.len() - 4096) as u64) as usize;
            data.truncate(keep);
        }
        let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(data));
        let table = read_partition_table(&*reader).ok();
        let _ = find_partitions(
            &reader,
            table.as_ref(),
            &SearchOptions::default(),
            &mut |_| {},
        )
        .unwrap();
    }
}
