//! Files and folders (milestone M12.3): the content of the named files is
//! overwritten through the filesystem, every named stream included on
//! NTFS, extended to the end of the last allocation unit so the slack goes
//! too, read back, truncated, renamed to a random name and deleted. What
//! this cannot reach is stated in [`crate::FILES_NOTE`]; the honest
//! follow-up is a free-space wipe of the volume.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::certificate::VerificationRecord;
use crate::engine::EraseEvent;
use crate::error::EraseError;
use crate::method::{Pass, VerifyMode};
use crate::pattern::{CHUNK_BYTES, Seed, fill_chunk};
use crate::platform;
use crate::target::TargetDescription;
use crate::verify::{ReadBackPaths, read_back_paths};

/// A named stream of a file (NTFS alternate data stream).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEntry {
    /// Stream name (`Zone.Identifier`).
    pub name: String,
    /// Length in bytes.
    pub size: u64,
}

/// One file to erase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path as resolved.
    pub path: String,
    /// Length of the main stream.
    pub size: u64,
    /// Named streams.
    #[serde(default)]
    pub streams: Vec<StreamEntry>,
    /// Whether the read-only attribute is set (cleared before writing).
    #[serde(default)]
    pub read_only: bool,
}

/// The files and folders of a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesDescription {
    /// Files, folders walked.
    pub entries: Vec<FileEntry>,
    /// Folders to remove once empty, deepest first.
    pub directories: Vec<String>,
    /// Bytes of content (every stream).
    pub total_bytes: u64,
    /// Number of named streams over every file.
    pub stream_count: usize,
    /// Allocation unit of the volume, used to overwrite the slack.
    pub allocation_unit: u64,
    /// Paths that were skipped and why (symbolic links, unreadable).
    #[serde(default)]
    pub skipped: Vec<String>,
    /// The random names the files were given before deletion (filled by
    /// the engine, for the post-erase scan).
    #[serde(default)]
    pub renamed: Vec<String>,
}

/// A file that could not be erased.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    /// The path.
    pub path: String,
    /// Why.
    pub error: String,
}

/// What the files erase produced.
#[derive(Debug)]
pub struct FilesOutcome {
    /// Bytes written over every pass.
    pub bytes_written: u64,
    /// Passes completed.
    pub passes_completed: usize,
    /// Files overwritten and deleted.
    pub erased: Vec<String>,
    /// Files that failed, with the error.
    pub failed: Vec<FileFailure>,
    /// The random names given before deletion.
    pub renamed: Vec<String>,
    /// The read-back, when run.
    pub verification: Option<VerificationRecord>,
    /// `Ok` when every pass completed.
    pub result: Result<(), EraseError>,
}

/// Rounds `size` up to the allocation unit (the slack goes with it).
const fn padded(size: u64, unit: u64) -> u64 {
    if unit <= 1 || size == 0 {
        return size;
    }
    let rem = size % unit;
    if rem == 0 { size } else { size + (unit - rem) }
}

