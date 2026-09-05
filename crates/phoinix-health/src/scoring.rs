//! Deterministic scoring: hard constraints + weighted evidence + confidence.

use serde::{Deserialize, Serialize};

use crate::evidence::{RecoveryEvidence, ZeroContentAssessment};
use crate::health::{HealthCategory, HealthReason, RecoveryHealth};
use crate::validate::ValidationStatus;

/// Tunable parameters of the scoring model. Defaults follow the M4
/// specification; they are development heuristics, not calibrated
/// probabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoringModel {
    /// Base likelihood for resident content in a valid deleted record.
    pub resident_base: u8,
    /// Base likelihood when every extent is known and every cluster is free.
    pub all_free_base: u8,
    /// Cap when any required cluster is allocated.
    pub cap_any_allocated: u8,
    /// Cap when at least 10 % of required clusters are allocated.
    pub cap_allocated_10: u8,
    /// Cap when at least 50 % of required clusters are allocated.
    pub cap_allocated_50: u8,
    /// Cap when every required cluster is allocated.
    pub cap_allocated_100: u8,
    /// Cap when the extent map is incomplete.
    pub cap_incomplete_extents: u8,
    /// Cap when the allocation map is unavailable for a non-resident stream.
    pub cap_no_allocation_map: u8,
    /// Cap when content validation reports damage.
    pub cap_validation_damaged: u8,
    /// Cap when content validation reports the structure as invalid.
    pub cap_validation_invalid: u8,
    /// Cap when zero-filled content contradicts the file's format.
    pub cap_zero_contradicts_format: u8,
    /// Cap when zero-filled content is suspicious for a recognised type.
    pub cap_zero_suspicious: u8,
    /// Confidence penalty when zero-filled content is ambiguous.
    pub ambiguous_zero_confidence_penalty: u8,
    /// Cap when the layout was reconstructed heuristically around allocated
    /// clusters.
    pub cap_heuristic_reconstruction: u8,
    /// Cap for a heuristic reconstruction (skipped clusters or an inferred
    /// start) whose content validates completely: the structure confirms the
    /// inferred layout, so the file is probably right, but the layout is
    /// still not proven.
    pub cap_heuristic_validated: u8,
    /// Confidence penalty when the start of the file had to be inferred.
    pub inferred_start_confidence_penalty: u8,
    /// Confidence penalty when the cluster chain is unknown and contiguity
    /// was assumed.
    pub assumed_contiguous_confidence_penalty: u8,
    /// Confidence penalty for a heuristic fragmented reconstruction.
    pub heuristic_confidence_penalty: u8,
    /// Bonus for a fully valid structure (within caps).
    pub bonus_valid_structure: u8,
    /// Penalty per fragment beyond the first, capped by `fragment_penalty_max`.
    pub fragment_penalty_per_extent: u8,
    /// Maximum fragmentation penalty.
    pub fragment_penalty_max: u8,
}

impl Default for ScoringModel {
    fn default() -> Self {
        Self {
            resident_base: 97,
            all_free_base: 92,
            cap_any_allocated: 79,
            cap_allocated_10: 59,
            cap_allocated_50: 34,
            cap_allocated_100: 15,
            cap_incomplete_extents: 79,
            cap_no_allocation_map: 74,
            cap_validation_damaged: 59,
            cap_validation_invalid: 34,
            cap_zero_contradicts_format: 20,
            cap_zero_suspicious: 59,
            ambiguous_zero_confidence_penalty: 25,
            cap_heuristic_reconstruction: 59,
            cap_heuristic_validated: 79,
            inferred_start_confidence_penalty: 20,
            assumed_contiguous_confidence_penalty: 10,
            heuristic_confidence_penalty: 30,
            bonus_valid_structure: 3,
            fragment_penalty_per_extent: 1,
            fragment_penalty_max: 5,
        }
    }
}

struct Scorer {
    likelihood: i32,
    cap: u8,
    positives: Vec<HealthReason>,
    negatives: Vec<HealthReason>,
}

