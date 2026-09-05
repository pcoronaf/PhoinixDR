//! M10 acceptance: ext2/3/4 deletion corpora are detected, assessed and
//! recovered, with journal-assisted metadata recovery on ext3/ext4.

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
use phoinix_core::FileSystemType;
use phoinix_fs::{
    AllocationView, DeletedFileProvider, FileSystemObjectId, ProbeRegistry, RecoveryCandidate,
};
use phoinix_fs_ext::{ExtProbe, ExtUndelete, ExtVolume, LayoutSource};
use phoinix_health::{
    CandidateSource, DeviceKind, HealthCategory, StorageEvidence, ZeroContentAssessment,
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

fn open(flavour: &str) -> (Vec<u8>, Arc<ExtVolume>, Value) {
    let image = load_gz(&format!("ext/{flavour}.img.gz"));
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image.clone()));
    let detection = ProbeRegistry::new()
        .with(Box::new(ExtProbe))
        .detect(&*reader);
    assert_eq!(detection.filesystem(), FileSystemType::Ext, "{detection:?}");
    assert!(
        detection.best.as_ref().unwrap().confidence >= 80,
        "{detection:?}"
    );
    let volume = Arc::new(ExtVolume::open(reader).unwrap());
    let m = manifest(&format!("ext/{flavour}.manifest.json"));
    assert_eq!(volume.superblock().flavour(), flavour);
    assert_eq!(
        volume.superblock().volume_name,
        m["label"].as_str().unwrap()
    );
    (image, volume, m)
}

fn inode_of(c: &RecoveryCandidate) -> u32 {
    match c.filesystem_object {
        FileSystemObjectId::Ext { inode, .. } => inode,
        ref other => panic!("not an ext object: {other}"),
    }
}

/// Checks every manifest row of a journaled image against the engine.
fn check_journaled_corpus(engine: &ExtUndelete, m: &Value, label: &str) {
    let candidates: Vec<RecoveryCandidate> = engine.deleted_files().map(Result::unwrap).collect();
    assert!(!candidates.is_empty(), "{label}: no candidates");
    let paths: Vec<String> = candidates
        .iter()
        .map(|c| c.original_path.clone().unwrap_or_default())
        .collect();
    for f in m["files"].as_array().unwrap() {
        let path = f["path"].as_str().unwrap();
        let inode = f["inode"].as_u64().unwrap() as u32;
        let expect = &f["expect"];
        let cand = candidates
            .iter()
            .find(|c| c.original_path.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("{label}: {path} not found; have {paths:?}"));
        assert!(cand.deleted);
        assert_eq!(inode_of(cand), inode, "{label}: {path} inode");
        assert_eq!(
            cand.original_name.as_deref(),
            Some(path.rsplit('/').next().unwrap()),
            "{label}: {path} name"
        );
        assert_eq!(
            cand.logical_size,
            Some(f["size"].as_u64().unwrap()),
            "{label}: {path} size"
        );
        assert!(cand.timestamps.modified.is_some(), "{label}: {path} mtime");
        assert!(cand.timestamps.created.is_some(), "{label}: {path} crtime");
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
        if expect["empty"].as_bool().unwrap_or(false) {
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("not applicable")),
                "{label}: {path}: {:?}",
                cand.health.reasons
            );
        } else {
            // Modern kernels clear the layout on deletion: it must come
            // from the journal.
            assert_eq!(
                cand.evidence.source,
                CandidateSource::Journal,
                "{label}: {path}: {:?}",
                cand.evidence.diagnostics
            );
            assert!(
                cand.evidence
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("journal transaction")),
                "{label}: {path}: {:?}",
                cand.evidence.diagnostics
            );
            let d = engine.deleted_inode(inode).unwrap();
            assert!(
                matches!(
                    d.layout.as_ref().unwrap().source,
                    LayoutSource::Journal {
                        checksum_ok: Some(true) | None,
                        ..
                    }
                ),
                "{label}: {path}: {:?}",
                d.layout.as_ref().map(|l| &l.source)
            );
            assert!(!cand.evidence.extents.stale, "{label}: {path}");
            assert!(cand.evidence.extents.complete, "{label}: {path}");
        }
        if let Some(n) = f["extents"].as_u64()
            && expect["exact"].as_bool().unwrap_or(false)
        {
            assert_eq!(
                u64::from(cand.evidence.extents.extent_count),
                n,
                "{label}: {path} extents"
            );
        }
        if expect["sparse"].as_bool().unwrap_or(false) {
            assert!(cand.evidence.extents.sparse, "{label}: {path}");
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
                "{label}: {path}: {:?}",
                cand.evidence.allocation
            );
            assert!(
                cand.health
                    .reasons
                    .iter()
                    .any(|r| r.text.contains("allocated to active filesystem data")),
                "{label}: {path}"
            );
            assert!(cand.path_uncertain, "{label}: {path}");
        } else {
            assert!(!cand.path_uncertain, "{label}: {path}");
            assert_eq!(
                cand.evidence.allocation.clusters_allocated, 0,
                "{label}: {path}: {:?}",
                cand.evidence.allocation
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
            assert_ne!(
                cand.evidence.content.zero_assessment,
                Some(ZeroContentAssessment::ContradictsFormat),
                "{label}: {path}"
            );
        } else {
            assert_ne!(
                sha,
                f["sha256"].as_str().unwrap(),
                "{label}: {path} cannot be exact"
            );
        }
        // Extents reported for deduplication match the recovered size.
        let extents = engine.content_extents(cand).unwrap();
        let covered: u64 = extents.iter().map(|e| e.length).sum();
        if cand.evidence.extents.sparse {
            assert!(covered < cand.logical_size.unwrap(), "{label}: {path}");
        } else {
            assert_eq!(covered, cand.logical_size.unwrap(), "{label}: {path}");
        }
    }
    for a in m["absent"].as_array().unwrap() {
        let path = a["path"].as_str().unwrap();
        assert!(
            !paths.iter().any(|p| p == path),
            "{label}: {path} must not be a candidate ({})",
            a["reason"]
        );
    }
}

