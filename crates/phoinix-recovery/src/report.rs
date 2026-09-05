//! The exportable recovery report: what was recovered, from which source,
//! under which case, with the hashes that let a reader verify every file.
//!
//! The report is a plain data structure serialised as JSON; Markdown and
//! HTML renderings are derived from it so that the three never disagree.

use std::path::{Path, PathBuf};

use phoinix_core::fmt::{bytes_si, grouped, iso8601_utc};
use phoinix_fs::RecoveryCandidate;
use phoinix_image::{AcquisitionInfo, ContainerInfo, HashVerification};
use serde::{Deserialize, Serialize};

use crate::RecoveryResult;

/// Case metadata supplied by the operator (or taken from the image's
/// acquisition header when the operator gives none).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CaseMetadata {
    /// Case number.
    pub case_number: Option<String>,
    /// Evidence number.
    pub evidence_number: Option<String>,
    /// Examiner.
    pub examiner: Option<String>,
    /// Free-form notes.
    pub notes: Option<String>,
}

impl CaseMetadata {
    /// Whether any field is set.
    #[must_use]
    pub fn any(&self) -> bool {
        self != &Self::default()
    }

    /// Fills unset fields from an acquisition header.
    #[must_use]
    pub fn with_acquisition_defaults(mut self, acquisition: Option<&AcquisitionInfo>) -> Self {
        if let Some(a) = acquisition {
            self.case_number = self.case_number.or_else(|| a.case_number.clone());
            self.evidence_number = self.evidence_number.or_else(|| a.evidence_number.clone());
            self.examiner = self.examiner.or_else(|| a.examiner.clone());
            self.notes = self.notes.or_else(|| a.notes.clone());
        }
        self
    }
}

/// The source the files were recovered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSource {
    /// Path as given.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Logical sector size.
    pub sector_size: u32,
    /// Whether the source is a block device.
    pub is_device: bool,
    /// The image container, for image files.
    pub container: Option<ContainerInfo>,
    /// Hash verification of the whole source, when it was run.
    pub verification: Option<HashVerification>,
}

/// The volume the candidates belong to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportVolume {
    /// Partition index, if any.
    pub partition: Option<u32>,
    /// Byte offset inside the source.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
    /// Filesystem label.
    pub filesystem: String,
}

/// One recovered (or failed) file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportItem {
    /// Candidate reference as printed by `scan`.
    pub reference: String,
    /// Display name.
    pub name: String,
    /// Original path, if known.
    pub original_path: Option<String>,
    /// Logical size, if known.
    pub size: Option<u64>,
    /// Recovery likelihood at recovery time.
    pub likelihood: u8,
    /// Assessment confidence.
    pub confidence: u8,
    /// Health category.
    pub category: String,
    /// How the candidate was found.
    pub source: String,
    /// Where the file was written.
    pub output_path: Option<String>,
    /// Bytes written.
    pub bytes_written: u64,
    /// SHA-256 of the written bytes.
    pub sha256: Option<String>,
    /// Whether every expected byte was written.
    pub complete: bool,
    /// Failure text.
    pub error: Option<String>,
    /// Writer diagnostics.
    pub diagnostics: Vec<String>,
}

/// Totals.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReportSummary {
    /// Candidates requested.
    pub requested: usize,
    /// Recovered completely.
    pub recovered: usize,
    /// Written but incomplete.
    pub partial: usize,
    /// Not written.
    pub failed: usize,
    /// Bytes written in total.
    pub bytes_written: u64,
}

/// The report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// Report format version.
    pub version: u32,
    /// Tool name.
    pub tool: String,
    /// Tool version.
    pub tool_version: String,
    /// When the report was generated (ISO-8601 UTC).
    pub generated_at: String,
    /// Case metadata.
    pub case: CaseMetadata,
    /// The source.
    pub source: ReportSource,
    /// The volume.
    pub volume: Option<ReportVolume>,
    /// Destination directory.
    pub destination: String,
    /// Recovered files.
    pub items: Vec<ReportItem>,
    /// Totals.
    pub summary: ReportSummary,
}

/// Rendering formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportFormat {
    /// The report structure as JSON.
    Json,
    /// A Markdown document.
    Markdown,
    /// A self-contained HTML page.
    Html,
}

