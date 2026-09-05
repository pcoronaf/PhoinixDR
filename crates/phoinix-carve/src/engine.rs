//! The carving engine: unallocated-space iteration, header search,
//! assembly, evidence, scoring and deduplication against metadata
//! candidates.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use phoinix_block::BlockReader;
use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_fs::{
    AllocationView, ByteRange, CandidateContent, CandidateTimestamps, DeletedFileProvider, Extent,
    ExtentStream, ExtentStreamCursor, FileSystemObjectId, FsError, RecoveryCandidate,
};
use phoinix_health::validate::{DEFAULT_BYTE_BUDGET, examine};
use phoinix_health::{
    AllocationEvidence, CandidateSource, ContentEvidence, ExtentEvidence, FileTypeDetection,
    MetadataEvidence, RecoveryDiagnostic, RecoveryEvidence, ScoringModel, StorageEvidence,
    ValidationResult, ValidationStatus, assess_zero_content, score,
};
use serde::{Deserialize, Serialize};

use crate::CarveError;
use crate::assemble::{Assembly, assembler_for};
use crate::probe::Probe;
use crate::scanner::{Hit, ScanOptions, ScanProgress, find_headers};
use crate::signature::{CarveSignature, SignatureSet};

/// Carving parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarveOptions {
    /// Header search parameters.
    pub scan: ScanOptions,
    /// Scan the whole volume instead of the unallocated ranges.
    pub whole_volume: bool,
    /// Drop assembled files shorter than this.
    pub min_size: u64,
    /// Run the content validators and zero sampling on carved files.
    pub examine_content: bool,
    /// Validator byte budget per file.
    pub byte_budget: u64,
}

impl Default for CarveOptions {
    fn default() -> Self {
        Self {
            scan: ScanOptions::default(),
            whole_volume: false,
            min_size: 0,
            examine_content: true,
            byte_budget: DEFAULT_BYTE_BUDGET,
        }
    }
}

/// Statistics of a carving run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CarveReport {
    /// Bytes scanned for headers.
    pub bytes_scanned: u64,
    /// Bytes that were eligible (unallocated, or the whole volume).
    pub bytes_eligible: u64,
    /// Header hits.
    pub hits: usize,
    /// Hits inside an already assembled, sound file (embedded thumbnails,
    /// ZIP members) that were skipped.
    pub nested_skipped: usize,
    /// Hits rejected by the assembler as false positives.
    pub rejected: usize,
    /// Hits dropped for being shorter than the minimum size.
    pub too_small: usize,
    /// Candidates produced.
    pub candidates: usize,
    /// Carved candidates merged into metadata candidates by
    /// [`CarveEngine::deduplicate`].
    pub merged_into_metadata: usize,
}

/// The carving engine over one volume.
pub struct CarveEngine {
    reader: Arc<dyn BlockReader>,
    space: Arc<dyn AllocationView>,
    signatures: Arc<SignatureSet>,
    options: CarveOptions,
    storage: StorageEvidence,
    model: ScoringModel,
    source_id: SourceId,
    filesystem: FileSystemType,
}

impl std::fmt::Debug for CarveEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CarveEngine")
            .field("source", &self.reader.describe())
            .field("signatures", &self.signatures.ids())
            .field("options", &self.options)
            .finish()
    }
}

impl CarveEngine {
    /// Creates an engine over `reader` using `space` to find free ranges.
    #[must_use]
    pub fn new(
        reader: Arc<dyn BlockReader>,
        space: Arc<dyn AllocationView>,
        filesystem: FileSystemType,
        storage: StorageEvidence,
    ) -> Self {
        let source_id = reader.id();
        Self {
            reader,
            space,
            signatures: Arc::new(SignatureSet::builtin()),
            options: CarveOptions::default(),
            storage,
            model: ScoringModel::default(),
            source_id,
            filesystem,
        }
    }

    /// Uses a custom signature set.
    #[must_use]
    pub fn with_signatures(mut self, signatures: SignatureSet) -> Self {
        self.signatures = Arc::new(signatures);
        self
    }

    /// Uses custom options.
    #[must_use]
    pub fn with_options(mut self, options: CarveOptions) -> Self {
        self.options = options;
        self
    }

    /// Uses a custom scoring model.
    #[must_use]
    pub fn with_model(mut self, model: ScoringModel) -> Self {
        self.model = model;
        self
    }

