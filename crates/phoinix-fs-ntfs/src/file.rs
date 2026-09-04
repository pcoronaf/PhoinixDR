//! Assembly of a logical file from one base record and its extensions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::attribute::{Attribute, AttributeBody, AttributeType};
use crate::attribute_list::parse_attribute_list;
use crate::data::{DataStorage, DataStreamDescriptor};
use crate::diagnostic::NtfsDiagnostic;
use crate::filename::{FileNameAttribute, preferred_name};
use crate::mft::Mft;
use crate::record::{FileRecord, FileReference};
use crate::runlist::{NtfsRun, decode_runlist};
use crate::standard_information::StandardInformation;
use crate::stream::NtfsDataStream;

/// Upper bound on an attribute list PHOINIX will read for one file.
const MAX_ATTRIBUTE_LIST_BYTES: u64 = 4 * 1024 * 1024;
/// Upper bound on extension records followed for one file.
const MAX_EXTENSION_RECORDS: usize = 4096;

/// A file (or directory) as reconstructed from its MFT records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfsFile {
    /// Base record reference.
    pub reference: FileReference,
    /// Whether the base record is in use.
    pub in_use: bool,
    /// Whether the record describes a directory.
    pub directory: bool,
    /// Whether the record is a base record (not an extension).
    pub is_base: bool,
    /// Hard link count from the header.
    pub hard_link_count: u16,
    /// `$FILE_NAME` attributes (all namespaces).
    pub names: Vec<FileNameAttribute>,
    /// `$DATA` streams.
    pub streams: Vec<DataStreamDescriptor>,
    /// `$STANDARD_INFORMATION`, if present.
    pub standard_information: Option<StandardInformation>,
    /// Whether the record carries an `$ATTRIBUTE_LIST`.
    pub has_attribute_list: bool,
    /// Extension records that contributed attributes.
    pub extension_records: Vec<u64>,
    /// Findings made during assembly.
    pub diagnostics: Vec<NtfsDiagnostic>,
}

impl NtfsFile {
    /// Preferred display name (Win32 over DOS), if any.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        preferred_name(&self.names).map(|n| n.name.as_str())
    }

    /// The preferred `$FILE_NAME` attribute, if any.
    #[must_use]
    pub fn preferred_name(&self) -> Option<&FileNameAttribute> {
        preferred_name(&self.names)
    }

    /// The unnamed (default) data stream, if any.
    #[must_use]
    pub fn unnamed_stream(&self) -> Option<&DataStreamDescriptor> {
        self.streams.iter().find(|s| s.is_unnamed())
    }

    /// A data stream by name (`None` for the unnamed stream).
    #[must_use]
    pub fn stream(&self, name: Option<&str>) -> Option<&DataStreamDescriptor> {
        match name {
            None | Some("") => self.unnamed_stream(),
            Some(n) => self.streams.iter().find(|s| s.name.as_deref() == Some(n)),
        }
    }

    /// Logical size of the unnamed stream, if any.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.unnamed_stream().map(|s| s.logical_size)
    }

    /// Builds the file from `base`, following `$ATTRIBUTE_LIST` into
    /// extension records read through `mft`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the base record itself cannot be interpreted
    /// at all; damage elsewhere becomes diagnostics.
    pub fn assemble(
        mft: &Mft,
        base: &FileRecord,
        cluster_size: u32,
        total_clusters: u64,
    ) -> Result<Self, NtfsError> {
        let mut builder = Builder::new(base, cluster_size, total_clusters);
        builder.consume_record(base, mft);

        if let Some(list) = builder.attribute_list.take() {
            builder.has_attribute_list = true;
            match builder.load_attribute_list(mft, list) {
                Ok(entries) => builder.follow_extensions(mft, entries),
                Err(e) => builder
                    .diagnostics
                    .push(NtfsDiagnostic::AttributeListIncomplete {
                        reason: e.to_string(),
                    }),
            }
        }
        Ok(builder.finish())
    }
}

