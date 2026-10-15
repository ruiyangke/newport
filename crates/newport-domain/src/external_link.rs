//! Policy for opening links supplied by untrusted remote terminal output.
/// Keep the URL intact except for scheme casing. Parsing/opening stays with the
/// platform opener; this boundary only permits bounded absolute HTTP(S) links.
pub fn http_url(value: &str) -> Option<String> {
    if value.len() > 16 * 1024 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (scheme, rest) = value.split_once("://")?;
    let scheme = if scheme.eq_ignore_ascii_case("https") {
        "https"
    } else if scheme.eq_ignore_ascii_case("http") {
        "http"
    } else {
        return None;
    };
    if rest.is_empty() || rest.starts_with(['/', '?', '#']) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

#[cfg(test)]
mod tests {
    use super::http_url;
    #[test]
    fn terminal_links_allow_only_bounded_http_targets() {
        assert_eq!(
            http_url("HTTPS://example.org/服务?q=a%20b").as_deref(),
            Some("https://example.org/服务?q=a%20b")
        );
        for value in [
            "javascript:alert(1)",
            "ssh://host",
            "file:///etc/passwd",
            "data:text/html,test",
            "//example.org",
            "https://",
            "https:///etc/passwd",
            "http://host\ncommand",
            "http://host\0",
            "http:// host",
            "https://host/unescaped space",
        ] {
            assert!(http_url(value).is_none(), "{value:?}");
        }
        assert!(http_url(&format!("https://host/{}", "x".repeat(16 * 1024))).is_none());
    }
}
