//! Recovery evidence, health scoring and explanations (ADR-0006).
//!
//! Recovery is treated as an evidence-aggregation problem:
//!
//! ```text
//!                     RecoveryEvidence
//!                           │
//!         ┌─────────────────┼───────────────────┐
//!  MetadataEvidence   ExtentEvidence     ContentEvidence
//!         │                 │                   │
//!         ├───────── AllocationEvidence ────────┤
//!         │                                     │
//!         └──────────── RecoveryHealth ─────────┘
//! ```
//!
//! Filesystem engines fill in [`RecoveryEvidence`]; [`score`] turns it into a
//! [`RecoveryHealth`] holding two independent numbers — recovery
//! *likelihood* and assessment *confidence* — together with the concrete
//! reasons behind them. The model is deterministic: hard constraints (a
//! reallocated cluster, a missing extent) cap the likelihood, and weighted
//! evidence adjusts within the cap. All thresholds are provisional
//! development heuristics until calibrated against a controlled corpus.
//!
//! The [`validate`] module provides the minimal structural validators used
//! to produce [`ContentEvidence`].

#![forbid(unsafe_code)]

mod evidence;
mod health;
mod scoring;
pub mod validate;

pub use evidence::{
    AllocationEvidence, ContentEvidence, DeviceKind, DiagnosticSeverity, ExtentEvidence,
    MetadataEvidence, RecoveryDiagnostic, RecoveryEvidence, StorageEvidence,
};
pub use health::{HealthCategory, HealthReason, RecoveryHealth};
pub use scoring::{ScoringModel, score};
pub use validate::{
    FileTypeDetection, ValidationCheck, ValidationResult, ValidationStatus, assess_zero_content,
    expected_type_from_name,
};
