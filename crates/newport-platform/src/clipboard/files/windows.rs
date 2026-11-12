use super::{Entry, EntryKind, Limits, MAX_FILE_ITEMS, ManifestError, valid_name, windows_policy};
use std::{
    collections::HashSet,
    fs::File,
    io,
    path::{Path, PathBuf},
};

pub(super) mod cache;
pub(super) mod filesystem;
mod security;

fn io_error(path: &Path, error: impl std::fmt::Display) -> ManifestError {
    let _ = error;
    ManifestError::Io(path.to_owned(), io::ErrorKind::Other)
}

fn name(path: &Path) -> Result<String, ManifestError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_name(name) && windows_policy::component(name).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| ManifestError::InvalidName(path.to_owned()))
}

fn modified(identity: &filesystem::Identity) -> Option<u32> {
    // FILETIME counts 100 ns ticks from 1601; SFTP v3 uses Unix seconds.
    let seconds = identity
        .written
        .checked_div(10_000_000)?
        .checked_sub(11_644_473_600)?;
    u32::try_from(seconds).ok()
}

/// Preserve Windows names exactly; every ancestor and descendant is opened
/// relative to a held directory, without traversing reparse points.
pub fn plan(paths: &[PathBuf], limits: &Limits) -> Result<Vec<Entry>, ManifestError> {
    if paths.is_empty() {
        return Err(ManifestError::NoItems);
    }
    if paths.len() > MAX_FILE_ITEMS {
        return Err(ManifestError::TooManyItems);
    }
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    let mut scheduled = 0u64;
    let mut names = HashSet::new();
    for path in paths {
        let top = name(path)?;
        if !names.insert(top.clone()) {
            return Err(ManifestError::DuplicateName(path.clone()));
        }
        scheduled += 1;
        if scheduled > limits.entries {
            return Err(ManifestError::TooManyEntries);
        }
        let file = filesystem::open_source(path).map_err(|e| io_error(path, e))?;
        let mut pending = vec![(file, path.clone(), PathBuf::from(top), None)];
        while let Some((file, source, relative, visited)) = pending.pop() {
            let identity = filesystem::identity(&file).map_err(|e| io_error(&source, e))?;
            if let Some(planned) = visited {
                if identity != planned {
                    return Err(io_error(&source, "Directory changed"));
                }
                continue;
            }
            if entries.len() as u64 >= limits.entries {
                return Err(ManifestError::TooManyEntries);
            }
            let size = if identity.directory {
                0
            } else {
                identity.bytes
            };
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= limits.total_bytes)
                .ok_or(ManifestError::TooManyBytes)?;
            entries.push(Entry {
                path: relative.clone(),
                kind: if identity.directory {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size,
                mode: if identity.directory { 0o755 } else { 0o644 },
                mtime: modified(&identity),
                source: source.clone(),
                identity: identity.clone(),
            });
            if identity.directory {
                let available = limits.entries.saturating_sub(scheduled) as usize;
                let children = filesystem::children(&file, available).map_err(|e| {
                    if e == "Tree entry count exceeded" {
                        ManifestError::TooManyEntries
                    } else {
                        io_error(&source, e)
                    }
                })?;
                scheduled += children.len() as u64;
                let mut opened = Vec::with_capacity(children.len());
                for child in children {
                    if !valid_name(&child) {
                        return Err(ManifestError::InvalidName(source.join(child)));
                    }
                    let next = filesystem::open_child(&file, &child)
                        .map_err(|e| io_error(&source.join(&child), e))?;
                    opened.push((next, source.join(&child), relative.join(&child), None));
                }
                pending.push((file, source, relative, Some(identity)));
                pending.extend(opened.into_iter().rev());
            }
        }
    }
    Ok(entries)
}

/// A writer-excluding handle to the same object recorded by the manifest.
pub fn open_source(entry: &Entry) -> io::Result<File> {
    let file = filesystem::open_source(&entry.source).map_err(io::Error::other)?;
    let reader = filesystem::stable_reader(&file).map_err(io::Error::other)?;
    validate_source(entry, &reader)?;
    Ok(reader)
}

pub fn validate_source(entry: &Entry, file: &File) -> io::Result<()> {
    let identity = filesystem::identity(file).map_err(io::Error::other)?;
    if identity.directory || identity != entry.identity {
        return Err(io::Error::other(
            "A copied file changed during the transfer",
        ));
    }
    Ok(())
}
