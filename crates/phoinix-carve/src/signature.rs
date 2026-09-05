//! Carving signatures: declarative header definitions bound to an assembler
//! that determines where the file ends.
//!
//! Simple types are pure data (header bytes, optional footer, size limit);
//! structured types name one of the built-in assemblers. Extra definitions
//! can be supplied as JSON without touching the engine.

use serde::{Deserialize, Serialize};

use crate::CarveError;

/// Bytes that must appear at a fixed offset from the file start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPattern {
    /// Offset of the pattern from the file start.
    pub offset: u32,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// How the end of a file is determined once its header matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblerKind {
    /// JPEG marker walk to the EOI marker.
    Jpeg,
    /// PNG chunk walk (with CRCs) to IEND.
    Png,
    /// GIF block walk to the trailer.
    Gif,
    /// BMP: declared file size.
    Bmp,
    /// PDF: linearization length or the last `%%EOF` of the update chain.
    Pdf,
    /// ZIP family: local headers, central directory, end record.
    Zip,
    /// SQLite: page size × page count.
    Sqlite,
    /// RIFF (WAV, AVI, WebP): declared chunk size.
    Riff,
    /// ISO base media (MP4, MOV, M4A, HEIC): box walk.
    Mp4,
    /// 7-Zip: start header.
    SevenZip,
    /// Generic: search for the footer bytes within the size limit.
    Footer,
    /// Generic: no structure known; the size limit bounds the file.
    HeaderOnly,
}

/// A carving signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarveSignature {
    /// Type identifier (`jpeg`, `pdf`); matches the health validators' ids.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Typical extension.
    pub extension: String,
    /// Header patterns; all must match.
    pub headers: Vec<HeaderPattern>,
    /// Footer bytes for [`AssemblerKind::Footer`].
    pub footer: Option<Vec<u8>>,
    /// Smallest plausible file.
    pub min_size: u64,
    /// Largest file the assembler will walk.
    pub max_size: u64,
    /// How the end is found.
    pub assembler: AssemblerKind,
}

impl CarveSignature {
    /// Bytes from the file start needed to test every header pattern.
    #[must_use]
    pub fn header_span(&self) -> usize {
        self.headers
            .iter()
            .map(|h| {
                usize::try_from(h.offset)
                    .unwrap_or(usize::MAX)
                    .saturating_add(h.bytes.len())
            })
            .max()
            .unwrap_or(0)
    }

    /// Whether `window` (starting at the candidate file start) matches.
    #[must_use]
    pub fn matches(&self, window: &[u8]) -> bool {
        !self.headers.is_empty()
            && self.headers.iter().all(|h| {
                let off = usize::try_from(h.offset).unwrap_or(usize::MAX);
                window
                    .get(off..off.saturating_add(h.bytes.len()))
                    .is_some_and(|w| w == h.bytes.as_slice())
            })
    }
}

/// One JSON signature definition (the on-disk form).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureSpec {
    /// Type identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Typical extension.
    pub extension: String,
    /// Header patterns as `{ "offset": 0, "hex": "25 50 44 46" }`.
    pub headers: Vec<HeaderSpec>,
    /// Footer bytes as hex, for the `footer` assembler.
    #[serde(default)]
    pub footer_hex: Option<String>,
    /// Smallest plausible size (default 0).
    #[serde(default)]
    pub min_size: u64,
    /// Largest size the assembler will walk.
    pub max_size: u64,
    /// Assembler name (`jpeg`, `png`, …, `footer`, `header_only`).
    pub assembler: AssemblerKind,
}

/// A header pattern in JSON form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderSpec {
    /// Offset from the file start.
    #[serde(default)]
    pub offset: u32,
    /// Hex bytes, spaces allowed.
    pub hex: String,
}

fn parse_hex(text: &str) -> Result<Vec<u8>, CarveError> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.is_empty() || clean.len() % 2 != 0 {
        return Err(CarveError::Signature(format!("bad hex {text:?}")));
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| {
            clean
                .get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| CarveError::Signature(format!("bad hex {text:?}")))
        })
        .collect()
}

impl TryFrom<SignatureSpec> for CarveSignature {
    type Error = CarveError;

