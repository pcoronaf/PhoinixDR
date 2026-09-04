//! M3 acceptance: allocated files are reconstructed byte-for-byte from a
//! real NTFS image (mkntfs + ntfs-3g) using only the native parser.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs
)]

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

use phoinix_block::{BlockError, BlockGeometry, BlockReader, MemoryReader, check_request};
use phoinix_core::SourceId;
use phoinix_fs_ntfs::{DataStorage, NtfsError, NtfsFile, NtfsVolume, ResolvedPath};
use phoinix_integration_tests::{Rng, load_gz, manifest};
use sha2::{Digest, Sha256};

fn open_fixture() -> (Arc<dyn BlockReader>, NtfsVolume) {
    let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(load_gz("ntfs/reader.img.gz")));
    let volume = NtfsVolume::open(reader.clone()).unwrap();
    (reader, volume)
}

/// Maps resolved path → (file, resolution) for every usable base record.
fn index(volume: &NtfsVolume) -> HashMap<String, (NtfsFile, ResolvedPath)> {
    let resolver = volume.resolver();
    let mut out = HashMap::new();
    for (_, result) in volume.files() {
        let Ok(file) = result else { continue };
        if !file.in_use {
            continue;
        }
        let resolved = resolver.resolve(&file);
        out.insert(resolved.path.clone(), (file, resolved));
    }
    out
}

#[test]
fn volume_metadata() {
    let (_, volume) = open_fixture();
    let boot = volume.boot();
    assert_eq!(boot.bytes_per_sector, 512);
    assert_eq!(boot.cluster_size, 4096);
    assert_eq!(boot.mft_record_size, 1024);
    assert!(!volume.mft().used_mirror);
    let info = volume.volume_information().unwrap();
    assert_eq!(info.name.as_deref(), Some("PHXREADER"));
    assert_eq!(info.version, Some((3, 1)));
}

#[test]
fn every_manifest_file_extracts_byte_exact() {
    let (_, volume) = open_fixture();
    let files = index(&volume);
    let m = manifest("ntfs/reader.manifest.json");
    let entries = m["files"].as_array().unwrap();
    assert!(!entries.is_empty());
    for entry in entries {
        let path = entry["path"].as_str().unwrap();
        let size = entry["size"].as_u64().unwrap();
        let expected = entry["sha256"].as_str().unwrap();
        let (file, resolved) = files
            .get(path)
            .unwrap_or_else(|| panic!("{path} not found; have {:?}", files.keys()));
        assert!(
            !resolved.uncertain,
            "{path} uncertain: {:?}",
            resolved.diagnostics
        );
        assert_eq!(file.size(), Some(size), "{path} size");
        // Positional read of the whole stream.
        let stream = volume.open_stream(file, None).unwrap();
        let data = stream.read_all(1 << 26).unwrap();
        assert_eq!(data.len() as u64, size);
        assert_eq!(
            hex::encode(Sha256::digest(&data)),
            expected,
            "{path} digest via read_all"
        );
        // Streaming read through the io::Read cursor in odd-sized chunks.
        let mut cursor = stream.cursor();
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 7001];
        loop {
            let n = cursor.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        assert_eq!(
            hex::encode(hasher.finalize()),
            expected,
            "{path} digest via cursor"
        );
    }
}

#[test]
fn storage_shapes_match_expectations() {
    let (_, volume) = open_fixture();
    let files = index(&volume);
    let resident = &files["\\resident.txt"].0;
    assert!(
        resident.unnamed_stream().unwrap().storage.is_resident(),
        "200-byte file should be resident"
    );

    let contiguous = &files["\\docs\\contiguous_1mib.bin"].0;
    assert_eq!(
        contiguous.unnamed_stream().unwrap().extent_count(),
        1,
        "1 MiB file should be one extent"
    );

    let fragmented = &files["\\docs\\fragmented.bin"].0;
    let s = fragmented.unnamed_stream().unwrap();
    assert!(
        s.extent_count() >= 4,
        "fragmented file has {} extents",
        s.extent_count()
    );
    match &s.storage {
        DataStorage::NonResident { complete, runs, .. } => {
            assert!(complete);
            // Runs are in logical (VCN) order even though LCNs interleave.
            assert!(runs.windows(2).all(|w| w[0].end_vcn() == w[1].vcn()));
        }
        other => panic!("unexpected storage {other:?}"),
    }

    let sparse = &files["\\sparse.bin"].0;
    let s = sparse.unnamed_stream().unwrap();
    assert!(
        s.has_sparse_runs(),
        "sparse file should have sparse runs: {:?}",
        s.storage
    );
    assert!(
        s.data_clusters() < 512,
        "sparse file should occupy few clusters"
    );

    let empty = &files["\\empty.txt"].0;
    assert_eq!(empty.size(), Some(0));

    let unicode = &files["\\docs\\nested\\deeper\\ünïcödé 文件 🚀.txt"].0;
    assert_eq!(unicode.name(), Some("ünïcödé 文件 🚀.txt"));
    assert!(
        matches!(
            unicode.preferred_name().unwrap().namespace,
            phoinix_fs_ntfs::FileNameNamespace::Win32
                | phoinix_fs_ntfs::FileNameNamespace::Win32AndDos
                | phoinix_fs_ntfs::FileNameNamespace::Posix
        ),
        "Unicode file should carry a long name"
    );

    // Alternate data stream written through xattrs.
    let m = manifest("ntfs/reader.manifest.json");
    let ads_path = m["ads"]["path"].as_str().unwrap();
    let ads_name = m["ads"]["stream"].as_str().unwrap();
    let host = &files[ads_path].0;
    let ads = host
        .stream(Some(ads_name))
        .expect("alternate data stream present");
    assert!(ads.logical_size > 0);
    let bytes = volume
        .open_stream(host, Some(ads_name))
        .unwrap()
        .read_all(1 << 20)
        .unwrap();
    assert_eq!(bytes.len() as u64, ads.logical_size);
}

