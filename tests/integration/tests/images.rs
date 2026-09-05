//! M11 acceptance: every image container reads back the FAT12 corpus
//! byte-exact, stored hashes verify, acquisition metadata is exposed, the
//! filesystem engines work through the container, and recovery reports
//! are written.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_device::open_source_described;
use phoinix_fs::DeletedFileProvider;
use phoinix_fs_fat::{FatUndelete, FatVolume};
use phoinix_health::{DeviceKind, StorageEvidence};
use phoinix_image::{ImageError, ImageFormat, detect_format, open_image, verify};
use phoinix_integration_tests::{Rng, load_gz, manifest, unpack_dir};
use phoinix_recovery::{CaseMetadata, RecoveryReport, ReportFormat};
use phoinix_session::Workspace;
use phoinix_session::dto::{RecoverEvent, RecoverRequest, ScanMode, ScanRequest};
use serde_json::Value;
use sha2::{Digest, Sha256};

struct Corpus {
    dir: tempfile::TempDir,
    raw: Vec<u8>,
    manifest: Value,
}

fn corpus() -> Corpus {
    let dir = tempfile::tempdir().unwrap();
    let _ = unpack_dir("images", dir.path());
    let manifest = manifest("images/manifest.json");
    let raw = load_gz("fat/fat12.img.gz");
    assert_eq!(
        hex::encode(Sha256::digest(&raw)),
        manifest["raw_sha256"].as_str().unwrap()
    );
    Corpus { dir, raw, manifest }
}

fn read_all(reader: &dyn BlockReader) -> Vec<u8> {
    let mut out = vec![0u8; reader.len() as usize];
    reader.read_exact_at(0, &mut out).unwrap();
    out
}

#[test]
fn every_container_reads_back_the_raw_bytes() {
    let c = corpus();
    for image in c.manifest["images"].as_array().unwrap() {
        let path = c.dir.path().join(image["open"].as_str().unwrap());
        let label = image["open"].as_str().unwrap();
        let format: ImageFormat = serde_json::from_value(image["format"].clone()).unwrap();
        assert_eq!(detect_format(&path).unwrap(), format, "{label}");
        let opened = open_image(&path).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(opened.info.format, format, "{label}");
        assert_eq!(
            opened.info.variant,
            image["variant"].as_str().unwrap(),
            "{label}"
        );
        assert_eq!(
            opened.info.segments.len(),
            image["segments"].as_u64().unwrap() as usize,
            "{label}: segments {:?}",
            opened.info.segments
        );
        assert!(
            opened.info.diagnostics.is_empty(),
            "{label}: {:?}",
            opened.info.diagnostics
        );
        let padded = image["padded"].as_bool().unwrap_or(false);
        let data = read_all(&*opened.reader);
        if padded {
            // VHD sizes are rounded up to a CHS geometry; the tail is zeros.
            assert!(data.len() >= c.raw.len(), "{label}");
            assert_eq!(&data[..c.raw.len()], &c.raw[..], "{label}");
            assert!(data[c.raw.len()..].iter().all(|b| *b == 0), "{label}");
        } else {
            assert_eq!(opened.info.size, c.raw.len() as u64, "{label}");
            assert_eq!(
                hex::encode(Sha256::digest(&data)),
                c.manifest["raw_sha256"].as_str().unwrap(),
                "{label}: content differs"
            );
        }
        // Random unaligned reads agree with the raw bytes.
        let mut rng = Rng::new(0x1a5e);
        for _ in 0..200 {
            let len = 1 + rng.below(70_000) as usize;
            let offset = rng.below((c.raw.len() - len) as u64) as usize;
            let mut buf = vec![0u8; len];
            opened
                .reader
                .read_exact_at(offset as u64, &mut buf)
                .unwrap();
            assert_eq!(
                buf,
                &c.raw[offset..offset + len],
                "{label}: read at {offset}"
            );
        }
        // Reads beyond the end are refused, never short.
        let mut one = [0u8; 1];
        assert!(
            opened
                .reader
                .read_at(opened.reader.len(), &mut one)
                .is_err()
        );
        // Hash verification against the stored hashes (EWF) or none.
        let v = verify(&*opened.reader, &opened.info.stored_hashes, &mut |_, _| {
            true
        })
        .unwrap();
        if image["stored_md5"].as_bool().unwrap_or(false) {
            assert_eq!(
                opened.info.stored_hashes.md5.as_deref(),
                Some(c.manifest["raw_md5"].as_str().unwrap()),
                "{label}"
            );
            assert_eq!(v.verified(), Some(true), "{label}: {v:?}");
        } else {
            assert_eq!(v.verified(), None, "{label}");
        }
        if !padded {
            assert_eq!(v.sha256, c.manifest["raw_sha256"].as_str().unwrap());
            assert_eq!(v.sha1, c.manifest["raw_sha1"].as_str().unwrap());
        }
        // Acquisition metadata.
        if let Some(acq) = image["acquisition"].as_object() {
            let a = opened.info.acquisition.as_ref().unwrap();
            let fields = [
                ("case_number", &a.case_number),
                ("evidence_number", &a.evidence_number),
                ("examiner", &a.examiner),
                ("description", &a.description),
                ("notes", &a.notes),
            ];
            for (k, v) in fields {
                if let Some(want) = acq.get(k) {
                    assert_eq!(v.as_deref(), want.as_str(), "{label}: {k}");
                }
            }
            assert!(a.acquisition_date.as_deref().unwrap().contains('T'));
            assert_eq!(a.operating_system.as_deref(), Some("Linux"));
        }
    }
}

