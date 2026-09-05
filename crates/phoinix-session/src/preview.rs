//! Content previews from reconstructed streams, without recovering the file
//! first. Content is hostile input: nothing here decodes it; images are
//! handed to the front-end's webview as base64, text is validated UTF-8,
//! everything else is a hex dump.

use std::io::Read;

use phoinix_core::fmt::hex_dump;
use phoinix_fs::{DeletedFileProvider, RecoveryCandidate};

use crate::dto::Preview;

/// Largest image handed to the front-end.
pub const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
/// Largest text preview.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Bytes shown in a hex dump.
pub const HEX_BYTES: usize = 2048;

fn mime_for(type_id: &str) -> Option<&'static str> {
    Some(match type_id {
        "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Base64 (standard alphabet, padded).
#[must_use]
pub fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        let idx = |shift: u32| {
            char::from(
                TABLE
                    .get(((n >> shift) & 63) as usize)
                    .copied()
                    .unwrap_or(b'='),
            )
        };
        out.push(idx(18));
        out.push(idx(12));
        out.push(if chunk.len() > 1 { idx(6) } else { '=' });
        out.push(if chunk.len() > 2 { idx(0) } else { '=' });
    }
    out
}

/// Whether `bytes` look like text: valid UTF-8 with few control characters.
#[must_use]
pub fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let control = text
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    control * 100 < text.chars().count().max(1)
}

/// Builds a preview for `candidate` through `provider`.
#[must_use]
pub fn preview(provider: &dyn DeletedFileProvider, candidate: &RecoveryCandidate) -> Preview {
    let type_id = candidate
        .evidence
        .content
        .detected_type
        .as_ref()
        .map(|t| t.id.clone());
    let len = candidate.logical_size.unwrap_or(0);
    if len == 0 {
        return Preview::Text {
            text: String::new(),
            truncated: false,
        };
    }
    let mut content = match provider.open_content(candidate) {
        Ok(c) => c,
        Err(e) => {
            return Preview::Unavailable {
                reason: e.to_string(),
            };
        }
    };
    if let Some(mime) = type_id.as_deref().and_then(mime_for) {
        if len > MAX_IMAGE_BYTES {
            return Preview::Unavailable {
                reason: format!("image larger than {} MiB", MAX_IMAGE_BYTES / (1024 * 1024)),
            };
        }
        let mut data = Vec::new();
        if let Err(e) = content.read_to_end(&mut data) {
            return Preview::Unavailable {
                reason: e.to_string(),
            };
        }
        return Preview::Image {
            mime: mime.to_owned(),
            bytes: data.len() as u64,
            base64: base64(&data),
        };
    }
    let mut head = vec![0u8; MAX_TEXT_BYTES.saturating_add(1)];
    let mut filled = 0usize;
    while filled < head.len() {
        let Some(tail) = head.get_mut(filled..) else {
            break;
        };
        match content.read(tail) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) => {
                return Preview::Unavailable {
                    reason: e.to_string(),
                };
            }
        }
    }
    head.truncate(filled);
    let truncated = head.len() > MAX_TEXT_BYTES;
    let shown = head.get(..head.len().min(MAX_TEXT_BYTES)).unwrap_or(&head);
    // Cut on a character boundary for the text check.
    let mut end = shown.len();
    while end > 0 && std::str::from_utf8(shown.get(..end).unwrap_or(&[])).is_err() {
        end -= 1;
    }
    let text_part = shown.get(..end).unwrap_or(&[]);
    if type_id.is_none() && looks_like_text(text_part) {
        return Preview::Text {
            text: String::from_utf8_lossy(text_part).into_owned(),
            truncated,
        };
    }
    let dump_len = shown.len().min(HEX_BYTES);
    Preview::Hex {
        dump: hex_dump(0, shown.get(..dump_len).unwrap_or(&[])),
        bytes: dump_len as u64,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn text_detection() {
        assert!(looks_like_text(b"hello\nworld\t!"));
        assert!(!looks_like_text(&[0, 1, 2, 3, 0xFF]));
        assert!(!looks_like_text(b""));
    }
}
