//! M4 acceptance: the deletion corpus (A–H, V) is detected, assessed and
//! recovered with SHA-256 verification.

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
use phoinix_fs::{DeletedFileProvider, FileSystemObjectId, RecoveryCandidate};
use phoinix_fs_ntfs::{NtfsError, NtfsUndelete, NtfsVolume};
use phoinix_health::validate::ValidationStatus;
use phoinix_health::{DeviceKind, HealthCategory, StorageEvidence};
use phoinix_integration_tests::{load_gz, manifest};
use phoinix_recovery::{RecoveryError, RecoveryRequest, RecoveryWriter};
use serde_json::Value;

struct Corpus {
    image: Vec<u8>,
    volume: Arc<NtfsVolume>,
    reader: MemoryReader,
    files: Vec<Value>,
}

fn corpus() -> Corpus {
    let image = load_gz("ntfs/undelete.img.gz");
    let reader = MemoryReader::new(image.clone());
    let volume = Arc::new(NtfsVolume::open(Arc::new(reader.clone())).unwrap());
    let m = manifest("ntfs/undelete.manifest.json");
    let files = m["files"].as_array().unwrap().clone();
    Corpus {
        image,
        volume,
        reader,
        files,
    }
}

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

/// Recovers `candidate` into a fresh temp dir and returns (sha256 hex, complete).
fn recover(engine: &NtfsUndelete, candidate: &RecoveryCandidate) -> (String, bool) {
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

#[test]
fn corpus_is_detected_assessed_and_recovered_per_manifest() {
    let c = corpus();
    let engine = NtfsUndelete::new(c.volume.clone(), storage());
    let by_record: HashMap<u64, RecoveryCandidate> = engine
        .deleted_files()
        .map(Result::unwrap)
        .filter_map(|cand| match &cand.filesystem_object {
            FileSystemObjectId::Ntfs {
                record,
                stream: None,
                ..
            } => Some((*record, cand)),
            _ => None,
        })
        .collect();
    assert!(
        by_record.len() >= 20,
        "found {} candidates",
        by_record.len()
    );

    for f in &c.files {
        let path = f["path"].as_str().unwrap();
        let record = f["record"].as_u64().unwrap();
        let scenario = f["scenario"].as_str().unwrap();
        let expect = &f["expect"];
        if scenario == "F" {
            continue; // covered by `malformed_records_are_diagnosed_not_fatal`
        }
        let cand = by_record
            .get(&record)
            .unwrap_or_else(|| panic!("{path} (record {record}) not among candidates"));
        assert!(cand.deleted);
        assert_eq!(
            cand.logical_size,
            Some(f["size"].as_u64().unwrap()),
            "{path} size"
        );

        // Path expectations.
        let uncertain = expect["path_uncertain"].as_bool().unwrap_or(false);
        assert_eq!(
            cand.path_uncertain, uncertain,
            "{path} uncertainty: {:?}",
            cand.original_path
        );
        if uncertain {
            assert!(
                cand.original_path.as_deref().unwrap().starts_with("\\?\\"),
                "{path}"
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("reused")),
                "{path} should explain the stale parent"
            );
        } else {
            assert_eq!(cand.original_path.as_deref(), Some(path), "{path}");
        }
        if expect["via_deleted_dir"].as_bool().unwrap_or(false) {
            assert!(
                cand.evidence
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("deleted directory")),
                "{path}"
            );
        }

        // Health expectations.
        if let Some(min) = expect["min"].as_str() {
            assert!(
                cand.health.category >= category(min),
                "{path}: {} < {min} ({:?})",
                cand.health.category,
                cand.health.reasons
            );
        }
        if let Some(max) = expect["max"].as_str() {
            assert!(
                cand.health.category <= category(max),
                "{path}: {} > {max}",
                cand.health.category
            );
        }
        if expect["resident"].as_bool().unwrap_or(false) {
            assert!(cand.evidence.extents.resident, "{path}");
        }
        if let Some(n) = expect["max_extents"].as_u64() {
            assert!(u64::from(cand.evidence.extents.extent_count) <= n, "{path}");
        }
        if let Some(n) = expect["min_extents"].as_u64() {
            assert!(
                u64::from(cand.evidence.extents.extent_count) >= n,
                "{path}: {} extents",
                cand.evidence.extents.extent_count
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("fragments")),
                "{path} should mention fragmentation"
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
                "{path}"
            );
        }
        if let Some(v) = expect["validation"].as_str() {
            let status = cand.evidence.content.validation.as_ref().unwrap().status;
            assert_eq!(format!("{status:?}").to_lowercase(), v, "{path}");
        }
        if let Some(pct) = expect["allocated_percent"].as_u64() {
            let a = &cand.evidence.allocation;
            assert_eq!(
                a.clusters_allocated,
                expect["allocated_clusters"].as_u64().unwrap(),
                "{path}"
            );
            assert_eq!(
                a.clusters_total,
                expect["total_clusters"].as_u64().unwrap(),
                "{path}"
            );
            let text: Vec<&str> = cand
                .health
                .reasons
                .iter()
                .map(|r| r.text.as_str())
                .collect();
            assert!(
                text.iter()
                    .any(|t| t.contains("allocated to active filesystem data")),
                "{path}: {text:?}"
            );
            assert!(
                !text
                    .iter()
                    .any(|t| t.to_lowercase().contains("overwritten")),
                "{path} must not claim overwrite"
            );
            if pct >= 50 {
                assert!(cand.health.likelihood <= 34, "{path}");
            }
        }
        if scenario == "G" {
            // The header is gone with the rest: no type can be detected, and
            // any validation that does run must not pass.
            if let Some(v) = &cand.evidence.content.validation {
                assert_ne!(v.status, ValidationStatus::Valid, "{path}");
            }
            assert!(
                cand.evidence.allocation.clusters_allocated == 0,
                "{path}: bitmap must still say free"
            );
            assert!(
                cand.evidence.content.zero_block_ratio.unwrap() > 0.5,
                "{path}"
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("zero-filled")),
                "{path}"
            );
        }

        // Recovery: byte-exact where expected, honestly different otherwise.
        let (sha, complete) = recover(&engine, cand);
        assert!(complete, "{path} should write every byte");
        if expect["exact"].as_bool().unwrap_or(false) {
            assert_eq!(
                sha,
                f["sha256"].as_str().unwrap(),
                "{path} must recover byte-exact"
            );
        } else {
            assert_ne!(
                sha,
                f["sha256"].as_str().unwrap(),
                "{path} cannot be exact; the writer must not pretend otherwise"
            );
        }
    }

    // D: health declines monotonically with the reallocated share.
    let mut d: Vec<(u64, u8)> = c
        .files
        .iter()
        .filter(|f| f["scenario"] == "D")
        .map(|f| {
            (
                f["expect"]["allocated_percent"].as_u64().unwrap(),
                by_record[&f["record"].as_u64().unwrap()].health.likelihood,
            )
        })
        .collect();
    d.sort();
    assert!(
        d.windows(2).all(|w| w[1].1 <= w[0].1),
        "D likelihoods not monotonic: {d:?}"
    );
    assert!(d.first().unwrap().1 > d.last().unwrap().1);

    // Nothing wrote to the source.
    assert_eq!(c.reader.data(), &c.image[..]);
}

