//! Plan of the files on the Mac pasteboard: every entry a transfer must create
//! on the receiving side, in creation order, with limits checked up front.
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const MAX_NAME_BYTES: usize = 255;
/// Pasteboard items one copy may hold.
pub(super) const MAX_ITEMS: usize = 10_000;
/// Pasteboard items the macOS reader returns, so that [`plan`] can refuse more
/// than [`MAX_ITEMS`] with [`ManifestError::TooManyItems`].
pub(super) const MAX_READ_ITEMS: usize = 10 * MAX_ITEMS;
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

/// Entries Finder creates inside folders (`Icon\r` holds a custom folder icon).
/// They are skipped inside copied folders, never as an explicitly copied item.
fn is_finder_metadata(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("Icon\r" | ".DS_Store")
    )
}

/// The form in which a Mac file system compares `name`: APFS and HFS+ volumes
/// are, by default, insensitive to case and to Unicode normalization, so two
/// names with the same key cannot share a folder there.
pub fn mac_name_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.nfd().flat_map(char::to_lowercase).nfd().collect()
}

/// Names are sent composed (NFC), as Linux tools write them. macOS hands out
/// paths decomposed, so `é` would otherwise arrive as `e` + U+0301.
fn name_of(path: &Path) -> Option<String> {
    use unicode_normalization::UnicodeNormalization;
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_name(name))
        .map(|name| name.nfc().collect())
}

struct Walk<'a> {
    limits: &'a Limits,
    entries: Vec<Entry>,
    bytes: u64,
}

impl Walk<'_> {
    fn push(&mut self, entry: Entry) -> Result<(), ManifestError> {
        if self.entries.len() as u64 >= self.limits.entries {
            return Err(ManifestError::TooManyEntries);
        }
        self.bytes = self
            .bytes
            .checked_add(entry.size)
            .filter(|total| *total <= self.limits.total_bytes)
            .ok_or(ManifestError::TooManyBytes)?;
        self.entries.push(entry);
        Ok(())
    }

    fn directory(
        &mut self,
        dir: &Path,
        relative: PathBuf,
        root: &Path,
        mode: u32,
        mtime: Option<u32>,
    ) -> Result<(), ManifestError> {
        self.push(Entry {
            path: relative.clone(),
            kind: EntryKind::Directory,
            size: 0,
            mode,
            mtime,
            source: dir.to_path_buf(),
        })?;
        let io = |error: std::io::Error| ManifestError::Io(dir.to_path_buf(), error.kind());
        let mut children = Vec::new();
        for child in fs::read_dir(dir).map_err(io)? {
            children.push(child.map_err(io)?.path());
        }
        children.sort();
        for path in children {
            if is_finder_metadata(&path) {
                continue;
            }
            let name = name_of(&path).ok_or_else(|| ManifestError::InvalidName(path.clone()))?;
            let child_relative = relative.join(&name);
            let meta = fs::symlink_metadata(&path)
                .map_err(|error| ManifestError::Io(path.clone(), error.kind()))?;
            let kind = meta.file_type();
            let mode = meta.mode() & 0o7777;
            if kind.is_symlink() {
                let target = fs::canonicalize(&path)
                    .ok()
                    .and_then(|target| link_target(&target, root, &relative))
                    .ok_or_else(|| ManifestError::EscapingLink(path.clone()))?;
                self.push(Entry {
                    path: child_relative,
                    kind: EntryKind::Symlink { target },
                    size: 0,
                    mode,
                    mtime: None,
                    source: path,
                })?;
            } else if kind.is_dir() {
                self.directory(&path, child_relative, root, mode, modified(&meta))?;
            } else if kind.is_file() {
                self.push(Entry {
                    path: child_relative,
                    kind: EntryKind::File,
                    size: meta.len(),
                    mode,
                    mtime: modified(&meta),
                    source: path,
                })?;
            } else {
                return Err(ManifestError::Unsupported(path));
            }
        }
        Ok(())
    }
}

