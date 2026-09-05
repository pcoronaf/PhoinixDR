//! Service-layer acceptance: inspect, scan with events and cancellation,
//! sessions on disk, recovery and previews, all without a GUI.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::io::Read;
use std::path::{Path, PathBuf};

use phoinix_core::FileSystemType;
use phoinix_health::{CandidateSource, HealthCategory};
use phoinix_session::dto::{
    CarveSettings, Preview, RecoverEvent, RecoverRequest, ScanEvent, ScanMode, ScanPhase,
    ScanRequest,
};
use phoinix_session::{ScanSession, Workspace};

fn fixture(name: &str, dir: &Path) -> PathBuf {
    let gz = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let mut decoder = flate2::read::GzDecoder::new(std::fs::File::open(&gz).unwrap());
    let mut data = Vec::new();
    decoder.read_to_end(&mut data).unwrap();
    let out = dir
        .join(Path::new(name).file_name().unwrap())
        .with_extension("");
    std::fs::write(&out, data).unwrap();
    out
}

fn manifest(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn request(source: &Path, mode: ScanMode) -> ScanRequest {
    ScanRequest {
        source: source.to_path_buf(),
        partition: None,
        mode,
        examine_content: true,
        carve: CarveSettings::default(),
    }
}

#[test]
fn inspect_describes_volumes() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path().join("sessions"));
    let img = fixture("volume/gpt-basic.img.gz", dir.path());
    let info = ws.inspect(&img).unwrap();
    assert!(!info.is_device);
    assert_eq!(info.scheme, "Gpt");
    assert!(info.volumes.len() >= 2, "{info:?}");
    assert!(info.volumes.iter().any(|v| v.supported), "{info:?}");
    let raw = fixture("carve/corpus.img.gz", dir.path());
    let info = ws.inspect(&raw).unwrap();
    assert_eq!(info.volumes.len(), 1);
    assert_eq!(info.volumes[0].filesystem, FileSystemType::Fat32);
    assert!(info.volumes[0].supported && info.volumes[0].confidence >= 85);
    assert!(ws.inspect(Path::new("/definitely/not/here.img")).is_err());
}

#[test]
fn quick_and_deep_scans_emit_events_and_persist() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path().join("sessions"));
    let img = fixture("carve/corpus.img.gz", dir.path());
    let m = manifest("carve/corpus.manifest.json");

    let mut events = Vec::new();
    let outcome = ws.scan(&request(&img, ScanMode::Quick), &mut |e| events.push(e));
    outcome.result.as_ref().unwrap();
    let session = outcome.session.unwrap();
    assert!(session.complete && session.finished.is_some());
    assert!(matches!(
        events[0],
        ScanEvent::Phase {
            phase: ScanPhase::Opening
        }
    ));
    assert!(matches!(events[1], ScanEvent::Started { .. }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ScanEvent::Candidates { items } if !items.is_empty()))
    );
    assert!(matches!(events.last().unwrap(), ScanEvent::Finished { .. }));
    let batched: usize = events
        .iter()
        .filter_map(|e| match e {
            ScanEvent::Candidates { items } => Some(items.len()),
            _ => None,
        })
        .sum();
    assert_eq!(batched, session.candidates.len());
    assert!(session.carving.is_none());
    let summary = session.summary();
    assert_eq!(summary.mode, ScanMode::Quick);
    assert_eq!(summary.carved, 0);
    assert!(summary.from_metadata >= 11, "{summary:?}");

    // Deep scan: carving phase, orphans found, merged hits reported.
    let mut events = Vec::new();
    let outcome = ws.scan(&request(&img, ScanMode::Deep), &mut |e| events.push(e));
    outcome.result.as_ref().unwrap();
    let mut session = outcome.session.unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        ScanEvent::Phase {
            phase: ScanPhase::Carving
        }
    )));
    assert!(events.iter().any(|e| matches!(e, ScanEvent::Progress { phase: ScanPhase::Carving, total: Some(t), .. } if *t > 0)));
    let report = session.carving.unwrap();
    assert!(report.merged_into_metadata >= 10, "{report:?}");
    let carved: Vec<_> = session
        .candidates
        .iter()
        .filter(|c| c.evidence.source == CandidateSource::FileCarving)
        .collect();
    assert_eq!(carved.len(), 3, "{report:?}");
    let orphan_pdf = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "/o/orphan.pdf")
        .unwrap();
    assert!(
        carved
            .iter()
            .any(|c| c.logical_size == orphan_pdf["size"].as_u64())
    );

    // Persist, list, reload.
    let path = ws.save_session(&mut session, None).unwrap();
    assert!(path.starts_with(ws.sessions_dir()));
    assert_eq!(session.file.as_deref(), Some(path.as_path()));
    let listed = ws.list_sessions();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, session.id);
    assert_eq!(listed[0].candidates, session.candidates.len());
    let loaded = ws.load_session(&path).unwrap();
    assert_eq!(loaded.candidates, session.candidates);
    assert_eq!(loaded.summary().carving, Some(report));
    assert_eq!(ws.current().unwrap().id, session.id);
    assert_eq!(loaded.summaries().len(), session.candidates.len());
    let summaries = loaded.summaries();
    let jpeg = summaries
        .iter()
        .find(|s| {
            s.type_id.as_deref() == Some("jpeg") && s.source == CandidateSource::FilesystemMetadata
        })
        .unwrap();
    assert!(jpeg.category >= HealthCategory::VeryGood);
    assert!(jpeg.path.as_deref().unwrap().starts_with("\\V\\"));

    // A newer format version is refused.
    let mut text: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    text["version"] = serde_json::Value::from(99);
    let bad = dir.path().join("bad.phx");
    std::fs::write(&bad, text.to_string()).unwrap();
    assert!(ScanSession::load(&bad).is_err());
}

