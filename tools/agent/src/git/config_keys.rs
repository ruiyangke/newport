//! Match normalized libgit2 config keys to an exact section/subsection.
/// `prefix` includes the final dot before the variable name. Variables cannot
/// contain dots; subsections (branch/remote names) can. Prefix-only matching
/// would incorrectly include a separate subsection such as `origin.backup`.
pub(super) fn in_section(key: &[u8], prefix: &str) -> bool {
    key.strip_prefix(prefix.as_bytes())
        .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains(&b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_exact_subsections_and_case() {
        assert!(in_section(b"remote.origin.url", "remote.origin."));
        assert!(!in_section(b"remote.origin.backup.url", "remote.origin."));
        assert!(in_section(
            b"remote.origin.backup.url",
            "remote.origin.backup."
        ));
        assert!(!in_section(b"remote.Origin.url", "remote.origin."));
        assert!(!in_section(b"remote.origin.", "remote.origin."));
    }
}
