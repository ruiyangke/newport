//! Windows representations use UTF-16 text and byte-offset CF_HTML headers.
pub(super) fn privacy_allows(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(&[1, 0, 0, 0])
}

pub(super) fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

pub(super) fn unicode_text(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains('\0') {
        return None;
    }
    let text = to_lf(text).replace('\n', "\r\n");
    Some(
        text.encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect(),
    )
}

pub(super) fn decode_unicode(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect::<Vec<_>>();
    let end = units.iter().position(|unit| *unit == 0)?;
    Some(to_lf(&String::from_utf16(&units[..end]).ok()?).into_bytes())
}

pub(super) fn html_document(fragment: &[u8]) -> Option<Vec<u8>> {
    let fragment = std::str::from_utf8(fragment).ok()?;
    if fragment.contains('\0') {
        return None;
    }
    const PREFIX: &str = "<html><body><!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment--></body></html>";
    let header = |start, end, fragment_start, fragment_end| {
        format!(
            "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{fragment_start:010}\r\nEndFragment:{fragment_end:010}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let fragment_start = start + PREFIX.len();
    let fragment_end = fragment_start + fragment.len();
    let end = fragment_end + SUFFIX.len();
    Some(
        format!(
            "{}{PREFIX}{fragment}{SUFFIX}\0",
            header(start, end, fragment_start, fragment_end)
        )
        .into_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_round_trip_normalizes_newlines_without_doubling_crlf() {
        let bytes = unicode_text("one\r\ntwo\n🦀\rthree".as_bytes()).unwrap();
        let units: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        assert_eq!(
            String::from_utf16(&units[..units.len() - 1]).unwrap(),
            "one\r\ntwo\r\n🦀\rthree"
        );
        assert_eq!(to_lf("one\r\ntwo\n"), "one\ntwo\n");
        assert!(unicode_text(b"a\0b").is_none());
        assert!(unicode_text(&[0xff]).is_none());
        assert_eq!(unicode_text(b"").unwrap(), vec![0, 0]);
    }
    #[test]
    fn privacy_requires_an_explicit_native_allow_value() {
        assert!(privacy_allows(&[1, 0, 0, 0]));
        assert!(privacy_allows(&[1, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!privacy_allows(&[0, 0, 0, 0]));
        assert!(!privacy_allows(&[2, 0, 0, 0]));
        assert!(!privacy_allows(&[1]));
    }
    #[test]
    fn native_text_requires_terminated_valid_utf16() {
        assert!(decode_unicode(&[0x00, 0xd8, 0, 0]).is_none());
        assert!(decode_unicode(&[b'a', 0]).is_none());
        assert!(decode_unicode(&[b'a', 0, 0]).is_none());
        assert_eq!(
            decode_unicode(&unicode_text("é🦀\r\nnext".as_bytes()).unwrap()).unwrap(),
            "é🦀\nnext".as_bytes()
        );
    }
    #[test]
    fn html_offsets_survive_multibyte_fragments_and_reject_nul() {
        let fragment = "<b>é 🦀</b>".as_bytes();
        let bytes = html_document(fragment).unwrap();
        assert_eq!(crate::html::html_fragment(&bytes), Some(fragment));
        assert!(html_document(b"a\0b").is_none());
    }
}