/// A non-resident `$DATA` piece before merging.
struct DataPiece {
    starting_vcn: u64,
    runs: Vec<NtfsRun>,
    real_size: u64,
    initialized_size: u64,
    allocated_size: u64,
    last_vcn: u64,
    flags: u16,
    compression_unit: u8,
    runlist_error: Option<String>,
}

enum PendingList {
    Resident(Vec<u8>),
    NonResident(Vec<NtfsRun>, u64),
}

struct Builder {
    reference: FileReference,
    in_use: bool,
    directory: bool,
    is_base: bool,
    hard_link_count: u16,
    cluster_size: u32,
    total_clusters: u64,
    names: Vec<FileNameAttribute>,
    resident_streams: Vec<DataStreamDescriptor>,
    pieces: BTreeMap<String, Vec<DataPiece>>,
    standard_information: Option<StandardInformation>,
    attribute_list: Option<PendingList>,
    has_attribute_list: bool,
    extension_records: Vec<u64>,
    diagnostics: Vec<NtfsDiagnostic>,
}

impl Builder {
    fn new(base: &FileRecord, cluster_size: u32, total_clusters: u64) -> Self {
        let h = base.header();
        Self {
            reference: base.reference(),
            in_use: h.in_use(),
            directory: h.is_directory(),
            is_base: h.is_base(),
            hard_link_count: h.hard_link_count,
            cluster_size,
            total_clusters,
            names: Vec::new(),
            resident_streams: Vec::new(),
            pieces: BTreeMap::new(),
            standard_information: None,
            attribute_list: None,
            has_attribute_list: false,
            extension_records: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn consume_record(&mut self, record: &FileRecord, _mft: &Mft) {
        for attr in record.attributes() {
            match attr {
                Ok(attr) => self.consume_attribute(record.number(), &attr),
                Err(e) => {
                    let offset = match &e {
                        NtfsError::InvalidAttribute { offset, .. } => *offset,
                        _ => 0,
                    };
                    self.diagnostics.push(NtfsDiagnostic::AttributeError {
                        offset,
                        reason: e.to_string(),
                    });
                }
            }
        }
    }

    fn consume_attribute(&mut self, record_number: u64, attr: &Attribute<'_>) {
        match attr.header.attribute_type {
            AttributeType::StandardInformation => {
                if let Some(value) = attr.resident_value()
                    && self.standard_information.is_none()
                {
                    match StandardInformation::parse(record_number, attr.offset, value) {
                        Ok(si) => self.standard_information = Some(si),
                        Err(e) => self.diagnostics.push(NtfsDiagnostic::AttributeError {
                            offset: attr.offset,
                            reason: e.to_string(),
                        }),
                    }
                }
            }
            AttributeType::FileName => {
                if let Some(value) = attr.resident_value() {
                    match FileNameAttribute::parse(record_number, attr.offset, value) {
                        Ok(fnm) => {
                            if fnm.name_invalid_utf16 {
                                self.diagnostics.push(NtfsDiagnostic::NameInvalidUtf16);
                            }
                            if !self.names.contains(&fnm) {
                                self.names.push(fnm);
                            }
                        }
                        Err(e) => self.diagnostics.push(NtfsDiagnostic::AttributeError {
                            offset: attr.offset,
                            reason: e.to_string(),
                        }),
                    }
                }
            }
            AttributeType::AttributeList => {
                if self.attribute_list.is_none() {
                    self.attribute_list = Some(match &attr.body {
                        AttributeBody::Resident { value, .. } => {
                            PendingList::Resident(value.to_vec())
                        }
                        AttributeBody::NonResident {
                            header,
                            mapping_pairs,
                        } => {
                            match decode_runlist(
                                mapping_pairs,
                                header.starting_vcn,
                                self.total_clusters,
                            ) {
                                Ok(runs) => PendingList::NonResident(runs, header.real_size),
                                Err(e) => {
                                    self.diagnostics.push(
                                        NtfsDiagnostic::AttributeListIncomplete {
                                            reason: e.to_string(),
                                        },
                                    );
                                    return;
                                }
                            }
                        }
                    });
                }
            }
            AttributeType::Data => {
                let name = attr.header.name.clone().filter(|n| !n.is_empty());
                match &attr.body {
                    AttributeBody::Resident { value, .. } => {
                        if self.resident_streams.iter().any(|s| s.name == name)
                            || self.pieces.contains_key(name.as_deref().unwrap_or(""))
                        {
                            return;
                        }
                        self.resident_streams.push(DataStreamDescriptor {
                            name,
                            logical_size: value.len() as u64,
                            storage: DataStorage::Resident {
                                value: value.to_vec(),
                            },
                            flags: attr.header.flags,
                        });
                    }
                    AttributeBody::NonResident {
                        header,
                        mapping_pairs,
                    } => {
                        let (runs, runlist_error) = match decode_runlist(
                            mapping_pairs,
                            header.starting_vcn,
                            self.total_clusters,
                        ) {
                            Ok(runs) => (runs, None),
                            Err(e) => (Vec::new(), Some(e.to_string())),
                        };
                        let key = name.clone().unwrap_or_default();
                        let pieces = self.pieces.entry(key).or_default();
                        if pieces.iter().any(|p| p.starting_vcn == header.starting_vcn) {
                            return;
                        }
                        pieces.push(DataPiece {
                            starting_vcn: header.starting_vcn,
                            runs,
                            real_size: header.real_size,
                            initialized_size: header.initialized_size,
                            allocated_size: header.allocated_size,
                            last_vcn: header.last_vcn,
                            flags: attr.header.flags,
                            compression_unit: header.compression_unit,
                            runlist_error,
                        });
                    }
                }
            }
            AttributeType::Unknown(code) => self
                .diagnostics
                .push(NtfsDiagnostic::UnknownAttribute { code }),
            _ => {}
        }
    }

    fn load_attribute_list(
        &mut self,
        _mft: &Mft,
        list: PendingList,
    ) -> Result<Vec<crate::attribute_list::AttributeListEntry>, NtfsError> {
        let bytes = match list {
            PendingList::Resident(bytes) => bytes,
            PendingList::NonResident(runs, len) => {
                let reader = _mft.stream().clone();
                // The list lives in clusters of the volume: read it through the
                // same reader the MFT stream uses.
                let stream = NtfsDataStream::non_resident(
                    volume_reader(&reader)?,
                    self.cluster_size,
                    runs,
                    len,
                    len,
                );
                stream.read_all(MAX_ATTRIBUTE_LIST_BYTES)?
            }
        };
        parse_attribute_list(self.reference.record, &bytes)
    }

    fn follow_extensions(
        &mut self,
        mft: &Mft,
        entries: Vec<crate::attribute_list::AttributeListEntry>,
    ) {
        let base = self.reference;
        let mut seen: Vec<u64> = Vec::new();
        for entry in entries {
            let rec = entry.reference.record;
            if rec == base.record || seen.contains(&rec) {
                continue;
            }
            if seen.len() >= MAX_EXTENSION_RECORDS {
                self.diagnostics
                    .push(NtfsDiagnostic::AttributeListIncomplete {
                        reason: "too many extension records".into(),
                    });
                break;
            }
            seen.push(rec);
            match mft.record(rec) {
                Ok(ext) => {
                    let b = ext.header().base_reference;
                    if b.record != base.record || b.sequence != base.sequence {
                        self.diagnostics
                            .push(NtfsDiagnostic::ExtensionRecordUnavailable {
                                record: rec,
                                reason: format!("base reference {b} does not match {base}"),
                            });
                        continue;
                    }
                    self.extension_records.push(rec);
                    self.consume_record(&ext, mft);
                }
                Err(e) => self
                    .diagnostics
                    .push(NtfsDiagnostic::ExtensionRecordUnavailable {
                        record: rec,
                        reason: e.to_string(),
                    }),
            }
        }
    }

    fn finish(mut self) -> NtfsFile {
        let mut streams = std::mem::take(&mut self.resident_streams);
        let cluster = u64::from(self.cluster_size.max(1));
        for (key, mut pieces) in std::mem::take(&mut self.pieces) {
            pieces.sort_by_key(|p| p.starting_vcn);
            let name = if key.is_empty() { None } else { Some(key) };
            let Some(head) = pieces.iter().find(|p| p.starting_vcn == 0) else {
                self.diagnostics.push(NtfsDiagnostic::RunlistIncomplete {
                    name: name.clone(),
                    covered_clusters: 0,
                    expected_clusters: 0,
                });
                continue;
            };
            let (real_size, initialized_size, allocated_size, flags, cu) = (
                head.real_size,
                head.initialized_size,
                head.allocated_size,
                head.flags,
                head.compression_unit,
            );
            let mut runs: Vec<NtfsRun> = Vec::new();
            let mut complete = true;
            for p in &pieces {
                if let Some(err) = &p.runlist_error {
                    self.diagnostics.push(NtfsDiagnostic::RunlistError {
                        name: name.clone(),
                        reason: err.clone(),
                    });
                    complete = false;
                }
                runs.extend(p.runs.iter().copied());
            }
            runs.sort_by_key(NtfsRun::vcn);
            // Contiguity: each run must start where the previous ended.
            let mut expected_vcn = 0u64;
            for r in &runs {
                if r.vcn() != expected_vcn {
                    complete = false;
                }
                expected_vcn = r.end_vcn();
            }
            let covered = expected_vcn;
            let expected_clusters = pieces
                .last()
                .map_or(0, |p| p.last_vcn.saturating_add(1))
                .max(allocated_size.div_ceil(cluster));
            if covered < expected_clusters {
                complete = false;
                self.diagnostics.push(NtfsDiagnostic::RunlistIncomplete {
                    name: name.clone(),
                    covered_clusters: covered,
                    expected_clusters,
                });
            }
            let storage = if flags & crate::attribute::FLAG_COMPRESSION_MASK != 0 {
                self.diagnostics
                    .push(NtfsDiagnostic::CompressedStream { name: name.clone() });
                DataStorage::UnsupportedCompressed {
                    runs,
                    real_size,
                    compression_unit: cu,
                }
            } else if flags & crate::attribute::FLAG_ENCRYPTED != 0 {
                self.diagnostics
                    .push(NtfsDiagnostic::EncryptedStream { name: name.clone() });
                DataStorage::UnsupportedEncrypted { runs, real_size }
            } else {
                DataStorage::NonResident {
                    runs,
                    real_size,
                    initialized_size,
                    allocated_size,
                    complete,
                }
            };
            streams.push(DataStreamDescriptor {
                name,
                logical_size: real_size,
                storage,
                flags,
            });
        }
        // Unnamed stream first, then named streams alphabetically.
        streams.sort_by(|a, b| a.name.cmp(&b.name));
        if self.names.is_empty() && !self.directory {
            self.diagnostics.push(NtfsDiagnostic::NoFileName);
        }
        NtfsFile {
            reference: self.reference,
            in_use: self.in_use,
            directory: self.directory,
            is_base: self.is_base,
            hard_link_count: self.hard_link_count,
            names: self.names,
            streams,
            standard_information: self.standard_information,
            has_attribute_list: self.has_attribute_list,
            extension_records: self.extension_records,
            diagnostics: self.diagnostics,
        }
    }
}

/// Extracts the volume reader underlying an MFT stream.
fn volume_reader(
    stream: &NtfsDataStream,
) -> Result<std::sync::Arc<dyn phoinix_block::BlockReader>, NtfsError> {
    stream
        .volume_reader()
        .ok_or_else(|| NtfsError::Unsupported("MFT stream is resident".into()))
}
