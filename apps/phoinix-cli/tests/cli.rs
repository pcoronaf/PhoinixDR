//! End-to-end tests of the `phoinix` binary against the committed fixtures:
//! the M0–M4 deliverable commands as a user runs them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn fixture(name: &str, dir: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let mut decoder = flate2::read::GzDecoder::new(std::fs::File::open(&src).unwrap());
    let mut data = Vec::new();
    decoder.read_to_end(&mut data).unwrap();
    let out = dir.join(Path::new(name).file_stem().unwrap());
    std::fs::write(&out, data).unwrap();
    out
}

fn phoinix(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phoinix"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn inspect_identifies_gpt_and_ntfs() {
    let dir = tempfile::tempdir().unwrap();
    let img = fixture("volume/gpt-basic.img.gz", dir.path());
    let (ok, out, err) = phoinix(&["inspect", img.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.contains("Scheme:       GPT"));
    assert!(out.contains("Données"));
    assert!(out.contains("NTFS (confidence 95%)"));
    let (ok, out, _) = phoinix(&["inspect", img.to_str().unwrap(), "--json"]);
    assert!(ok);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["partition_table"]["scheme"], "gpt");
    assert_eq!(json["volumes"].as_array().unwrap().len(), 3);
}

#[test]
fn ntfs_commands_read_allocated_files() {
    let dir = tempfile::tempdir().unwrap();
    let img = fixture("ntfs/reader.img.gz", dir.path());
    let (ok, out, err) = phoinix(&["ntfs", "info", img.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.contains("Label:             PHXREADER"));
    let (ok, out, _) = phoinix(&["ntfs", "ls", img.to_str().unwrap(), "--json"]);
    assert!(ok);
    let entries: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
    let frag = entries
        .iter()
        .find(|e| e["path"] == "\\docs\\fragmented.bin")
        .unwrap();
    let record = frag["record"].as_u64().unwrap().to_string();
    let target = dir.path().join("frag.bin");
    let (ok, _, err) = phoinix(&[
        "ntfs",
        "extract",
        img.to_str().unwrap(),
        "--record",
        &record,
        "--output",
        target.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/ntfs/reader.manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "\\docs\\fragmented.bin")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&target).unwrap())),
        expected
    );
}

#[test]
fn scan_explain_recover_vertical_slice() {
    let dir = tempfile::tempdir().unwrap();
    let img = fixture("ntfs/undelete.img.gz", dir.path());
    let image = img.to_str().unwrap();

    let (ok, out, err) = phoinix(&["scan", image, "--deleted", "--json"]);
    assert!(ok, "{err}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let candidates: Vec<serde_json::Value> = json["candidates"].as_array().unwrap().clone();
    let photo = candidates
        .iter()
        .find(|c| c["original_name"] == "photo.jpg")
        .expect("photo.jpg candidate");
    assert_eq!(photo["health"]["category"], "excellent");
    let id = photo["filesystem_object"]["record"]
        .as_u64()
        .unwrap()
        .to_string();

    let (ok, out, _) = phoinix(&["scan", image, "--min-health", "excellent"]);
    assert!(ok);
    assert!(out.contains("photo.jpg"));
    assert!(
        !out.contains("wiped.jpg"),
        "very poor candidates must be filtered out:\n{out}"
    );

    let (ok, out, err) = phoinix(&["explain", image, &id]);
    assert!(ok, "{err}");
    assert!(out.contains("Recovery likelihood:"));
    assert!(out.contains("Assessment confidence:"));
    assert!(
        out.contains("✓ The JPEG image structure validates successfully"),
        "{out}"
    );
    assert!(
        out.contains("All 16 required clusters are currently free"),
        "{out}"
    );

    let dest = dir.path().join("recovered");
    let (ok, out, err) = phoinix(&[
        "recover",
        image,
        &id,
        "--output",
        dest.to_str().unwrap(),
        "--preserve-tree",
    ]);
    assert!(ok, "{err}");
    let recovered = dest.join("docs").join("photo.jpg");
    assert!(recovered.exists(), "{out}");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/ntfs/undelete.manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "\\docs\\photo.jpg")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&recovered).unwrap())),
        expected
    );
    assert!(
        out.contains(expected),
        "the CLI reports the SHA-256 it verified"
    );

    // Refuses to recover onto the image itself.
    let (ok, _, err) = phoinix(&["recover", image, &id, "--output", image]);
    assert!(!ok);
    assert!(err.contains("source image"), "{err}");
}