impl Scorer {
    fn cap_at(&mut self, value: u8) {
        self.cap = self.cap.min(value);
    }

    fn good(&mut self, text: impl Into<String>) {
        self.positives.push(HealthReason::positive(text));
    }

    fn bad(&mut self, text: impl Into<String>) {
        self.negatives.push(HealthReason::negative(text));
    }
}

/// Confidence penalty (0–25) proportional to the share of unknown clusters.
#[allow(clippy::cast_possible_truncation)]
fn unknown_penalty(unknown: u64, total: u64) -> i32 {
    if total == 0 {
        return 0;
    }
    (25.0 * unknown as f64 / total as f64)
        .round()
        .clamp(0.0, 25.0) as i32
}

/// Rounds a ratio to a whole percentage; the clamp makes the cast exact.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percent(ratio: f64) -> u32 {
    (ratio * 100.0).round().clamp(0.0, 100.0) as u32
}

/// Scales a cap by a share in `0.0..=1.0`; the clamp makes the cast exact.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled_cap(cap: u8, share: f64) -> u8 {
    (f64::from(cap) * share).round().clamp(0.0, 255.0) as u8
}

/// Scores `evidence` with `model`.
#[must_use]
pub fn score(evidence: &RecoveryEvidence, model: &ScoringModel) -> RecoveryHealth {
    let mut s = Scorer {
        likelihood: 100,
        cap: 100,
        positives: Vec::new(),
        negatives: Vec::new(),
    };
    let m = &evidence.metadata;
    let x = &evidence.extents;
    let a = &evidence.allocation;
    let c = &evidence.content;

    // ---- metadata ---------------------------------------------------------
    if m.valid_record {
        s.good("Valid deleted metadata record");
    } else {
        s.bad("Metadata record is damaged");
        s.likelihood -= 20;
    }
    if m.filename_available {
        s.good("Original filename is available");
    } else {
        s.bad("Original filename is not available");
        s.likelihood -= 3;
    }
    if m.original_parent_available && m.parent_reference_valid {
        s.good("Original parent directory is available");
    } else if m.original_parent_available {
        s.bad("Parent directory record was reused; the original path is uncertain");
    } else {
        s.bad("Original path could not be reconstructed");
    }
    if m.logical_size_available {
        s.good("Original size is available");
    } else {
        s.bad("Original size is unknown");
        s.likelihood -= 5;
    }

    let empty = m.logical_size == Some(0);
    if empty {
        s.good("The file is empty: there is no content to recover beyond its metadata");
    }

    // ---- unsupported content -------------------------------------------------
    if x.encrypted {
        s.bad("Content is EFS-encrypted and unusable without the keys");
        s.cap_at(10);
    }
    if x.compressed {
        s.bad("Content is NTFS-compressed; PHOINIX cannot decompress it yet");
        s.cap_at(0);
    }

    // ---- extents and allocation -----------------------------------------------
    if x.resident {
        s.good("Content is stored inside the metadata record; no clusters can have been reused");
        s.likelihood = s.likelihood.min(i32::from(model.resident_base));
    } else {
        if x.complete {
            s.good("All file extents were reconstructed");
        } else {
            // Scale the cap by the share of the file that can actually be located.
            let known = match (x.total_clusters, x.expected_clusters) {
                (Some(t), Some(e)) if e > 0 => (t as f64 / e as f64).clamp(0.0, 1.0),
                (Some(0), _) => 0.0,
                _ => 0.5,
            };
            if known == 0.0 {
                s.bad("No extent of the content could be located");
            } else {
                s.bad(format!(
                    "The extent map is incomplete; only {}% of the content can be located",
                    percent(known)
                ));
            }
            let cap = scaled_cap(model.cap_incomplete_extents, known);
            s.cap_at(cap.max(if known == 0.0 { 0 } else { 5 }));
        }
        if a.map_available && a.clusters_total > 0 {
            let ratio = a.allocated_ratio().unwrap_or(0.0);
            if a.clusters_allocated == 0 && a.clusters_unknown == 0 {
                s.good(format!(
                    "All {} required clusters are currently free",
                    group(a.clusters_total)
                ));
                s.likelihood = s.likelihood.min(i32::from(model.all_free_base));
            } else {
                if a.clusters_allocated > 0 {
                    if x.heuristic && a.clusters_allocated < a.clusters_total {
                        // The heuristic already skipped these clusters; whether
                        // the file was fragmented around them or overwritten by
                        // them is undecidable from allocation alone.
                        s.bad(format!(
                            "{}% of the clusters in the assumed span ({} of {}) are allocated to active filesystem data and were skipped",
                            percent(ratio),
                            group(a.clusters_allocated),
                            group(a.clusters_total)
                        ));
                    } else {
                        s.bad(format!(
                            "{}% of the required clusters ({} of {}) are currently allocated to active filesystem data",
                            percent(ratio),
                            group(a.clusters_allocated),
                            group(a.clusters_total)
                        ));
                        if a.clusters_allocated == a.clusters_total {
                            s.cap_at(model.cap_allocated_100);
                        } else if ratio >= 0.5 {
                            s.cap_at(model.cap_allocated_50);
                        } else if ratio >= 0.1 {
                            s.cap_at(model.cap_allocated_10);
                        } else {
                            s.cap_at(model.cap_any_allocated);
                        }
                    }
                }
                if a.clusters_unknown > 0 {
                    s.bad(format!(
                        "Allocation state of {} clusters is unknown",
                        group(a.clusters_unknown)
                    ));
                    s.cap_at(model.cap_no_allocation_map);
                }
                s.likelihood = s.likelihood.min(i32::from(model.all_free_base));
            }
        } else if x.total_clusters.is_some_and(|t| t > 0) {
            s.bad("The allocation map is unavailable; cluster reuse cannot be checked");
            s.cap_at(model.cap_no_allocation_map);
        }
        if !x.resident && !x.chain_known {
            // A complete structural validation of the reconstructed content
            // supports the inferred layout, so the cap is relaxed (not
            // lifted): the layout is still inferred, not proven.
            let validated = evidence
                .content
                .validation
                .as_ref()
                .is_some_and(|v| v.status == ValidationStatus::Valid);
            let heuristic_cap = if validated {
                model.cap_heuristic_validated
            } else {
                model.cap_heuristic_reconstruction
            };
            if x.start_inferred {
                s.bad("The recorded start cluster was untrustworthy; the start was inferred from free clusters and their content (heuristic)");
                s.cap_at(heuristic_cap);
            }
            if x.heuristic {
                s.bad("The cluster chain is gone; the layout was reconstructed by skipping clusters now used by other files (heuristic)");
                s.cap_at(heuristic_cap);
            } else if !x.start_inferred {
                s.bad("The cluster chain is gone; the file is assumed to have been stored contiguously");
            }
            if validated && (x.heuristic || x.start_inferred) {
                s.good("The content validates completely, which supports the inferred layout");
            }
        }
        if x.extent_count > 1 {
            let penalty = i32::from(model.fragment_penalty_per_extent)
                * i32::from(x.extent_count.saturating_sub(1).min(255) as u8);
            s.likelihood -= penalty.min(i32::from(model.fragment_penalty_max));
            s.bad(format!("The file consists of {} fragments", x.extent_count));
        }
        if x.sparse {
            s.positives.push(HealthReason::positive(
                "Sparse regions are reconstructed as zeros, as on the original",
            ));
        }
    }

    // ---- content ----------------------------------------------------------------
    let zero_pct = c.zero_block_ratio.map_or(0, percent);
    match c.zero_assessment {
        None => {}
        Some(ZeroContentAssessment::Expected) => {
            if zero_pct > 0 {
                s.good(format!(
                    "{zero_pct}% of the sampled content is zero-filled, as expected for this file"
                ));
            }
        }
        Some(ZeroContentAssessment::Plausible) => {
            if zero_pct > 0 {
                s.good(format!("{zero_pct}% of the sampled content is zero-filled, which is consistent with its format"));
            }
        }
        Some(ZeroContentAssessment::Suspicious) => {
            s.bad(format!(
                "{zero_pct}% of the sampled content is zero-filled, which is unusual for this type"
            ));
            s.cap_at(model.cap_zero_suspicious);
        }
        Some(ZeroContentAssessment::ContradictsFormat) => {
            let kind = c
                .detected_type
                .as_ref()
                .or(c.expected_type.as_ref())
                .map_or_else(|| "file".to_owned(), |t| t.name.clone());
            s.bad(format!(
                "{zero_pct}% of the sampled content is zero-filled, which a {kind} cannot be; the clusters were probably discarded or reused even though the filesystem reports them as free"
            ));
            s.cap_at(model.cap_zero_contradicts_format);
        }
        Some(ZeroContentAssessment::Ambiguous) => {
            s.bad(format!(
                "{zero_pct}% of the sampled content is zero-filled; PHOINIX cannot tell wiped data from a legitimately zero-filled file of unknown type"
            ));
        }
    }
    if empty {
        s.good("Content validation is not applicable to an empty file");
    }
    if let Some(v) = &c.validation
        && !empty
    {
        let type_name = c
            .detected_type
            .as_ref()
            .map_or_else(|| "file".to_owned(), |t| t.name.clone());
        match v.status {
            ValidationStatus::Valid => {
                s.good(format!("The {type_name} structure validates successfully"));
                s.likelihood += i32::from(model.bonus_valid_structure);
            }
            ValidationStatus::MostlyValid => {
                s.good(format!("The {type_name} structure is largely intact"))
            }
            ValidationStatus::Damaged => {
                s.bad(format!(
                    "The reconstructed {type_name} structure is damaged"
                ));
                s.cap_at(model.cap_validation_damaged);
            }
            ValidationStatus::Invalid => {
                s.bad(format!(
                    "The reconstructed content is not a valid {type_name}"
                ));
                s.cap_at(model.cap_validation_invalid);
            }
            ValidationStatus::Unknown => {}
        }
        for check in v.checks.iter().filter(|c| !c.passed) {
            s.negatives.push(HealthReason::negative(format!(
                "{}: {}",
                check.name, check.detail
            )));
        }
    } else if let Some(t) = &c.detected_type {
        s.positives.push(HealthReason::positive(format!(
            "Content signature matches {}",
            t.name
        )));
    }

    // ---- storage ------------------------------------------------------------------
    if evidence.storage.rotational == Some(false) && !x.resident {
        s.bad("Source appears to be SSD storage; TRIM/discard may reduce recoverability even where filesystem metadata remains");
    }

    for d in &evidence.diagnostics {
        match d.severity {
            crate::evidence::DiagnosticSeverity::Info => {
                s.positives.push(HealthReason::positive(d.message.clone()))
            }
            crate::evidence::DiagnosticSeverity::Warning => {
                s.negatives.push(HealthReason::negative(d.message.clone()))
            }
        }
    }

    if empty {
        s.likelihood = s.likelihood.min(i32::from(model.resident_base));
    }
    let likelihood = u8::try_from(s.likelihood.clamp(0, i32::from(s.cap))).unwrap_or(0);
    let confidence = confidence(evidence, model);
    let mut reasons = s.positives;
    reasons.extend(s.negatives);
    RecoveryHealth {
        likelihood,
        confidence,
        category: HealthCategory::from_likelihood(likelihood),
        reasons,
    }
}

