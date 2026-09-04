//! Signature-based file type detection.

use super::FileTypeDetection;

/// A file signature.
#[derive(Debug, Clone, Copy)]
pub struct Signature {
    /// Type identifier.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Typical extension.
    pub extension: &'static str,
    /// Offset of the magic bytes.
    pub offset: usize,
    /// Magic bytes.
    pub magic: &'static [u8],
}

impl Signature {
    /// Whether `head` carries this signature.
    #[must_use]
    pub fn matches(&self, head: &[u8]) -> bool {
        head.get(self.offset..self.offset + self.magic.len())
            .is_some_and(|b| b == self.magic)
    }

    /// Converts to a detection.
    #[must_use]
    pub fn detection(&self) -> FileTypeDetection {
        FileTypeDetection {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            extension: self.extension.to_owned(),
        }
    }
}

/// Known signatures, most specific first.
pub const SIGNATURES: &[Signature] = &[
    Signature {
        id: "jpeg",
        name: "JPEG image",
        extension: "jpg",
        offset: 0,
        magic: &[0xFF, 0xD8, 0xFF],
    },
    Signature {
        id: "png",
        name: "PNG image",
        extension: "png",
        offset: 0,
        magic: &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
    },
    Signature {
        id: "gif",
        name: "GIF image",
        extension: "gif",
        offset: 0,
        magic: b"GIF8",
    },
    Signature {
        id: "bmp",
        name: "BMP image",
        extension: "bmp",
        offset: 0,
        magic: b"BM",
    },
    Signature {
        id: "tiff",
        name: "TIFF image",
        extension: "tif",
        offset: 0,
        magic: &[0x49, 0x49, 0x2A, 0x00],
    },
    Signature {
        id: "tiff",
        name: "TIFF image",
        extension: "tif",
        offset: 0,
        magic: &[0x4D, 0x4D, 0x00, 0x2A],
    },
    Signature {
        id: "pdf",
        name: "PDF document",
        extension: "pdf",
        offset: 0,
        magic: b"%PDF-",
    },
    Signature {
        id: "zip",
        name: "ZIP archive",
        extension: "zip",
        offset: 0,
        magic: &[b'P', b'K', 0x03, 0x04],
    },
    Signature {
        id: "zip",
        name: "ZIP archive (empty)",
        extension: "zip",
        offset: 0,
        magic: &[b'P', b'K', 0x05, 0x06],
    },
    Signature {
        id: "rar",
        name: "RAR archive",
        extension: "rar",
        offset: 0,
        magic: b"Rar!\x1A\x07",
    },
    Signature {
        id: "7z",
        name: "7-Zip archive",
        extension: "7z",
        offset: 0,
        magic: &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C],
    },
    Signature {
        id: "gzip",
        name: "gzip archive",
        extension: "gz",
        offset: 0,
        magic: &[0x1F, 0x8B],
    },
    Signature {
        id: "sqlite",
        name: "SQLite database",
        extension: "sqlite",
        offset: 0,
        magic: b"SQLite format 3\0",
    },
    Signature {
        id: "ole",
        name: "OLE compound document",
        extension: "doc",
        offset: 0,
        magic: &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
    },
    Signature {
        id: "mp4",
        name: "MP4/MOV video",
        extension: "mp4",
        offset: 4,
        magic: b"ftyp",
    },
    Signature {
        id: "riff",
        name: "RIFF container (WAV/AVI/WebP)",
        extension: "riff",
        offset: 0,
        magic: b"RIFF",
    },
    Signature {
        id: "flac",
        name: "FLAC audio",
        extension: "flac",
        offset: 0,
        magic: b"fLaC",
    },
    Signature {
        id: "mp3",
        name: "MP3 audio (ID3)",
        extension: "mp3",
        offset: 0,
        magic: b"ID3",
    },
    Signature {
        id: "elf",
        name: "ELF executable",
        extension: "elf",
        offset: 0,
        magic: &[0x7F, b'E', b'L', b'F'],
    },
    Signature {
        id: "pe",
        name: "Windows executable",
        extension: "exe",
        offset: 0,
        magic: b"MZ",
    },
    Signature {
        id: "xml",
        name: "XML document",
        extension: "xml",
        offset: 0,
        magic: b"<?xml",
    },
    Signature {
        id: "tar",
        name: "tar archive",
        extension: "tar",
        offset: 257,
        magic: b"ustar",
    },
];

/// Detects the type of content from its first bytes.
#[must_use]
pub fn detect_type(head: &[u8]) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.matches(head))
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

    #[test]
    fn detects_common_types() {
        assert_eq!(detect_type(b"%PDF-1.7\n").map(|s| s.id), Some("pdf"));
        assert_eq!(
            detect_type(&[0xFF, 0xD8, 0xFF, 0xE0]).map(|s| s.id),
            Some("jpeg")
        );
        assert_eq!(detect_type(b"PK\x03\x04rest").map(|s| s.id), Some("zip"));
        assert_eq!(detect_type(b"hello").map(|s| s.id), None);
        assert_eq!(detect_type(b"").map(|s| s.id), None);
    }
}