    fn try_from(spec: SignatureSpec) -> Result<Self, CarveError> {
        if spec.id.is_empty() || spec.headers.is_empty() {
            return Err(CarveError::Signature(
                "a signature needs an id and at least one header".into(),
            ));
        }
        let headers = spec
            .headers
            .iter()
            .map(|h| {
                Ok(HeaderPattern {
                    offset: h.offset,
                    bytes: parse_hex(&h.hex)?,
                })
            })
            .collect::<Result<Vec<_>, CarveError>>()?;
        let footer = spec.footer_hex.as_deref().map(parse_hex).transpose()?;
        if spec.assembler == AssemblerKind::Footer && footer.is_none() {
            return Err(CarveError::Signature(format!(
                "{}: the footer assembler needs footer_hex",
                spec.id
            )));
        }
        Ok(Self {
            id: spec.id,
            name: spec.name,
            extension: spec.extension,
            headers,
            footer,
            min_size: spec.min_size,
            max_size: spec.max_size.max(1),
            assembler: spec.assembler,
        })
    }
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

fn sig(
    id: &str,
    name: &str,
    extension: &str,
    headers: &[(u32, &[u8])],
    min_size: u64,
    max_size: u64,
    assembler: AssemblerKind,
) -> CarveSignature {
    CarveSignature {
        id: id.into(),
        name: name.into(),
        extension: extension.into(),
        headers: headers
            .iter()
            .map(|(offset, bytes)| HeaderPattern {
                offset: *offset,
                bytes: bytes.to_vec(),
            })
            .collect(),
        footer: None,
        min_size,
        max_size,
        assembler,
    }
}

/// The built-in signatures.
#[must_use]
pub fn builtin_signatures() -> Vec<CarveSignature> {
    vec![
        sig(
            "jpeg",
            "JPEG image",
            "jpg",
            &[(0, &[0xFF, 0xD8, 0xFF])],
            128,
            128 * MIB,
            AssemblerKind::Jpeg,
        ),
        sig(
            "png",
            "PNG image",
            "png",
            &[(0, &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])],
            64,
            512 * MIB,
            AssemblerKind::Png,
        ),
        sig(
            "gif",
            "GIF image",
            "gif",
            &[(0, b"GIF8"), (5, b"a")],
            32,
            128 * MIB,
            AssemblerKind::Gif,
        ),
        sig(
            "bmp",
            "Windows bitmap",
            "bmp",
            &[(0, b"BM")],
            26,
            512 * MIB,
            AssemblerKind::Bmp,
        ),
        sig(
            "pdf",
            "PDF document",
            "pdf",
            &[(0, b"%PDF-")],
            64,
            2 * GIB,
            AssemblerKind::Pdf,
        ),
        sig(
            "zip",
            "ZIP archive",
            "zip",
            &[(0, b"PK\x03\x04")],
            22,
            4 * GIB,
            AssemblerKind::Zip,
        ),
        sig(
            "sqlite",
            "SQLite database",
            "sqlite",
            &[(0, b"SQLite format 3\0")],
            512,
            4 * GIB,
            AssemblerKind::Sqlite,
        ),
        sig(
            "riff",
            "RIFF container",
            "riff",
            &[(0, b"RIFF")],
            12,
            4 * GIB,
            AssemblerKind::Riff,
        ),
        sig(
            "mp4",
            "ISO media file",
            "mp4",
            &[(4, b"ftyp")],
            32,
            16 * GIB,
            AssemblerKind::Mp4,
        ),
        sig(
            "7z",
            "7-Zip archive",
            "7z",
            &[(0, &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])],
            32,
            16 * GIB,
            AssemblerKind::SevenZip,
        ),
    ]
}

/// A set of signatures indexed for fast matching.
#[derive(Debug, Clone)]
pub struct SignatureSet {
    signatures: Vec<CarveSignature>,
    /// Signature indices whose first header pattern sits at offset 0, keyed
    /// by its first byte.
    anchored: Vec<Vec<usize>>,
    /// Signature indices whose patterns all sit at a non-zero offset.
    unanchored: Vec<usize>,
    max_header_span: usize,
}

impl Default for SignatureSet {
    fn default() -> Self {
        Self::builtin()
    }
}

impl SignatureSet {
    /// The built-in set.
    #[must_use]
    pub fn builtin() -> Self {
        Self::from_signatures(builtin_signatures())
    }

    /// A set from explicit signatures.
    #[must_use]
    pub fn from_signatures(signatures: Vec<CarveSignature>) -> Self {
        let mut anchored: Vec<Vec<usize>> = vec![Vec::new(); 256];
        let mut unanchored = Vec::new();
        let mut max_header_span = 0usize;
        for (i, s) in signatures.iter().enumerate() {
            max_header_span = max_header_span.max(s.header_span());
            match s
                .headers
                .iter()
                .find(|h| h.offset == 0)
                .and_then(|h| h.bytes.first())
            {
                Some(b) => {
                    if let Some(bucket) = anchored.get_mut(usize::from(*b)) {
                        bucket.push(i);
                    }
                }
                None => unanchored.push(i),
            }
        }
        Self {
            signatures,
            anchored,
            unanchored,
            max_header_span,
        }
    }