#[test]
fn directories_and_system_files() {
    let (_, volume) = open_fixture();
    let files = index(&volume);
    assert!(files["\\docs"].0.directory);
    assert!(files["\\docs\\nested\\deeper"].0.directory);
    let root = volume.file(5).unwrap();
    assert!(root.directory);
    assert_eq!(root.name(), Some("."));
    let mft = volume.file(0).unwrap();
    assert_eq!(mft.name(), Some("$MFT"));
    assert_eq!(mft.size(), Some(volume.mft().record_count() * 1024));
    let bitmap = phoinix_fs_ntfs::ClusterBitmap::load(&volume).unwrap();
    assert_eq!(bitmap.total_clusters(), volume.total_clusters());
    assert!(bitmap.allocated_clusters() > 100);
}

#[test]
fn source_is_never_written() {
    let data = load_gz("ntfs/reader.img.gz");
    let reader = MemoryReader::new(data.clone());
    let volume = NtfsVolume::open(Arc::new(reader.clone())).unwrap();
    for (_, f) in volume.files() {
        if let Ok(f) = f
            && f.stream(None).is_some_and(|s| s.storage.is_readable())
        {
            let _ = volume.open_stream(&f, None).unwrap().read_all(1 << 26);
        }
    }
    assert_eq!(reader.data(), &data[..]);
}

struct SharedReader {
    id: SourceId,
    data: Arc<Mutex<Vec<u8>>>,
    len: u64,
    geometry: BlockGeometry,
}

impl BlockReader for SharedReader {
    fn id(&self) -> SourceId {
        self.id
    }
    fn len(&self) -> u64 {
        self.len
    }
    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.len, offset, buffer.len())?;
        let data = self.data.lock().unwrap();
        let start = offset as usize;
        buffer.copy_from_slice(&data[start..start + buffer.len()]);
        Ok(buffer.len())
    }
}

/// Corrupts the boot sector and MFT in many deterministic ways; the parser
/// must never panic, never read outside the source, and errors must be typed.
#[test]
fn corrupted_mft_never_panics() {
    let shared = Arc::new(Mutex::new(load_gz("ntfs/reader.img.gz")));
    let len = shared.lock().unwrap().len();
    // MFT lives at LCN 4 (16 KiB) for ~121 records of 1 KiB.
    let mft_start = 4 * 4096;
    let mft_len: u64 = 124 * 1024;
    let mut rng = Rng::new(0x5EED);
    for round in 0..250u32 {
        let mut undo = Vec::new();
        {
            let mut data = shared.lock().unwrap();
            let flips = 1 + rng.below(12) as usize;
            for _ in 0..flips {
                let pos = if rng.below(10) == 0 {
                    rng.below(512) as usize
                } else {
                    mft_start + rng.below(mft_len) as usize
                };
                undo.push((pos, data[pos]));
                match rng.below(4) {
                    0 => data[pos] ^= 1 << rng.below(8),
                    1 => data[pos] = 0xFF,
                    2 => data[pos] = 0,
                    _ => data[pos] = rng.next_u64() as u8,
                }
            }
        }
        let keep = if round % 9 == 0 {
            512 + rng.below((len - 512) as u64) as usize
        } else {
            len
        };
        let reader: Arc<dyn BlockReader> = Arc::new(SharedReader {
            id: SourceId::new(),
            data: shared.clone(),
            len: keep as u64,
            geometry: BlockGeometry::SECTOR_512,
        });
        match NtfsVolume::open(reader) {
            Ok(volume) => {
                let resolver = volume.resolver();
                for (_, f) in volume.files() {
                    let Ok(f) = f else { continue };
                    let _ = resolver.resolve(&f);
                    if let Ok(stream) = volume.open_stream(&f, None) {
                        let mut buf = vec![0u8; 8192];
                        let _ = stream.read_at(0, &mut buf);
                        let _ = stream.read_at(stream.len().saturating_sub(1), &mut buf);
                    }
                }
                let _ = phoinix_fs_ntfs::ClusterBitmap::load(&volume);
            }
            Err(e) => {
                let _: &NtfsError = &e;
            }
        }
        let mut data = shared.lock().unwrap();
        for (pos, byte) in undo.into_iter().rev() {
            data[pos] = byte;
        }
    }
}
