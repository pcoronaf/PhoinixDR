//! The filesystem-level free-space wipe: files written through the mounted
//! volume until it is full, rewritten for every further pass, read back,
//! then deleted. Needs no raw device access and leaves the volume in use.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::certificate::VerificationRecord;
use crate::engine::EraseEvent;
use crate::error::EraseError;
use crate::method::{Pass, VerifyMode};
use crate::pattern::{CHUNK_BYTES, Seed, fill_chunk};
use crate::platform;
use crate::target::TargetDescription;
use crate::verify::{ReadBackPaths, read_back_paths};

/// Largest fill file (keeps every filesystem's file-size limit at bay).
pub const FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// What the fill produced.
#[derive(Debug)]
pub struct FillOutcome {
    /// Bytes written over every pass.
    pub bytes_written: u64,
    /// Passes completed.
    pub passes_completed: usize,
    /// Fill files created.
    pub files: usize,
    /// Bytes the fill covered (the free space found).
    pub covered: u64,
    /// The read-back, when run.
    pub verification: Option<VerificationRecord>,
    /// `Ok` when every pass completed.
    pub result: Result<(), EraseError>,
}

/// The fill directory on `mount` for a run.
#[must_use]
pub fn fill_dir(mount: &Path, seed_hex: &str) -> PathBuf {
    mount.join(format!(
        "PhoinixDR-wipe-{}",
        seed_hex.get(..8).unwrap_or("run")
    ))
}

fn is_full(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    ) || e.raw_os_error() == Some(28)
        || e.raw_os_error() == Some(112)
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
            done: done.min(self.total.max(done)),
            total: self.total.max(done),
            bytes_per_sec: self.tick_bytes.saturating_mul(1000) / millis,
            unwritable: 0,
        });
        self.last_tick = Instant::now();
        self.tick_bytes = 0;
    }
}

