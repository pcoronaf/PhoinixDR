//! The sanitization certificate (NIST SP 800-88 §4.8 record).

use std::path::{Path, PathBuf};

use phoinix_core::ByteRange;
use phoinix_core::fmt::{bytes_si, iso8601_utc};
use serde::{Deserialize, Serialize};

use crate::method::{EraseMethod, VerifyMode};
use crate::target::TargetDescription;

/// Who ran the erase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operator {
    /// Operator name.
    pub name: Option<String>,
    /// Organisation.
    pub organisation: Option<String>,
    /// Asset, case or ticket number.
    pub asset: Option<String>,
    /// Free-form notes.
    pub notes: Option<String>,
}

/// Outcome of the read-back verification.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    /// What was verified.
    pub mode: VerifyMode,
    /// Blocks read back.
    pub blocks_checked: u64,
    /// Blocks that did not match the last pass.
    pub blocks_mismatched: u64,
    /// Bytes read back.
    pub bytes_checked: u64,
    /// Bytes skipped because they could not be written.
    pub skipped_unwritable_bytes: u64,
    /// Offset of the first mismatch, if any.
    pub first_mismatch: Option<u64>,
    /// Whether the verification was cut short.
    pub cancelled: bool,
    /// Whether every checked block matched.
    pub passed: bool,
}

/// Status of the erase the certificate describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificateStatus {
    /// Writing has started and not finished.
    InProgress,
    /// Every pass completed.
    Completed,
    /// Stopped by the user; the target is partially erased.
    Cancelled,
    /// Stopped by an error; the target is partially erased.
    Failed,
}

/// Rendering formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateFormat {
    /// JSON.
    Json,
    /// Markdown.
    Markdown,
    /// Self-contained HTML.
    Html,
}

impl CertificateFormat {
    /// The format an output path implies.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("md" | "markdown") => Self::Markdown,
            Some("html" | "htm") => Self::Html,
            _ => Self::Json,
        }
    }
}

/// The certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    /// Format version.
    pub version: u32,
    /// Tool name.
    pub tool: String,
    /// Tool version.
    pub tool_version: String,
    /// Attribution.
    pub author: String,
    /// Status.
    pub status: CertificateStatus,
    /// Start time, ISO-8601 UTC.
    pub started_at: String,
    /// End time, ISO-8601 UTC, once finished.
    pub finished_at: Option<String>,
    /// The target as described before writing.
    pub target: TargetDescription,
    /// The method.
    pub method: EraseMethod,
    /// Pass labels in order.
    pub passes: Vec<String>,
    /// Passes completed.
    pub passes_completed: usize,
    /// Hex seed of the random passes (lets the certificate's reader
    /// regenerate and re-verify them).
    pub seed: String,
    /// Bytes written in total (all passes).
    pub bytes_written: u64,
    /// Ranges the device refused to write; these bytes are NOT erased.
    pub unwritable: Vec<ByteRange>,
    /// The operator.
    pub operator: Operator,
    /// The verification, once done.
    pub verification: Option<VerificationRecord>,
    /// The disclaimer.
    pub disclaimer: String,
    /// The erase warning.
    pub warning: String,
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0);
    iso8601_utc(secs, 0)
}

impl Certificate {
    /// A certificate for an erase that is about to start.
    #[must_use]
    pub fn start(
        target: &TargetDescription,
        method: &EraseMethod,
        seed_hex: &str,
        operator: Operator,
    ) -> Self {
        Self {
            version: 1,
            tool: "PhoinixDR".into(),
            tool_version: env!("CARGO_PKG_VERSION").into(),
            author: "by @pcoronaf".into(),
            status: CertificateStatus::InProgress,
            started_at: now_iso(),
            finished_at: None,
            target: target.clone(),
            method: method.clone(),
            passes: method.passes().iter().map(|p| p.pattern.label()).collect(),
            passes_completed: 0,
            seed: seed_hex.to_owned(),
            bytes_written: 0,
            unwritable: Vec::new(),
            operator,
            verification: None,
            disclaimer: phoinix_core::DISCLAIMER.to_owned(),
            warning: crate::ERASE_WARNING.to_owned(),
        }
    }

    /// Marks the certificate finished with `status`.
    pub fn finish(&mut self, status: CertificateStatus) {
        self.status = status;
        self.finished_at = Some(now_iso());
    }

    /// Bytes that were not erased.
    #[must_use]
    pub fn unwritable_bytes(&self) -> u64 {
        self.unwritable.iter().map(|r| r.length).sum()
    }

    /// One-line verdict.
    #[must_use]
    pub fn verdict(&self) -> String {
        match self.status {
            CertificateStatus::InProgress => "in progress".into(),
            CertificateStatus::Cancelled => format!(
                "cancelled after {} of {} pass(es): the target is PARTIALLY erased",
                self.passes_completed,
                self.passes.len()
            ),
            CertificateStatus::Failed => "failed: the target is PARTIALLY erased".into(),
            CertificateStatus::Completed => {
                let mut v = format!("{} pass(es) completed", self.passes.len());
                if self.unwritable_bytes() > 0 {
                    v.push_str(&format!(
                        "; {} in {} region(s) could NOT be written and are not erased",
                        bytes_si(self.unwritable_bytes()),
                        self.unwritable.len()
                    ));
                }
                match &self.verification {
                    Some(r) if r.mode == VerifyMode::None => v.push_str("; not verified"),
                    Some(r) if r.passed => v.push_str(&format!(
                        "; verified: {} blocks read back, all match",
                        r.blocks_checked
                    )),
                    Some(r) => v.push_str(&format!(
                        "; VERIFICATION FAILED: {} of {} blocks do not match",
                        r.blocks_mismatched, r.blocks_checked
                    )),
                    None => v.push_str("; not verified"),
                }
                v
            }
        }
    }

