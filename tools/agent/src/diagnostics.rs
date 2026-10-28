//! Best-effort bounded metadata logs. Never record clipboard bytes or URLs.
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
const LIMIT: u64 = 256 * 1024;
static WRITER: Mutex<()> = Mutex::new(());

pub(crate) fn formats(values: &BTreeMap<String, Vec<u8>>) -> String {
    format!(
        "formats={} bytes={} png_bytes={} text={} html={}",
        values.len(),
        values.values().map(Vec::len).sum::<usize>(),
        values.get("image/png").map_or(0, Vec::len),
        values.contains_key("text/plain"),
        values.contains_key("text/html")
    )
}

pub(crate) fn event(snapshot: &Path, event: &str, details: &str) {
    let Ok(_guard) = WRITER.lock() else { return };
    let path = snapshot.with_file_name("diagnostics.log");
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return;
    };
    let Ok(meta) = file.metadata() else { return };
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
    {
        return;
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let snapshot_meta = std::fs::symlink_metadata(snapshot).ok();
    let inode = snapshot_meta.as_ref().map(|m| m.ino());
    let age_ms = snapshot_meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_millis());
    // Bounded escaped fields prevent hostile format names from injecting log lines.
    let details: String = details.chars().take(512).collect();
    let line = format!(
        "time_ms={time} pid={} snapshot={inode:?} age_ms={age_ms:?} event={event} details={details:?}\n",
        std::process::id()
    );
    if meta.len() + line.len() as u64 > LIMIT && file.set_len(0).is_err() {
        return;
    }
    let _ = file.write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_are_private_bounded_and_exclude_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("snapshot.tar");
        let values = BTreeMap::from([("image/png".into(), b"secret clipboard bytes".to_vec())]);
        for _ in 0..4000 {
            event(&snapshot, "published", &formats(&values));
        }
        let path = dir.path().join("diagnostics.log");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("png_bytes=22"));
        assert!(!text.contains("secret clipboard bytes"));
        assert!(text.len() as u64 <= LIMIT);
        assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }
    #[test]
    fn unsafe_log_destination_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keep");
        std::fs::write(&target, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("diagnostics.log")).unwrap();
        event(&dir.path().join("snapshot.tar"), "published", "metadata");
        assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
    }
}
