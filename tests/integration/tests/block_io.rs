//! M1 integration test: a RAW image yields deterministic bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::io::Write;

use phoinix_block::{BlockReader, BlockReaderExt, RawImage, SourceFingerprint};
use sha2::{Digest, Sha256};

/// Deterministic pseudo-random content: the same bytes on every platform.
fn content(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect()
}

#[test]
fn raw_image_reads_identical_bytes_at_every_offset() {
    let data = content(4 * 1024 * 1024);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();

    let image = RawImage::open(file.path()).unwrap();
    assert_eq!(image.len(), data.len() as u64);

    // Ground-truth digests of specific windows; these must not change across
    // platforms or implementations.
    let cases: [(u64, usize); 5] = [
        (0, 512),
        (511, 2),
        (1_000_000, 65_536),
        (4 * 1024 * 1024 - 4096, 4096),
        (12_345, 1),
    ];
    for (offset, len) in cases {
        let got = image.read_vec(offset, len).unwrap();
        let expected = &data[offset as usize..offset as usize + len];
        assert_eq!(
            Sha256::digest(&got),
            Sha256::digest(expected),
            "offset {offset} len {len}"
        );
    }

    let sector = image.read_sector(3).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&sector)),
        hex::encode(Sha256::digest(&data[1536..2048]))
    );

    let fp = SourceFingerprint::compute(&image).unwrap();
    assert_eq!(fp.size, data.len() as u64);
    assert_eq!(
        fp.first_mib_sha256.as_slice(),
        Sha256::digest(&data[..1 << 20]).as_slice()
    );
    assert_eq!(
        fp.last_mib_sha256.unwrap().as_slice(),
        Sha256::digest(&data[3 << 20..]).as_slice()
    );
}

#[test]
fn content_generator_is_stable() {
    // Pin the generator so that a change to it cannot silently move the
    // ground truth used above.
    let digest = Sha256::digest(content(65_536));
    assert_eq!(
        hex::encode(digest),
        "418d8095dcc764112d972a6301fbb84578961bbac9ce59bddf7450e46186a143"
    );
}