    /// Renders in `format`.
    ///
    /// # Errors
    ///
    /// Returns a serialisation error for JSON.
    pub fn render(&self, format: CertificateFormat) -> Result<String, serde_json::Error> {
        Ok(match format {
            CertificateFormat::Json => serde_json::to_string_pretty(self)?,
            CertificateFormat::Markdown => self.to_markdown(),
            CertificateFormat::Html => self.to_html(),
        })
    }

    /// Writes the certificate to `path` in the format its extension implies.
    ///
    /// # Errors
    ///
    /// Returns an I/O error.
    pub fn write_to(&self, path: &Path) -> std::io::Result<PathBuf> {
        let text = self
            .render(CertificateFormat::from_path(path))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
        Ok(path.to_path_buf())
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let t = &self.target;
        let mut rows = vec![
            ("Status", self.verdict()),
            ("Started", self.started_at.clone()),
            (
                "Finished",
                self.finished_at.clone().unwrap_or_else(|| "–".into()),
            ),
            ("Target", format!("{} {}", t.target.kind(), t.path)),
        ];
        if let Some(d) = &t.device {
            rows.push(("Device", d.display_name.clone()));
            rows.push((
                "Serial number",
                d.serial.clone().unwrap_or_else(|| "unknown".into()),
            ));
            rows.push(("Capacity", bytes_si(d.size)));
            rows.push((
                "Medium",
                match d.rotational {
                    Some(true) => "rotational".into(),
                    Some(false) => "solid state".into(),
                    None => "unknown".into(),
                },
            ));
        }
        rows.push((
            "Range",
            format!("offset {} · {}", t.offset, bytes_si(t.length)),
        ));
        if let Some(p) = &t.partition {
            rows.push(("Partition", format!("{} ({})", p.index, p.type_name)));
        }
        rows.push(("Method", self.method.label()));
        rows.push(("Passes", self.passes.join(", ")));
        rows.push(("Bytes written", bytes_si(self.bytes_written)));
        rows.push(("Random seed", self.seed.clone()));
        if let Some(v) = &self.verification {
            rows.push((
                "Verification",
                format!(
                    "{:?}: {} blocks ({}) read back, {} mismatched{}",
                    v.mode,
                    v.blocks_checked,
                    bytes_si(v.bytes_checked),
                    v.blocks_mismatched,
                    v.first_mismatch
                        .map(|o| format!(", first at offset {o}"))
                        .unwrap_or_default()
                ),
            ));
        }
        for (k, v) in [
            ("Operator", &self.operator.name),
            ("Organisation", &self.operator.organisation),
            ("Asset / case", &self.operator.asset),
            ("Notes", &self.operator.notes),
        ] {
            if let Some(v) = v {
                rows.push((k, v.clone()));
            }
        }
        rows.push((
            "Tool",
            format!("{} {} {}", self.tool, self.tool_version, self.author),
        ));
        rows
    }

    /// Markdown rendering.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Media sanitization certificate\n\n");
        for (k, v) in self.rows() {
            out.push_str(&format!("- **{k}:** {v}\n"));
        }
        if !self.unwritable.is_empty() {
            out.push_str("\n## Regions NOT erased (the device refused to write)\n\n");
            for r in &self.unwritable {
                out.push_str(&format!("- offset {} · {}\n", r.offset, bytes_si(r.length)));
            }
        }
        out.push_str(&format!("\n*{}*\n\n*{}*\n", self.warning, self.disclaimer));
        out
    }

    /// HTML rendering.
    #[must_use]
    pub fn to_html(&self) -> String {
        let mut out = String::from(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Media sanitization certificate</title>\n<style>body{font-family:system-ui,sans-serif;margin:2rem;color:#222;max-width:60rem}dl{display:grid;grid-template-columns:max-content 1fr;gap:.3rem 1rem}dt{font-weight:600}dd{margin:0}.bad{color:#a00;font-weight:600}.good{color:#060}em{color:#555}</style></head><body>\n<h1>Media sanitization certificate</h1>\n<dl>\n",
        );
        for (k, v) in self.rows() {
            let class = if k == "Status" {
                if matches!(self.status, CertificateStatus::Completed)
                    && self.unwritable.is_empty()
                    && self
                        .verification
                        .as_ref()
                        .is_none_or(|r| r.passed || r.mode == VerifyMode::None)
                {
                    " class=\"good\""
                } else {
                    " class=\"bad\""
                }
            } else {
                ""
            };
            out.push_str(&format!("<dt>{}</dt><dd{class}>{}</dd>\n", esc(k), esc(&v)));
        }
        out.push_str("</dl>\n");
        if !self.unwritable.is_empty() {
            out.push_str(
                "<h2 class=\"bad\">Regions NOT erased (the device refused to write)</h2>\n<ul>\n",
            );
            for r in &self.unwritable {
                out.push_str(&format!(
                    "<li>offset {} · {}</li>\n",
                    r.offset,
                    esc(&bytes_si(r.length))
                ));
            }
            out.push_str("</ul>\n");
        }
        out.push_str(&format!(
            "<p><em>{}</em></p>\n<p><em>{}</em></p>\n</body></html>\n",
            esc(&self.warning),
            esc(&self.disclaimer)
        ));
        out
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