impl ReportFormat {
    /// The format an output path implies (`.md`, `.html`/`.htm`, else JSON).
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

impl RecoveryReport {
    /// Starts a report.
    #[must_use]
    pub fn new(
        case: CaseMetadata,
        source: ReportSource,
        volume: Option<ReportVolume>,
        destination: &Path,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
            .unwrap_or(0);
        Self {
            version: 1,
            tool: "PhoinixDR".into(),
            tool_version: env!("CARGO_PKG_VERSION").into(),
            generated_at: iso8601_utc(now, 0),
            case,
            source,
            volume,
            destination: destination.display().to_string(),
            items: Vec::new(),
            summary: ReportSummary::default(),
        }
    }

    /// Records the outcome of one candidate.
    pub fn push(
        &mut self,
        reference: &str,
        candidate: Option<&RecoveryCandidate>,
        outcome: Result<&RecoveryResult, &str>,
    ) {
        let (name, original_path, size, likelihood, confidence, category, source) = match candidate
        {
            Some(c) => (
                c.display_name(),
                c.original_path.clone(),
                c.logical_size,
                c.health.likelihood,
                c.health.confidence,
                c.health.category.to_string(),
                format!("{:?}", c.evidence.source).to_lowercase(),
            ),
            None => (
                String::new(),
                None,
                None,
                0,
                0,
                String::new(),
                String::new(),
            ),
        };
        let item = match outcome {
            Ok(r) => ReportItem {
                reference: reference.to_owned(),
                name,
                original_path,
                size,
                likelihood,
                confidence,
                category,
                source,
                output_path: Some(r.output_path.display().to_string()),
                bytes_written: r.bytes_written,
                sha256: r.sha256.clone(),
                complete: r.complete,
                error: None,
                diagnostics: r.diagnostics.iter().map(|d| d.message.clone()).collect(),
            },
            Err(e) => ReportItem {
                reference: reference.to_owned(),
                name,
                original_path,
                size,
                likelihood,
                confidence,
                category,
                source,
                output_path: None,
                bytes_written: 0,
                sha256: None,
                complete: false,
                error: Some(e.to_owned()),
                diagnostics: Vec::new(),
            },
        };
        self.summary.requested += 1;
        if item.error.is_some() {
            self.summary.failed += 1;
        } else if item.complete {
            self.summary.recovered += 1;
        } else {
            self.summary.partial += 1;
        }
        self.summary.bytes_written = self
            .summary
            .bytes_written
            .saturating_add(item.bytes_written);
        self.items.push(item);
    }

    /// Renders the report.
    ///
    /// # Errors
    ///
    /// Returns an error only for JSON serialisation failures.
    pub fn render(&self, format: ReportFormat) -> Result<String, serde_json::Error> {
        Ok(match format {
            ReportFormat::Json => serde_json::to_string_pretty(self)?,
            ReportFormat::Markdown => self.to_markdown(),
            ReportFormat::Html => self.to_html(),
        })
    }

    /// Writes the report to `path` in the format its extension implies.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be written.
    pub fn write_to(&self, path: &Path) -> std::io::Result<PathBuf> {
        let text = self
            .render(ReportFormat::from_path(path))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
        Ok(path.to_path_buf())
    }

