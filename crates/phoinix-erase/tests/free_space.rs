//! M12.2: free-space and slack wipes of the fixture images leave the
//! allocated content untouched, and the mounted (filesystem-level) wipe
//! fills a directory's volume and cleans up after itself.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use phoinix_block::MemoryReader;
use phoinix_core::ByteRange;
use phoinix_erase::certificate::CertificateStatus;
use phoinix_erase::{
    EraseEvent, EraseMethod, EraseRequest, EraseTarget, Operator, PostScan, VerifyMode, describe,
    run_erase,
};
use phoinix_fs::AllocationView;
use phoinix_integration_tests::load_gz;

fn request(target: EraseTarget, confirmation: &str, method: EraseMethod) -> EraseRequest {
    EraseRequest {
        target,
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

fn in_ranges(ranges: &[ByteRange], offset: u64) -> bool {
    ranges
        .iter()
        .any(|r| offset >= r.offset && offset < r.end())
}

/// Erases the free space of a fixture copy and checks every byte: zero
/// inside the ranges, original outside.
fn check_free_space(fixture: &str, slack: bool, open: fn(Vec<u8>) -> Arc<dyn AllocationView>) {
    let dir = tempfile::tempdir().unwrap();
    let original = load_gz(fixture);
    let path = dir.path().join("volume.img");
    std::fs::write(&path, &original).unwrap();
    let view = open(original.clone());
    let cs = view.cluster_size();
    let target = EraseTarget::FreeSpace {
        path: path.clone(),
        partition: None,
        slack,
    };
    let desc = describe(&target, &Default::default()).unwrap();
    assert!(desc.refusals.is_empty(), "{:?}", desc.refusals);
    let space = desc.space.as_ref().unwrap();
    assert!(space.map_available);
    assert_eq!(
        space.free_bytes,
        view.free_ranges()
            .unwrap()
            .iter()
            .map(|r| r.length)
            .sum::<u64>()
    );
    assert_eq!(space.slack, slack);
    if slack {
        assert!(space.slack_bytes > 0, "{fixture}: no slack");
        assert!(space.slack_bytes < 64 * cs);
    }
    assert_eq!(
        desc.length,
        desc.ranges.iter().map(|r| r.length).sum::<u64>()
    );
    let ranges = desc.ranges.clone();

    let mut req = request(target, "volume.img", EraseMethod::Zero);
    req.certificate = Some(dir.path().join("cert.md"));
    let cancel = AtomicBool::new(false);
    let mut progress_total = 0;
    let out = run_erase(
        &req,
        &mut |e| {
            if let EraseEvent::Progress { total, .. } = e {
                progress_total = total;
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(out.result.is_ok(), "{fixture}: {:?}", out.result);
    assert_eq!(progress_total, desc.length);
    assert_eq!(out.summary.bytes_written, desc.length);
    let v = out.summary.verification.as_ref().unwrap();
    assert!(v.passed, "{fixture}: {v:?}");
    assert_eq!(v.bytes_checked, desc.length);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after.len(), original.len());
    let mut zeroed = 0u64;
    for (i, (a, b)) in after.iter().zip(original.iter()).enumerate() {
        if in_ranges(&ranges, i as u64) {
            assert_eq!(
                *a, 0,
                "{fixture}: byte {i} inside the erased ranges is not zero"
            );
            zeroed += 1;
        } else {
            assert_eq!(
                a, b,
                "{fixture}: byte {i} outside the erased ranges changed"
            );
        }
    }
    assert_eq!(zeroed, desc.length);
    // The engine still opens the volume and sees the same free space.
    let again = open(after);
    assert_eq!(again.free_ranges().unwrap(), view.free_ranges().unwrap());
    let md = std::fs::read_to_string(dir.path().join("cert.md")).unwrap();
    assert!(md.contains("Free space"), "{md}");
    assert!(
        md.contains(if slack {
            "File slack:** "
        } else {
            "not included"
        }),
        "{md}"
    );
    assert_eq!(out.certificate.status, CertificateStatus::Completed);
}

#[test]
fn fat32_free_space_is_zeroed_and_files_survive() {
    check_free_space("fat/fat32.img.gz", false, |data| {
        let v =
            Arc::new(phoinix_fs_fat::FatVolume::open(Arc::new(MemoryReader::new(data))).unwrap());
        Arc::new(phoinix_fs_fat::FatUndelete::new(v, Default::default()))
    });
}

#[test]
fn ntfs_free_space_and_slack_are_zeroed_and_files_survive() {
    check_free_space("ntfs/reader.img.gz", true, |data| {
        let v =
            Arc::new(phoinix_fs_ntfs::NtfsVolume::open(Arc::new(MemoryReader::new(data))).unwrap());
        Arc::new(phoinix_fs_ntfs::NtfsUndelete::new(v, Default::default()))
    });
}

#[test]
fn ext4_free_space_and_slack_are_zeroed_and_files_survive() {
    check_free_space("ext/ext4.img.gz", true, |data| {
        let v =
            Arc::new(phoinix_fs_ext::ExtVolume::open(Arc::new(MemoryReader::new(data))).unwrap());
        Arc::new(phoinix_fs_ext::ExtUndelete::new(v, Default::default()))
    });
}

#[test]
fn exfat_free_space_is_zeroed_and_files_survive() {
    check_free_space("exfat/undelete.img.gz", false, |data| {
        let v = Arc::new(
            phoinix_fs_exfat::ExfatVolume::open(Arc::new(MemoryReader::new(data))).unwrap(),
        );
        Arc::new(phoinix_fs_exfat::ExfatUndelete::new(v, Default::default()))
    });
}

#[test]
fn free_space_of_a_partitioned_image_needs_a_partition_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disk.img");
    std::fs::write(&path, load_gz("volume/mbr-extended.img.gz")).unwrap();
    let err = describe(
        &EraseTarget::FreeSpace {
            path: path.clone(),
            partition: None,
            slack: false,
        },
        &Default::default(),
    )
    .expect_err("ambiguous");
    assert!(err.to_string().contains("partition"), "{err}");
    let missing = describe(
        &EraseTarget::FreeSpace {
            path,
            partition: Some(99),
            slack: false,
        },
        &Default::default(),
    )
    .unwrap();
    assert!(
        missing
            .refusals
            .iter()
            .any(|r| matches!(r, phoinix_erase::Refusal::NoSuchPartition { index: 99 }))
    );
}

#[test]
fn a_raw_file_without_a_filesystem_is_not_a_free_space_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raw.img");
    std::fs::write(&path, vec![0x5Au8; 1024 * 1024]).unwrap();
    let err = describe(
        &EraseTarget::FreeSpace {
            path,
            partition: None,
            slack: false,
        },
        &Default::default(),
    )
    .expect_err("no filesystem");
    assert!(err.to_string().contains("no supported filesystem"), "{err}");
}

#[test]
fn mounted_wipe_fills_the_volume_up_to_the_reserve_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    // Skipped where the free-space query is unsupported or the volume is
    // nearly full.
    let Ok((_, free)) = phoinix_erase::platform::free_space(dir.path()) else {
        return;
    };
    let window: u64 = 40 * 1024 * 1024;
    if free < 4 * window {
        return;
    }
    let target = EraseTarget::MountedFreeSpace {
        mount: dir.path().to_path_buf(),
        reserve: free - window,
    };
    let desc = describe(&target, &Default::default()).unwrap();
    assert!(desc.refusals.is_empty(), "{:?}", desc.refusals);
    let space = desc.space.as_ref().unwrap();
    assert!(space.mount.is_some());
    assert!(!desc.confirmation.is_empty());
    assert!(
        desc.warnings
            .iter()
            .any(|w| w.contains("through the filesystem"))
    );
    assert!(desc.length <= window + 4 * 1024 * 1024);

    let mut req = request(target, &desc.confirmation, EraseMethod::Dod3);
    req.certificate = Some(dir.path().join("cert.html"));
    let cancel = AtomicBool::new(false);
    let mut passes = Vec::new();
    let mut max_done = 0u64;
    let out = run_erase(
        &req,
        &mut |e| match e {
            EraseEvent::Pass { pattern, .. } => passes.push(pattern),
            EraseEvent::Progress { done, .. } => max_done = max_done.max(done),
            _ => {}
        },
        &cancel,
    )
    .unwrap();
    assert!(out.result.is_ok(), "{:?}", out.result);
    assert_eq!(passes, vec!["0x00", "0xFF", "random"]);
    let covered = out.certificate.target.length;
    assert!(
        covered >= 8 * 1024 * 1024 && covered <= window + 16 * 1024 * 1024,
        "covered {covered} for a window of {window}"
    );
    assert_eq!(out.summary.bytes_written, 3 * covered);
    assert!(max_done >= covered - 4 * 1024 * 1024);
    let v = out.summary.verification.as_ref().unwrap();
    assert!(v.passed, "{v:?}");
    assert_eq!(v.bytes_checked, covered);
    // The fill files and their directory are gone; only the certificate
    // remains.
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["cert.html".to_owned()], "{left:?}");
    let html = std::fs::read_to_string(dir.path().join("cert.html")).unwrap();
    assert!(html.contains("Mounted at"));
    assert!(html.contains("written through the filesystem"));
}

#[test]
fn mounted_wipe_cancels_without_leaving_files() {
    let dir = tempfile::tempdir().unwrap();
    let Ok((_, free)) = phoinix_erase::platform::free_space(dir.path()) else {
        return;
    };
    if free < 2048 * 1024 * 1024 {
        return;
    }
    let target = EraseTarget::MountedFreeSpace {
        mount: dir.path().to_path_buf(),
        reserve: free - 512 * 1024 * 1024,
    };
    let desc = describe(&target, &Default::default()).unwrap();
    let req = request(target, &desc.confirmation, EraseMethod::Zero);
    let cancel = AtomicBool::new(false);
    let mut cancelled = None;
    let out = run_erase(
        &req,
        &mut |e| match e {
            EraseEvent::Progress { done, .. } if done >= 4 * 1024 * 1024 => {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            EraseEvent::Cancelled { resume, .. } => cancelled = Some(resume),
            _ => {}
        },
        &cancel,
    )
    .unwrap();
    assert!(matches!(
        out.result,
        Err(phoinix_erase::EraseError::Cancelled)
    ));
    assert_eq!(cancelled, Some(None));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert_eq!(out.certificate.status, CertificateStatus::Cancelled);
}
