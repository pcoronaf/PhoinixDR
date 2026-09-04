//! Filename sanitising for the host filesystem.

use std::path::PathBuf;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes one path component safe on Windows and Unix: invalid characters
/// become `_`, trailing dots and spaces are trimmed, reserved device names
/// are prefixed, and an empty result becomes `unnamed`.
#[must_use]
pub fn sanitize_component(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    if out.is_empty() || out == "." || out == ".." {
        out = "unnamed".to_owned();
    }
    out
}

/// Converts an original path such as `\Users\Pablo\doc.docx` (or
/// `\?\doc.docx` when uncertain) into a relative directory path of
/// sanitised components, excluding the final filename.
#[must_use]
pub fn sanitize_relative_path(original_path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    let mut parts: Vec<&str> = original_path
        .split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    parts.pop(); // filename
    for part in parts {
        if part == "?" {
            out.push("_uncertain");
        } else {
            out.push(sanitize_component(part));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components() {
        assert_eq!(
            sanitize_component("report:final?.docx"),
            "report_final_.docx"
        );
        assert_eq!(sanitize_component("trailing. . "), "trailing");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("con.txt"), "_con.txt");
        assert_eq!(sanitize_component(""), "unnamed");
        assert_eq!(sanitize_component(".."), "unnamed");
        assert_eq!(
            sanitize_component("ünïcödé 文件 🚀.txt"),
            "ünïcödé 文件 🚀.txt"
        );
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            sanitize_relative_path("\\Users\\Pablo\\doc.docx"),
            PathBuf::from("Users").join("Pablo")
        );
        assert_eq!(
            sanitize_relative_path("\\?\\doc.docx"),
            PathBuf::from("_uncertain")
        );
        assert_eq!(sanitize_relative_path("doc.docx"), PathBuf::new());
    }
}
