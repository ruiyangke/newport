//! Files nothing reads any more, removed when this process takes ownership of
//! the profile: one Newport runs per profile, so no write is in progress.
//! Interrupted saves leave temporary files and folders (one may hold a copy of
//! the account sign-in), and earlier builds left files no code uses.
use std::{fs, path::Path};

/// Prefixes of temporary names in the profile folder: `tempfile`'s, and staging
/// folders of interrupted migrations.
const TEMPORARY: [&str; 3] = [".tmp", ".needle-migrate-", ".files-migrate-"];
/// What earlier builds wrote that nothing reads: a startup migration marker and
/// the Needle operation journals. Files or folders. (`porthop.lock` is not one:
/// released builds still lock it, see `instance::LOCK_FILES`.)
const RETIRED: [&str; 3] = [
    "newport-startup-migrated",
    "needle-operations.json",
    "needle-operations.json.servers",
];

pub(crate) fn sweep(directory: &Path) {
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let temporary = name
                .to_str()
                .is_some_and(|name| TEMPORARY.iter().any(|prefix| name.starts_with(prefix)));
            if temporary {
                remove(&entry.path());
            }
        }
    }
    for retired in RETIRED {
        let path = directory.join(retired);
        remove(&path);
    }
}

fn remove(path: &Path) {
    let removed = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(_) => return,
    };
    if let Err(error) = removed {
        log::warn!("Could not remove {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_removes_leftovers_and_keeps_everything_else() {
        let profile = tempfile::tempdir().unwrap();
        let root = profile.path();
        let keep = [
            "preferences.json",
            "account.stronghold",
            "ui-state.json",
            "tmp-notes",
            "porthop.lock",
        ];
        for name in keep {
            fs::write(root.join(name), b"x").unwrap();
        }
        fs::create_dir(root.join(".tmpmx3k7R")).unwrap();
        fs::write(root.join(".tmpmx3k7R/account.stronghold"), b"copy").unwrap();
        fs::write(root.join(".tmpAbCd"), b"half").unwrap();
        fs::write(root.join("newport-startup-migrated"), b"1").unwrap();
        fs::create_dir(root.join("needle-operations.json.servers")).unwrap();
        fs::write(
            root.join("needle-operations.json.servers/legacy.json"),
            b"{}",
        )
        .unwrap();
        fs::create_dir(root.join("needle-operations.json.servers/0b5f")).unwrap();
        sweep(root);
        for name in keep {
            assert!(root.join(name).exists(), "{name} was removed");
        }
        assert!(!root.join("needle-operations.json.servers").exists());
        for gone in [
            ".tmpmx3k7R",
            ".tmpAbCd",
            "newport-startup-migrated",
            "needle-operations.json.servers/legacy.json",
        ] {
            assert!(!root.join(gone).exists(), "{gone} stayed");
        }
    }
}
