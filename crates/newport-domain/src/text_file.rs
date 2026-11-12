//! Content checks shared by text-file readers and editors. Never decode lossily.
pub const EDIT_LIMIT: usize = 16 * 1024 * 1024;
pub const HIGHLIGHT_LIMIT: usize = 512 * 1024;
// Shaping a single multi-megabyte line is not viewport-virtualized by GPUI yet.
pub const MAX_EDIT_LINE: usize = 32 * 1024;
pub fn has_long_lines(text: &str) -> bool {
    text.split('\n').any(|line| line.len() > MAX_EDIT_LINE)
}

pub fn has_binary_signature(bytes: &[u8]) -> bool {
    [
        b"%PDF-".as_slice(),
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"\x7fELF",
        b"SQLite format 3\0",
        b"\x1f\x8b",
        b"\x00asm",
        b"\xd0\xcf\x11\xe0",
        b"\xca\xfe\xba\xbe",
        b"\xcf\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
    ]
    .iter()
    .any(|signature| bytes.starts_with(signature))
        || (bytes.starts_with(b"RIFF")
            && bytes.len() >= 12
            && matches!(&bytes[8..12], b"WEBP" | b"WAVE" | b"AVI "))
}

/// NUL is a strong binary signal. Sparse control characters can legitimately
/// occur in generated source, regex literals and logs; a dense run is not text.
pub fn has_binary_controls(bytes: &[u8]) -> bool {
    bytes.contains(&0)
        || bytes
            .iter()
            .filter(|&&b| matches!(b, 1..=8 | 14..=31 | 127))
            .count()
            > bytes.len() / 100
}

pub fn validate(text: &str) -> Result<(), &'static str> {
    if has_binary_signature(text.as_bytes()) || has_binary_controls(text.as_bytes()) {
        Err("This is a binary file. Download it from Files to open it in a compatible app.")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_readable_binary_headers_and_control_data() {
        assert!(super::validate(&format!("{}\u{1b}\u{85}", "source ".repeat(100))).is_ok());
        assert!(super::validate(&format!("{}\0", "source ".repeat(100))).is_err());
        for text in ["%PDF-1.7\nobject", "GIF89a readable", "a\0b", "data\u{1b}"] {
            assert!(super::validate(text).is_err(), "{text:?}");
        }
        for text in ["", "世界\t🙂\r\n", "// PDF file reader", "{\"key\":42}"] {
            assert!(super::validate(text).is_ok());
        }
    }
}
