//! M12.3: the post-erase scan of a files target counts only the erased
//! names (it needs raw access to the volume, so it is exercised through
//! the scanner directly on an image).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::atomic::AtomicBool;

use phoinix_erase::{EraseTarget, Guards, PostScan, PostScanner};
use phoinix_integration_tests::load_gz;
use phoinix_session::SessionScanner;

#[test]
fn a_files_scan_counts_only_the_named_files() {
    // A description for files whose names exist as deleted records in
    // the FAT32 corpus, with the volume node pointing at the image.
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("volume.img");
    std::fs::write(&image, load_gz("fat/fat32.img.gz")).unwrap();
    let one = dir.path().join("photo.jpg");
    let two = dir.path().join("nothing-like-this.bin");
    std::fs::write(&one, b"x").unwrap();
    std::fs::write(&two, b"y").unwrap();
    let mut desc = phoinix_erase::describe(
        &EraseTarget::Files {
            paths: vec![one, two],
        },
        &Guards::default(),
    )
    .unwrap();
    let space = desc
        .space
        .get_or_insert_with(|| phoinix_erase::SpaceDescription {
            filesystem: "FAT32".into(),
            volume_offset: 0,
            volume_length: 0,
            cluster_size: 4096,
            map_available: true,
            free_bytes: 0,
            free_ranges: 0,
            slack: false,
            slack_bytes: 0,
            slack_ranges: 0,
            mount: None,
            node: None,
            label: None,
            total_bytes: None,
        });
    space.node = Some(image.display().to_string());
    let record = SessionScanner.scan(&desc, PostScan::Quick, &mut |_| {}, &AtomicBool::new(false));
    assert!(record.ran, "{record:?}");
    // photo.jpg is a deleted record with content in the corpus; the other
    // name is not on the volume at all.
    assert_eq!(record.candidates, 1, "{record:?}");
    assert_eq!(record.with_content, 1, "{record:?}");
    assert!(!record.passed);
    assert!(record.note.as_deref().unwrap_or("").contains("erased name"));
}
