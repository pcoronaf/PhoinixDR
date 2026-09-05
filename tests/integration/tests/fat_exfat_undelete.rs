//! M7 acceptance: FAT12/16/32 and exFAT deletion corpora are detected,
//! assessed and recovered.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use phoinix_block::{BlockReader, MemoryReader};
use phoinix_core::FileSystemType;
use phoinix_fs::{DeletedFileProvider, ProbeRegistry, RecoveryCandidate};
use phoinix_fs_exfat::{ExFatProbe, ExfatUndelete, ExfatVolume};
use phoinix_fs_fat::{FatProbe, FatUndelete, FatVolume};
use phoinix_health::{DeviceKind, HealthCategory, StorageEvidence, ZeroContentAssessment};
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

fn category(name: &str) -> HealthCategory {
    HealthCategory::parse(name).unwrap()
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

/// Whether a recovered path matches the original one. FAT short names lose
/// their first character on deletion (`?MALL.TXT`) and are upper case, so
/// components are compared case-insensitively with `?` as a wildcard for
/// the first character.
fn path_matches(original: &str, recovered: &str) -> bool {
    let a: Vec<&str> = original.split('\\').filter(|p| !p.is_empty()).collect();
    let b: Vec<&str> = recovered.split('\\').filter(|p| !p.is_empty()).collect();
    a.len() == b.len()
        && a.iter().zip(&b).all(|(o, r)| {
            let (o, r) = (o.to_uppercase(), r.to_uppercase());
            o == r || (r.starts_with('?') && o.len() == r.len() && o.get(1..) == r.get(1..))
        })
}

/// Checks every manifest row against the engine's candidates.
fn check_corpus(engine: &dyn DeletedFileProvider, files: &[Value], label: &str) {
    let candidates: Vec<RecoveryCandidate> = engine.deleted_files().map(Result::unwrap).collect();
    assert!(!candidates.is_empty(), "{label}: no candidates");
    let by_path: HashMap<String, &RecoveryCandidate> = candidates
        .iter()
        .map(|c| (c.original_path.clone().unwrap(), c))
        .collect();
    for f in files {
        let path = f["path"].as_str().unwrap();
        let expect = &f["expect"];
        let cand = candidates
            .iter()
            .find(|c| path_matches(path, c.original_path.as_deref().unwrap()))
            .unwrap_or_else(|| panic!("{label}: {path} not found; have {:?}", by_path.keys()));
        assert!(cand.deleted);
        assert_eq!(
            cand.logical_size,
            Some(f["size"].as_u64().unwrap()),
            "{label}: {path} size"
        );
        let original_name = path.rsplit('\\').next().unwrap();
        let name = cand.original_name.as_deref().unwrap();
        assert!(
            path_matches(original_name, name),
            "{label}: {path} name {name:?}"
        );
        if expect["long_name"].as_bool().unwrap_or(false) {
            assert_eq!(
                name, original_name,
                "{label}: long name must be reconstructed exactly"
            );
        }
        if let Some(min) = expect["min"].as_str() {
            assert!(
                cand.health.category >= category(min),
                "{label}: {path} {} < {min}: {:?}",
                cand.health.category,
                cand.health.reasons
            );
        }
        if let Some(max) = expect["max"].as_str() {
            assert!(
                cand.health.category <= category(max),
                "{label}: {path} {} > {max}: {:?}",
                cand.health.category,
                cand.health.reasons
            );
        }
        if let Some(mc) = expect["max_confidence"].as_u64() {
            assert!(
                u64::from(cand.health.confidence) <= mc,
                "{label}: {path} confidence {}",
                cand.health.confidence
            );
        }
        if expect["empty"].as_bool().unwrap_or(false) {
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("not applicable")),
                "{label}: {path}"
            );
        }
        if expect["via_deleted_dir"].as_bool().unwrap_or(false) {
            assert!(
                cand.evidence
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("deleted directory")),
                "{label}: {path}"
            );
        }
        if let Some(t) = expect["type"].as_str() {
            assert_eq!(
                cand.evidence
                    .content
                    .detected_type
                    .as_ref()
                    .map(|d| d.id.as_str()),
                Some(t),
                "{label}: {path}"
            );
        }
        if let Some(v) = expect["validation"].as_str() {
            let status = cand.evidence.content.validation.as_ref().unwrap().status;
            assert_eq!(format!("{status:?}").to_lowercase(), v, "{label}: {path}");
        }
        if expect["reallocated"].as_bool().unwrap_or(false) {
            assert!(
                cand.evidence.allocation.clusters_allocated > 0,
                "{label}: {path} should have reallocated clusters: {:?}",
                cand.evidence.allocation
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("allocated to active filesystem data")),
                "{label}: {path}"
            );
        }
        if expect["heuristic"].as_bool().unwrap_or(false) {
            assert!(
                cand.evidence.extents.heuristic,
                "{label}: {path} should be a heuristic reconstruction: {:?}",
                cand.evidence.extents
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("heuristic")),
                "{label}: {path}"
            );
        }
        if let Some(n) = expect["min_extents"].as_u64() {
            assert!(
                u64::from(cand.evidence.extents.extent_count) >= n,
                "{label}: {path} extents {}",
                cand.evidence.extents.extent_count
            );
        }
        let (sha, complete) = recover(engine, cand);
        assert!(complete, "{label}: {path} not complete");
        if expect["exact"].as_bool().unwrap_or(false) {
            assert_eq!(
                sha,
                f["sha256"].as_str().unwrap(),
                "{label}: {path} must be byte-exact ({:?})",
                cand.health.reasons
            );
        } else {
            assert_ne!(
                sha,
                f["sha256"].as_str().unwrap(),
                "{label}: {path} cannot be exact"
            );
        }
        // Zero assessment must never be a format contradiction for exact files.
        if expect["exact"].as_bool().unwrap_or(false) {
            assert_ne!(
                cand.evidence.content.zero_assessment,
                Some(ZeroContentAssessment::ContradictsFormat),
                "{label}: {path}"
            );
        }
    }
}