#[test]
fn malformed_records_are_diagnosed_not_fatal() {
    let c = corpus();
    let engine = NtfsUndelete::new(c.volume.clone(), storage());
    let all: Vec<RecoveryCandidate> = engine.deleted_files().map(Result::unwrap).collect();
    let find = |record: u64| {
        all.iter().find(|x| matches!(x.filesystem_object, FileSystemObjectId::Ntfs { record: r, .. } if r == record))
    };
    for f in c.files.iter().filter(|f| f["scenario"] == "F") {
        let record = f["record"].as_u64().unwrap();
        match f["expect"]["corruption"].as_str().unwrap() {
            "usa" => {
                assert!(
                    matches!(c.volume.file(record), Err(NtfsError::FixupMismatch { .. })),
                    "usa"
                );
                assert!(find(record).is_none());
            }
            "attr" => {
                // The record parses but its attributes are unusable: no candidate, typed diagnostics.
                let file = c.volume.file(record).unwrap();
                assert!(file.names.is_empty() && file.streams.is_empty());
                assert!(!file.diagnostics.is_empty());
                assert!(find(record).is_none());
            }
            "runlist" => {
                let cand = find(record).expect("runlist candidate");
                assert!(!cand.evidence.extents.complete);
                assert_eq!(
                    cand.health.category,
                    HealthCategory::Unrecoverable,
                    "{:?}",
                    cand.health
                );
                assert!(
                    cand.health
                        .reasons
                        .iter()
                        .any(|r| r.text.contains("No extent"))
                );
            }
            "namelen" => {
                let cand = find(record).expect("namelen candidate");
                assert!(cand.original_name.is_none());
                assert!(!cand.evidence.metadata.filename_available);
                assert!(
                    cand.evidence
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("name length"))
                );
                // The data itself is intact.
                let (sha, complete) = recover(&engine, cand);
                assert!(complete);
                assert_eq!(sha, f["sha256"].as_str().unwrap());
            }
            other => panic!("unknown corruption {other}"),
        }
    }
    // Enumeration continued past the corrupt records.
    let max_record = all
        .iter()
        .map(|x| match x.filesystem_object {
            FileSystemObjectId::Ntfs { record, .. } => record,
            _ => 0,
        })
        .max()
        .unwrap();
    assert!(max_record > 100, "scan stopped early at {max_record}");
}

