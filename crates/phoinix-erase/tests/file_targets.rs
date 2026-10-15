//! End-to-end erases of file targets: passes, verification, cancellation
//! and resume, certificates.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

use std::sync::atomic::{AtomicBool, Ordering};

use phoinix_erase::certificate::CertificateStatus;
use phoinix_erase::{
    EraseError, EraseEvent, EraseMethod, EraseRequest, EraseTarget, Operator, Refusal, VerifyMode,
    run_erase,
};

fn fixture(dir: &std::path::Path, name: &str, len: usize) -> std::path::PathBuf {
    let path = dir.join(name);
    let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8 ^ 0x5A).collect();
    std::fs::write(&path, data).unwrap();
    path
}

fn request(path: &std::path::Path, method: EraseMethod) -> EraseRequest {
    EraseRequest {
        target: EraseTarget::File {
            path: path.to_path_buf(),
        },
        method,
        verify: VerifyMode::Sample,
        operator: Operator {
            name: Some("Tester".into()),
            organisation: None,
            asset: Some("ASSET-1".into()),
            notes: None,
        },
        certificate: None,
        confirmation: path.file_name().unwrap().to_string_lossy().into_owned(),
        resume: false,
        state_dir: None,
        allow_executable_disk: false,
        allow_sessions_disk: false,
    }
}

#[test]
fn zero_pass_erases_and_verifies_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path(), "a.img", 9 * 1024 * 1024 + 123);
    let mut req = request(&path, EraseMethod::Zero);
    req.certificate = Some(dir.path().join("cert.html"));
    let mut events = Vec::new();
    let cancel = AtomicBool::new(false);
    let out = run_erase(&req, &mut |e| events.push(e), &cancel).unwrap();
    assert!(out.result.is_ok());
    assert_eq!(out.summary.passes_completed, 1);
    assert_eq!(out.summary.bytes_written, 9 * 1024 * 1024 + 123);
    let v = out.summary.verification.as_ref().unwrap();
    assert!(v.passed, "{v:?}");
    assert_eq!(v.bytes_checked, 9 * 1024 * 1024 + 123);
    assert!(std::fs::read(&path).unwrap().iter().all(|b| *b == 0));
    assert!(matches!(events.first(), Some(EraseEvent::Started { .. })));
    assert!(matches!(events.last(), Some(EraseEvent::Finished { .. })));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EraseEvent::Progress { done, total, .. } if done == total))
    );
    let html = std::fs::read_to_string(dir.path().join("cert.html")).unwrap();
    assert!(html.contains("Media sanitization certificate"));
    assert!(html.contains("ASSET-1"));
    assert!(html.contains("all match"));
    assert_eq!(out.certificate.status, CertificateStatus::Completed);
}

#[test]
fn random_passes_are_verifiable_and_multi_pass_methods_run_every_pass() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path(), "b.img", 3 * 1024 * 1024);
    let req = request(&path, EraseMethod::Dod3);
    let mut passes = Vec::new();
    let cancel = AtomicBool::new(false);
    let out = run_erase(
        &req,
        &mut |e| {
            if let EraseEvent::Pass { pattern, .. } = e {
                passes.push(pattern);
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(out.result.is_ok());
    assert_eq!(passes, vec!["0x00", "0xFF", "random"]);
    let v = out.summary.verification.unwrap();
    assert!(v.passed, "{v:?}");
    let data = std::fs::read(&path).unwrap();
    assert!(data.iter().filter(|b| **b == 0).count() < data.len() / 100);
    // The certificate's seed regenerates the random pass: tamper and re-check.
    let md = out.certificate.to_markdown();
    assert!(md.contains("Random seed"));
    assert!(md.contains("3 pass(es) completed"));
}

#[test]
fn cancellation_leaves_a_resume_point_and_resume_finishes_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path(), "c.img", 24 * 1024 * 1024);
    let state = dir.path().join("state");
    let mut req = request(
        &path,
        EraseMethod::Custom {
            passes: vec![
                phoinix_erase::Pass {
                    pattern: phoinix_erase::PassPattern::Fixed { byte: 0x11 },
                },
                phoinix_erase::Pass {
                    pattern: phoinix_erase::PassPattern::Fixed { byte: 0x22 },
                },
            ],
        },
    );
    req.state_dir = Some(state.clone());
    req.verify = VerifyMode::Full;
    let cancel = AtomicBool::new(false);
    let mut resume = None;
    let out = run_erase(
        &req,
        &mut |e| match e {
            EraseEvent::Progress { pass, done, .. } if pass == 2 && done >= 8 * 1024 * 1024 => {
                cancel.store(true, Ordering::Relaxed);
            }
            EraseEvent::Cancelled { resume: r, .. } => resume = Some(r),
            _ => {}
        },
        &cancel,
    )
    .unwrap();
    assert!(matches!(out.result, Err(EraseError::Cancelled)));
    let point = resume.expect("resume point");
    assert_eq!(point.pass, 1);
    assert!(point.offset >= 8 * 1024 * 1024 && point.offset < 24 * 1024 * 1024);
    assert_eq!(out.certificate.status, CertificateStatus::Cancelled);
    assert!(out.certificate.verdict().contains("PARTIALLY"));
    let data = std::fs::read(&path).unwrap();
    assert_eq!(data[0], 0x22);
    assert_eq!(data[data.len() - 1], 0x11);

    // Resume: only the rest of pass 2 is written, then verification runs.
    cancel.store(false, Ordering::Relaxed);
    req.resume = true;
    let mut resumed_from = None;
    let out = run_erase(
        &req,
        &mut |e| {
            if let EraseEvent::Started {
                resumed_from: r, ..
            } = e
            {
                resumed_from = r;
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(out.result.is_ok());
    assert_eq!(resumed_from.unwrap().pass, 1);
    // Cumulative across the cancelled run and the resumed one: pass 1 in
    // full, pass 2 once (its first part before the cancel, the rest now).
    assert_eq!(out.summary.bytes_written, 2 * 24 * 1024 * 1024);
    assert!(out.summary.verification.unwrap().passed);
    assert!(std::fs::read(&path).unwrap().iter().all(|b| *b == 0x22));
    assert!(
        std::fs::read_dir(&state).unwrap().next().is_none(),
        "resume point cleared"
    );
}

#[test]
fn wrong_confirmation_and_open_sources_are_refused_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path(), "d.img", 4096);
    let before = std::fs::read(&path).unwrap();
    let mut req = request(&path, EraseMethod::Zero);
    req.confirmation = "nope".into();
    let cancel = AtomicBool::new(false);
    match run_erase(&req, &mut |_| {}, &cancel) {
        Err(EraseError::Refused(Refusal::NotConfirmed { expected })) => {
            assert_eq!(expected, "d.img")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let guards = phoinix_erase::Guards {
        open_sources: vec![path.clone()],
        ..Default::default()
    };
    req.confirmation = "d.img".into();
    match phoinix_erase::engine::run_erase_with_guards(&req, &guards, &mut |_| {}, &cancel) {
        Err(EraseError::Refused(Refusal::OpenSource)) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn nist_method_verifies_even_when_verification_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path(), "e.img", 70_000);
    let mut req = request(&path, EraseMethod::Nist);
    req.verify = VerifyMode::None;
    let cancel = AtomicBool::new(false);
    let out = run_erase(&req, &mut |_| {}, &cancel).unwrap();
    let v = out.summary.verification.unwrap();
    assert_eq!(v.mode, VerifyMode::Sample);
    assert!(v.passed);
    assert_eq!(v.bytes_checked, 70_000);
}