#[test]
fn background_scan_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path().join("sessions"));
    let img = fixture("ntfs/undelete.img.gz", dir.path());
    let handle = ws.start_scan(request(&img, ScanMode::Deep));
    handle.cancel();
    let events: Vec<ScanEvent> = handle.events.iter().collect();
    let outcome = handle.wait();
    assert!(
        matches!(
            outcome.result,
            Err(phoinix_session::SessionError::Cancelled)
        ),
        "{:?}",
        outcome.result
    );
    let session = outcome.session.unwrap();
    assert!(!session.complete);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ScanEvent::Cancelled { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ScanEvent::Finished { .. }))
    );

    // Uncancelled background scan completes and streams candidates.
    let handle = ws.start_scan(request(&img, ScanMode::Quick));
    let events: Vec<ScanEvent> = handle.events.iter().collect();
    let outcome = handle.wait();
    outcome.result.unwrap();
    assert!(matches!(events.last().unwrap(), ScanEvent::Finished { summary } if summary.complete));
}

#[test]
fn recovery_and_previews_through_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path().join("sessions"));
    let img = fixture("carve/corpus.img.gz", dir.path());
    let m = manifest("carve/corpus.manifest.json");
    let outcome = ws.scan(&request(&img, ScanMode::Deep), &mut |_| {});
    let session = ws.set_current(outcome.session.unwrap());
    let by_name = |tail: &str| {
        session
            .candidates
            .iter()
            .find(|c| c.display_name().to_lowercase().ends_with(tail))
            .unwrap()
    };
    let photo = by_name("hoto.jpg");
    let docx = by_name("proposal.docx");
    let orphan_pdf = session
        .candidates
        .iter()
        .find(|c| {
            c.evidence.source == CandidateSource::FileCarving
                && c.filesystem_object.short_reference().ends_with(
                    &m["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|f| f["path"] == "/o/orphan.pdf")
                        .unwrap()["offset"]
                        .as_u64()
                        .unwrap()
                        .to_string(),
                )
        })
        .unwrap();

    // Previews: image as base64, ZIP as hex, carved PDF as hex (no text detection).
    match ws.preview(photo.id).unwrap() {
        Preview::Image {
            mime,
            base64,
            bytes,
        } => {
            assert_eq!(mime, "image/jpeg");
            assert_eq!(bytes, photo.logical_size.unwrap());
            assert!(base64.starts_with("/9j/"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(ws.preview(docx.id).unwrap(), Preview::Hex { .. }));
    assert!(matches!(
        ws.preview(orphan_pdf.id).unwrap(),
        Preview::Hex { .. }
    ));
    assert!(ws.preview(phoinix_core::CandidateId::new()).is_err());

    // Destination safety: an image source is never "dangerous" unless the
    // destination is the image itself.
    let dest = dir.path().join("out");
    let info = ws.destination_info(&dest).unwrap();
    assert!(!info.dangerous, "{info:?}");
    let onto_image = ws.destination_info(&img).unwrap();
    assert!(onto_image.dangerous && onto_image.overwrites_source_image);

    // Recover a metadata candidate and a carved one in one request.
    let req = RecoverRequest {
        candidates: vec![photo.id, orphan_pdf.id],
        destination: dest.clone(),
        preserve_tree: true,
        preserve_timestamps: true,
        hash: true,
        overwrite: false,
        allow_same_device: false,
    };
    let mut events = Vec::new();
    let items = ws.recover(&req, &mut |e| events.push(e)).unwrap();
    assert_eq!(items.len(), 2);
    assert!(matches!(events[0], RecoverEvent::Started { total: 2, .. }));
    assert!(matches!(
        events.last().unwrap(),
        RecoverEvent::Finished { failures: 0, .. }
    ));
    let files = m["files"].as_array().unwrap();
    let photo_sha = files.iter().find(|f| f["path"] == "/v/photo.jpg").unwrap()["sha256"]
        .as_str()
        .unwrap();
    let pdf_sha = files.iter().find(|f| f["path"] == "/o/orphan.pdf").unwrap()["sha256"]
        .as_str()
        .unwrap();
    assert_eq!(
        items[0].result.as_ref().unwrap().sha256.as_deref(),
        Some(photo_sha)
    );
    assert_eq!(
        items[1].result.as_ref().unwrap().sha256.as_deref(),
        Some(pdf_sha)
    );
    assert!(
        items[0]
            .result
            .as_ref()
            .unwrap()
            .output_path
            .starts_with(dest.join("V"))
    );
    // Unknown candidate id is an error before anything is written.
    let bad = RecoverRequest {
        candidates: vec![phoinix_core::CandidateId::new()],
        ..req
    };
    assert!(ws.recover(&bad, &mut |_| {}).is_err());
}