#[test]
fn filesystem_engines_work_through_containers() {
    let c = corpus();
    for name in [
        "e01.E01",
        "split.E01",
        "stream.vmdk",
        "disk.vhdx",
        "raw.001",
    ] {
        let path = c.dir.path().join(name);
        let opened = open_source_described(&path).unwrap();
        let reader: Arc<dyn BlockReader> = opened.reader;
        if name == "raw.001" {
            assert_eq!(
                opened.container.as_ref().unwrap().format,
                ImageFormat::SplitRaw
            );
        }
        let volume = Arc::new(FatVolume::open(reader).unwrap_or_else(|e| panic!("{name}: {e}")));
        assert_eq!(volume.variant().filesystem_type(), FileSystemType::Fat12);
        let engine = FatUndelete::new(
            volume,
            StorageEvidence {
                device_kind: DeviceKind::Image,
                ..Default::default()
            },
        );
        let candidates: Vec<_> = engine.deleted_files().map(Result::unwrap).collect();
        let photo = candidates
            .iter()
            .find(|c| {
                c.original_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().ends_with("hoto.jpg"))
            })
            .unwrap_or_else(|| panic!("{name}: photo.jpg not found"));
        let m = manifest("fat/fat12.manifest.json");
        let want = m["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"].as_str().unwrap().ends_with("photo.jpg"))
            .unwrap()["sha256"]
            .as_str()
            .unwrap();
        let mut content = engine.open_content(photo).unwrap();
        let mut bytes = Vec::new();
        std::io::copy(&mut content, &mut bytes).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&bytes)), want, "{name}");
    }
}

#[test]
fn unsupported_and_damaged_containers_are_refused_cleanly() {
    let c = corpus();
    // EWF2 signature: refused with a clear message, never parsed.
    let ex01 = c.dir.path().join("fake.Ex01");
    let mut data = vec![0u8; 4096];
    data[..8].copy_from_slice(b"EVF2\x0d\x0a\x81\x00");
    std::fs::write(&ex01, &data).unwrap();
    assert!(matches!(
        open_image(&ex01),
        Err(ImageError::Unsupported(m)) if m.contains("Ex01")
    ));
    // A differencing VHD needs its parent.
    let fixed = std::fs::read(c.dir.path().join("fixed.vhd")).unwrap();
    let mut diff = fixed.clone();
    let footer = diff.len() - 512;
    diff[footer + 60..footer + 64].copy_from_slice(&4u32.to_be_bytes());
    let diff_path = c.dir.path().join("diff.vhd");
    std::fs::write(&diff_path, &diff).unwrap();
    assert!(matches!(
        open_image(&diff_path),
        Err(ImageError::Unsupported(m)) if m.contains("differencing")
    ));
    // A missing EWF segment is reported by name.
    let missing = c.dir.path().join("split.E04");
    let saved = std::fs::read(&missing).unwrap();
    std::fs::remove_file(&missing).unwrap();
    let opened = open_image(&c.dir.path().join("split.E01")).unwrap();
    assert!(
        opened
            .info
            .diagnostics
            .iter()
            .any(|d| d.contains("missing")),
        "{:?}",
        opened.info.diagnostics
    );
    let mut buf = vec![0u8; 4096];
    assert!(
        opened
            .reader
            .read_at(opened.reader.len() - 4096, &mut buf)
            .is_err()
    );
    std::fs::write(&missing, saved).unwrap();
    // Corrupting compressed chunks surfaces as read errors or checksum
    // diagnostics, never as a panic and never as silently wrong data
    // passing the hash check.
    let original = std::fs::read(c.dir.path().join("e01.E01")).unwrap();
    let mut rng = Rng::new(0xE01);
    let path = c.dir.path().join("damaged.E01");
    let mut ever_failed = false;
    for round in 0..60u32 {
        let mut data = original.clone();
        let flips = 1 + rng.below(12) as usize;
        for _ in 0..flips {
            let pos = rng.below(data.len() as u64) as usize;
            data[pos] ^= 1 << rng.below(8);
        }
        if round % 7 == 0 {
            data.truncate(1024 + rng.below((data.len() - 1024) as u64) as usize);
        }
        std::fs::write(&path, &data).unwrap();
        if let Ok(img) = open_image(&path) {
            let stored = img.info.stored_hashes.clone();
            match verify(&*img.reader, &stored, &mut |_, _| true) {
                Ok(v) => {
                    if v.verified() == Some(true) {
                        // Damage outside the data (header, table2, …) is fine.
                        continue;
                    }
                    ever_failed = true;
                }
                Err(_) => ever_failed = true,
            }
        } else {
            ever_failed = true;
        }
    }
    assert!(ever_failed);
}