    /// The signature set.
    #[must_use]
    pub fn signatures(&self) -> &SignatureSet {
        &self.signatures
    }

    /// The options.
    #[must_use]
    pub const fn options(&self) -> &CarveOptions {
        &self.options
    }

    /// The ranges that will be scanned.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError`] if the allocation structures cannot be read.
    pub fn ranges(&self) -> Result<Vec<ByteRange>, CarveError> {
        if self.options.whole_volume || !self.space.map_available() {
            return Ok(vec![ByteRange {
                offset: 0,
                length: self.space.volume_len().min(self.reader.len()),
            }]);
        }
        Ok(self.space.free_ranges()?)
    }

    /// Runs the deep scan: header search, assembly, scoring.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError`] for I/O failures; individual hits never fail
    /// the scan.
    pub fn carve(
        &self,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> Result<(Vec<RecoveryCandidate>, CarveReport), CarveError> {
        let ranges = self.ranges()?;
        let mut report = CarveReport {
            bytes_eligible: ranges.iter().map(|r| r.length).sum(),
            ..Default::default()
        };
        let hits = find_headers(
            &*self.reader,
            &ranges,
            &self.signatures,
            &self.options.scan,
            &mut |p| {
                report.bytes_scanned = p.bytes_scanned;
                progress(p);
            },
        )?;
        report.hits = hits.len();
        tracing::info!(
            hits = hits.len(),
            ranges = ranges.len(),
            eligible = report.bytes_eligible,
            "header search complete"
        );
        let mut candidates = Vec::new();
        let mut covered_until = 0u64;
        let mut probe = Probe::new(&*self.reader, self.reader.len());
        for hit in hits {
            if hit.offset < covered_until {
                report.nested_skipped += 1;
                continue;
            }
            let Some(signature) = self.signatures.get(hit.signature) else {
                continue;
            };
            match self.assemble_hit(&mut probe, &hit, signature) {
                Ok(Some(assembly)) => {
                    if assembly.length < self.options.min_size.max(signature.min_size) {
                        report.too_small += 1;
                        continue;
                    }
                    let sound = matches!(
                        assembly.status,
                        ValidationStatus::Valid | ValidationStatus::MostlyValid
                    );
                    if sound {
                        covered_until = hit.offset.saturating_add(assembly.length);
                    }
                    candidates.push(self.build_candidate(hit.offset, signature, assembly));
                }
                Ok(None) => report.rejected += 1,
                Err(e) => {
                    tracing::debug!(offset = hit.offset, error = %e, "hit skipped");
                    report.rejected += 1;
                }
            }
        }
        report.candidates = candidates.len();
        Ok((candidates, report))
    }

    fn assemble_hit(
        &self,
        probe: &mut Probe<'_>,
        hit: &Hit,
        signature: &CarveSignature,
    ) -> Result<Option<Assembly>, CarveError> {
        let assembler = assembler_for(signature);
        assembler.assemble(probe, hit.offset, signature.max_size)
    }

    /// Rebuilds the candidate at `offset`. With `type_id` only that
    /// signature is tried; otherwise every matching signature is, in set
    /// order.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError::NotFound`] if no signature matches there.
    pub fn candidate_at(
        &self,
        offset: u64,
        type_id: Option<&str>,
    ) -> Result<RecoveryCandidate, CarveError> {
        let mut probe = Probe::new(&*self.reader, self.reader.len());
        let window = probe.read_available(offset, self.signatures.max_header_span().max(16))?;
        let matches: Vec<usize> = self.signatures.matches_at(&window).collect();
        for index in matches {
            let Some(signature) = self.signatures.get(index) else {
                continue;
            };
            if let Some(wanted) = type_id
                && signature.id != wanted
                && !refines_to(signature, wanted)
            {
                continue;
            }
            let hit = Hit {
                offset,
                signature: index,
            };
            if let Some(assembly) = self.assemble_hit(&mut probe, &hit, signature)? {
                return Ok(self.build_candidate(offset, signature, assembly));
            }
        }
        Err(CarveError::NotFound(format!(
            "no {} signature matches at offset {offset}",
            type_id.unwrap_or("carving")
        )))
    }

    fn build_candidate(
        &self,
        offset: u64,
        signature: &CarveSignature,
        assembly: Assembly,
    ) -> RecoveryCandidate {
        let length = assembly.length;
        let type_id = assembly
            .type_id
            .clone()
            .unwrap_or_else(|| signature.id.clone());
        let type_name = assembly
            .type_name
            .clone()
            .unwrap_or_else(|| signature.name.clone());
        let extension = assembly
            .extension
            .clone()
            .unwrap_or_else(|| signature.extension.clone());
        let cs = self.space.cluster_size().max(1);
        let mut diagnostics = vec![RecoveryDiagnostic::info(format!(
            "Carved from volume offset {offset}: {} signature",
            signature.name
        ))];
        if !assembly.end_known {
            diagnostics.push(RecoveryDiagnostic::warning(format!(
                "The end of the file could not be determined; {length} bytes were carved up to the last plausible structure or the size limit"
            )));
        }
        let summary = self.space.summarize(ByteRange { offset, length });
        let allocation = AllocationEvidence {
            clusters_total: summary.total(),
            clusters_free: summary.free,
            clusters_allocated: summary.allocated,
            clusters_unknown: summary.unknown,
            map_available: self.space.map_available(),
        };
        let clusters = length.div_ceil(cs);
        let extents = ExtentEvidence {
            resident: false,
            complete: true,
            extent_count: 1,
            total_clusters: Some(clusters),
            expected_clusters: Some(clusters),
            sparse: false,
            compressed: false,
            encrypted: false,
            chain_known: false,
            heuristic: false,
            start_inferred: false,
        };
        let metadata = MetadataEvidence {
            valid_record: false,
            filename_available: false,
            original_parent_available: false,
            parent_reference_valid: false,
            logical_size_available: assembly.end_known,
            logical_size: assembly.end_known.then_some(length),
            timestamps_available: false,
        };
        let expected_type = Some(FileTypeDetection {
            id: type_id.clone(),
            name: type_name.clone(),
            extension: extension.clone(),
        });
        let assembly_result =
            ValidationResult::with_status(assembly.status, assembly.checks.clone());
        let mut content = ContentEvidence::default();
        if self.options.examine_content && length > 0 {
            let stream = self.stream(offset, length);
            let mut cursor = stream.cursor();
            match examine(&mut cursor, length, self.options.byte_budget) {
                Ok(c) => content = c,
                Err(e) => diagnostics.push(RecoveryDiagnostic::warning(format!(
                    "Content could not be examined: {e}"
                ))),
            }
        }
        // Merge the assembler's structural checks with the validator's: the
        // worse status wins; the validator's checks are kept in full and
        // the assembler contributes the checks that failed (they say why
        // the end could not be found).
        content.validation = Some(match content.validation.take() {
            Some(v) => {
                let status = worse(v.status, assembly.status);
                let mut checks: Vec<_> =
                    assembly.checks.into_iter().filter(|c| !c.passed).collect();
                checks.extend(v.checks);
                ValidationResult::with_status(status, checks)
            }
            None => assembly_result,
        });
        if content.detected_type.is_none() {
            content.detected_type = expected_type.clone();
        }
        content.zero_assessment = assess_zero_content(
            content.zero_block_ratio.unwrap_or(0.0),
            content.head_is_zero,
            false,
            content.detected_type.as_ref(),
            expected_type.as_ref(),
            content.validation.as_ref(),
        );
        content.expected_type = expected_type;
        let evidence = RecoveryEvidence {
            source: CandidateSource::FileCarving,
            metadata,
            extents,
            allocation,
            content,
            storage: self.storage.clone(),
            diagnostics,
        };
        let health = score(&evidence, &self.model);
        RecoveryCandidate {
            id: CandidateId::new(),
            source_id: self.source_id,
            filesystem: self.filesystem,
            filesystem_object: FileSystemObjectId::Carved {
                offset,
                type_id,
                extension,
            },
            original_name: None,
            original_path: None,
            path_uncertain: true,
            logical_size: Some(length),
            deleted: true,
            timestamps: CandidateTimestamps::default(),
            evidence,
            health,
        }
    }

    fn stream(&self, offset: u64, length: u64) -> ExtentStream {
        let available = self.reader.len().saturating_sub(offset).min(length);
        ExtentStream::new(
            self.reader.clone(),
            vec![Extent {
                offset,
                length: available,
            }],
            available,
        )
    }

    /// Folds carved candidates into the metadata candidates whose content
    /// starts at the same offset: the metadata candidate keeps the name,
    /// path and timestamps, and gains a diagnostic saying that carving
    /// found the same content and how it assessed it. `extents_of` returns
    /// the content extents of a metadata candidate. Returns the surviving
    /// carved candidates and how many were merged.
    pub fn deduplicate(
        carved: Vec<RecoveryCandidate>,
        metadata: &mut [RecoveryCandidate],
        extents_of: &dyn Fn(&RecoveryCandidate) -> Option<Vec<Extent>>,
    ) -> (Vec<RecoveryCandidate>, usize) {
        let mut by_start: HashMap<u64, usize> = HashMap::new();
        for (i, m) in metadata.iter().enumerate() {
            if let Some(first) = extents_of(m).and_then(|x| x.first().copied()) {
                let entry = by_start.entry(first.offset).or_insert(i);
                if metadata
                    .get(*entry)
                    .is_some_and(|e| m.health.likelihood > e.health.likelihood)
                {
                    *entry = i;
                }
            }
        }
        let mut merged = 0usize;
        let mut kept = Vec::new();
        for c in carved {
            let FileSystemObjectId::Carved { offset, .. } = &c.filesystem_object else {
                kept.push(c);
                continue;
            };
            match by_start.get(offset).and_then(|i| metadata.get_mut(*i)) {
                Some(m) => {
                    merged += 1;
                    let status = c.evidence.content.validation.as_ref().map_or_else(
                        || "unknown".to_owned(),
                        |v| format!("{:?}", v.status).to_lowercase(),
                    );
                    let type_name = c
                        .evidence
                        .content
                        .detected_type
                        .as_ref()
                        .map_or_else(|| "unknown type".to_owned(), |t| t.name.clone());
                    m.evidence.diagnostics.push(RecoveryDiagnostic::info(format!(
                        "Signature carving found the same content at this offset ({type_name}, {} bytes, structure {status}, carved likelihood {}%)",
                        c.logical_size.unwrap_or(0),
                        c.health.likelihood
                    )));
                }
                None => kept.push(c),
            }
        }
        (kept, merged)
    }
}

fn refines_to(signature: &CarveSignature, wanted: &str) -> bool {
    match signature.id.as_str() {
        "zip" => matches!(wanted, "docx" | "xlsx" | "pptx" | "odf" | "jar"),
        "riff" => matches!(wanted, "wav" | "avi" | "webp"),
        "mp4" => matches!(wanted, "mov" | "m4a" | "heic" | "avif" | "3gp"),
        _ => false,
    }
}

fn worse(a: ValidationStatus, b: ValidationStatus) -> ValidationStatus {
    let rank = |s: ValidationStatus| match s {
        ValidationStatus::Valid => 0,
        ValidationStatus::MostlyValid => 1,
        ValidationStatus::Unknown => 2,
        ValidationStatus::Damaged => 3,
        ValidationStatus::Invalid => 4,
    };
    if rank(b) > rank(a) { b } else { a }
}

struct Content {
    cursor: ExtentStreamCursor,
}

impl Read for Content {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl CandidateContent for Content {
    fn len(&self) -> u64 {
        self.cursor.stream().len()
    }
}

impl DeletedFileProvider for CarveEngine {
    fn deleted_files(&self) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_> {
        match self.carve(&mut |_| {}) {
            Ok((candidates, _)) => Box::new(candidates.into_iter().map(Ok)),
            Err(e) => Box::new(std::iter::once(Err(e.into()))),
        }
    }