/// Runs the fill. `passes` is the method's pass list, `reserve` the bytes
/// to leave free, `verify` the read-back mode.
#[allow(clippy::too_many_lines)]
pub fn run(
    desc: &TargetDescription,
    passes: &[Pass],
    seed: &Seed,
    reserve: u64,
    verify: VerifyMode,
    sink: &mut dyn FnMut(EraseEvent),
    cancel: &AtomicBool,
) -> FillOutcome {
    let mount = PathBuf::from(&desc.path);
    let dir = fill_dir(&mount, &seed.to_hex());
    let mut out = FillOutcome {
        bytes_written: 0,
        passes_completed: 0,
        files: 0,
        covered: 0,
        verification: None,
        result: Ok(()),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        out.result = Err(EraseError::io(&dir, e));
        return out;
    }
    tracing::info!(dir = %dir.display(), reserve, "filling the free space through the filesystem");
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut sizes: Vec<u64> = Vec::new();
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
                total: desc.length,
                last_tick: Instant::now(),
                tick_bytes: 0,
            };
            let mut rel: u64 = 0;
            if pass_index == 0 {
                // Fill until the volume is full (or the reserve is reached).
                let mut file_index = 0usize;
                let mut current: Option<(PathBuf, File)> = None;
                let fill_result: Result<(), EraseError> = (|| {
                    'files: loop {
                        let path = dir.join(format!("wipe-{file_index:04}.bin"));
                        let file = File::create(&path).map_err(|e| EraseError::io(&path, e))?;
                        current = Some((path.clone(), file));
                        let mut written_in_file: u64 = 0;
                        while written_in_file < FILE_BYTES {
                            if cancel.load(Ordering::Relaxed) {
                                sizes.push(written_in_file);
                                return Err(EraseError::Cancelled);
                            }
                            if reserve > 0
                                && platform::free_space(&mount)
                                    .is_ok_and(|(_, free)| free <= reserve)
                            {
                                sizes.push(written_in_file);
                                break 'files;
                            }
                            let chunk = rel / CHUNK_BYTES as u64;
                            let within = usize::try_from(rel % CHUNK_BYTES as u64).unwrap_or(0);
                            if within == 0 {
                                fill_chunk(&pass.pattern, seed, pass_index, chunk, &mut buf);
                            }
                            let want = (CHUNK_BYTES - within).min(
                                usize::try_from(FILE_BYTES - written_in_file)
                                    .unwrap_or(CHUNK_BYTES),
                            );
                            let piece = buf.get(within..within + want).unwrap_or(&[]);
                            let Some((_, file)) = current.as_mut() else {
                                break 'files;
                            };
                            match write_all_counting(file, piece) {
                                Ok(n) => {
                                    rel += n;
                                    written_in_file += n;
                                    out.bytes_written += n;
                                    progress.tick(rel, n, false);
                                    if n < want as u64 {
                                        sizes.push(written_in_file);
                                        break 'files;
                                    }
                                }
                                Err(e) if is_full(&e) => {
                                    sizes.push(written_in_file);
                                    break 'files;
                                }
                                Err(e) => return Err(EraseError::io(&path, e)),
                            }
                        }
                        // The file reached its size cap: close it and open
                        // the next one.
                        sizes.push(written_in_file);
                        if let Some((p, f)) = current.take() {
                            f.sync_all().map_err(|e| EraseError::io(&p, e))?;
                        }
                        file_index += 1;
                    }
                    Ok(())
                })();
                // Flush the last file to the medium before anything else: a
                // deleted file with dirty pages might never reach the disk.
                if let Some((p, f)) = current.take()
                    && let Err(e) = f.sync_all()
                {
                    tracing::warn!(path = %p.display(), error = %e, "fill file not flushed");
                }
                fill_result?;
                out.files = sizes.len();
                out.covered = sizes.iter().sum();
                progress.total = out.covered;
                progress.tick(rel, 0, true);
            } else {
                // Rewrite every fill file in place with this pass's data.
                for (file_index, size) in sizes.iter().enumerate() {
                    let path = dir.join(format!("wipe-{file_index:04}.bin"));
                    let mut file = File::options()
                        .write(true)
                        .open(&path)
                        .map_err(|e| EraseError::io(&path, e))?;
                    let mut in_file: u64 = 0;
                    while in_file < *size {
                        if cancel.load(Ordering::Relaxed) {
                            return Err(EraseError::Cancelled);
                        }
                        let chunk = rel / CHUNK_BYTES as u64;
                        let within = usize::try_from(rel % CHUNK_BYTES as u64).unwrap_or(0);
                        if within == 0 {
                            fill_chunk(&pass.pattern, seed, pass_index, chunk, &mut buf);
                        }
                        let want = (CHUNK_BYTES - within)
                            .min(usize::try_from(size - in_file).unwrap_or(CHUNK_BYTES));
                        let piece = buf.get(within..within + want).unwrap_or(&[]);
                        file.write_all(piece)
                            .map_err(|e| EraseError::io(&path, e))?;
                        rel += want as u64;
                        in_file += want as u64;
                        out.bytes_written += want as u64;
                        progress.tick(rel, want as u64, false);
                    }
                    file.sync_all().map_err(|e| EraseError::io(&path, e))?;
                }
                progress.tick(rel, 0, true);
            }
            out.passes_completed = pass_index + 1;
            tracing::info!(pass = pass_index + 1, bytes = rel, "pass finished");
        }
        Ok(())
    })();
    out.result = result;
    out.files = sizes.len();
    out.covered = sizes.iter().sum();

    if out.result.is_ok()
        && let Some(last) = passes.last()
    {
        let mut progress = |done, total| sink(EraseEvent::Verifying { done, total });
        let items: Vec<(PathBuf, u64)> = sizes
            .iter()
            .enumerate()
            .map(|(i, size)| (dir.join(format!("wipe-{i:04}.bin")), *size))
            .collect();
        out.verification = Some(read_back_paths(
            &ReadBackPaths {
                items: &items,
                last: &last.pattern,
                seed,
                pass_index: passes.len() - 1,
                mode: verify,
            },
            &mut progress,
            cancel,
        ));
    }
    // Always remove the fill files, whatever happened.
    for (file_index, _) in sizes.iter().enumerate() {
        let path = dir.join(format!("wipe-{file_index:04}.bin"));
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(path = %path.display(), error = %e, "fill file not removed");
        }
    }
    if let Err(e) = std::fs::remove_dir(&dir) {
        tracing::warn!(dir = %dir.display(), error = %e, "fill directory not removed");
    }
    out
}

/// Writes as much of `piece` as fits; a full volume ends the write with the
/// count written so far.
fn write_all_counting(file: &mut File, piece: &[u8]) -> io::Result<u64> {
    let mut done = 0usize;
    while done < piece.len() {
        match file.write(piece.get(done..).unwrap_or(&[])) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_full(&e) && done > 0 => break,
            Err(e) => return Err(e),
        }
    }
    Ok(done as u64)
}