#[test]
fn recovery_reports_are_written_through_the_service_layer() {
    let c = corpus();
    let sessions = tempfile::tempdir().unwrap();
    let ws = Workspace::new(sessions.path().join("sessions"));
    let e01 = c.dir.path().join("e01.E01");
    let info = ws.inspect(&e01).unwrap();
    let container = info.container.as_ref().unwrap();
    assert_eq!(container.format, ImageFormat::Ewf);
    assert_eq!(
        container
            .acquisition
            .as_ref()
            .unwrap()
            .case_number
            .as_deref(),
        Some("PHX-011")
    );
    let mut ticks = 0;
    let v = ws
        .verify_source(&e01, &mut |_, _| {
            ticks += 1;
            true
        })
        .unwrap();
    assert!(ticks >= 1);
    assert_eq!(v.verified(), Some(true));

    let outcome = ws.scan(
        &ScanRequest {
            source: e01.clone(),
            partition: None,
            volume: None,
            mode: ScanMode::Quick,
            examine_content: true,
            carve: Default::default(),
        },
        &mut |_| {},
    );
    let session = ws.set_current(outcome.session.unwrap());
    assert_eq!(session.container.as_ref().unwrap().format, ImageFormat::Ewf);
    let ids: Vec<_> = session.candidates.iter().take(3).map(|c| c.id).collect();
    let dest = sessions.path().join("out");
    let report_path = sessions.path().join("report.html");
    let req = RecoverRequest {
        candidates: ids.clone(),
        destination: dest.clone(),
        preserve_tree: false,
        preserve_timestamps: true,
        hash: true,
        overwrite: false,
        allow_same_device: false,
        case: Some(CaseMetadata {
            examiner: Some("Integration Test".into()),
            ..Default::default()
        }),
        report: Some(report_path.clone()),
        verify_source: true,
    };
    let mut events = Vec::new();
    let items = ws.recover(&req, &mut |e| events.push(e)).unwrap();
    assert_eq!(items.len(), ids.len());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RecoverEvent::Verifying { .. }))
    );
    let finished = events
        .iter()
        .find_map(|e| match e {
            RecoverEvent::Finished { report, .. } => Some(report.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(finished.as_deref(), Some(report_path.as_path()));
    let html = std::fs::read_to_string(&report_path).unwrap();
    assert!(html.contains("PHX-011") && html.contains("Integration Test"));
    assert!(html.contains("stored hashes match"));
    for item in &items {
        let sha = item.result.as_ref().unwrap().sha256.as_ref().unwrap();
        assert!(html.contains(sha.as_str()));
    }
    // The same report as JSON round-trips.
    let json_path = sessions.path().join("report.json");
    let req = RecoverRequest {
        report: Some(json_path.clone()),
        verify_source: false,
        overwrite: true,
        ..req
    };
    ws.recover(&req, &mut |_| {}).unwrap();
    let parsed: RecoveryReport =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(parsed.summary.requested, ids.len());
    assert_eq!(parsed.case.case_number.as_deref(), Some("PHX-011"));
    assert!(parsed.source.verification.is_none());
    assert_eq!(
        ReportFormat::from_path(Path::new("x.md")),
        ReportFormat::Markdown
    );
    let _: PathBuf = parsed
        .render(ReportFormat::Markdown)
        .map(|_| json_path)
        .unwrap();
}
