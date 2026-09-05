//! M8 acceptance: the carving corpus is found in unallocated space,
//! deduplicated against metadata candidates, recovered and assessed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::io::Write;
use std::sync::Arc;

use phoinix_block::{BlockReader, MemoryReader};
use phoinix_carve::{CarveEngine, CarveOptions};
use phoinix_core::FileSystemType;
use phoinix_fs::{
    AllocationView, DeletedFileProvider, FileSystemObjectId, RecoveryCandidate, WholeSource,
};
use phoinix_fs_fat::{FatUndelete, FatVolume};
use phoinix_health::{
    CandidateSource, DeviceKind, HealthCategory, StorageEvidence, ValidationStatus,
};
use phoinix_integration_tests::{Rng, load_gz, manifest};
use phoinix_recovery::{RecoveryRequest, RecoveryWriter};
use serde_json::Value;

fn storage() -> StorageEvidence {
    StorageEvidence {
        device_kind: DeviceKind::Image,
        rotational: None,
        trim_supported: None,
        trim_state_known: false,
    }
}

fn recover(engine: &dyn DeletedFileProvider, candidate: &RecoveryCandidate) -> (String, bool) {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("source.img");
    std::fs::File::create(&image)
        .unwrap()
        .write_all(b"placeholder")
        .unwrap();
    let writer =
        RecoveryWriter::new(engine, &image, RecoveryRequest::new(dir.path().join("out"))).unwrap();
    let r = writer.recover(candidate).unwrap();
    (r.sha256.unwrap(), r.complete)
}

fn carved_offset(c: &RecoveryCandidate) -> Option<u64> {
    match &c.filesystem_object {
        FileSystemObjectId::Carved { offset, .. } => Some(*offset),
        _ => None,
    }
}

struct Corpus {
    image: Vec<u8>,
    files: Vec<Value>,
}

fn corpus() -> Corpus {
    let m = manifest("carve/corpus.manifest.json");
    Corpus {
        image: load_gz("carve/corpus.img.gz"),
        files: m["files"].as_array().unwrap().clone(),
    }
}

#[test]
fn deep_scan_of_unallocated_space() {
    let corpus = corpus();
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(corpus.image.clone()));
    let volume = Arc::new(FatVolume::open(reader.clone()).unwrap());
    let fat = Arc::new(FatUndelete::new(volume, storage()));
    let space: Arc<dyn AllocationView> = fat.clone();
    assert!(space.map_available());
    let free: u64 = space.free_ranges().unwrap().iter().map(|r| r.length).sum();
    assert!(
        free > 30 * 1024 * 1024 && free < space.volume_len(),
        "{free}"
    );

    let carver = CarveEngine::new(reader.clone(), space, FileSystemType::Fat32, storage());
    let mut progress = 0;
    let (carved, mut report) = carver.carve(&mut |_| progress += 1).unwrap();
    assert!(progress >= 1);
    let expected_hits = corpus
        .files
        .iter()
        .filter(|f| !f["expect"]["live"].as_bool().unwrap_or(false))
        .count();
    assert_eq!(report.hits, expected_hits, "{report:?}");
    assert_eq!(report.rejected, 0, "{report:?}");
    assert_eq!(carved.len(), expected_hits);

    let mut metadata: Vec<RecoveryCandidate> = fat.deleted_files().map(Result::unwrap).collect();
    let extents_of = |c: &RecoveryCandidate| fat.content_extents(c).ok();
    let (kept, merged) = CarveEngine::deduplicate(carved, &mut metadata, &extents_of);
    report.merged_into_metadata = merged;

    for f in &corpus.files {
        let path = f["path"].as_str().unwrap();
        let offset = f["offset"].as_u64().unwrap();
        let expect = &f["expect"];
        let kind = f["type"].as_str().unwrap();
        if expect["live"].as_bool().unwrap_or(false) {
            assert!(
                kept.iter().all(|c| carved_offset(c) != Some(offset)),
                "{path}: allocated files must not be carved from unallocated space"
            );
            continue;
        }
        if expect["merged"].as_bool().unwrap_or(false) {
            assert!(
                kept.iter().all(|c| carved_offset(c) != Some(offset)),
                "{path}: should have merged into its metadata candidate"
            );
            let name_tail = &path.rsplit('/').next().unwrap()[1..];
            let m = metadata
                .iter()
                .find(|m| {
                    m.original_name
                        .as_deref()
                        .unwrap()
                        .to_lowercase()
                        .ends_with(name_tail)
                })
                .unwrap_or_else(|| panic!("{path}: no metadata candidate"));
            assert!(
                m.evidence.diagnostics.iter().any(|d| d
                    .message
                    .contains("Signature carving found the same content")),
                "{path}: {:?}",
                m.evidence.diagnostics
            );
            continue;
        }
        // Orphans: only carving finds them.
        assert!(expect["orphan"].as_bool().unwrap_or(false), "{path}");
        let c = kept
            .iter()
            .find(|c| carved_offset(c) == Some(offset))
            .unwrap_or_else(|| panic!("{path}: not carved at {offset}"));
        assert_eq!(c.evidence.source, CandidateSource::FileCarving);
        let FileSystemObjectId::Carved { type_id, .. } = &c.filesystem_object else {
            panic!("{path}: not a carved object")
        };
        assert_eq!(type_id, kind, "{path}");
        assert!(c.original_name.is_none() && c.original_path.is_none());
        let status = c.evidence.content.validation.as_ref().unwrap().status;
        let expected_status = match expect["status"].as_str().unwrap() {
            "valid" => ValidationStatus::Valid,
            "damaged" => ValidationStatus::Damaged,
            other => panic!("{other}"),
        };
        assert_eq!(
            status, expected_status,
            "{path}: {:?}",
            c.evidence.content.validation
        );
        let (sha, complete) = recover(&carver, c);
        assert!(complete);
        if expect["exact"].as_bool().unwrap() {
            assert_eq!(sha, f["sha256"].as_str().unwrap(), "{path}");
            assert_eq!(c.logical_size, Some(f["size"].as_u64().unwrap()));
            assert!(
                c.health.category >= HealthCategory::VeryGood,
                "{path}: {:?}",
                c.health
            );
            assert!(
                c.health.confidence >= 60 && c.health.confidence <= 85,
                "{path}: {:?}",
                c.health
            );
        } else {
            assert_ne!(sha, f["sha256"].as_str().unwrap(), "{path} cannot be exact");
            assert!(
                c.health.category <= HealthCategory::Poor,
                "{path}: {:?}",
                c.health
            );
            assert!(
                c.health
                    .reasons
                    .iter()
                    .any(|r| !r.positive && r.text.contains("damaged")),
                "{path}: {:?}",
                c.health.reasons
            );
        }
        // Deterministic re-derivation from the short reference.
        let object = carver
            .object_from_reference(&c.filesystem_object.short_reference())
            .unwrap();
        let again = carver.candidate(&object).unwrap();
        assert_eq!(again.filesystem_object, c.filesystem_object);
        assert_eq!(again.health, c.health);
    }
    let expected_merged = corpus
        .files
        .iter()
        .filter(|f| f["expect"]["merged"].as_bool().unwrap_or(false))
        .count();
    assert_eq!(merged, expected_merged, "{report:?}");
    assert_eq!(kept.len(), expected_hits - expected_merged);
}