#[test]
fn candidate_addressing_is_deterministic() {
    let c = corpus();
    let engine = NtfsUndelete::new(c.volume.clone(), storage());
    let first = engine
        .deleted_files()
        .map(Result::unwrap)
        .find(|x| x.original_name.as_deref() == Some("photo.jpg"))
        .unwrap();
    let again = engine.candidate(&first.filesystem_object).unwrap();
    assert_eq!(again.health, first.health);
    assert_eq!(again.evidence, first.evidence);
    assert_eq!(again.original_path, first.original_path);
    assert!(
        engine
            .candidate(&FileSystemObjectId::Ntfs {
                record: 5,
                sequence: 0,
                stream: None
            })
            .is_err(),
        "root is not a candidate"
    );
}

#[test]
fn recovery_refuses_destinations_that_would_overwrite_the_source_image() {
    let c = corpus();
    let engine = NtfsUndelete::new(c.volume.clone(), storage());
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("undelete.img");
    std::fs::write(&image, &c.image).unwrap();
    let err = RecoveryWriter::new(&engine, &image, RecoveryRequest::new(&image)).unwrap_err();
    assert!(matches!(err, RecoveryError::DangerousDestination(_)));
    // Next to the image is fine.
    assert!(
        RecoveryWriter::new(
            &engine,
            &image,
            RecoveryRequest::new(dir.path().join("out"))
        )
        .is_ok()
    );
}

#[test]
fn source_length_is_respected_when_reading_candidates() {
    // A truncated copy of the image: every candidate read must fail cleanly
    // rather than read beyond the source.
    let c = corpus();
    let cut = c.image[..c.image.len() / 2].to_vec();
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(cut));
    let volume = Arc::new(NtfsVolume::open(reader).unwrap());
    let engine = NtfsUndelete::new(volume, storage());
    for cand in engine.deleted_files().map(Result::unwrap) {
        if let Ok(mut content) = engine.open_content(&cand) {
            let mut sink = Vec::new();
            let _ = std::io::copy(&mut content, &mut sink);
        }
    }
}
