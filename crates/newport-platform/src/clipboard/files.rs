//! Bounded clipboard file manifests and native filesystem ownership.
use newport_clipboard::MAX_FILE_ITEMS;
use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::plan;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::cache::{
    DownloadDirectory, RetainedFiles, cache_base, open_retained, sweep_cache,
};
#[cfg(windows)]
pub use windows::{open_source, plan, validate_source};
#[cfg(any(windows, test))]
mod windows_policy;

const MAX_NAME_BYTES: usize = 255;
/// Entries (files, directories and links) one transfer may hold.
const MAX_ENTRIES: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub total_bytes: u64,
    pub entries: u64,
}

impl Limits {
    pub const CLIPBOARD: Self = Self {
        total_bytes: 2 << 30,
        entries: MAX_ENTRIES,
    };
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    NoItems,
    TooManyItems,
    InvalidName(PathBuf),
    /// Two top-level items with the same name.
    DuplicateName(PathBuf),
    /// A socket, device or FIFO.
    Unsupported(PathBuf),
    /// A link pointing outside the copied tree, or nowhere.
    EscapingLink(PathBuf),
    TooManyEntries,
    TooManyBytes,
    Io(PathBuf, std::io::ErrorKind),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    /// A link whose relative target stays inside the copied tree.
    Symlink {
        target: PathBuf,
    },
}

/// One thing to create on the receiving side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the destination root, starting with the top-level name.
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Content bytes; zero for directories and links.
    pub size: u64,
    /// Unix permission bits.
    pub mode: u32,
    /// Modification time, in seconds since the Unix epoch; None when it does
    /// not fit (SFTP v3 times are 32 bits).
    pub mtime: Option<u32>,
    /// Where to read the contents on this machine.
    pub source: PathBuf,
    #[cfg(windows)]
    pub(super) identity: windows::filesystem::Identity,
}

/// Whether a transfer may create a file named `name` on either side.
pub fn valid_name(name: &str) -> bool {
    !(name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\')))
}

/// The form in which a Mac file system compares `name`: APFS and HFS+ volumes
/// are, by default, insensitive to case and to Unicode normalization, so two
/// names with the same key cannot share a folder there.
pub fn mac_name_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.nfd().flat_map(char::to_lowercase).nfd().collect()
}

/// Whether a server filename can be represented exactly on this host.
pub fn valid_local_name(name: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    #[cfg(windows)]
    {
        windows_policy::component(name).is_ok()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// The host filesystem's conservative collision key for downloaded names.
pub fn local_name_key(name: &str) -> String {
    #[cfg(windows)]
    {
        name.to_uppercase()
    }
    #[cfg(not(windows))]
    {
        mac_name_key(name)
    }
}
