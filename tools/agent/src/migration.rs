//! Porthop compatibility lives here. Active commands use Newport names.
use std::{
    env, fs, io,
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
    value.starts_with("/tmp/newport-wl-")
        || value.starts_with("/tmp/porthop-wl-")
        || crate::paths::integration_display_path(value)
}
pub fn native_clipboard_disabled() -> bool {
    env::var("NEWPORT_CLIPBOARD_NATIVE")
        .or_else(|_| env::var("PORTHOP_CLIPBOARD_NATIVE"))
        .as_deref()
        == Ok("0")
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
pub(crate) fn plan_commands(
    directory: &Path,
    transaction: &mut crate::helper_transaction::Transaction,
) -> io::Result<()> {
    use crate::helper_transaction::Snapshot;
    let browser = directory.join(LEGACY_BROWSER);
    let before = transaction.read(&browser)?;
    if matches!(&before, Snapshot::Link(target) if matches!(target.to_str(), Some("porthop-agent" | "porthop-open" | "newport-agent")))
    {
        transaction.plan(&browser, before, Snapshot::Link("newport-agent".into()))?;
    }
    for (name, marker) in [
        ("porthop-clip", "# Porthop clipboard helper"),
        ("porthop-open", "# Porthop browser helper"),
    ] {
        let path = directory.join(name);
        let before = transaction.read(&path)?;
        if matches!(before, Snapshot::File { .. }) {
            let bytes = transaction.bytes(&before)?;
            if std::str::from_utf8(&bytes).is_ok_and(|s| s.lines().any(|l| l.starts_with(marker))) {
                transaction.plan(&path, before, Snapshot::Missing)?;
            }
        }
    }
    let old = directory.join("porthop-agent");
    let before = transaction.read(&old)?;
    if matches!(before, Snapshot::File { .. }) {
        let bytes = transaction.bytes(&before)?;
        if bytes
            .windows(b"porthop-agent/1".len())
            .any(|w| w.starts_with(b"porthop-agent/") && matches!(w.last(), Some(b'1'..=b'5')))
        {
            transaction.plan(&old, before, Snapshot::Link("newport-agent".into()))?;
        }
    }
    Ok(())
}