/// Walks the given paths into files and directories. Symbolic links are
/// never followed; unreadable entries are listed as skipped.
///
/// # Errors
///
/// Returns [`EraseError`] when a path does not exist.
pub fn collect(paths: &[PathBuf]) -> Result<FilesDescription, EraseError> {
    let mut entries = Vec::new();
    let mut directories = Vec::new();
    let mut skipped = Vec::new();
    let mut stream_count = 0usize;
    let mut unit = 0u64;
    for root in paths {
        let meta = std::fs::symlink_metadata(root).map_err(|e| EraseError::io(root, e))?;
        if unit == 0 {
            unit = platform::allocation_unit(root).unwrap_or(4096);
        }
        if meta.file_type().is_symlink() {
            skipped.push(format!("{} (symbolic link; not followed)", root.display()));
            continue;
        }
        if meta.is_dir() {
            let mut stack = vec![root.clone()];
            let mut seen_dirs = Vec::new();
            while let Some(dir) = stack.pop() {
                seen_dirs.push(dir.clone());
                let Ok(read) = std::fs::read_dir(&dir) else {
                    skipped.push(format!("{} (unreadable folder)", dir.display()));
                    continue;
                };
                for entry in read.flatten() {
                    let path = entry.path();
                    let Ok(m) = std::fs::symlink_metadata(&path) else {
                        skipped.push(format!("{} (unreadable)", path.display()));
                        continue;
                    };
                    if m.file_type().is_symlink() {
                        skipped.push(format!("{} (symbolic link; not followed)", path.display()));
                    } else if m.is_dir() {
                        stack.push(path);
                    } else if m.is_file() {
                        let streams = platform::file_streams(&path);
                        stream_count += streams.len();
                        entries.push(FileEntry {
                            path: path.display().to_string(),
                            size: m.len(),
                            streams,
                            read_only: m.permissions().readonly(),
                        });
                    } else {
                        skipped.push(format!("{} (not a regular file)", path.display()));
                    }
                }
            }
            // Deepest first, so that every folder is empty when removed.
            seen_dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
            directories.extend(seen_dirs.iter().map(|d| d.display().to_string()));
        } else if meta.is_file() {
            let streams = platform::file_streams(root);
            stream_count += streams.len();
            entries.push(FileEntry {
                path: root.display().to_string(),
                size: meta.len(),
                streams,
                read_only: meta.permissions().readonly(),
            });
        } else {
            skipped.push(format!("{} (not a regular file)", root.display()));
        }
    }
    let total_bytes = entries
        .iter()
        .map(|e| e.size + e.streams.iter().map(|s| s.size).sum::<u64>())
        .sum();
    Ok(FilesDescription {
        entries,
        directories,
        total_bytes,
        stream_count,
        allocation_unit: unit.max(1),
        skipped,
        renamed: Vec::new(),
    })
}

/// Bytes the passes will write per pass: every stream padded to the
/// allocation unit.
#[must_use]
pub fn bytes_per_pass(files: &FilesDescription) -> u64 {
    let unit = files.allocation_unit;
    files
        .entries
        .iter()
        .map(|e| padded(e.size, unit) + e.streams.iter().map(|s| padded(s.size, unit)).sum::<u64>())
        .sum()
}

/// The stream paths of an entry: the file itself, then every named stream
/// (`file:name` on Windows).
fn stream_paths(entry: &FileEntry) -> Vec<(PathBuf, u64)> {
    let mut out = vec![(PathBuf::from(&entry.path), entry.size)];
    for s in &entry.streams {
        out.push((PathBuf::from(format!("{}:{}", entry.path, s.name)), s.size));
    }
    out
}

fn random_name(seed: &Seed, index: usize, len: usize) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(seed.0);
    h.update(index.to_le_bytes());
    let digest = h.finalize();
    let hex = hex::encode(digest);
    hex.chars().take(len.clamp(8, 32)).collect()
}

struct Progress<'a> {
    sink: &'a mut dyn FnMut(EraseEvent),
    pass: usize,
    passes: usize,
    total: u64,
    last_tick: Instant,
    tick_bytes: u64,
}

impl Progress<'_> {
    fn tick(&mut self, done: u64, wrote: u64, force: bool) {
        self.tick_bytes += wrote;
        if !force
            && self.last_tick.elapsed() < Duration::from_millis(250)
            && self.tick_bytes < 16 * 1024 * 1024
        {
            return;
        }
        let millis = u64::try_from(self.last_tick.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        (self.sink)(EraseEvent::Progress {
            pass: self.pass,
            passes: self.passes,
            done: done.min(self.total),
            total: self.total,
            bytes_per_sec: self.tick_bytes.saturating_mul(1000) / millis,
            unwritable: 0,
        });
        self.last_tick = Instant::now();
        self.tick_bytes = 0;
    }
}

