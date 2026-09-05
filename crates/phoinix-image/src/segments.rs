//! Discovery of the files that make up a multi-file image.

use std::path::{Path, PathBuf};

/// The extension that follows `ext` in an EWF sequence: `E01` … `E99`,
/// then `EAA` … `EZZ`, `FAA` … `ZZZ`. The case of the letters is kept.
#[must_use]
pub fn next_ewf_extension(ext: &str) -> Option<String> {
    let chars: Vec<char> = ext.chars().collect();
    if chars.len() != 3 {
        return None;
    }
    let upper = chars.iter().any(|c| c.is_ascii_uppercase());
    let (first, second, third) = (chars.first()?, chars.get(1)?, chars.get(2)?);
    if second.is_ascii_digit() && third.is_ascii_digit() {
        let n: u32 = format!("{second}{third}").parse().ok()?;
        return if n < 99 {
            Some(format!("{first}{:02}", n + 1))
        } else {
            let (a, z) = if upper { ('A', 'Z') } else { ('a', 'z') };
            let _ = z;
            Some(format!("{first}{a}{a}"))
        };
    }
    if !second.is_ascii_alphabetic() || !third.is_ascii_alphabetic() {
        return None;
    }
    let (a, z) = if upper { ('A', 'Z') } else { ('a', 'z') };
    let bump =
        |c: char| -> Option<char> { (c < z).then(|| char::from_u32(u32::from(c) + 1)).flatten() };
    if let Some(t) = bump(*third) {
        return Some(format!("{first}{second}{t}"));
    }
    if let Some(s) = bump(*second) {
        return Some(format!("{first}{s}{a}"));
    }
    let f = bump(*first)?;
    Some(format!("{f}{a}{a}"))
}

/// The first extension of the EWF sequence `ext` belongs to (`E07` → `E01`).
#[must_use]
pub fn first_ewf_extension(ext: &str) -> Option<String> {
    let first = ext.chars().next()?;
    (ext.len() == 3).then(|| format!("{first}01"))
}

/// Every segment file of the EWF image `path` belongs to, in order,
/// starting from the first segment of its sequence.
#[must_use]
pub fn ewf_segments(path: &Path) -> Vec<PathBuf> {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return vec![path.to_path_buf()];
    };
    let Some(first) = first_ewf_extension(ext) else {
        return vec![path.to_path_buf()];
    };
    let mut out = Vec::new();
    let mut current = first;
    for _ in 0..100_000 {
        let candidate = path.with_extension(&current);
        if !candidate.is_file() {
            break;
        }
        out.push(candidate);
        let Some(next) = next_ewf_extension(&current) else {
            break;
        };
        current = next;
    }
    if out.is_empty() {
        out.push(path.to_path_buf());
    }
    out
}

/// Every file of a split RAW image (`disk.001`, `disk.002` … or
/// `disk.000` …, or `disk.aa`, `disk.ab` …), in order, when `path` is
/// one of them. Returns `None` when `path` does not look like a segment
/// or has no siblings.
#[must_use]
pub fn split_raw_segments(path: &Path) -> Option<Vec<PathBuf>> {
    let ext = path.extension()?.to_str()?;
    let mut out = Vec::new();
    if ext.len() == 3 && ext.chars().all(|c| c.is_ascii_digit()) {
        let start = if path.with_extension("000").is_file() {
            0
        } else {
            1
        };
        for n in start..100_000u32 {
            let candidate = path.with_extension(format!("{n:03}"));
            if !candidate.is_file() {
                break;
            }
            out.push(candidate);
        }
    } else if ext.len() == 2 && ext.chars().all(|c| c.is_ascii_lowercase()) {
        let mut chars = ['a', 'a'];
        loop {
            let candidate = path.with_extension(format!("{}{}", chars[0], chars[1]));
            if !candidate.is_file() {
                break;
            }
            out.push(candidate);
            if chars[1] < 'z' {
                chars[1] = char::from_u32(u32::from(chars[1]) + 1).unwrap_or('z');
            } else if chars[0] < 'z' {
                chars[0] = char::from_u32(u32::from(chars[0]) + 1).unwrap_or('z');
                chars[1] = 'a';
            } else {
                break;
            }
        }
    } else {
        return None;
    }
    (out.len() > 1).then_some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn ewf_sequence() {
        assert_eq!(next_ewf_extension("E01").unwrap(), "E02");
        assert_eq!(next_ewf_extension("E99").unwrap(), "EAA");
        assert_eq!(next_ewf_extension("EAA").unwrap(), "EAB");
        assert_eq!(next_ewf_extension("EAZ").unwrap(), "EBA");
        assert_eq!(next_ewf_extension("EZZ").unwrap(), "FAA");
        assert_eq!(next_ewf_extension("e09").unwrap(), "e10");
        assert_eq!(next_ewf_extension("s99").unwrap(), "saa");
        assert!(next_ewf_extension("ZZZ").is_none());
        assert_eq!(first_ewf_extension("E17").unwrap(), "E01");
    }

    #[test]
    fn split_sequences_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        for n in 1..=3 {
            std::fs::write(dir.path().join(format!("disk.{n:03}")), b"x").unwrap();
        }
        let segs = split_raw_segments(&dir.path().join("disk.002")).unwrap();
        assert_eq!(segs.len(), 3);
        assert!(segs[0].ends_with("disk.001"));
        for s in ["aa", "ab"] {
            std::fs::write(dir.path().join(format!("part.{s}")), b"x").unwrap();
        }
        assert_eq!(
            split_raw_segments(&dir.path().join("part.aa"))
                .unwrap()
                .len(),
            2
        );
        assert!(split_raw_segments(&dir.path().join("lonely.001")).is_none());
        for n in 1..=2 {
            std::fs::write(dir.path().join(format!("case.E{n:02}")), b"x").unwrap();
        }
        assert_eq!(ewf_segments(&dir.path().join("case.E02")).len(), 2);
    }
}
