//! Behavioural tests for the reader contract (spec M1 required unit tests).

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

use phoinix_block::{
    BlockError, BlockGeometry, BlockReader, BlockReaderExt, MAX_SINGLE_READ, MemoryReader,
    RawImage, SubrangeReader,
};
use sha2::{Digest, Sha256};

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8)
        .collect()
}

fn readers() -> Vec<(String, Arc<dyn BlockReader>, Vec<u8>)> {
    let data = pattern(8192);
    let mut out: Vec<(String, Arc<dyn BlockReader>, Vec<u8>)> = Vec::new();
    out.push((
        "memory".into(),
        Arc::new(MemoryReader::new(data.clone())),
        data.clone(),
    ));

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();
    let raw = RawImage::open(file.path()).unwrap();
    // Keep the temp file alive by leaking the handle for the test duration.
    std::mem::forget(file);
    out.push(("raw".into(), Arc::new(raw), data.clone()));

    let parent: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(pattern(16384)));
    let sub = SubrangeReader::with_bounds(parent, 4096, 8192).unwrap();
    out.push((
        "subrange".into(),
        Arc::new(sub),
        pattern(16384)[4096..12288].to_vec(),
    ));
    out
}

#[test]
fn read_beginning_middle_and_final_byte() {
    for (name, r, data) in readers() {
        assert_eq!(r.len(), 8192, "{name}");
        assert_eq!(r.read_vec(0, 16).unwrap(), &data[..16], "{name} beginning");
        assert_eq!(
            r.read_vec(4000, 100).unwrap(),
            &data[4000..4100],
            "{name} middle"
        );
        assert_eq!(
            r.read_vec(8191, 1).unwrap(),
            &data[8191..],
            "{name} final byte"
        );
    }
}

#[test]
fn read_beyond_end_is_an_error_not_a_short_read() {
    for (name, r, _) in readers() {
        let mut buf = [0u8; 2];
        assert!(
            matches!(
                r.read_at(8191, &mut buf),
                Err(BlockError::OutOfBounds { .. })
            ),
            "{name}"
        );
        assert!(
            matches!(
                r.read_at(8192, &mut buf),
                Err(BlockError::OutOfBounds { .. })
            ),
            "{name}"
        );
        assert!(
            matches!(
                r.read_at(9000, &mut buf),
                Err(BlockError::OutOfBounds { .. })
            ),
            "{name}"
        );
        assert!(
            matches!(r.read_vec(8000, 1000), Err(BlockError::OutOfBounds { .. })),
            "{name}"
        );
    }
}

#[test]
fn zero_length_reads_succeed_inside_bounds() {
    for (name, r, _) in readers() {
        let mut buf = [];
        assert_eq!(r.read_at(0, &mut buf).unwrap(), 0, "{name}");
        assert_eq!(r.read_at(8192, &mut buf).unwrap(), 0, "{name} at end");
        assert!(
            matches!(
                r.read_at(8193, &mut buf),
                Err(BlockError::OutOfBounds { .. })
            ),
            "{name} past end"
        );
    }
}

#[test]
fn integer_overflow_is_rejected() {
    for (name, r, _) in readers() {
        let mut buf = [0u8; 1];
        assert!(
            matches!(
                r.read_at(u64::MAX, &mut buf),
                Err(BlockError::OutOfBounds { .. })
            ),
            "{name}"
        );
    }
}

#[test]
fn oversized_single_request_is_rejected() {
    let r = MemoryReader::zeroed(MAX_SINGLE_READ + 4096);
    let mut buf = vec![0u8; MAX_SINGLE_READ + 1];
    assert!(matches!(
        r.read_at(0, &mut buf),
        Err(BlockError::RequestTooLarge { .. })
    ));
    assert!(matches!(
        r.read_vec(0, MAX_SINGLE_READ + 1),
        Err(BlockError::RequestTooLarge { .. })
    ));
    // read_exact_at splits large requests instead of failing.
    r.read_exact_at(0, &mut buf).unwrap();
}