    /// The Markdown rendering.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let line = |out: &mut String, s: String| {
            out.push_str(&s);
            out.push('\n');
        };
        line(&mut out, "# Recovery report".into());
        line(&mut out, String::new());
        line(
            &mut out,
            format!(
                "Generated {} by {} {}.",
                self.generated_at, self.tool, self.tool_version
            ),
        );
        line(&mut out, String::new());
        line(&mut out, "## Case".into());
        line(&mut out, String::new());
        for (k, v) in self.case_rows() {
            line(&mut out, format!("- **{k}:** {v}"));
        }
        if !self.case.any() {
            line(&mut out, "- no case metadata recorded".into());
        }
        line(&mut out, String::new());
        line(&mut out, "## Source".into());
        line(&mut out, String::new());
        for (k, v) in self.source_rows() {
            line(&mut out, format!("- **{k}:** {v}"));
        }
        line(&mut out, String::new());
        line(&mut out, "## Files".into());
        line(&mut out, String::new());
        line(
            &mut out,
            "| Reference | Name | Original path | Size | Health | Result | SHA-256 |".into(),
        );
        line(&mut out, "|---|---|---|---|---|---|---|".into());
        for i in &self.items {
            line(
                &mut out,
                format!(
                    "| {} | {} | {} | {} | {} | {} | {} |",
                    md_cell(&i.reference),
                    md_cell(&i.name),
                    md_cell(i.original_path.as_deref().unwrap_or("")),
                    i.size.map(bytes_si).unwrap_or_default(),
                    if i.category.is_empty() {
                        String::new()
                    } else {
                        format!("{} {}%", i.category, i.likelihood)
                    },
                    md_cell(&item_result(i)),
                    i.sha256.clone().unwrap_or_default()
                ),
            );
        }
        line(&mut out, String::new());
        line(&mut out, "## Summary".into());
        line(&mut out, String::new());
        let s = &self.summary;
        line(
            &mut out,
            format!(
                "{} requested, {} recovered, {} partial, {} failed, {} written.",
                s.requested,
                s.recovered,
                s.partial,
                s.failed,
                bytes_si(s.bytes_written)
            ),
        );
        out
    }

    /// The HTML rendering (self-contained, no scripts).
    #[must_use]
    pub fn to_html(&self) -> String {
        let mut out = String::new();
        out.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Recovery report</title>\n<style>body{font-family:system-ui,sans-serif;margin:2rem;color:#222}table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:.3rem .5rem;text-align:left;font-size:.9rem}th{background:#f3f3f3}code{font-family:ui-monospace,monospace;font-size:.85rem}.bad{color:#a00}.good{color:#060}</style></head><body>\n");
        out.push_str("<h1>Recovery report</h1>\n");
        out.push_str(&format!(
            "<p>Generated {} by {} {}.</p>\n",
            esc(&self.generated_at),
            esc(&self.tool),
            esc(&self.tool_version)
        ));
        out.push_str("<h2>Case</h2>\n<dl>\n");
        for (k, v) in self.case_rows() {
            out.push_str(&format!("<dt>{}</dt><dd>{}</dd>\n", esc(k), esc(&v)));
        }
        if !self.case.any() {
            out.push_str("<dd>no case metadata recorded</dd>\n");
        }
        out.push_str("</dl>\n<h2>Source</h2>\n<dl>\n");
        for (k, v) in self.source_rows() {
            out.push_str(&format!("<dt>{}</dt><dd>{}</dd>\n", esc(k), esc(&v)));
        }
        out.push_str("</dl>\n<h2>Files</h2>\n<table><thead><tr><th>Reference</th><th>Name</th><th>Original path</th><th>Size</th><th>Health</th><th>Result</th><th>SHA-256</th></tr></thead><tbody>\n");
        for i in &self.items {
            let class = if i.error.is_some() || !i.complete {
                "bad"
            } else {
                "good"
            };
            out.push_str(&format!(
                "<tr class=\"{class}\"><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td></tr>\n",
                esc(&i.reference),
                esc(&i.name),
                esc(i.original_path.as_deref().unwrap_or("")),
                i.size.map(bytes_si).unwrap_or_default(),
                if i.category.is_empty() {
                    String::new()
                } else {
                    format!("{} {}%", esc(&i.category), i.likelihood)
                },
                esc(&item_result(i)),
                i.sha256.clone().unwrap_or_default()
            ));
        }
        let s = &self.summary;
        out.push_str(&format!(
            "</tbody></table>\n<h2>Summary</h2>\n<p>{} requested, {} recovered, {} partial, {} failed, {} written.</p>\n</body></html>\n",
            s.requested,
            s.recovered,
            s.partial,
            s.failed,
            bytes_si(s.bytes_written)
        ));
        out
    }

    fn case_rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(v) = &self.case.case_number {
            rows.push(("Case number", v.clone()));
        }
        if let Some(v) = &self.case.evidence_number {
            rows.push(("Evidence number", v.clone()));
        }
        if let Some(v) = &self.case.examiner {
            rows.push(("Examiner", v.clone()));
        }
        if let Some(v) = &self.case.notes {
            rows.push(("Notes", v.clone()));
        }
        rows
    }

    fn source_rows(&self) -> Vec<(&'static str, String)> {
        let s = &self.source;
        let mut rows = vec![
            ("Path", s.path.clone()),
            (
                "Size",
                format!("{} bytes ({})", grouped(s.size), bytes_si(s.size)),
            ),
            (
                "Kind",
                if s.is_device {
                    "block device".to_owned()
                } else {
                    "image file".to_owned()
                },
            ),
        ];
        if let Some(c) = &s.container {
            rows.push(("Container", format!("{} ({})", c.format, c.variant)));
            if c.segments.len() > 1 {
                rows.push(("Segments", c.segments.len().to_string()));
            }
            if let Some(cmp) = &c.compression {
                rows.push(("Compression", cmp.clone()));
            }
            if let Some(md5) = &c.stored_hashes.md5 {
                rows.push(("Stored MD5", md5.clone()));
            }
            if let Some(sha1) = &c.stored_hashes.sha1 {
                rows.push(("Stored SHA-1", sha1.clone()));
            }
            if let Some(a) = &c.acquisition {
                if let Some(v) = &a.description {
                    rows.push(("Image description", v.clone()));
                }
                if let Some(v) = &a.acquisition_date {
                    rows.push(("Acquired", v.clone()));
                }
                if let Some(v) = &a.software_version {
                    rows.push(("Acquisition software", v.clone()));
                }
            }
            if let Some(n) = c.acquisition_errors {
                rows.push(("Acquisition read errors", n.to_string()));
            }
            for d in &c.diagnostics {
                rows.push(("Container note", d.clone()));
            }
        }
        if let Some(v) = &s.verification {
            rows.push(("Computed MD5", v.md5.clone()));
            rows.push(("Computed SHA-1", v.sha1.clone()));
            rows.push(("Computed SHA-256", v.sha256.clone()));
            rows.push((
                "Hash verification",
                match v.verified() {
                    Some(true) => "stored hashes match".to_owned(),
                    Some(false) => "STORED HASHES DO NOT MATCH".to_owned(),
                    None => "no stored hash to compare with".to_owned(),
                },
            ));
        }
        if let Some(v) = &self.volume {
            rows.push((
                "Volume",
                format!(
                    "{} at offset {}, {} ({})",
                    v.filesystem,
                    grouped(v.offset),
                    bytes_si(v.length),
                    v.partition
                        .map_or_else(|| "whole source".to_owned(), |p| format!("partition {p}"))
                ),
            ));
        }
        rows.push(("Destination", self.destination.clone()));
        rows
    }
}

