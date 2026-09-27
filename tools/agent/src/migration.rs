//! Porthop compatibility lives here. Active commands use Newport names.
use std::{
    env, fs, io,
    os::unix::fs::{symlink, MetadataExt},
    path::{Path, PathBuf},
};

pub const LEGACY_BROWSER: &str = "porthop-browser";

pub fn cache_root(home: &Path) -> io::Result<PathBuf> {
    let legacy = home.join(".cache/porthop/clipboard");
    match fs::symlink_metadata(&legacy) {
        Ok(_) => Ok(legacy),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(home.join(".cache/newport/clipboard")),
        Err(e) => Err(e),
    }
}
pub fn fish_config(directory: &Path) -> PathBuf {
    let legacy = directory.join("porthop.fish");
    if fs::symlink_metadata(&legacy).is_ok() {
        legacy
    } else {
        directory.join("newport.fish")
    }
}
pub fn shell_lock_directory(home: &Path) -> PathBuf {
    let legacy = home.join(".local/share/porthop");
    if legacy.exists() {
        legacy
    } else {
        home.join(".local/share/newport")
    }
}
pub fn shell_start(line: &str) -> bool {
    matches!(line, "# >>> Newport >>>" | "# >>> Porthop >>>")
}
pub fn shell_end(line: &str) -> bool {
    matches!(line, "# <<< Newport <<<" | "# <<< Porthop <<<")
}
pub fn fake_wayland(value: &str) -> bool {
    value.starts_with("/tmp/newport-wl-") || value.starts_with("/tmp/porthop-wl-")
}
pub fn native_clipboard_disabled() -> bool {
    env::var("NEWPORT_CLIPBOARD_NATIVE")
        .or_else(|_| env::var("PORTHOP_CLIPBOARD_NATIVE"))
        .as_deref()
        == Ok("0")
}
pub fn forward_browser() -> bool {
    env::var("NEWPORT_OPEN_ON_MAC")
        .or_else(|_| env::var("PORTHOP_OPEN_ON_MAC"))
        .as_deref()
        == Ok("1")
}
pub fn helper_script(prefix: &[u8]) -> bool {
    prefix.starts_with(b"#!/usr/bin/env bash\n# Newport")
        || prefix.starts_with(b"#!/usr/bin/env bash\n# Porthop")
}
pub fn managed_alias(target: &Path) -> bool {
    matches!(
        target.to_str(),
        Some("newport-agent" | "porthop-agent" | "porthop-clip" | "porthop-open")
    )
}
pub fn commands(directory: &Path) -> io::Result<()> {
    let browser = directory.join(LEGACY_BROWSER);
    if fs::read_link(&browser).is_ok_and(|p| {
        matches!(
            p.to_str(),
            Some("porthop-agent" | "porthop-open" | "newport-agent")
        )
    }) {
        fs::remove_file(&browser)?;
        symlink("newport-agent", &browser)?;
    }
    for (name, marker) in [
        ("porthop-clip", "# Porthop clipboard helper"),
        ("porthop-open", "# Porthop browser helper"),
    ] {
        let path = directory.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
            && fs::read_to_string(&path).is_ok_and(|s| s.lines().any(|l| l.starts_with(marker)))
        {
            fs::remove_file(path)?;
        }
    }
    // Old shell snippets may still call the previous binary. Only replace a
    // recognizable, owned legacy executable; preserve unrelated files/links.
    let old = directory.join("porthop-agent");
    if fs::symlink_metadata(&old).is_ok_and(|m| {
        m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.len() <= 64 * 1024 * 1024
    }) {
        let bytes = fs::read(&old)?;
        if bytes
            .windows(b"porthop-agent/1".len())
            .any(|w| w.starts_with(b"porthop-agent/") && matches!(w.last(), Some(b'1'..=b'5')))
        {
            let staging = tempfile::tempdir_in(directory)?;
            let link = staging.path().join("agent");
            symlink("newport-agent", &link)?;
            fs::rename(link, old)?;
        }
    }
    Ok(())
}
