//! Validation and redaction for remote URLs crossing the RPC boundary.
use super::protocol::Error;
pub(super) fn validate_url(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::invalid("Invalid remote URL."));
    }
    if value.starts_with('/') {
        return Ok(());
    }
    if !value.contains("://")
        && value.split_once(':').is_some_and(|(host, path)| {
            !host.is_empty() && !host.contains('/') && !host.starts_with('-') && !path.is_empty()
        })
    {
        return Ok(());
    }
    if let Ok(url) = url::Url::parse(value) {
        if ["ssh", "https", "git", "file"].contains(&url.scheme())
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
        {
            return Ok(());
        }
    }
    Err(Error::invalid("Use SSH, HTTPS, git:// or a local path without embedded passwords, query strings or fragments."))
}
pub(super) fn display_url(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        if let Ok(mut url) = url::Url::parse(value) {
            let _ = url.set_password(None);
            if url.scheme() == "https" || url.scheme() == "http" {
                let _ = url.set_username("");
            }
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        } else if validate_url(value).is_ok() {
            value.into()
        } else {
            "[unsupported remote URL]".into()
        }
    })
}