#[test]
fn ext4_and_ext3_corpora_are_recovered_through_the_journal() {
    for flavour in ["ext4", "ext3"] {
        let (image, volume, m) = open(flavour);
        assert!(m["journaled"].as_bool().unwrap());
        let journal = volume.journal().expect("journal");
        assert!(
            journal.info().logged_blocks > 0,
            "{flavour}: {:?}",
            journal.info()
        );
        let engine = ExtUndelete::new(volume.clone(), storage());
        check_journaled_corpus(&engine, &m, flavour);

        // Deterministic addressing by inode number, with and without the
        // generation.
        let first = engine.deleted_files().map(Result::unwrap).next().unwrap();
        let again = engine.candidate(&first.filesystem_object).unwrap();
        assert_eq!(again.health, first.health);
        let parsed = engine
            .object_from_reference(&first.filesystem_object.short_reference())
            .unwrap();
        assert_eq!(
            inode_of(&engine.candidate(&parsed).unwrap()),
            inode_of(&first)
        );
        let wrong = FileSystemObjectId::Ext {
            inode: inode_of(&first),
            generation: 1,
        };
        assert!(engine.candidate(&wrong).is_err());

        // The allocation view agrees with the superblock's free count.
        let free: u64 = engine.free_ranges().unwrap().iter().map(|r| r.length).sum();
        let sb = volume.superblock();
        assert_eq!(
            free / u64::from(sb.block_size),
            sb.free_blocks,
            "{flavour}: free blocks"
        );
        assert_eq!(
            AllocationView::volume_len(&engine),
            sb.blocks_count * u64::from(sb.block_size)
        );

        // Source untouched.
        let mem = MemoryReader::new(image.clone());
        let v = ExtVolume::open(Arc::new(mem.clone())).unwrap();
        let _ = ExtUndelete::new(Arc::new(v), storage())
            .deleted_files()
            .count();
        assert_eq!(mem.data(), &image[..]);
    }
}