#[test]
fn scan_and_recover_on_fat_and_exfat() {
    let dir = tempfile::tempdir().unwrap();
    for (fixture_name, name, expected_fs, category) in [
        ("fat/fat32.img.gz", "photo.jpg", "FAT32", "excellent"),
        ("fat/fat12.img.gz", "report.pdf", "FAT12", "excellent"),
        // Windows-style deletion on a large FAT32 volume: start inferred.
        ("fat/fat32w.img.gz", "proposal.docx", "FAT32", "good"),
        ("exfat/undelete.img.gz", "photo.jpg", "exFAT", "excellent"),
    ] {
        let img = fixture(fixture_name, dir.path());
        let image = img.to_str().unwrap();
        let (ok, out, err) = phoinix(&["scan", image, "--deleted"]);
        assert!(ok, "{err}");
        assert!(
            out.contains(&format!("on the {expected_fs} volume")),
            "{out}"
        );
        let (ok, out, _) = phoinix(&["scan", image, "--deleted", "--json"]);
        assert!(ok);
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        let candidates: Vec<serde_json::Value> = json["candidates"].as_array().unwrap().clone();
        // FAT short names lose their first character: match on the tail.
        let tail = &name[1..];
        let cand = candidates
            .iter()
            .find(|c| {
                c["original_name"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
                    .ends_with(tail)
            })
            .unwrap_or_else(|| panic!("{fixture_name}: {name} not found in {out}"));
        assert_eq!(
            cand["health"]["category"], category,
            "{fixture_name}: {cand}"
        );
        let id = cand["filesystem_object"]["entry_offset"]
            .as_u64()
            .unwrap()
            .to_string();
        let (ok, out, err) = phoinix(&["explain", image, &id]);
        assert!(ok, "{err}");
        assert!(out.contains("validates successfully"), "{out}");
        let dest = dir
            .path()
            .join(format!("out-{}", fixture_name.replace('/', "-")));
        let (ok, out, err) = phoinix(&["recover", image, &id, "--output", dest.to_str().unwrap()]);
        assert!(ok, "{err}");
        let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(fixture_name.replace(".img.gz", ".manifest.json"));
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
        let expected = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"].as_str().unwrap().to_lowercase().ends_with(name))
            .unwrap()["sha256"]
            .as_str()
            .unwrap();
        assert!(
            out.contains(expected),
            "{fixture_name}: recovered digest mismatch:\n{out}"
        );
    }
}

#[test]
fn deep_scan_carves_orphans_and_recovers_them() {
    let dir = tempfile::tempdir().unwrap();
    let img = fixture("carve/corpus.img.gz", dir.path());
    let image = img.to_str().unwrap();
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/carve/corpus.manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let orphan = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "/o/orphan.pdf")
        .unwrap();
    let offset = orphan["offset"].as_u64().unwrap();

    // Quick scan: no carved rows.
    let (ok, out, _) = phoinix(&["scan", image]);
    assert!(ok);
    assert!(!out.contains("carved-"), "{out}");

    // Deep scan: table and JSON.
    let (ok, out, err) = phoinix(&["scan", image, "--deep"]);
    assert!(ok, "{err}");
    assert!(out.contains(&format!("c{offset}")), "{out}");
    assert!(out.contains("carved-"), "{out}");
    assert!(out.contains("merged into filesystem candidates"), "{out}");
    let (ok, out, _) = phoinix(&["scan", image, "--deep", "--json"]);
    assert!(ok);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["filesystem"], "FAT32");
    assert!(
        json["carving"]["merged_into_metadata"].as_u64().unwrap() >= 10,
        "{}",
        json["carving"]
    );
    let carved: Vec<&serde_json::Value> = json["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["evidence"]["source"] == "file_carving")
        .collect();
    assert_eq!(carved.len(), 3, "{out}");
    assert!(
        carved
            .iter()
            .any(|c| c["filesystem_object"]["offset"] == offset)
    );

    // Type filter and whole-volume carving.
    let (ok, out, _) = phoinix(&[
        "scan",
        image,
        "--deep",
        "--carve-only",
        "--carve-types",
        "pdf",
        "--json",
    ]);
    assert!(ok);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        json["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["filesystem_object"]["type_id"] == "pdf")
    );
    let (ok, _, err) = phoinix(&["scan", image, "--deep", "--carve-types", "nope"]);
    assert!(!ok);
    assert!(err.contains("unknown signature"), "{err}");

    // Explain and recover a carved candidate by its reference.
    let reference = format!("c{offset}");
    let (ok, out, err) = phoinix(&["explain", image, &reference]);
    assert!(ok, "{err}");
    assert!(out.contains("signature carving"), "{out}");
    assert!(out.contains("PDF document"), "{out}");
    let dest = dir.path().join("carved-out");
    let (ok, out, err) = phoinix(&[
        "recover",
        image,
        &reference,
        "--output",
        dest.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains(orphan["sha256"].as_str().unwrap()), "{out}");
    assert!(dest.join(format!("carved-{offset:012}.pdf")).exists());
    let (ok, _, err) = phoinix(&["recover", image, "c12", "--output", dest.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("failed"), "{err}");
}

#[test]
fn partitions_are_found_and_lost_volumes_are_scanned() {
    let dir = tempfile::tempdir().unwrap();
    // A raw area: 1 MiB of zeros, the NTFS undelete corpus with its primary
    // boot sector destroyed, then the FAT32 corpus.
    let ntfs = std::fs::read(fixture("ntfs/undelete.img.gz", dir.path())).unwrap();
    let fat = std::fs::read(fixture("fat/fat32.img.gz", dir.path())).unwrap();
    let mut image = vec![0u8; 1024 * 1024];
    let ntfs_start = image.len();
    image.extend_from_slice(&ntfs);
    for b in &mut image[ntfs_start..ntfs_start + 512] {
        *b = 0;
    }
    image.extend(std::iter::repeat_n(0, 1024 * 1024));
    let fat_start = image.len();
    image.extend_from_slice(&fat);
    let raw = dir.path().join("raw.bin");
    std::fs::write(&raw, &image).unwrap();
    let raw_s = raw.to_str().unwrap();

    // Without the search there is nothing to scan.
    let (ok, _, err) = phoinix(&["scan", raw_s]);
    assert!(!ok);
    assert!(err.contains("no undelete engine"), "{err}");

    let (ok, out, err) = phoinix(&["partitions", raw_s]);
    assert!(ok, "{err}");
    assert!(out.contains("NTFS") && out.contains("FAT32"), "{out}");
    assert!(out.contains("backup boot sector"), "{out}");
    assert!(out.contains("LOST"), "{out}");
    let (ok, out, _) = phoinix(&["partitions", raw_s, "--json"]);
    assert!(ok);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let candidates = json["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{out}");
    let ntfs_c = candidates
        .iter()
        .find(|c| c["filesystem"] == "ntfs")
        .unwrap();
    assert_eq!(ntfs_c["start"].as_u64().unwrap(), ntfs_start as u64);
    assert_eq!(ntfs_c["found_via"], "backup_boot_sector");
    assert_eq!(ntfs_c["relation"]["kind"], "lost");
    assert_eq!(ntfs_c["repairs"].as_array().unwrap().len(), 1);
    let ntfs_index = candidates
        .iter()
        .position(|c| c["filesystem"] == "ntfs")
        .unwrap()
        + 1;
    let fat_c = candidates
        .iter()
        .find(|c| c["filesystem"] == "fat32")
        .unwrap();
    assert_eq!(fat_c["start"].as_u64().unwrap(), fat_start as u64);

    // --lost mounts the candidate with its repair: the corpus is scannable
    // and a candidate recovers byte-exactly.
    let (ok, out, err) = phoinix(&["scan", raw_s, "--lost", &ntfs_index.to_string(), "--json"]);
    assert!(ok, "{err}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["filesystem"], "NTFS");
    let rows = json["candidates"].as_array().unwrap();
    assert!(rows.len() >= 5, "{out}");
    let best = rows
        .iter()
        .find(|c| c["health"]["category"] == "excellent")
        .unwrap();
    let reference = best["filesystem_object"]["record"]
        .as_u64()
        .unwrap()
        .to_string();
    let dest = dir.path().join("lost-out");
    let (ok, out, err) = phoinix(&[
        "recover",
        raw_s,
        "--lost",
        &ntfs_index.to_string(),
        &reference,
        "--output",
        dest.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("SHA-256"), "{out}");

    // --at addresses the FAT32 volume by offset (its boot sector is intact).
    let (ok, out, err) = phoinix(&[
        "scan",
        raw_s,
        "--at",
        &fat_start.to_string(),
        "--length",
        &fat.len().to_string(),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("FAT32 volume"), "{out}");
    let (ok, _, err) = phoinix(&["scan", raw_s, "--lost", "9"]);
    assert!(!ok);
    assert!(err.contains("does not exist"), "{err}");
}
