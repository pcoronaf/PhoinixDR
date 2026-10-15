//! M12.2: the slack ranges every engine reports lie inside allocated
//! clusters, never overlap the free space, and are non-empty on the
//! deletion corpora (which hold files whose size is not a cluster
//! multiple).

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
use phoinix_fs::{AllocationView, ByteRange};
use phoinix_fs_exfat::{ExfatUndelete, ExfatVolume, entry_set_checksum};
use phoinix_fs_ext::{ExtUndelete, ExtVolume};
use phoinix_fs_fat::{FatUndelete, FatVolume};
use phoinix_fs_ntfs::{NtfsUndelete, NtfsVolume};
use phoinix_health::{DeviceKind, StorageEvidence};
use phoinix_integration_tests::load_gz;

fn storage() -> StorageEvidence {
    StorageEvidence {
        device_kind: DeviceKind::Image,
        rotational: None,
        trim_supported: None,
        trim_state_known: false,
    }
}

fn reader(name: &str) -> Arc<dyn BlockReader> {
    Arc::new(MemoryReader::new(load_gz(name)))
}

fn check(view: &dyn AllocationView, name: &str) {
    let slack = view
        .slack_ranges()
        .unwrap()
        .unwrap_or_else(|| panic!("{name}: no slack support"));
    assert!(!slack.is_empty(), "{name}: no slack found");
    let cs = view.cluster_size();
    let free = view.free_ranges().unwrap();
    let mut total = 0u64;
    for r in &slack {
        assert!(
            r.length > 0 && r.length < 64 * cs,
            "{name}: odd slack {r:?}"
        );
        assert!(
            r.end() <= view.volume_len(),
            "{name}: {r:?} past the volume"
        );
        let s = view.summarize(*r);
        assert_eq!(s.free, 0, "{name}: slack {r:?} lies in free clusters");
        assert_eq!(s.unknown, 0, "{name}: slack {r:?} lies in unknown clusters");
        assert!(
            !free
                .iter()
                .any(|f| f.offset < r.end() && f.end() > r.offset),
            "{name}: slack {r:?} overlaps the free space"
        );
        total += r.length;
    }
    // Sorted and merged.
    for w in slack.windows(2) {
        assert!(
            w[0].end() < w[1].offset,
            "{name}: unmerged {:?} {:?}",
            w[0],
            w[1]
        );
    }
    assert!(
        total < view.volume_len() / 4,
        "{name}: slack is too large to be slack"
    );
}

/// Patches the size of one live file of a FAT corpus so that it ends
/// inside its last cluster, and returns the image and the expected slack.
fn fat_with_slack(name: &str) -> (Vec<u8>, ByteRange) {
    let mut image = load_gz(name);
    let volume = FatVolume::open(Arc::new(MemoryReader::new(image.clone()))).unwrap();
    let cs = u64::from(volume.cluster_size());
    let live = volume
        .walk()
        .unwrap()
        .into_iter()
        .find(|w| {
            !w.entry.deleted && !w.entry.attributes.is_directory() && w.entry.size >= 4 * cs as u32
        })
        .expect("a live file of at least four clusters");
    let clusters = volume.reconstruct(&live.entry).unwrap().clusters;
    let new_size = u32::try_from(3 * cs + 1000).unwrap();
    let at = usize::try_from(live.entry.entry_offset).unwrap() + 28;
    image[at..at + 4].copy_from_slice(&new_size.to_le_bytes());
    let last = volume.boot().cluster_offset(clusters[3]).unwrap();
    (
        image,
        ByteRange {
            offset: last + 1000,
            length: cs - 1000,
        },
    )
}

#[test]
fn fat_slack_is_the_tail_of_the_last_cluster() {
    for name in ["fat/fat16.img.gz", "fat/fat32.img.gz"] {
        let (image, expected) = fat_with_slack(name);
        let volume = Arc::new(FatVolume::open(Arc::new(MemoryReader::new(image))).unwrap());
        let engine = FatUndelete::new(volume, storage());
        let slack = engine.slack_ranges().unwrap().unwrap();
        assert_eq!(slack, vec![expected], "{name}");
        check(&engine, name);
    }
}

#[test]
fn exfat_slack_is_the_tail_of_the_last_cluster() {
    let mut image = load_gz("exfat/undelete.img.gz");
    let volume = ExfatVolume::open(Arc::new(MemoryReader::new(image.clone()))).unwrap();
    let cs = u64::from(volume.cluster_size());
    let live = volume
        .walk()
        .unwrap()
        .into_iter()
        .find(|w| !w.entry.deleted && !w.entry.is_directory() && w.entry.data_length >= 4 * cs)
        .expect("a live file of at least four clusters");
    let clusters = volume.reconstruct(&live.entry).unwrap().clusters;
    // The stream extension entry follows the file entry; patch both
    // lengths and refresh the set checksum.
    let base = usize::try_from(live.entry.entry_offset).unwrap();
    let new_size = 3 * cs + 1000;
    image[base + 32 + 8..base + 32 + 16].copy_from_slice(&new_size.to_le_bytes());
    image[base + 32 + 24..base + 32 + 32].copy_from_slice(&new_size.to_le_bytes());
    let set_len = 32 * (1 + usize::from(live.entry.secondary_count));
    let sum = entry_set_checksum(&image[base..base + set_len], false);
    image[base + 2..base + 4].copy_from_slice(&sum.to_le_bytes());
    let expected = ByteRange {
        offset: volume.boot().cluster_offset(clusters[3]).unwrap() + 1000,
        length: cs - 1000,
    };
    let volume = Arc::new(ExfatVolume::open(Arc::new(MemoryReader::new(image))).unwrap());
    let engine = ExfatUndelete::new(volume, storage());
    assert_eq!(engine.slack_ranges().unwrap().unwrap(), vec![expected]);
    check(&engine, "exfat");
}

#[test]
fn ntfs_slack_skips_resident_sparse_and_exact_streams() {
    // reader.img: partial_cluster.bin (5000 of 8192), a 3000-byte file in
    // one cluster, a sparse file, resident and cluster-exact files.
    let volume = Arc::new(NtfsVolume::open(reader("ntfs/reader.img.gz")).unwrap());
    let engine = NtfsUndelete::new(volume, storage());
    let slack = engine.slack_ranges().unwrap().unwrap();
    assert_eq!(slack.len(), 2, "{slack:?}");
    let total: u64 = slack.iter().map(|r| r.length).sum();
    assert_eq!(total, (8192 - 5000) + (4096 - 3000));
    assert!(slack.iter().any(|r| r.length == 3192));
    assert!(slack.iter().any(|r| r.length == 1096));
    check(&engine, "ntfs");
}

#[test]
fn ext_slack_is_inside_allocated_clusters() {
    for name in ["ext/ext2.img.gz", "ext/ext3.img.gz", "ext/ext4.img.gz"] {
        let volume = Arc::new(ExtVolume::open(reader(name)).unwrap());
        let engine = ExtUndelete::new(volume, storage());
        check(&engine, name);
    }
}
