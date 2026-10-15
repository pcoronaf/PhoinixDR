//! M12.2: the post-erase scan looks at an erased target with the recovery
//! engines and reports what a recovery would still return.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

use std::sync::atomic::AtomicBool;

use phoinix_erase::{
    EraseEvent, EraseMethod, EraseRequest, EraseTarget, Guards, Operator, PostScan, VerifyMode,
    run_erase_with,
};
use phoinix_integration_tests::load_gz;
use phoinix_session::SessionScanner;

fn request(target: EraseTarget, confirmation: &str, post_scan: PostScan) -> EraseRequest {
    EraseRequest {
        target,
        method: EraseMethod::Zero,
        verify: VerifyMode::Sample,
        operator: Operator::default(),
        certificate: None,
        confirmation: confirmation.to_owned(),
        resume: false,
        state_dir: None,
        allow_executable_disk: false,
        allow_sessions_disk: false,
        post_scan,
    }
}

#[test]
fn free_space_wipe_leaves_no_recoverable_content_on_fat32() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volume.img");
    std::fs::write(&path, load_gz("fat/fat32.img.gz")).unwrap();
    let req = request(
        EraseTarget::FreeSpace {
            path,
            partition: None,
            slack: false,
        },
        "volume.img",
        PostScan::Deep,
    );
    let mut phases = Vec::new();
    let out = run_erase_with(
        &req,
        &Guards::default(),
        Some(&SessionScanner),
        &mut |e| {
            if let EraseEvent::Scanning { phase, .. } = e {
                phases.push(phase);
            }
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(out.result.is_ok());
    let scan = out.summary.scan.expect("scan record");
    assert!(scan.ran, "{scan:?}");
    assert_eq!(scan.mode, PostScan::Deep);
    assert_eq!(scan.filesystem.as_deref(), Some("FAT32"));
    assert!(scan.candidates > 5, "{scan:?}");
    assert_eq!(scan.with_content, 0, "{scan:?}");
    assert_eq!(scan.carved, 0, "{scan:?}");
    assert!(scan.bytes_scanned > 0);
    assert!(scan.passed, "{scan:?}");
    assert!(phases.iter().any(|p| p == "metadata"));
    assert!(phases.iter().any(|p| p == "carving"));
    assert!(
        out.certificate
            .verdict()
            .contains("found nothing recoverable")
    );
    assert!(out.certificate.to_html().contains("Post-erase scan"));
}

#[test]
fn the_scan_before_a_wipe_still_finds_the_deleted_files() {
    // The same image, not erased: the scanner reports the corpus's
    // recoverable files, which proves it looks.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("volume.img");
    std::fs::write(&path, load_gz("fat/fat32.img.gz")).unwrap();
    let desc = phoinix_erase::describe(
        &EraseTarget::FreeSpace {
            path,
            partition: None,
            slack: false,
        },
        &Guards::default(),
    )
    .unwrap();
    let record = phoinix_erase::PostScanner::scan(
        &SessionScanner,
        &desc,
        PostScan::Quick,
        &mut |_| {},
        &AtomicBool::new(false),
    );
    assert!(record.ran, "{record:?}");
    assert!(record.with_content > 3, "{record:?}");
    assert!(!record.passed);
    assert!(record.summary().contains("FOUND"));
}

#[test]
fn a_zeroed_image_has_no_filesystem_and_the_deep_scan_carves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disk.img");
    std::fs::write(&path, load_gz("fat/fat16.img.gz")).unwrap();
    let req = request(EraseTarget::File { path }, "disk.img", PostScan::Deep);
    let out = run_erase_with(
        &req,
        &Guards::default(),
        Some(&SessionScanner),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(out.result.is_ok());
    let scan = out.summary.scan.expect("scan record");
    assert!(scan.ran, "{scan:?}");
    assert_eq!(scan.candidates, 0);
    assert_eq!(scan.carved, 0);
    assert!(scan.passed, "{scan:?}");
    assert!(
        scan.filesystem.as_deref().is_none_or(|f| f == "Unknown"),
        "{scan:?}"
    );
    let quick = request(
        EraseTarget::File {
            path: dir.path().join("disk.img"),
        },
        "disk.img",
        PostScan::Quick,
    );
    let out = run_erase_with(
        &quick,
        &Guards::default(),
        Some(&SessionScanner),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let scan = out.summary.scan.expect("scan record");
    assert!(scan.passed, "{scan:?}");
    assert!(
        scan.note.as_deref().unwrap_or("").contains("no filesystem"),
        "{scan:?}"
    );
}