    /// Parses a JSON array of [`SignatureSpec`] and appends it.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError::Signature`] for malformed definitions.
    pub fn with_json(self, text: &str) -> Result<Self, CarveError> {
        let specs: Vec<SignatureSpec> =
            serde_json::from_str(text).map_err(|e| CarveError::Signature(e.to_string()))?;
        let mut signatures = self.signatures;
        for spec in specs {
            let s = CarveSignature::try_from(spec)?;
            // A definition with an existing id replaces the built-in one.
            signatures.retain(|existing| existing.id != s.id);
            signatures.push(s);
        }
        Ok(Self::from_signatures(signatures))
    }

    /// Keeps only the signatures whose id is in `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError::Signature`] if an id is unknown.
    pub fn only(self, ids: &[String]) -> Result<Self, CarveError> {
        for id in ids {
            if !self.signatures.iter().any(|s| &s.id == id) {
                return Err(CarveError::Signature(format!(
                    "unknown signature {id:?}; known: {}",
                    self.ids().join(", ")
                )));
            }
        }
        let kept = self
            .signatures
            .into_iter()
            .filter(|s| ids.contains(&s.id))
            .collect();
        Ok(Self::from_signatures(kept))
    }

    /// The signatures.
    #[must_use]
    pub fn signatures(&self) -> &[CarveSignature] {
        &self.signatures
    }

    /// Signature ids.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.signatures.iter().map(|s| s.id.clone()).collect()
    }

    /// The signature at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&CarveSignature> {
        self.signatures.get(index)
    }

    /// Index of the signature with `id`.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.signatures.iter().position(|s| s.id == id)
    }

    /// Bytes needed from a candidate start to test every signature.
    #[must_use]
    pub const fn max_header_span(&self) -> usize {
        self.max_header_span
    }

    /// Indices of the signatures matching at the start of `window`.
    pub fn matches_at<'s>(&'s self, window: &'s [u8]) -> impl Iterator<Item = usize> + 's {
        let first = window.first().copied().map(usize::from);
        let anchored = first
            .and_then(|b| self.anchored.get(b))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        anchored
            .iter()
            .chain(self.unanchored.iter())
            .copied()
            .filter(move |i| self.signatures.get(*i).is_some_and(|s| s.matches(window)))
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
    use super::*;

    #[test]
    fn builtin_matching() {
        let set = SignatureSet::builtin();
        let hits: Vec<&str> = set
            .matches_at(b"%PDF-1.4\n")
            .map(|i| set.get(i).unwrap().id.as_str())
            .collect();
        assert_eq!(hits, vec!["pdf"]);
        let hits: Vec<&str> = set
            .matches_at(b"\0\0\0\x18ftypmp42")
            .map(|i| set.get(i).unwrap().id.as_str())
            .collect();
        assert_eq!(hits, vec!["mp4"]);
        assert_eq!(set.matches_at(b"GIF87a").count(), 1);
        assert_eq!(set.matches_at(b"GIF8xb").count(), 0);
        assert_eq!(set.matches_at(b"").count(), 0);
        assert!(set.max_header_span() >= 16);
    }

    #[test]
    fn json_definitions_extend_and_override() {
        let json = r#"[
          {"id":"foo","name":"Foo file","extension":"foo",
           "headers":[{"offset":0,"hex":"46 4F 4F"}],"footer_hex":"454e44",
           "max_size":1024,"assembler":"footer"},
          {"id":"pdf","name":"PDF (custom)","extension":"pdf",
           "headers":[{"hex":"255044462d"}],"max_size":10,"assembler":"pdf"}
        ]"#;
        let set = SignatureSet::builtin().with_json(json).unwrap();
        let foo = set.get(set.index_of("foo").unwrap()).unwrap();
        assert_eq!(foo.footer.as_deref(), Some(&b"END"[..]));
        assert_eq!(set.get(set.index_of("pdf").unwrap()).unwrap().max_size, 10);
        assert_eq!(set.signatures().iter().filter(|s| s.id == "pdf").count(), 1);
        assert!(
            SignatureSet::builtin()
                .with_json("[{\"id\":\"x\"}]")
                .is_err()
        );
        assert!(
            SignatureSet::builtin()
                .with_json(r#"[{"id":"x","name":"x","extension":"x","headers":[{"hex":"zz"}],"max_size":1,"assembler":"header_only"}]"#)
                .is_err()
        );
        let only = SignatureSet::builtin()
            .only(&["jpeg".to_owned(), "png".to_owned()])
            .unwrap();
        assert_eq!(only.signatures().len(), 2);
        assert!(SignatureSet::builtin().only(&["nope".to_owned()]).is_err());
    }
}
