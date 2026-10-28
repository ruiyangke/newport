pub(super) fn html_fragment(bytes: &[u8]) -> Option<&[u8]> {
    // CF_HTML offsets are byte positions, not Unicode character indices.
    let header_end = bytes
        .iter()
        .position(|byte| *byte == b'<')
        .unwrap_or(bytes.len());
    let header = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let offset = |name: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name))?
            .trim()
            .parse()
            .ok()
    };
    bytes.get(offset("StartFragment:")?..offset("EndFragment:")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_offsets_are_bounded_byte_positions() {
        let prefix = "Version:1.0\r\nStartFragment:0000000065\r\nEndFragment:0000000067\r\n";
        let start = prefix.len() + 4;
        let input = format!(
            "Version:1.0\r\nStartFragment:{start:010}\r\nEndFragment:{:010}\r\n<!--é-->",
            start + 2
        );
        assert_eq!(html_fragment(input.as_bytes()), Some("é".as_bytes()));
        assert!(html_fragment(b"StartFragment:999\nEndFragment:1000\n<x>").is_none());
        assert!(html_fragment(b"StartFragment:40\nEndFragment:1\n<x>").is_none());
        assert!(html_fragment(b"StartFragment:-1\nEndFragment:1\n<x>").is_none());
    }
}