fn modified(meta: &fs::Metadata) -> Option<u32> {
    u32::try_from(meta.mtime()).ok()
}

/// The target of a link, relative to the directory `link_dir` (itself relative
/// to the copied tree's `root`), or None when it leaves the tree.
fn link_target(target: &Path, root: &Path, link_dir: &Path) -> Option<PathBuf> {
    let inside = target.strip_prefix(root).ok()?;
    let depth = link_dir.components().count() - 1;
    let mut relative: PathBuf = std::iter::repeat_n(Component::ParentDir, depth).collect();
    relative.push(inside);
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    use unicode_normalization::UnicodeNormalization;
    Some(PathBuf::from(relative.to_str()?.nfc().collect::<String>()))
}

/// Every entry of the copied items, parents before children, with the top-level
/// names first. Anything that cannot be sent exactly is an error.
pub fn plan(paths: &[PathBuf], limits: &Limits) -> Result<Vec<Entry>, ManifestError> {
    if paths.is_empty() {
        return Err(ManifestError::NoItems);
    }
    if paths.len() > MAX_ITEMS {
        return Err(ManifestError::TooManyItems);
    }
    let mut walk = Walk {
        limits,
        entries: Vec::new(),
        bytes: 0,
    };
    for path in paths {
        let name = name_of(path).ok_or_else(|| ManifestError::InvalidName(path.clone()))?;
        if walk
            .entries
            .iter()
            .any(|entry| entry.path == Path::new(&name))
        {
            return Err(ManifestError::DuplicateName(path.clone()));
        }
        let meta =
            fs::metadata(path).map_err(|error| ManifestError::Io(path.clone(), error.kind()))?;
        let mode = meta.mode() & 0o7777;
        if meta.is_file() {
            walk.push(Entry {
                path: PathBuf::from(&name),
                kind: EntryKind::File,
                size: meta.len(),
                mode,
                mtime: modified(&meta),
                source: path.clone(),
            })?;
        } else if meta.is_dir() {
            let root = fs::canonicalize(path)
                .map_err(|error| ManifestError::Io(path.clone(), error.kind()))?;
            walk.directory(&root, PathBuf::from(&name), &root, mode, modified(&meta))?;
        } else {
            return Err(ManifestError::Unsupported(path.clone()));
        }
    }
    Ok(walk.entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub(crate) fn new() -> Self {
            let path = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("newport-clipboard-files-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, bytes: usize) {
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn tree() -> TempDir {
        let dir = TempDir::new();
        write(&dir.0.join("report.pdf"), 10);
        fs::create_dir_all(dir.0.join("photos/raw")).unwrap();
        write(&dir.0.join("photos/a.jpg"), 100);
        write(&dir.0.join("photos/raw/b.raw"), 1000);
        fs::create_dir(dir.0.join("empty")).unwrap();
        dir
    }

    fn summary(entries: &[Entry]) -> Vec<(String, &'static str, u64)> {
        entries
            .iter()
            .map(|entry| {
                let kind = match entry.kind {
                    EntryKind::File => "file",
                    EntryKind::Directory => "dir",
                    EntryKind::Symlink { .. } => "link",
                };
                (entry.path.to_str().unwrap().to_owned(), kind, entry.size)
            })
            .collect()
    }

    #[test]
    fn names_are_sent_composed() {
        let dir = TempDir::new();
        let decomposed = "e\u{301}t\u{e9}";
        write(&dir.0.join(decomposed), 1);
        fs::create_dir(dir.0.join("d")).unwrap();
        write(&dir.0.join("d").join(decomposed), 1);
        let entries = plan(
            &[dir.0.join(decomposed), dir.0.join("d")],
            &Limits::CLIPBOARD,
        )
        .unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["\u{e9}t\u{e9}", "d", "d/\u{e9}t\u{e9}"]);
        assert_eq!(entries[0].source, dir.0.join(decomposed));
    }

    #[test]
    fn plan_lists_parents_before_children_with_top_level_names_first() {
        let dir = tree();
        let paths = ["report.pdf", "photos", "empty"].map(|name| dir.0.join(name));
        let entries = plan(&paths, &Limits::CLIPBOARD).unwrap();
        let expected = [
            ("report.pdf", "file", 10),
            ("photos", "dir", 0),
            ("photos/a.jpg", "file", 100),
            ("photos/raw", "dir", 0),
            ("photos/raw/b.raw", "file", 1000),
            ("empty", "dir", 0),
        ]
        .map(|(path, kind, size)| (path.to_owned(), kind, size));
        assert_eq!(summary(&entries), expected);
        assert_eq!(entries[0].source, paths[0]);
        let mode = fs::metadata(dir.0.join("report.pdf")).unwrap().mode() & 0o7777;
        assert_eq!(entries[0].mode, mode);
    }

    #[test]
    fn limits_return_typed_errors() {
        let dir = tree();
        let all = ["report.pdf", "photos", "empty"].map(|name| dir.0.join(name));
        let limits = |total_bytes, entries| Limits {
            total_bytes,
            entries,
        };
        // report.pdf, photos, raw, a.jpg, b.raw, empty
        assert!(plan(&all, &limits(1110, 6)).is_ok());
        assert_eq!(
            plan(&all, &limits(1109, 6)),
            Err(ManifestError::TooManyBytes)
        );
        assert_eq!(
            plan(&all, &limits(1110, 5)),
            Err(ManifestError::TooManyEntries)
        );
        assert_eq!(plan(&[], &Limits::CLIPBOARD), Err(ManifestError::NoItems));
        assert_eq!(Limits::CLIPBOARD.entries, 10_000);
        assert_eq!(Limits::CLIPBOARD.total_bytes, 2 * 1024 * 1024 * 1024);

        let many = vec![dir.0.join("report.pdf"); MAX_ITEMS + 1];
        assert_eq!(
            plan(&many, &Limits::CLIPBOARD),
            Err(ManifestError::TooManyItems)
        );
        assert_eq!(
            plan(
                &[dir.0.join("report.pdf"), dir.0.join("report.pdf")],
                &Limits::CLIPBOARD
            ),
            Err(ManifestError::DuplicateName(dir.0.join("report.pdf")))
        );
    }

    #[test]
    fn a_tree_beyond_the_old_header_capacity_is_planned() {
        let dir = TempDir::new();
        let big = dir.0.join("big");
        fs::create_dir(&big).unwrap();
        for index in 0..400 {
            write(&big.join(format!("{index}.txt")), 1);
        }
        let entries = plan(&[big], &Limits::CLIPBOARD).unwrap();
        assert_eq!(entries.len(), 401);
    }

    #[test]
    fn unsendable_entries_are_named_not_skipped() {
        let dir = tree();
        let fifo = dir.0.join("photos/pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(
            plan(&[dir.0.join("photos")], &Limits::CLIPBOARD),
            Err(ManifestError::Unsupported(
                dir.0.join("photos").join("pipe")
            ))
        );
        fs::remove_file(&fifo).unwrap();
    }

    #[test]
    fn links_stay_inside_the_tree_and_carry_relative_targets() {
        let dir = tree();
        symlink("a.jpg", dir.0.join("photos/inside")).unwrap();
        symlink(
            dir.0.join("photos/raw/b.raw"),
            dir.0.join("photos/absolute"),
        )
        .unwrap();
        symlink("../a.jpg", dir.0.join("photos/raw/up")).unwrap();
        let entries = plan(&[dir.0.join("photos")], &Limits::CLIPBOARD).unwrap();
        let target = |path: &str| {
            entries
                .iter()
                .find(|entry| entry.path == Path::new(path))
                .map(|entry| entry.kind.clone())
        };
        let link = |target: &str| {
            Some(EntryKind::Symlink {
                target: PathBuf::from(target),
            })
        };
        assert_eq!(target("photos/inside"), link("a.jpg"));
        assert_eq!(target("photos/absolute"), link("raw/b.raw"));
        assert_eq!(target("photos/raw/up"), link("../a.jpg"));
        assert!(entries.iter().all(|entry| !entry.path.is_absolute()));

        symlink(dir.0.join("report.pdf"), dir.0.join("photos/outside")).unwrap();
        assert_eq!(
            plan(&[dir.0.join("photos")], &Limits::CLIPBOARD),
            Err(ManifestError::EscapingLink(
                dir.0.join("photos").join("outside")
            ))
        );
        fs::remove_file(dir.0.join("photos/outside")).unwrap();
        symlink("missing", dir.0.join("photos/dangling")).unwrap();
        assert_eq!(
            plan(&[dir.0.join("photos")], &Limits::CLIPBOARD),
            Err(ManifestError::EscapingLink(
                dir.0.join("photos").join("dangling")
            ))
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        let dir = TempDir::new();
        let bad = dir.0.join("tab\tname");
        write(&bad, 1);
        assert_eq!(
            plan(std::slice::from_ref(&bad), &Limits::CLIPBOARD),
            Err(ManifestError::InvalidName(bad))
        );
        let long = dir.0.join("é".repeat(128));
        write(&long, 1);
        assert_eq!(
            plan(std::slice::from_ref(&long), &Limits::CLIPBOARD),
            Err(ManifestError::InvalidName(long))
        );
        let root = PathBuf::from("/");
        assert_eq!(
            plan(std::slice::from_ref(&root), &Limits::CLIPBOARD),
            Err(ManifestError::InvalidName(root))
        );
        for good in [".hidden", "名前 ✓.txt", "..."] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "a\nb", "\u{7f}"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        let unicode = dir.0.join("名前 ✓.txt");
        write(&unicode, 3);
        assert!(plan(&[unicode], &Limits::CLIPBOARD).is_ok());
    }

    #[test]
    fn finder_metadata_inside_folders_is_skipped_but_other_control_names_are_refused() {
        let dir = TempDir::new();
        let folder = dir.0.join("styled");
        fs::create_dir(&folder).unwrap();
        write(&folder.join("a.txt"), 1);
        write(&folder.join("Icon\r"), 0);
        write(&folder.join(".DS_Store"), 5);
        let entries = plan(std::slice::from_ref(&folder), &Limits::CLIPBOARD).unwrap();
        assert_eq!(
            summary(&entries),
            [
                ("styled".to_owned(), "dir", 0),
                ("styled/a.txt".to_owned(), "file", 1)
            ]
        );

        // An item the user copied explicitly is never skipped.
        let icon = folder.join("Icon\r");
        assert_eq!(
            plan(std::slice::from_ref(&icon), &Limits::CLIPBOARD),
            Err(ManifestError::InvalidName(icon))
        );
        let store = folder.join(".DS_Store");
        assert_eq!(plan(&[store], &Limits::CLIPBOARD).unwrap().len(), 1);

        write(&folder.join("bad\nname"), 1);
        assert_eq!(
            plan(std::slice::from_ref(&folder), &Limits::CLIPBOARD),
            Err(ManifestError::InvalidName(folder.join("bad\nname")))
        );
    }

    #[test]
    fn mac_name_keys_ignore_case_and_normalization() {
        assert_eq!(mac_name_key("Readme.TXT"), mac_name_key("README.txt"));
        assert_eq!(mac_name_key("caf\u{e9}"), mac_name_key("Cafe\u{301}"));
        assert_eq!(
            mac_name_key("\u{c9}t\u{e9}"),
            mac_name_key("e\u{301}T\u{c9}")
        );
        assert_ne!(mac_name_key("cafe"), mac_name_key("café"));
        assert_ne!(mac_name_key("a.txt"), mac_name_key("a.txt "));
    }
}