#[test]
fn fat_corpora_are_recovered() {
    for variant in ["fat12", "fat16", "fat32"] {
        let image = load_gz(&format!("fat/{variant}.img.gz"));
        let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image.clone()));
        let detection = ProbeRegistry::new()
            .with(Box::new(FatProbe))
            .detect(&*reader);
        let expected_fs = match variant {
            "fat12" => FileSystemType::Fat12,
            "fat16" => FileSystemType::Fat16,
            _ => FileSystemType::Fat32,
        };
        assert_eq!(
            detection.filesystem(),
            expected_fs,
            "{variant}: {detection:?}"
        );
        assert!(detection.best.as_ref().unwrap().confidence >= 85);
        let volume = Arc::new(FatVolume::open(reader.clone()).unwrap());
        assert_eq!(volume.variant().filesystem_type(), expected_fs);
        assert_eq!(
            volume.fat().mirror_consistent,
            Some(true),
            "{variant}: FAT mirror should match"
        );
        let engine = FatUndelete::new(volume, storage());
        let m = manifest(&format!("fat/{variant}.manifest.json"));
        check_corpus(&engine, m["files"].as_array().unwrap(), variant);
        // Deterministic addressing by entry offset.
        let first = engine.deleted_files().map(Result::unwrap).next().unwrap();
        let again = engine.candidate(&first.filesystem_object).unwrap();
        assert_eq!(again.health, first.health);
        let parsed = engine
            .object_from_reference(&first.filesystem_object.short_reference())
            .unwrap();
        assert_eq!(parsed, first.filesystem_object);
        // Source untouched.
        let mem = MemoryReader::new(image.clone());
        let _ = FatVolume::open(Arc::new(mem.clone()))
            .unwrap()
            .walk()
            .unwrap();
        assert_eq!(mem.data(), &image[..]);
    }
}

#[test]
fn exfat_corpus_is_recovered() {
    let image = load_gz("exfat/undelete.img.gz");
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image.clone()));
    let detection = ProbeRegistry::new()
        .with(Box::new(ExFatProbe))
        .detect(&*reader);
    assert_eq!(detection.filesystem(), FileSystemType::ExFat);
    assert!(
        detection.best.as_ref().unwrap().confidence >= 90,
        "{detection:?}"
    );
    let volume = Arc::new(ExfatVolume::open(reader.clone()).unwrap());
    assert_eq!(volume.label(), Some("PHXEXFAT"));
    assert_eq!(volume.boot_checksum_ok(), Some(true));
    assert!(volume.bitmap().is_some());
    let engine = ExfatUndelete::new(volume, storage());
    let m = manifest("exfat/undelete.manifest.json");
    check_corpus(&engine, m["files"].as_array().unwrap(), "exfat");
    // Contiguous (NoFatChain) deleted files keep a known layout.
    let medium = engine
        .deleted_files()
        .map(Result::unwrap)
        .find(|c| c.original_name.as_deref() == Some("medium.bin"))
        .unwrap();
    assert!(
        medium.evidence.extents.chain_known,
        "{:?}",
        medium.evidence.extents
    );
    assert!(medium.health.confidence >= 80, "{:?}", medium.health);
}

#[test]
fn corrupted_fat_structures_never_panic() {
    for name in ["fat/fat32.img.gz", "exfat/undelete.img.gz"] {
        let original = load_gz(name);
        let mut rng = Rng::new(0xFA7);
        for round in 0..120u32 {
            let mut data = original.clone();
            // Corrupt the boot sector, FAT region and the first directory clusters.
            let region = 2 * 1024 * 1024usize.min(data.len() / 2);
            let flips = 1 + rng.below(16) as usize;
            for _ in 0..flips {
                let pos = rng.below(region as u64) as usize;
                match rng.below(3) {
                    0 => data[pos] ^= 1 << rng.below(8),
                    1 => data[pos] = 0xFF,
                    _ => data[pos] = rng.next_u64() as u8,
                }
            }
            if round % 5 == 0 {
                let keep = 512 + rng.below((data.len() - 512) as u64) as usize;
                data.truncate(keep);
            }
            let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(data));
            if name.starts_with("fat") {
                if let Ok(v) = FatVolume::open(reader) {
                    let engine = FatUndelete::new(Arc::new(v), storage());
                    for c in engine.deleted_files().flatten().take(200) {
                        if let Ok(mut content) = engine.open_content(&c) {
                            let mut sink = Vec::new();
                            let _ = std::io::copy(&mut content, &mut sink);
                        }
                    }
                }
            } else if let Ok(v) = ExfatVolume::open(reader) {
                let engine = ExfatUndelete::new(Arc::new(v), storage());
                for c in engine.deleted_files().flatten().take(200) {
                    if let Ok(mut content) = engine.open_content(&c) {
                        let mut sink = Vec::new();
                        let _ = std::io::copy(&mut content, &mut sink);
                    }
                }
            }
        }
    }
}