#[test]
fn subrange_translation_and_bounds() {
    let parent_data = pattern(16384);
    let parent: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(parent_data.clone()));
    let sub = SubrangeReader::with_bounds(parent.clone(), 1000, 500).unwrap();
    assert_eq!(sub.len(), 500);
    assert_eq!(sub.read_vec(0, 10).unwrap(), &parent_data[1000..1010]);
    assert_eq!(sub.read_vec(490, 10).unwrap(), &parent_data[1490..1500]);
    assert!(matches!(
        sub.read_vec(495, 10),
        Err(BlockError::OutOfBounds { .. })
    ));
    assert_eq!(sub.to_parent_offset(500).unwrap(), 1500);
    assert!(sub.to_parent_offset(501).is_err());

    // A subrange cannot exceed its parent.
    assert!(matches!(
        SubrangeReader::with_bounds(parent.clone(), 16000, 1000),
        Err(BlockError::OutOfBounds { .. })
    ));
    assert!(matches!(
        SubrangeReader::with_bounds(parent.clone(), u64::MAX, 1),
        Err(BlockError::IntegerOverflow)
    ));
    // Nested subranges compose.
    let inner = SubrangeReader::with_bounds(Arc::new(sub), 100, 100).unwrap();
    assert_eq!(inner.read_vec(0, 4).unwrap(), &parent_data[1100..1104]);
}

#[test]
fn short_source_is_reported_by_read_exact() {
    // A file that shrinks after opening produces a short read from the OS.
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&pattern(4096)).unwrap();
    file.flush().unwrap();
    let raw = RawImage::open(file.path()).unwrap();
    file.as_file().set_len(1024).unwrap();
    let mut buf = vec![0u8; 4096];
    match raw.read_exact_at(0, &mut buf) {
        Err(BlockError::ShortRead {
            expected: 4096,
            actual,
        }) => assert_eq!(actual, 1024),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn four_k_sector_metadata_and_sector_reads() {
    let data = pattern(16384);
    let r = MemoryReader::with_geometry(data.clone(), BlockGeometry::SECTOR_4K);
    assert_eq!(r.geometry().logical_sector_size, 4096);
    assert_eq!(r.geometry().physical_sector_size, Some(4096));
    assert_eq!(r.read_sector(1).unwrap(), &data[4096..8192]);
    assert_eq!(r.read_sectors(2, 2).unwrap(), &data[8192..16384]);
    assert!(matches!(
        r.read_sector(4),
        Err(BlockError::OutOfBounds { .. })
    ));
    let r512 = MemoryReader::new(data.clone());
    assert_eq!(r512.read_sector(1).unwrap(), &data[512..1024]);
}

#[test]
fn raw_image_deterministic_sha256() {
    let data = pattern(1 << 20);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();
    let raw = RawImage::open(file.path()).unwrap();
    let bytes = raw.read_vec(12_345, 65_536).unwrap();
    let expected = Sha256::digest(&data[12_345..12_345 + 65_536]);
    assert_eq!(Sha256::digest(&bytes).as_slice(), expected.as_slice());
    assert_eq!(raw.describe(), file.path().display().to_string());
}

#[test]
fn missing_file_maps_to_source_unavailable() {
    let err = RawImage::open("/definitely/not/here.img").unwrap_err();
    assert!(matches!(err, BlockError::SourceUnavailable), "{err:?}");
}

#[test]
fn concurrent_readers_do_not_race() {
    let data = pattern(1 << 20);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();
    let raw: Arc<dyn BlockReader> = Arc::new(RawImage::open(file.path()).unwrap());
    let data = Arc::new(data);
    let handles: Vec<_> = (0..8u64)
        .map(|t| {
            let raw = raw.clone();
            let data = data.clone();
            std::thread::spawn(move || {
                for i in 0..200u64 {
                    let off = ((t * 7919 + i * 104_729) % ((1 << 20) - 4096)) as usize;
                    let got = raw.read_vec(off as u64, 4096).unwrap();
                    assert_eq!(&got[..], &data[off..off + 4096]);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}
