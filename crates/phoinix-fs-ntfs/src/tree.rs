//! Path reconstruction from `$FILE_NAME.parent` references.
//!
//! Full directory-index parsing is not required for M3: walking parent
//! references upward is sufficient and also works for deleted files, whose
//! records are no longer indexed.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::diagnostic::NtfsDiagnostic;
use crate::file::NtfsFile;
use crate::mft::ROOT_RECORD;
use crate::record::FileReference;
use crate::volume::NtfsVolume;

/// Maximum number of parent links followed.
pub const MAX_PARENT_DEPTH: usize = 1024;

/// A reconstructed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPath {
    /// Windows-style path (`\Users\Pablo\document.docx`). When the chain is
    /// uncertain the path starts with `\?\`.
    pub path: String,
    /// Whether some ancestor could not be trusted (reused record, unreadable
    /// record, loop, depth limit).
    pub uncertain: bool,
    /// Whether some ancestor directory is itself deleted.
    pub via_deleted_directory: bool,
    /// Findings.
    pub diagnostics: Vec<NtfsDiagnostic>,
}

#[derive(Debug, Clone)]
struct CachedRecord {
    name: Option<String>,
    parent: Option<FileReference>,
    sequence: u16,
    in_use: bool,
}

/// Resolves paths by walking parent references, with a per-instance cache.
pub struct PathResolver<'a> {
    volume: &'a NtfsVolume,
    cache: RefCell<HashMap<u64, Result<CachedRecord, String>>>,
}

impl<'a> PathResolver<'a> {
    /// Creates a resolver over `volume`.
    #[must_use]
    pub fn new(volume: &'a NtfsVolume) -> Self {
        Self {
            volume,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn lookup(&self, record: u64) -> Result<CachedRecord, String> {
        if let Some(cached) = self.cache.borrow().get(&record) {
            return cached.clone();
        }
        let loaded = self.volume.file(record).map(|f| CachedRecord {
            name: f.name().map(str::to_owned),
            parent: f.preferred_name().map(|n| n.parent),
            sequence: f.reference.sequence,
            in_use: f.in_use,
        });
        let result = loaded.map_err(|e: NtfsError| e.to_string());
        self.cache.borrow_mut().insert(record, result.clone());
        result
    }

    /// Resolves the path of `file`.
    #[must_use]
    pub fn resolve(&self, file: &NtfsFile) -> ResolvedPath {
        let mut diagnostics = Vec::new();
        let mut uncertain = false;
        let mut via_deleted = false;
        let mut components: Vec<String> = Vec::new();

        let Some(own) = file.preferred_name() else {
            return ResolvedPath {
                path: format!("\\?\\<record {}>", file.reference.record),
                uncertain: true,
                via_deleted_directory: false,
                diagnostics: vec![NtfsDiagnostic::NoFileName],
            };
        };
        components.push(own.name.clone());
        let mut parent = own.parent;
        let mut visited: HashSet<u64> = HashSet::new();
        visited.insert(file.reference.record);

        for depth in 0..=MAX_PARENT_DEPTH {
            if parent.record == ROOT_RECORD {
                break;
            }
            if depth == MAX_PARENT_DEPTH {
                diagnostics.push(NtfsDiagnostic::PathDepthExceeded);
                uncertain = true;
                break;
            }
            if !visited.insert(parent.record) {
                diagnostics.push(NtfsDiagnostic::PathLoop);
                uncertain = true;
                break;
            }
            let entry = match self.lookup(parent.record) {
                Ok(e) => e,
                Err(reason) => {
                    diagnostics.push(NtfsDiagnostic::ParentUnreadable {
                        parent: parent.record,
                        reason,
                    });
                    uncertain = true;
                    break;
                }
            };
            // A freed record has its sequence number bumped once; a reused
            // record has been bumped and is in use again (or bumped further).
            let sequence_matches = entry.sequence == parent.sequence;
            let freed_once = !entry.in_use && entry.sequence == parent.sequence.wrapping_add(1);
            if !(sequence_matches || freed_once) {
                diagnostics.push(NtfsDiagnostic::ParentReferenceStale {
                    parent: parent.record,
                    expected_sequence: parent.sequence,
                    actual_sequence: entry.sequence,
                });
                uncertain = true;
                break;
            }
            if !entry.in_use {
                diagnostics.push(NtfsDiagnostic::ParentDeleted {
                    parent: parent.record,
                });
                via_deleted = true;
            }
            let Some(name) = entry.name else {
                diagnostics.push(NtfsDiagnostic::ParentUnnamed {
                    parent: parent.record,
                });
                uncertain = true;
                break;
            };
            components.push(name);
            match entry.parent {
                Some(p) => parent = p,
                None => {
                    uncertain = true;
                    break;
                }
            }
        }

        components.reverse();
        let mut path = String::new();
        if uncertain {
            path.push_str("\\?");
        }
        for c in &components {
            path.push('\\');
            path.push_str(c);
        }
        ResolvedPath {
            path,
            uncertain,
            via_deleted_directory: via_deleted,
            diagnostics,
        }
    }
}
