use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

pub const LIMIT: usize = 32 * 1024 * 1024;

/// Open once: atomic snapshot replacement cannot mix formats from two copies.
/// Never extract paths, follow archive links, or retain stale clipboard data.
pub fn read(path: &Path) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.len() > (LIMIT + 1024 * 1024) as u64
        || meta.modified()?.elapsed().unwrap_or(Duration::MAX) >= Duration::from_secs(120)
    {
        return Err(io::Error::other(
            "clipboard snapshot expired or exceeds the limit",
        ));
    }
    let mut result = BTreeMap::new();
    let mut size: usize = 0;
    for (index, entry) in tar::Archive::new(file).entries()?.enumerate() {
        if index >= 257 {
            return Err(io::Error::other("too many clipboard formats"));
        }
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        if name == "TARGETS" {
            continue;
        }
        if result.len() >= 256
            || name.len() > 256
            || name.chars().any(char::is_control)
            || name
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(io::Error::other("invalid clipboard formats"));
        }
        let len = usize::try_from(entry.size()).map_err(io::Error::other)?;
        size = size
            .checked_add(len)
            .filter(|s| *s <= LIMIT)
            .ok_or_else(|| io::Error::other("clipboard exceeds the limit"))?;
        let mut bytes = Vec::with_capacity(len);
        entry.read_to_end(&mut bytes)?;
        result.insert(name, bytes);
    }
    Ok(result)
}
