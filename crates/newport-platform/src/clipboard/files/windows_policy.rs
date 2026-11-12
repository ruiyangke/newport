//! Names and local file URIs representable without Windows path rewriting.
pub(crate) fn component(name: &str) -> Result<(), String> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.ends_with(['.', ' '])
        || name.encode_utf16().count() > 255
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err("File name cannot be represented exactly on Windows".into());
    }
    let base = name
        .split('.')
        .next()
        .unwrap()
        .trim_end_matches(' ')
        .to_uppercase();
    let device = matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        base.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit()
                || matches!(suffix, "¹" | "²" | "³")
        })
    });
    if device {
        return Err("Windows device names cannot be transferred".into());
    }
    Ok(())
}

/// Only local drive paths are admitted. UNC/device namespaces and alternate
/// streams need separate authorization and transport semantics.
pub(crate) fn drive_path(path: &str) -> Result<(char, Vec<&str>), String> {
    let path = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let bytes = path.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'/' | b'\\')
    {
        return Err("Only absolute local drive paths are supported on Windows".into());
    }
    let parts: Vec<_> = path[3..].split(['/', '\\']).collect();
    for part in &parts {
        component(part)?;
    }
    Ok((bytes[0] as char, parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambiguous_windows_names_are_refused_without_rewriting() {
        for name in [
            "",
            ".",
            "..",
            "name.",
            "name ",
            "a:b",
            "a/b",
            "a\\b",
            "a?b",
            "CON",
            "con.txt",
            "con .txt",
            "LPT1",
            "com¹.log",
            "NUL",
        ] {
            assert!(component(name).is_err(), "{name:?}");
        }
        for name in ["世界 #%.txt", "console", "COM10", "é.txt", "é.txt"] {
            assert!(component(name).is_ok(), "{name}");
        }
        assert!(component(&"😀".repeat(128)).is_err());
    }
    #[test]
    fn paths_cannot_change_namespaces_or_acquire_path_syntax() {
        assert!(drive_path("C:\\test\\世界.txt").is_ok());
        assert!(drive_path("\\\\?\\C:\\test\\a").is_ok());
        for path in [
            "C:a",
            "\\\\server\\share\\a",
            "\\\\.\\C:\\a",
            "C:\\a\\..\\b",
            "C:\\a:stream",
            "C:\\a\\",
        ] {
            assert!(drive_path(path).is_err(), "{path}");
        }
    }
}
