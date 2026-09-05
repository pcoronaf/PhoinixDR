//! The recovery writer.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use phoinix_fs::{DeletedFileProvider, FsError, RecoveryCandidate};
use phoinix_health::RecoveryDiagnostic;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::destination::{DestinationCheck, check_destination};
use crate::names::{sanitize_component, sanitize_relative_path};

/// What to recover and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRequest {
    /// Destination directory (created if missing).
    pub destination: PathBuf,
    /// Recreate the original directory tree under the destination.
    pub preserve_tree: bool,
    /// Apply the original modification time to the output.
    pub preserve_timestamps: bool,
    /// Compute SHA-256 while writing.
    pub hash_after_write: bool,
    /// Allow a destination on the same disk as a device source (expert
    /// override; never the default).
    pub allow_same_device: bool,
    /// Overwrite an existing file instead of choosing a new name.
    pub overwrite: bool,
}

impl RecoveryRequest {
    /// Safe defaults for `destination`.
    #[must_use]
    pub fn new(destination: impl Into<PathBuf>) -> Self {
        Self {
            destination: destination.into(),
            preserve_tree: false,
            preserve_timestamps: true,
            hash_after_write: true,
            allow_same_device: false,
            overwrite: false,
        }
    }
}

/// Outcome of one recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryResult {
    /// Where the data was written.
    pub output_path: PathBuf,
    /// Bytes the candidate declared.
    pub bytes_expected: Option<u64>,
    /// Bytes actually written.
    pub bytes_written: u64,
    /// SHA-256 of the written bytes (lowercase hex), if requested.
    pub sha256: Option<String>,
    /// Whether every expected byte was written.
    pub complete: bool,
    /// Findings.
    pub diagnostics: Vec<RecoveryDiagnostic>,
}

/// Recovery failures.
#[derive(Debug, Error)]
pub enum RecoveryError {
    /// The destination lies on the source disk.
    #[error("{0}")]
    DangerousDestination(String),

    /// The destination exists but is not a directory.
    #[error("destination {0} is not a directory")]
    NotADirectory(PathBuf),

    /// The candidate's content cannot be produced.
    #[error(transparent)]
    Fs(#[from] FsError),

    /// An I/O error on the destination.
    #[error("I/O error writing {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Writes candidates to a destination.
pub struct RecoveryWriter<'a> {
    provider: &'a dyn DeletedFileProvider,
    source_path: PathBuf,
    request: RecoveryRequest,
    check: DestinationCheck,
}

impl std::fmt::Debug for RecoveryWriter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryWriter")
            .field("source_path", &self.source_path)
            .field("request", &self.request)
            .finish()
    }
}

impl<'a> RecoveryWriter<'a> {
    /// Prepares a writer, checking the destination against the source.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError::DangerousDestination`] unless
    /// `request.allow_same_device` is set, and [`RecoveryError::NotADirectory`]
    /// or I/O errors if the destination cannot be used.
    pub fn new(
        provider: &'a dyn DeletedFileProvider,
        source_path: &Path,
        request: RecoveryRequest,
    ) -> Result<Self, RecoveryError> {
        let check = check_destination(source_path, &request.destination);
        let overridden = request.allow_same_device && !check.overwrites_source_image;
        if check.is_dangerous() && !overridden {
            return Err(RecoveryError::DangerousDestination(
                check
                    .warning()
                    .unwrap_or_else(|| "dangerous destination".into()),
            ));
        }
        if let Some(w) = check.warning() {
            tracing::warn!("{w}");
        }
        if request.destination.exists() {
            if !request.destination.is_dir() {
                return Err(RecoveryError::NotADirectory(request.destination.clone()));
            }
        } else {
            fs::create_dir_all(&request.destination).map_err(|e| RecoveryError::Io {
                path: request.destination.clone(),
                source: e,
            })?;
        }
        Ok(Self {
            provider,
            source_path: source_path.to_path_buf(),
            request,
            check,
        })
    }

    /// The destination check performed at construction.
    #[must_use]
    pub const fn destination_check(&self) -> &DestinationCheck {
        &self.check
    }