#[test]
fn ext4_journal_tags_are_checksummed() {
    let (_, volume, _) = open("ext4");
    let journal = volume.journal().unwrap();
    let with_checksum = journal.info().incompat & 0x10 != 0;
    assert!(with_checksum, "{:?}", journal.info());
    let engine = ExtUndelete::new(volume.clone(), storage());
    for d in engine.deleted_inodes().unwrap() {
        if let Some(LayoutSource::Journal { checksum_ok, .. }) =
            d.layout.as_ref().map(|l| &l.source)
        {
            assert_eq!(*checksum_ok, Some(true), "inode {}", d.number);
        }
    }
}

/// Without a journal only the inode table speaks: deletion times survive,
/// but the kernel cleared sizes, block maps and even the directory
/// entries, so nothing can be located or named.
#[test]
fn ext2_corpus_yields_deleted_inodes_without_layouts() {
    let (_, volume, m) = open("ext2");
    assert!(!m["journaled"].as_bool().unwrap());
    assert!(volume.journal().is_none());
    assert_eq!(volume.superblock().block_size, 1024);
    assert!(volume.groups().len() > 1, "several block groups expected");
    let engine = ExtUndelete::new(volume, storage());
    let candidates: Vec<RecoveryCandidate> = engine.deleted_files().map(Result::unwrap).collect();
    for f in m["files"].as_array().unwrap() {
        let path = f["path"].as_str().unwrap();
        let inode = f["inode"].as_u64().unwrap() as u32;
        let Some(cand) = candidates.iter().find(|c| inode_of(c) == inode) else {
            // The reused inode is alive again and has no journal history.
            assert_eq!(f["scenario"], "D", "{path} (inode {inode}) missing");
            continue;
        };
        assert!(cand.deleted);
        assert_eq!(cand.evidence.source, CandidateSource::FilesystemMetadata);
        assert!(
            cand.logical_size.is_none(),
            "{path}: {:?}",
            cand.logical_size
        );
        assert_eq!(
            cand.health.category,
            HealthCategory::Unrecoverable,
            "{path}"
        );
        assert!(
            cand.evidence
                .diagnostics
                .iter()
                .any(|d| d.message.contains("Deleted at")),
            "{path}: {:?}",
            cand.evidence.diagnostics
        );
        assert!(
            cand.evidence
                .diagnostics
                .iter()
                .any(|d| d.message.contains("no journal")),
            "{path}: {:?}",
            cand.evidence.diagnostics
        );
        assert!(engine.open_content(cand).is_err(), "{path}");
        assert!(engine.content_extents(cand).unwrap().is_empty());
    }
}

#[test]
fn corrupted_ext_structures_never_panic() {
    for name in ["ext/ext4.img.gz", "ext/ext2.img.gz"] {
        let original = load_gz(name);
        let mut rng = Rng::new(0xE47);
        for round in 0..60u32 {
            let mut data = original.clone();
            // Superblock, descriptors, bitmaps, inode tables and the start
            // of the journal all live in the first megabytes.
            let region = (6 * 1024 * 1024usize).min(data.len() / 2);
            let flips = 1 + rng.below(24) as usize;
            for _ in 0..flips {
                let pos = rng.below(region as u64) as usize;
                match rng.below(3) {
                    0 => data[pos] ^= 1 << rng.below(8),
                    1 => data[pos] = 0xFF,
                    _ => data[pos] = rng.next_u64() as u8,
                }
            }
            if round % 5 == 0 {
                let keep = 2048 + rng.below((data.len() - 2048) as u64) as usize;
                data.truncate(keep);
            }
            let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(data));
            if let Ok(v) = ExtVolume::open(reader) {
                let engine = ExtUndelete::new(Arc::new(v), storage());
                for c in engine.deleted_files().flatten().take(100) {
                    if let Ok(mut content) = engine.open_content(&c) {
                        let mut sink = Vec::new();
                        let _ = std::io::copy(&mut content, &mut sink);
                    }
                }
                let _ = engine.free_ranges();
            }
        }
    }
}