fn item_result(i: &ReportItem) -> String {
    match (&i.error, i.complete) {
        (Some(e), _) => format!("failed: {e}"),
        (None, true) => format!(
            "recovered, {} → {}",
            bytes_si(i.bytes_written),
            i.output_path.as_deref().unwrap_or("")
        ),
        (None, false) => format!(
            "PARTIAL, {} → {}",
            bytes_si(i.bytes_written),
            i.output_path.as_deref().unwrap_or("")
        ),
    }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn sample() -> RecoveryReport {
        let mut r = RecoveryReport::new(
            CaseMetadata {
                case_number: Some("C-1".into()),
                ..Default::default()
            },
            ReportSource {
                path: "disk.E01".into(),
                size: 4096,
                sector_size: 512,
                is_device: false,
                container: None,
                verification: None,
            },
            None,
            Path::new("/out"),
        );
        let result = RecoveryResult {
            output_path: PathBuf::from("/out/a.txt"),
            bytes_expected: Some(3),
            bytes_written: 3,
            sha256: Some("abc".into()),
            complete: true,
            diagnostics: Vec::new(),
        };
        r.push("12", None, Ok(&result));
        r.push("13", None, Err("no layout <b>"));
        r
    }

    #[test]
    fn renders_every_format() {
        let r = sample();
        assert_eq!(r.summary.recovered, 1);
        assert_eq!(r.summary.failed, 1);
        let json = r.render(ReportFormat::Json).unwrap();
        let back: RecoveryReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        let md = r.to_markdown();
        assert!(md.contains("| 12 |") && md.contains("Case number:** C-1"));
        let html = r.to_html();
        assert!(html.contains("&lt;b&gt;") && !html.contains("<b>"));
        assert_eq!(
            ReportFormat::from_path(Path::new("r.HTML")),
            ReportFormat::Html
        );
        assert_eq!(
            ReportFormat::from_path(Path::new("r.md")),
            ReportFormat::Markdown
        );
        assert_eq!(ReportFormat::from_path(Path::new("r")), ReportFormat::Json);
    }

    #[test]
    fn case_defaults_come_from_acquisition() {
        let acq = AcquisitionInfo {
            case_number: Some("A".into()),
            examiner: Some("E".into()),
            ..Default::default()
        };
        let case = CaseMetadata {
            examiner: Some("me".into()),
            ..Default::default()
        }
        .with_acquisition_defaults(Some(&acq));
        assert_eq!(case.case_number.as_deref(), Some("A"));
        assert_eq!(case.examiner.as_deref(), Some("me"));
    }
}