    /// The source path.
    #[must_use]
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Chooses the output path for `candidate`.
    fn output_path(&self, candidate: &RecoveryCandidate) -> PathBuf {
        let mut dir = self.request.destination.clone();
        if self.request.preserve_tree
            && let Some(p) = &candidate.original_path
        {
            dir = dir.join(sanitize_relative_path(p));
        }
        let name = sanitize_component(&candidate.display_name());
        let mut path = dir.join(&name);
        if !self.request.overwrite {
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            let mut n = 1u32;
            while path.exists() {
                path = dir.join(format!("{stem} ({n}){ext}"));
                n += 1;
            }
        }
        path
    }

    /// Recovers one candidate.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError::Fs`] if the content cannot be opened and
    /// [`RecoveryError::Io`] for destination failures. A read failure part
    /// way through is *not* an error: the partial output is kept under a
    /// `.partial` name and reported in the result.
    pub fn recover(&self, candidate: &RecoveryCandidate) -> Result<RecoveryResult, RecoveryError> {
        let mut content = self.provider.open_content(candidate)?;
        let expected = content.len();
        let path = self.output_path(candidate);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| RecoveryError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let io = |e: std::io::Error| RecoveryError::Io {
            path: path.clone(),
            source: e,
        };
        let mut file = File::create(&path).map_err(io)?;
        let mut hasher = self.request.hash_after_write.then(Sha256::new);
        let mut written = 0u64;
        let mut buf = vec![0u8; 1024 * 1024];
        let mut diagnostics = Vec::new();
        let mut read_error: Option<String> = None;
        loop {
            let n = match content.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    read_error = Some(e.to_string());
                    break;
                }
            };
            let chunk = buf.get(..n).unwrap_or(&[]);
            file.write_all(chunk).map_err(io)?;
            if let Some(h) = hasher.as_mut() {
                h.update(chunk);
            }
            written += n as u64;
        }
        file.flush().map_err(io)?;

        let complete = read_error.is_none() && written == expected;
        let mut output_path = path.clone();
        if !complete {
            let message = match &read_error {
                Some(e) => format!(
                    "Content could not be read completely ({e}); {written} of {expected} bytes were written"
                ),
                None => format!("{written} of {expected} bytes were written"),
            };
            diagnostics.push(RecoveryDiagnostic::warning(message));
            let partial = path.with_extension(match path.extension() {
                Some(ext) => format!("{}.partial", ext.to_string_lossy()),
                None => "partial".to_owned(),
            });
            drop(file);
            fs::rename(&path, &partial).map_err(io)?;
            output_path = partial;
        } else if self.request.preserve_timestamps
            && let Some(modified) = candidate.timestamps.modified
            && let Ok(secs) = u64::try_from(modified)
        {
            let time = UNIX_EPOCH + Duration::from_secs(secs);
            let accessed = candidate
                .timestamps
                .accessed
                .and_then(|a| u64::try_from(a).ok())
                .map_or(time, |a| UNIX_EPOCH + Duration::from_secs(a));
            let times = fs::FileTimes::new()
                .set_modified(time)
                .set_accessed(accessed);
            if let Err(e) = file.set_times(times) {
                diagnostics.push(RecoveryDiagnostic::info(format!(
                    "Original timestamps could not be applied: {e}"
                )));
            }
        }
        let sha256 = hasher.map(|h| h.finalize().iter().map(|b| format!("{b:02x}")).collect());
        tracing::info!(output = %output_path.display(), written, expected, complete, "recovered candidate");
        Ok(RecoveryResult {
            output_path,
            bytes_expected: Some(expected),
            bytes_written: written,
            sha256,
            complete,
            diagnostics,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]

    use std::io::{self, Read};

    use phoinix_core::{CandidateId, FileSystemType, SourceId};
    use phoinix_fs::{CandidateContent, CandidateTimestamps, FileSystemObjectId};
    use phoinix_health::{RecoveryEvidence, RecoveryHealth};

    use super::*;

    struct FakeContent {
        data: Vec<u8>,
        pos: usize,
        fail_at: Option<usize>,
    }

    impl Read for FakeContent {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(f) = self.fail_at
                && self.pos >= f
            {
                return Err(io::Error::other("simulated unreadable extent"));
            }
            let end = self
                .data
                .len()
                .min(self.pos + buf.len())
                .min(self.fail_at.unwrap_or(usize::MAX));
            let n = end - self.pos;
            buf[..n].copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    impl CandidateContent for FakeContent {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
    }

    struct FakeProvider {
        data: Vec<u8>,
        fail_at: Option<usize>,
    }

    impl DeletedFileProvider for FakeProvider {
        fn deleted_files(
            &self,
        ) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_> {
            Box::new(std::iter::empty())
        }
        fn candidate(&self, _: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError> {
            Err(FsError::NotFound("n/a".into()))
        }
        fn object_from_reference(&self, _: &str) -> Result<FileSystemObjectId, FsError> {
            Err(FsError::NotFound("n/a".into()))
        }
        fn open_content(
            &self,
            _: &RecoveryCandidate,
        ) -> Result<Box<dyn CandidateContent>, FsError> {
            Ok(Box::new(FakeContent {
                data: self.data.clone(),
                pos: 0,
                fail_at: self.fail_at,
            }))
        }
    }

    fn candidate(name: &str, path: &str) -> RecoveryCandidate {
        RecoveryCandidate {
            id: CandidateId::new(),
            source_id: SourceId::nil(),
            filesystem: FileSystemType::Ntfs,
            filesystem_object: FileSystemObjectId::Ntfs {
                record: 64,
                sequence: 2,
                stream: None,
            },
            original_name: Some(name.into()),
            original_path: Some(path.into()),
            path_uncertain: false,
            logical_size: Some(5),
            deleted: true,
            timestamps: CandidateTimestamps {
                modified: Some(1_700_000_000),
                ..Default::default()
            },
            evidence: RecoveryEvidence::default(),
            health: RecoveryHealth::unknown("test"),
        }
    }

    #[test]
    fn recovers_with_hash_tree_and_duplicate_names() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("src.img");
        fs::write(&image, b"image").unwrap();
        let provider = FakeProvider {
            data: b"hello".to_vec(),
            fail_at: None,
        };
        let mut req = RecoveryRequest::new(dir.path().join("out"));
        req.preserve_tree = true;
        let writer = RecoveryWriter::new(&provider, &image, req).unwrap();
        let c = candidate("doc:x.txt", "\\Users\\Pablo\\doc:x.txt");
        let r1 = writer.recover(&c).unwrap();
        assert!(r1.complete);
        assert_eq!(r1.bytes_written, 5);
        assert_eq!(
            r1.sha256.as_deref(),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
        assert_eq!(r1.output_path, dir.path().join("out/Users/Pablo/doc_x.txt"));
        assert_eq!(fs::read(&r1.output_path).unwrap(), b"hello");
        let mtime = fs::metadata(&r1.output_path).unwrap().modified().unwrap();
        assert_eq!(
            mtime.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            1_700_000_000
        );
        let r2 = writer.recover(&c).unwrap();
        assert_eq!(
            r2.output_path,
            dir.path().join("out/Users/Pablo/doc_x (1).txt")
        );
    }

    #[test]
    fn partial_read_keeps_partial_output_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("src.img");
        fs::write(&image, b"image").unwrap();
        let provider = FakeProvider {
            data: vec![7u8; 3000],
            fail_at: Some(1000),
        };
        let writer = RecoveryWriter::new(
            &provider,
            &image,
            RecoveryRequest::new(dir.path().join("out")),
        )
        .unwrap();
        let r = writer.recover(&candidate("big.bin", "\\big.bin")).unwrap();
        assert!(!r.complete);
        assert_eq!(r.bytes_written, 1000);
        assert!(r.output_path.to_string_lossy().ends_with("big.bin.partial"));
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("1000 of 3000"))
        );
    }

    #[test]
    fn refuses_to_write_onto_the_source_image() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("src.img");
        fs::write(&image, b"image").unwrap();
        let provider = FakeProvider {
            data: Vec::new(),
            fail_at: None,
        };
        let err = RecoveryWriter::new(&provider, &image, RecoveryRequest::new(&image)).unwrap_err();
        assert!(matches!(err, RecoveryError::DangerousDestination(_)));
    }
}