/// Overwrites one stream: `[0, size)` first (in place, so NTFS resident
/// data is rewritten inside its record), then up to the padded length.
#[allow(clippy::too_many_arguments)]
fn overwrite_stream(
    path: &Path,
    size: u64,
    padded_len: u64,
    pass: &Pass,
    pass_index: usize,
    seed: &Seed,
    rel: &mut u64,
    buf: &mut [u8],
    progress: &mut Progress<'_>,
    cancel: &AtomicBool,
) -> Result<(), EraseError> {
    let mut file = File::options()
        .write(true)
        .open(path)
        .map_err(|e| EraseError::io(path, e))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| EraseError::io(path, e))?;
    let mut in_stream: u64 = 0;
    for end in [size, padded_len] {
        while in_stream < end {
            if cancel.load(Ordering::Relaxed) {
                return Err(EraseError::Cancelled);
            }
            let chunk = *rel / CHUNK_BYTES as u64;
            let within = usize::try_from(*rel % CHUNK_BYTES as u64).unwrap_or(0);
            if within == 0 {
                fill_chunk(&pass.pattern, seed, pass_index, chunk, buf);
            }
            let want =
                (CHUNK_BYTES - within).min(usize::try_from(end - in_stream).unwrap_or(CHUNK_BYTES));
            let piece = buf.get(within..within + want).unwrap_or(&[]);
            file.write_all(piece).map_err(|e| EraseError::io(path, e))?;
            *rel += want as u64;
            in_stream += want as u64;
            progress.tick(*rel, want as u64, false);
        }
    }
    file.sync_all().map_err(|e| EraseError::io(path, e))
}