#[test]
fn whole_volume_and_raw_carving_find_allocated_files_too() {
    let corpus = corpus();
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(corpus.image.clone()));
    let volume = Arc::new(FatVolume::open(reader.clone()).unwrap());
    let fat = Arc::new(FatUndelete::new(volume, storage()));
    let live = corpus
        .files
        .iter()
        .find(|f| f["expect"]["live"].as_bool().unwrap_or(false))
        .unwrap();
    let live_offset = live["offset"].as_u64().unwrap();

    // With the filesystem's map but the whole volume: the live file is
    // found and its allocated clusters cap the score.
    let carver = CarveEngine::new(
        reader.clone(),
        fat.clone(),
        FileSystemType::Fat32,
        storage(),
    )
    .with_options(CarveOptions {
        whole_volume: true,
        ..Default::default()
    });
    let (carved, report) = carver.carve(&mut |_| {}).unwrap();
    assert_eq!(report.hits, corpus.files.len(), "{report:?}");
    let c = carved
        .iter()
        .find(|c| carved_offset(c) == Some(live_offset))
        .unwrap();
    assert!(c.evidence.allocation.clusters_allocated > 0);
    assert!(
        c.health.category <= HealthCategory::VeryPoor,
        "{:?}",
        c.health
    );
    let (sha, _) = recover(&carver, c);
    assert_eq!(sha, live["sha256"].as_str().unwrap());

    // Raw source without any filesystem knowledge: same hits, allocation
    // unknown.
    let raw = CarveEngine::new(
        reader.clone(),
        Arc::new(WholeSource::new(reader.len(), 512)),
        FileSystemType::Unknown,
        storage(),
    );
    let (carved, report) = raw.carve(&mut |_| {}).unwrap();
    assert_eq!(report.hits, corpus.files.len(), "{report:?}");
    assert_eq!(report.bytes_eligible, reader.len());
    let c = carved
        .iter()
        .find(|c| carved_offset(c) == Some(live_offset))
        .unwrap();
    assert!(!c.evidence.allocation.map_available);
    assert!(c.evidence.allocation.clusters_unknown > 0);
    assert!(
        c.health
            .reasons
            .iter()
            .any(|r| r.text.contains("allocation map is unavailable")),
        "{:?}",
        c.health.reasons
    );
}

#[test]
fn corrupted_images_never_panic_the_carver() {
    let original = load_gz("carve/corpus.img.gz");
    let mut rng = Rng::new(0xCA5E);
    for round in 0..40u32 {
        let mut data = original.clone();
        let flips = 1 + rng.below(64) as usize;
        for _ in 0..flips {
            // Hit the first megabyte (structures and files) most of the time.
            let region = if rng.below(4) == 0 {
                data.len()
            } else {
                1024 * 1024
            };
            let pos = rng.below(region as u64) as usize;
            match rng.below(3) {
                0 => data[pos] ^= 1 << rng.below(8),
                1 => data[pos] = 0xFF,
                _ => data[pos] = rng.next_u64() as u8,
            }
        }
        if round % 7 == 0 {
            let keep = 4096 + rng.below((data.len() - 4096) as u64) as usize;
            data.truncate(keep);
        }
        let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(data));
        let space: Arc<dyn AllocationView> = match FatVolume::open(reader.clone()) {
            Ok(v) => Arc::new(FatUndelete::new(Arc::new(v), storage())),
            Err(_) => Arc::new(WholeSource::new(reader.len(), 512)),
        };
        let carver = CarveEngine::new(reader, space, FileSystemType::Fat32, storage())
            .with_options(CarveOptions {
                whole_volume: round % 2 == 0,
                ..Default::default()
            });
        let (candidates, _) = carver.carve(&mut |_| {}).unwrap();
        for c in candidates.iter().take(50) {
            if let Ok(mut content) = carver.open_content(c) {
                let mut sink = Vec::new();
                let _ = std::io::copy(&mut content, &mut sink);
            }
        }
    }
}