/// Confidence reflects how much PHOINIX actually knows, independent of
/// whether the news is good.
fn confidence(e: &RecoveryEvidence, model: &ScoringModel) -> u8 {
    let mut c: i32 = 100;
    if !e.metadata.valid_record {
        c -= 30;
    }
    if !e.metadata.logical_size_available {
        c -= 10;
    }
    let empty = e.metadata.logical_size == Some(0);
    if !e.extents.resident {
        if !e.extents.complete {
            c -= 25;
        }
        if !e.allocation.map_available {
            c -= 25;
        } else if e.allocation.clusters_unknown > 0 && e.allocation.clusters_total > 0 {
            c -= unknown_penalty(e.allocation.clusters_unknown, e.allocation.clusters_total);
        }
    }
    if !empty {
        match &e.content.validation {
            Some(v) if v.status != ValidationStatus::Unknown => {}
            _ => c -= 15,
        }
        if e.content.zero_block_ratio.is_none() {
            c -= 5;
        }
        if e.content.zero_assessment == Some(ZeroContentAssessment::Ambiguous) {
            c -= i32::from(model.ambiguous_zero_confidence_penalty);
        }
    }
    if e.storage.rotational.is_none() {
        c -= 3;
    }
    if e.storage.rotational == Some(false) && !e.storage.trim_state_known && !e.extents.resident {
        c -= 10;
    }
    if !e.extents.resident && !e.extents.chain_known {
        c -= i32::from(if e.extents.heuristic {
            model.heuristic_confidence_penalty
        } else {
            model.assumed_contiguous_confidence_penalty
        });
        if e.extents.start_inferred {
            c -= i32::from(model.inferred_start_confidence_penalty);
        }
    }
    if e.extents.compressed || e.extents.encrypted {
        c = c.max(90);
    }
    u8::try_from(c.clamp(0, 100)).unwrap_or(0)
}