/// Runs the files erase.
#[allow(clippy::too_many_lines)]
pub fn run(
    desc: &TargetDescription,
    files: &FilesDescription,
    passes: &[Pass],
    seed: &Seed,
    verify: VerifyMode,
    sink: &mut dyn FnMut(EraseEvent),
    cancel: &AtomicBool,
) -> FilesOutcome {
    let mut out = FilesOutcome {
        bytes_written: 0,
        passes_completed: 0,
        erased: Vec::new(),
        failed: Vec::new(),
        renamed: Vec::new(),
        verification: None,
        result: Ok(()),
    };
    let unit = files.allocation_unit;
    let per_pass = bytes_per_pass(files);
    let mut buf = vec![0u8; CHUNK_BYTES];
    // Streams in writing order with their padded lengths.
    let mut items: Vec<(PathBuf, u64)> = Vec::new();
    for e in &files.entries {
        for (path, size) in stream_paths(e) {
            items.push((path, padded(size, unit)));
        }
    }
    tracing::info!(
        files = files.entries.len(),
        streams = files.stream_count,
        bytes = files.total_bytes,
        per_pass,
        "erasing files through the filesystem"
    );
    // Clear read-only attributes first.
    for e in files.entries.iter().filter(|e| e.read_only) {
        if let Ok(meta) = std::fs::metadata(&e.path) {
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            if let Err(err) = std::fs::set_permissions(&e.path, perms) {
                tracing::warn!(path = %e.path, error = %err, "read-only attribute not cleared");
            }
        }
    }
    let result = (|| -> Result<(), EraseError> {
        for (pass_index, pass) in passes.iter().enumerate() {
            sink(EraseEvent::Pass {
                pass: pass_index + 1,
                passes: passes.len(),
                pattern: pass.pattern.label(),
            });
            let mut progress = Progress {
                sink,
                pass: pass_index + 1,
                passes: passes.len(),
                total: per_pass,
                last_tick: Instant::now(),
                tick_bytes: 0,
            };
            let mut rel: u64 = 0;
            for e in &files.entries {
                if out.failed.iter().any(|f| f.path == e.path) {
                    // Skip the streams of a file that already failed, but
                    // keep the relative position in step with the plan.
                    rel += stream_paths(e)
                        .iter()
                        .map(|(_, s)| padded(*s, unit))
                        .sum::<u64>();
                    continue;
                }
                for (path, size) in stream_paths(e) {
                    let padded_len = padded(size, unit);
                    let before = rel;
                    match overwrite_stream(
                        &path,
                        size,
                        padded_len,
                        pass,
                        pass_index,
                        seed,
                        &mut rel,
                        &mut buf,
                        &mut progress,
                        cancel,
                    ) {
                        Ok(()) => out.bytes_written += rel - before,
                        Err(EraseError::Cancelled) => return Err(EraseError::Cancelled),
                        Err(err) => {
                            tracing::warn!(path = %path.display(), error = %err, "stream not overwritten");
                            out.failed.push(FileFailure {
                                path: e.path.clone(),
                                error: err.to_string(),
                            });
                            out.bytes_written += rel - before;
                            rel = before + padded_len;
                            break;
                        }
                    }
                }
            }
            progress.tick(rel, 0, true);
            out.passes_completed = pass_index + 1;
            tracing::info!(pass = pass_index + 1, "pass finished");
        }
        Ok(())
    })();
    out.result = result;

    if out.result.is_ok()
        && let Some(last) = passes.last()
    {
        let failed: Vec<String> = out.failed.iter().map(|f| f.path.clone()).collect();
        let items_ok: Vec<(PathBuf, u64)> = files
            .entries
            .iter()
            .filter(|e| !failed.contains(&e.path))
            .flat_map(|e| {
                stream_paths(e)
                    .into_iter()
                    .map(|(p, s)| (p, padded(s, unit)))
            })
            .collect();
        let mut progress = |done, total| sink(EraseEvent::Verifying { done, total });
        out.verification = Some(read_back_paths(
            &ReadBackPaths {
                items: &items_ok,
                last: &last.pattern,
                seed,
                pass_index: passes.len() - 1,
                mode: verify,
            },
            &mut progress,
            cancel,
        ));
        // Truncate, rename, delete.
        for (index, e) in files.entries.iter().enumerate() {
            if failed.contains(&e.path) {
                continue;
            }
            let path = PathBuf::from(&e.path);
            let step = (|| -> std::io::Result<String> {
                File::options().write(true).open(&path)?.set_len(0)?;
                let name_len = path
                    .file_name()
                    .map_or(12, |n| n.to_string_lossy().chars().count());
                let new_name = random_name(seed, index, name_len);
                let renamed = path.with_file_name(&new_name);
                let final_path = if std::fs::rename(&path, &renamed).is_ok() {
                    renamed
                } else {
                    path.clone()
                };
                std::fs::remove_file(&final_path)?;
                Ok(new_name)
            })();
            match step {
                Ok(new_name) => {
                    out.renamed.push(new_name);
                    out.erased.push(e.path.clone());
                }
                Err(err) => out.failed.push(FileFailure {
                    path: e.path.clone(),
                    error: err.to_string(),
                }),
            }
        }
        for dir in &files.directories {
            if let Err(err) = std::fs::remove_dir(dir) {
                tracing::debug!(dir, error = %err, "folder not removed");
                out.failed.push(FileFailure {
                    path: dir.clone(),
                    error: format!("folder not removed: {err}"),
                });
            }
        }
    }
    let _ = &desc.path;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_rounds_up_to_the_unit() {
        assert_eq!(padded(0, 4096), 0);
        assert_eq!(padded(1, 4096), 4096);
        assert_eq!(padded(4096, 4096), 4096);
        assert_eq!(padded(4097, 4096), 8192);
        assert_eq!(padded(10, 1), 10);
    }

    #[test]
    fn random_names_keep_the_length_and_differ_by_index() {
        let seed = Seed([7u8; 32]);
        let a = random_name(&seed, 0, 12);
        let b = random_name(&seed, 1, 12);
        assert_eq!(a.len(), 12);
        assert_ne!(a, b);
        assert_eq!(random_name(&seed, 0, 3).len(), 8);
        assert_eq!(random_name(&seed, 0, 200).len(), 32);
    }
}
