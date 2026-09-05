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
    let candidates: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
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
    for (fixture_name, name, expected_fs) in [
        ("fat/fat32.img.gz", "photo.jpg", "FAT32"),
        ("fat/fat12.img.gz", "report.pdf", "FAT12"),
        ("exfat/undelete.img.gz", "photo.jpg", "exFAT"),
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
        let candidates: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
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
            cand["health"]["category"], "excellent",
            "{fixture_name}: {cand}"
        );
        let id = cand["filesystem_object"]["entry_offset"]
            .as_u64()
            .unwrap()
            .to_string();
        let (ok, out, err) = phoinix(&["explain", image, &id]);
        assert!(ok, "{err}");
        assert!(out.contains("validates successfully"), "{out}");
        let dest = dir.path().join(format!("out-{expected_fs}"));
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