    fn candidate(&self, object: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError> {
        match object {
            FileSystemObjectId::Carved {
                offset, type_id, ..
            } => {
                let wanted = (!type_id.is_empty()).then_some(type_id.as_str());
                Ok(self.candidate_at(*offset, wanted)?)
            }
            other => Err(FsError::NotFound(format!("{other} is not a carved object"))),
        }
    }

    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError> {
        let (offset, type_id) =
            FileSystemObjectId::parse_carved_reference(text).ok_or_else(|| {
                FsError::NotFound(format!(
                    "invalid carved reference {text:?}; expected c<offset> or c<offset>:<type>"
                ))
            })?;
        Ok(FileSystemObjectId::Carved {
            offset,
            type_id: type_id.unwrap_or("").to_owned(),
            extension: String::new(),
        })
    }

    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError> {
        let FileSystemObjectId::Carved { offset, .. } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not a carved object",
                candidate.filesystem_object
            )));
        };
        let length = candidate.logical_size.unwrap_or(0);
        Ok(Box::new(Content {
            cursor: self.stream(*offset, length).cursor(),
        }))
    }

    fn content_extents(&self, candidate: &RecoveryCandidate) -> Result<Vec<Extent>, FsError> {
        let FileSystemObjectId::Carved { offset, .. } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not a carved object",
                candidate.filesystem_object
            )));
        };
        Ok(self
            .stream(*offset, candidate.logical_size.unwrap_or(0))
            .extents()
            .to_vec())
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
    use phoinix_block::MemoryReader;
    use phoinix_fs::WholeSource;
    use phoinix_health::HealthCategory;

    use super::*;
    use crate::assemble::{bmp, gif, jpeg, mp4, pdf, png, riff, sevenzip, sqlite, zip};

    /// Lays `files` out at sector-aligned offsets separated by a zero gap.
    fn layout(files: &[Vec<u8>]) -> (Vec<u8>, Vec<u64>) {
        let mut image = vec![0u8; 4096];
        let mut offsets = Vec::new();
        for f in files {
            offsets.push(image.len() as u64);
            image.extend_from_slice(f);
            let pad = (512 - image.len() % 512) % 512 + 1024;
            image.extend(std::iter::repeat_n(0, pad));
        }
        image.extend(std::iter::repeat_n(0, 8192));
        (image, offsets)
    }

    fn engine(image: Vec<u8>) -> CarveEngine {
        let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(image));
        let len = reader.len();
        CarveEngine::new(
            reader,
            Arc::new(WholeSource::new(len, 512)),
            FileSystemType::Unknown,
            StorageEvidence::default(),
        )
        .with_options(CarveOptions {
            whole_volume: true,
            ..Default::default()
        })
    }

    #[test]
    fn carves_every_builtin_type_from_a_raw_image() {
        let entropy: Vec<u8> = (0..3000u32).map(|i| (i % 253) as u8).collect();
        let files = vec![
            jpeg::tests::sample_jpeg(&entropy),
            png::tests::sample_png(&[9u8; 2000]),
            gif::tests::sample_gif(),
            bmp::tests::sample_bmp(),
            pdf::tests::sample_pdf("carve me"),
            zip::tests::sample_zip(
                &[
                    ("[Content_Types].xml", b"<Types/>"),
                    ("xl/workbook.xml", b"<wb/>"),
                ],
                true,
            ),
            sqlite::tests::sample_sqlite(2),
            riff::tests::sample_wav(600),
            mp4::tests::sample_mp4(),
            sevenzip::tests::sample_7z(300, 20),
        ];
        let expected = [
            "jpeg", "png", "gif", "bmp", "pdf", "xlsx", "sqlite", "wav", "mp4", "7z",
        ];
        let (image, offsets) = layout(&files);
        let e = engine(image.clone());
        let mut progress_calls = 0;
        let (candidates, report) = e.carve(&mut |_| progress_calls += 1).unwrap();
        assert!(progress_calls >= 1);
        assert_eq!(report.hits, files.len(), "{report:?}");
        assert_eq!(candidates.len(), files.len(), "{report:?}");
        for ((c, off), (file, ty)) in candidates
            .iter()
            .zip(&offsets)
            .zip(files.iter().zip(expected))
        {
            let FileSystemObjectId::Carved {
                offset, type_id, ..
            } = &c.filesystem_object
            else {
                panic!("not carved: {c:?}");
            };
            assert_eq!(*offset, *off);
            assert_eq!(type_id, ty, "{c:?}");
            assert_eq!(
                c.logical_size,
                Some(file.len() as u64),
                "{ty}: {:?}",
                c.evidence.content.validation
            );
            assert_eq!(c.evidence.source, CandidateSource::FileCarving);
            assert!(c.evidence.metadata.logical_size_available, "{ty}");
            assert!(
                c.health.category >= HealthCategory::Good,
                "{ty}: {:?}",
                c.health
            );
            assert!(
                c.display_name().starts_with("carved-"),
                "{}",
                c.display_name()
            );
            // Recovery round trip through the provider contract.
            let mut content = e.open_content(c).unwrap();
            let mut out = Vec::new();
            content.read_to_end(&mut out).unwrap();
            assert_eq!(out, *file, "{ty} content");
            assert_eq!(
                e.content_extents(c).unwrap(),
                vec![Extent {
                    offset: *off,
                    length: file.len() as u64
                }]
            );
            // Re-derivable from its reference.
            let object = e
                .object_from_reference(&c.filesystem_object.short_reference())
                .unwrap();
            let again = e.candidate(&object).unwrap();
            assert_eq!(again.filesystem_object, c.filesystem_object);
            assert_eq!(again.health, c.health);
        }
        assert!(e.object_from_reference("12").is_err());
        assert!(e.candidate_at(4096 + 7, None).is_err());
    }

    #[test]
    fn nested_hits_are_skipped_and_damage_is_reported() {
        // A ZIP whose members are a JPEG and a PNG: the inner headers are
        // sector-aligned on purpose so that the scanner sees them.
        let entropy: Vec<u8> = (0..600u32).map(|i| (i % 251) as u8).collect();
        let inner_jpeg = jpeg::tests::sample_jpeg(&entropy);
        let pad = vec![b'x'; 512 - 35];
        let archive = zip::tests::sample_zip(&[("p.txt", &pad), ("a.jpg", &inner_jpeg)], false);
        // Then a truncated PNG followed by foreign data.
        let mut png = png::tests::sample_png(&[3u8; 4000]);
        png.truncate(png.len() - 300);
        let (mut image, offsets) = layout(&[archive.clone(), png.clone()]);
        // Foreign data right after the truncated PNG.
        let tail = offsets[1] as usize + png.len();
        for (i, b) in image[tail..tail + 2000].iter_mut().enumerate() {
            *b = (i % 7) as u8 + 1;
        }
        let e = engine(image);
        let (candidates, report) = e.carve(&mut |_| {}).unwrap();
        assert_eq!(candidates.len(), 2, "{report:?} {candidates:#?}");
        assert!(report.nested_skipped >= 1, "{report:?}");
        let png_c = &candidates[1];
        assert!(!png_c.evidence.metadata.logical_size_available);
        assert_eq!(
            png_c.evidence.content.validation.as_ref().unwrap().status,
            ValidationStatus::Damaged
        );
        assert!(png_c.health.likelihood <= 59, "{:?}", png_c.health);
        assert!(
            png_c
                .evidence
                .diagnostics
                .iter()
                .any(|d| d.message.contains("could not be determined"))
        );
    }

    #[test]
    fn deduplicates_against_metadata_candidates() {
        let files = vec![gif::tests::sample_gif(), bmp::tests::sample_bmp()];
        let (image, offsets) = layout(&files);
        let e = engine(image);
        let (carved, _) = e.carve(&mut |_| {}).unwrap();
        assert_eq!(carved.len(), 2);
        // A metadata candidate for the GIF that scores higher, and one for
        // the BMP that scores lower.
        let mut strong = carved[0].clone();
        strong.filesystem_object = FileSystemObjectId::Fat { entry_offset: 1 };
        strong.original_name = Some("anim.gif".into());
        strong.health.likelihood = 97;
        let mut weak = carved[1].clone();
        weak.filesystem_object = FileSystemObjectId::Fat { entry_offset: 2 };
        weak.original_name = Some("pic.bmp".into());
        weak.health.likelihood = 10;
        let mut metadata = vec![strong, weak];
        let extents_of = |m: &RecoveryCandidate| -> Option<Vec<Extent>> {
            let FileSystemObjectId::Fat { entry_offset } = &m.filesystem_object else {
                return None;
            };
            let off = offsets[*entry_offset as usize - 1];
            Some(vec![Extent {
                offset: off,
                length: 10,
            }])
        };
        let (kept, merged) = CarveEngine::deduplicate(carved, &mut metadata, &extents_of);
        assert_eq!(merged, 2);
        assert!(kept.is_empty());
        assert!(
            metadata
                .iter()
                .all(|m| m.evidence.diagnostics.iter().any(|d| d
                    .message
                    .contains("Signature carving found the same content")))
        );
        // A carved hit nobody claims survives.
        let (carved, _) = e.carve(&mut |_| {}).unwrap();
        let (kept, merged) = CarveEngine::deduplicate(carved, &mut [], &extents_of);
        assert_eq!((kept.len(), merged), (2, 0));
    }
}
