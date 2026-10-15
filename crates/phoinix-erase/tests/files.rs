//! M12.3: files and folders are overwritten through the filesystem,
//! padded to the allocation unit, read back, renamed and deleted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use phoinix_erase::certificate::CertificateStatus;
use phoinix_erase::{
    EraseError, EraseEvent, EraseMethod, EraseRequest, EraseTarget, Operator, PostScan, Refusal,
    VerifyMode, describe, run_erase,
};

fn request(paths: Vec<PathBuf>, confirmation: &str, method: EraseMethod) -> EraseRequest {
    EraseRequest {
        target: EraseTarget::Files { paths },
        method,
        verify: VerifyMode::Full,
        operator: Operator::default(),
        certificate: None,
        confirmation: confirmation.to_owned(),
        resume: false,
        state_dir: None,
        allow_executable_disk: false,
        allow_sessions_disk: false,
        post_scan: PostScan::None,
    }
}

fn tree(root: &Path) -> Vec<(PathBuf, usize)> {
    let sizes = [
        ("a.txt", 20usize),
        ("b.bin", 5000),
        ("sub/c.dat", 4096),
        ("sub/deeper/d.bin", 5 * 1024 * 1024 + 17),
        ("sub/empty.txt", 0),
    ];
    let mut out = Vec::new();
    for (name, len) in sizes {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data: Vec<u8> = (0..len).map(|i| (i % 253) as u8 | 1).collect();
        std::fs::write(&path, data).unwrap();
        out.push((path, len));
    }
    out
}

#[test]
fn files_and_folders_are_overwritten_padded_verified_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("docs");
    let files = tree(&root);
    let readonly = &files[1].0;
    let mut perms = std::fs::metadata(readonly).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(readonly, perms).unwrap();

    let desc = describe(
        &EraseTarget::Files {
            paths: vec![root.clone()],
        },
        &Default::default(),
    )
    .unwrap();
    assert!(desc.refusals.is_empty(), "{:?}", desc.refusals);
    let fd = desc.files.as_ref().unwrap();
    assert_eq!(fd.entries.len(), 5);
    assert_eq!(fd.directories.len(), 3);
    assert_eq!(
        fd.total_bytes,
        files.iter().map(|(_, n)| *n as u64).sum::<u64>()
    );
    assert!(fd.allocation_unit >= 512);
    assert!(fd.entries.iter().any(|e| e.read_only));
    assert_eq!(desc.confirmation, "docs");
    assert!(desc.warnings.iter().any(|w| w.contains("free-space wipe")));
    let unit = fd.allocation_unit;
    let padded = |n: u64| if n == 0 { 0 } else { n.div_ceil(unit) * unit };
    let per_pass: u64 = files.iter().map(|(_, n)| padded(*n as u64)).sum();
    assert_eq!(desc.length, per_pass);

    let mut req = request(vec![root.clone()], "docs", EraseMethod::Zero);
    req.certificate = Some(dir.path().join("cert.md"));
    let cancel = AtomicBool::new(false);
    let mut checked_before_delete = false;
    let out = run_erase(
        &req,
        &mut |e| {
            if let EraseEvent::Verifying { .. } = e
                && !checked_before_delete
            {
                // At this point the files still exist, overwritten and
                // padded to the allocation unit.
                checked_before_delete = true;
                for (path, n) in &files {
                    let data = std::fs::read(path).unwrap();
                    assert_eq!(data.len() as u64, padded(*n as u64), "{}", path.display());
                    assert!(data.iter().all(|b| *b == 0), "{}", path.display());
                }
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(out.result.is_ok(), "{:?}", out.result);
    assert!(checked_before_delete);
    assert_eq!(out.summary.bytes_written, per_pass);
    assert_eq!(out.summary.files_erased, 5);
    assert!(
        out.summary.files_failed.is_empty(),
        "{:?}",
        out.summary.files_failed
    );
    let v = out.summary.verification.as_ref().unwrap();
    assert!(v.passed, "{v:?}");
    assert_eq!(v.bytes_checked, per_pass);
    assert!(!root.exists(), "the folder should be gone");
    assert_eq!(out.certificate.files_erased.len(), 5);
    assert_eq!(
        out.certificate.target.files.as_ref().unwrap().renamed.len(),
        5
    );
    assert_eq!(out.certificate.status, CertificateStatus::Completed);
    let md = std::fs::read_to_string(dir.path().join("cert.md")).unwrap();
    assert!(md.contains("## Files erased"), "{md}");
    assert!(md.contains("5 file(s) erased"), "{md}");
    assert!(md.contains("Follow with a free-space wipe"), "{md}");
}

#[test]
fn cancelling_leaves_the_files_in_place_partially_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("docs");
    let files = tree(&root);
    let req = request(vec![root.clone()], "docs", EraseMethod::Dod3);
    let cancel = AtomicBool::new(false);
    let mut cancelled = None;
    let out = run_erase(
        &req,
        &mut |e| match e {
            EraseEvent::Pass { pass: 2, .. } => cancel.store(true, Ordering::Relaxed),
            EraseEvent::Cancelled { resume, .. } => cancelled = Some(resume),
            _ => {}
        },
        &cancel,
    )
    .unwrap();
    assert!(matches!(out.result, Err(EraseError::Cancelled)));
    assert_eq!(cancelled, Some(None));
    assert_eq!(out.summary.passes_completed, 1);
    assert_eq!(out.summary.files_erased, 0);
    for (path, n) in &files {
        assert!(path.exists(), "{} should remain", path.display());
        if *n > 0 {
            let data = std::fs::read(path).unwrap();
            assert!(
                data.iter().all(|b| *b == 0),
                "{} was not overwritten",
                path.display()
            );
        }
    }
    assert_eq!(out.certificate.status, CertificateStatus::Cancelled);
}

#[test]
fn the_running_executable_and_symlinks_are_refused_or_skipped() {
    let exe = std::env::current_exe().unwrap();
    let desc = describe(
        &EraseTarget::Files { paths: vec![exe] },
        &Default::default(),
    )
    .unwrap();
    assert!(
        desc.refusals
            .iter()
            .any(|r| matches!(r, Refusal::Unsupported { reason } if reason.contains("executable")))
    );
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"hello").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let desc = describe(
            &EraseTarget::Files {
                paths: vec![link.clone()],
            },
            &Default::default(),
        )
        .unwrap();
        let fd = desc.files.as_ref().unwrap();
        assert!(fd.entries.is_empty());
        assert_eq!(fd.skipped.len(), 1);
        assert!(desc.warnings.iter().any(|w| w.contains("symbolic link")));
        assert!(target.exists());
    }
    let missing = describe(
        &EraseTarget::Files {
            paths: vec![PathBuf::from("/definitely/not/here.txt")],
        },
        &Default::default(),
    );
    assert!(missing.is_err());
}

#[test]
fn several_paths_confirm_with_the_item_count() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("one.txt");
    let b = dir.path().join("two.txt");
    std::fs::write(&a, vec![1u8; 1000]).unwrap();
    std::fs::write(&b, vec![2u8; 3000]).unwrap();
    let desc = describe(
        &EraseTarget::Files {
            paths: vec![a.clone(), b.clone()],
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(desc.confirmation, "2 items");
    let req = request(vec![a.clone(), b.clone()], "2 items", EraseMethod::Random);
    let out = run_erase(&req, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(out.result.is_ok());
    assert_eq!(out.summary.files_erased, 2);
    assert!(!a.exists() && !b.exists());
    assert!(
        dir.path().exists(),
        "only the named files go, not their folder"
    );
}