fn group(v: u64) -> String {
    phoinix_core::fmt::grouped(v)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::float_cmp
    )]

    use super::*;
    use crate::evidence::{AllocationEvidence, ExtentEvidence, MetadataEvidence};
    use crate::validate::{ValidationCheck, ValidationResult};

    fn good_metadata() -> MetadataEvidence {
        MetadataEvidence {
            valid_record: true,
            filename_available: true,
            original_parent_available: true,
            parent_reference_valid: true,
            logical_size_available: true,
            logical_size: Some(409_600),
            timestamps_available: true,
        }
    }

    fn non_resident(extents: u32, clusters: u64, allocated: u64) -> RecoveryEvidence {
        RecoveryEvidence {
            metadata: good_metadata(),
            extents: ExtentEvidence {
                resident: false,
                complete: true,
                extent_count: extents,
                total_clusters: Some(clusters),
                ..Default::default()
            },
            allocation: AllocationEvidence {
                clusters_total: clusters,
                clusters_free: clusters - allocated,
                clusters_allocated: allocated,
                clusters_unknown: 0,
                map_available: true,
            },
            content: crate::evidence::ContentEvidence {
                zero_block_ratio: Some(0.0),
                bytes_examined: 4096,
                ..Default::default()
            },
            storage: crate::evidence::StorageEvidence {
                rotational: Some(true),
                ..Default::default()
            },
            diagnostics: Vec::new(),
        }
    }

    fn valid(name: &str) -> ValidationResult {
        ValidationResult {
            status: ValidationStatus::Valid,
            checks: vec![ValidationCheck {
                name: name.into(),
                passed: true,
                detail: "ok".into(),
            }],
        }
    }

    #[test]
    fn resident_valid_record_is_excellent() {
        let mut e = non_resident(0, 0, 0);
        e.extents = ExtentEvidence {
            resident: true,
            complete: true,
            ..Default::default()
        };
        e.allocation = AllocationEvidence::default();
        e.content.validation = Some(valid("text"));
        let h = score(&e, &ScoringModel::default());
        assert_eq!(h.category, HealthCategory::Excellent, "{h:?}");
        assert!(h.likelihood >= 97);
        assert!(h.confidence >= 90);
        assert!(
            h.reasons
                .iter()
                .any(|r| r.positive && r.text.contains("inside the metadata record"))
        );
    }

    #[test]
    fn all_free_contiguous_is_very_good_or_better() {
        let mut e = non_resident(1, 181, 0);
        e.content.validation = Some(valid("ZIP"));
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood >= 92 && h.likelihood <= 95, "{h:?}");
        assert!(matches!(
            h.category,
            HealthCategory::VeryGood | HealthCategory::Excellent
        ));
        assert!(h.reasons.iter().any(|r| {
            r.text
                .contains("All 181 required clusters are currently free")
        }));
    }

    #[test]
    fn fragmentation_alone_does_not_make_a_file_poor() {
        let h = score(&non_resident(100, 1000, 0), &ScoringModel::default());
        assert!(h.likelihood >= 80, "{h:?}");
        assert!(
            h.reasons
                .iter()
                .any(|r| !r.positive && r.text.contains("100 fragments"))
        );
    }

    #[test]
    fn allocation_caps_are_monotonic_and_worded_carefully() {
        let model = ScoringModel::default();
        let ratios = [0u64, 1, 10, 25, 50, 100];
        let mut last = 101u8;
        for allocated in ratios {
            let h = score(&non_resident(1, 100, allocated), &model);
            assert!(
                h.likelihood <= last,
                "allocated {allocated}: {} > {last}",
                h.likelihood
            );
            last = h.likelihood;
            if allocated > 0 {
                let text = h
                    .reasons
                    .iter()
                    .find(|r| !r.positive)
                    .map(|r| r.text.clone())
                    .unwrap_or_default();
                assert!(
                    text.contains("allocated to active filesystem data"),
                    "{text}"
                );
                assert!(
                    !text.to_lowercase().contains("overwritten"),
                    "must not claim overwrite: {text}"
                );
            }
        }
        assert!(score(&non_resident(1, 100, 1), &model).likelihood <= 79);
        assert!(score(&non_resident(1, 100, 10), &model).likelihood <= 59);
        assert!(score(&non_resident(1, 100, 50), &model).likelihood <= 34);
        assert!(score(&non_resident(1, 100, 100), &model).likelihood <= 15);
    }

    #[test]
    fn validation_cannot_compensate_for_reused_clusters() {
        let mut e = non_resident(1, 100, 60);
        e.content.validation = Some(valid("JPEG"));
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 34, "{h:?}");
        assert!(h.confidence >= 90);
    }

    #[test]
    fn zeroed_content_contradicting_the_format_scores_low() {
        let mut e = non_resident(1, 100, 0);
        e.content.zero_block_ratio = Some(0.9);
        e.content.expected_type = Some(crate::FileTypeDetection {
            id: "zip".into(),
            name: "ZIP archive".into(),
            extension: "zip".into(),
        });
        e.content.zero_assessment = Some(ZeroContentAssessment::ContradictsFormat);
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 20, "{h:?}");
        assert!(
            h.reasons
                .iter()
                .any(|r| r.text.contains("ZIP archive cannot be"))
        );
    }

    #[test]
    fn ambiguous_zeros_lower_confidence_not_likelihood() {
        let mut e = non_resident(1, 100, 0);
        e.content.zero_block_ratio = Some(1.0);
        e.content.zero_assessment = Some(ZeroContentAssessment::Ambiguous);
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood >= 85, "{h:?}");
        assert!(h.confidence <= 60, "{h:?}");
        assert!(
            h.reasons
                .iter()
                .any(|r| !r.positive && r.text.contains("cannot tell"))
        );
        e.content.zero_assessment = Some(ZeroContentAssessment::Expected);
        e.extents.sparse = true;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood >= 85 && h.confidence >= 75, "{h:?}");
    }

    #[test]
    fn empty_file_is_excellent_with_validation_not_applicable() {
        let mut e = non_resident(0, 0, 0);
        e.metadata.logical_size = Some(0);
        e.extents = ExtentEvidence {
            resident: true,
            complete: true,
            ..Default::default()
        };
        e.allocation = AllocationEvidence {
            map_available: true,
            ..Default::default()
        };
        e.content = crate::evidence::ContentEvidence::default();
        let h = score(&e, &ScoringModel::default());
        assert_eq!(h.category, HealthCategory::Excellent, "{h:?}");
        assert!(h.confidence >= 90, "{h:?}");
        assert!(h.reasons.iter().any(|r| r.text.contains("not applicable")));
    }

    #[test]
    fn incomplete_extents_scale_with_located_share() {
        let mut none = non_resident(0, 0, 0);
        none.extents.complete = false;
        none.extents.total_clusters = Some(0);
        none.extents.expected_clusters = Some(10);
        none.allocation = AllocationEvidence {
            map_available: true,
            ..Default::default()
        };
        assert_eq!(score(&none, &ScoringModel::default()).likelihood, 0);
        let mut half = non_resident(1, 50, 0);
        half.extents.complete = false;
        half.extents.expected_clusters = Some(100);
        let h = score(&half, &ScoringModel::default());
        assert!(h.likelihood <= 40 && h.likelihood >= 30, "{h:?}");
    }

    #[test]
    fn incomplete_extents_and_missing_map_lower_confidence() {
        let mut e = non_resident(3, 100, 0);
        e.extents.complete = false;
        e.allocation.map_available = false;
        e.allocation.clusters_total = 0;
        e.content.validation = None;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 74);
        assert!(h.confidence <= 40, "{h:?}");
    }

    #[test]
    fn compressed_and_encrypted() {
        let mut e = non_resident(1, 10, 0);
        e.extents.compressed = true;
        assert_eq!(score(&e, &ScoringModel::default()).likelihood, 0);
        let mut e = non_resident(1, 10, 0);
        e.extents.encrypted = true;
        assert!(score(&e, &ScoringModel::default()).likelihood <= 10);
    }

    #[test]
    fn unknown_chain_lowers_confidence_and_heuristics_cap_likelihood() {
        let mut e = non_resident(1, 10, 0);
        e.extents.chain_known = false;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood >= 85, "{h:?}");
        assert!(h.confidence <= 75, "{h:?}");
        e.extents.heuristic = true;
        e.extents.extent_count = 3;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 59, "{h:?}");
        assert!(h.reasons.iter().any(|r| r.text.contains("heuristic")));
    }

    #[test]
    fn heuristic_reconstruction_is_not_double_penalised() {
        // Half the assumed span is allocated but the heuristic skipped it.
        let mut e = non_resident(4, 64, 32);
        e.extents.chain_known = false;
        e.extents.heuristic = true;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood >= 50 && h.likelihood <= 59, "{h:?}");
        assert!(h.reasons.iter().any(|r| r.text.contains("were skipped")));
        // The whole span is taken: nothing of the file can remain there.
        let mut e = non_resident(1, 64, 64);
        e.extents.chain_known = false;
        e.extents.heuristic = true;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 15, "{h:?}");
    }

    #[test]
    fn inferred_start_is_capped_and_relaxed_by_validation() {
        let mut e = non_resident(1, 10, 0);
        e.extents.chain_known = false;
        e.extents.start_inferred = true;
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 59, "{h:?}");
        assert!(h.confidence <= 60, "{h:?}");
        assert!(
            h.reasons
                .iter()
                .any(|r| r.text.contains("start was inferred"))
        );
        // A complete validation supports the inferred layout: Good, not Poor.
        e.content.validation = Some(valid("PDF"));
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood > 59 && h.likelihood <= 79, "{h:?}");
        assert!(
            h.reasons
                .iter()
                .any(|r| r.text.contains("supports the inferred layout"))
        );
        // Reused clusters still dominate.
        let mut e = non_resident(1, 64, 64);
        e.extents.chain_known = false;
        e.extents.start_inferred = true;
        e.content.validation = Some(valid("PDF"));
        let h = score(&e, &ScoringModel::default());
        assert!(h.likelihood <= 15, "{h:?}");
    }

    #[test]
    fn model_is_serialisable() {
        let json = serde_json::to_string(&ScoringModel::default()).unwrap();
        let back: ScoringModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ScoringModel::default());
    }
}
