//! Immutable recovery snapshots made while profile ownership is reserved and
//! before any credential read, database opening, or background worker.
use newport_platform::filesystem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const BACKUPS: &str = "upgrade-backups";
/// Snapshots kept: the one for this version and the one before.
const KEEP: usize = 2;
/// Left out of snapshots: the backups themselves, the live socket and lock,
/// and the account sign-in, so a device credential never outlives signing out
/// in an old snapshot. Restoring one means signing in again.
const NOT_SNAPSHOTTED: [&str; 5] = [
    BACKUPS,
    "native-client",
    super::instance::LOCK_FILES[0],
    super::instance::LOCK_FILES[1],
    super::account::FILE,
];

/// Temporary files and folders an interrupted write left behind.
fn leftover(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(".tmp") || name.starts_with(".pending-"))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    version: String,
    files: BTreeMap<PathBuf, String>,
}

pub(crate) fn preserve(directory: &Path, version: &str) -> Result<(), String> {
    preserve_inner(directory, version).map_err(|error| {
        format!(
            "Cannot preserve the profile before upgrading: {error}. Original data was not changed."
        )
    })
}

fn preserve_inner(directory: &Path, version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err("Invalid upgrade version".into());
    }
    let root = directory.join(BACKUPS);
    let destination = root.join(format!("native-{version}"));
    // Do not follow links or replace an incomplete/modified recovery snapshot.
    if root.try_exists().map_err(|e| e.to_string())? {
        regular_directory(&root)?;
    } else {
        filesystem::create_private_directory(&root).map_err(|e| e.to_string())?;
    }
    if destination.try_exists().map_err(|e| e.to_string())? {
        regular_directory(&destination)?;
        let manifest_path = destination.join("manifest.json");
        if !fs::symlink_metadata(&manifest_path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err("Recovery manifest is not a regular file".into());
        }
        let manifest: Manifest = serde_json::from_reader(
            fs::File::open(manifest_path)
                .map_err(|e| e.to_string())?
                .take(8 * 1024 * 1024),
        )
        .map_err(|e| e.to_string())?;
        let mut files = BTreeMap::new();
        inventory(
            &destination.join("data"),
            Path::new(""),
            None,
            false,
            &mut files,
        )?;
        if manifest.schema != 1 || manifest.version != version || manifest.files != files {
            return Err(
                "Recovery snapshot verification failed; preserve it for manual recovery".into(),
            );
        }
        // A previous attempt may have published successfully but failed while
        // syncing its parent. Retry durability before opening the profile.
        filesystem::sync_directory(&root).map_err(|e| e.to_string())?;
        filesystem::sync_directory(directory).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let staging = tempfile::Builder::new()
        .prefix(".pending-")
        .tempdir_in(&root)
        .map_err(|e| e.to_string())?;
    filesystem::protect_directory(staging.path()).map_err(|e| e.to_string())?;
    let data = staging.path().join("data");
    filesystem::create_private_directory(&data).map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    inventory(directory, Path::new(""), Some(&data), true, &mut files)?;
    let mut verified = BTreeMap::new();
    inventory(&data, Path::new(""), None, false, &mut verified)?;
    if files != verified {
        return Err("Recovery copy verification failed".into());
    }
    let manifest = Manifest {
        schema: 1,
        version: version.into(),
        files,
    };
    let mut file = filesystem::private_file_options()
        .write(true)
        .create_new(true)
        .open(staging.path().join("manifest.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    filesystem::sync_directory(staging.path()).map_err(|e| e.to_string())?;
    filesystem::publish_directory(staging.path(), &destination).map_err(|e| e.to_string())?;
    filesystem::sync_directory(&root).map_err(|e| e.to_string())?;
    filesystem::sync_directory(directory).map_err(|e| e.to_string())?;
    prune(&root, &destination);
    Ok(())
}

/// Keeps the new snapshot and the one made before it; older ones and staging
/// folders a crash left behind go. This runs while the profile is reserved,
/// so no other preserve is writing here. A snapshot that can't be removed
/// stays; it only costs space.
fn prune(root: &Path, newest: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut snapshots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !directory {
            continue;
        }
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(".pending-"))
        {
            let _ = fs::remove_dir_all(&path);
        } else if name
            .to_str()
            .is_some_and(|name| name.starts_with("native-"))
            && path != newest
        {
            let made = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            snapshots.push((made, path));
        }
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.0));
    for (_, old) in snapshots.into_iter().skip(KEEP - 1) {
        let _ = fs::remove_dir_all(old);
    }
}

fn regular_directory(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        Ok(())
    } else {
        Err(format!("{} is not a regular directory", path.display()))
    }
}

fn hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn inventory(
    source: &Path,
    relative: &Path,
    destination: Option<&Path>,
    profile: bool,
    files: &mut BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    regular_directory(&source.join(relative))?;
    for entry in fs::read_dir(source.join(relative)).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if profile
            && relative.as_os_str().is_empty()
            && (NOT_SNAPSHOTTED.iter().any(|s| name == *s) || leftover(&name))
        {
            continue;
        }
        let path = relative.join(name);
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            if let Some(destination) = destination {
                filesystem::create_private_directory(destination.join(&path))
                    .map_err(|e| e.to_string())?;
            }
            inventory(source, &path, destination, profile, files)?;
        } else if kind.is_file() {
            let digest = hash(&entry.path())?;
            if let Some(destination) = destination {
                let copied = destination.join(&path);
                fs::copy(entry.path(), &copied).map_err(|e| e.to_string())?;
                filesystem::protect_file(&copied).map_err(|e| e.to_string())?;
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&copied)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            files.insert(path, digest);
        } else {
            return Err(format!(
                "Unsupported profile entry {}; no links or special files are followed",
                path.display()
            ));
        }
    }
    if let Some(destination) = destination {
        filesystem::sync_directory(&destination.join(relative)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn restored_snapshot_includes_committed_rows_still_in_sqlite_wal() {
        use sqlx::{
            Connection,
            sqlite::{SqliteConnectOptions, SqliteJournalMode},
        };
        let profile = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(profile.path().join("metrics.sqlite3"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("wal_autocheckpoint", "0");
        let mut db = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sentinel(value TEXT)")
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sentinel VALUES ('retained history')")
            .execute(&mut db)
            .await
            .unwrap();
        assert!(
            fs::metadata(profile.path().join("metrics.sqlite3-wal"))
                .unwrap()
                .len()
                > 0
        );
        preserve(profile.path(), "0.3.1").unwrap();
        // Restore a copy, never open the immutable recovery snapshot in SQLite.
        let restored = tempfile::tempdir().unwrap();
        let saved = profile.path().join("upgrade-backups/native-0.3.1/data");
        inventory(
            &saved,
            Path::new(""),
            Some(restored.path()),
            false,
            &mut BTreeMap::new(),
        )
        .unwrap();
        db.close().await.unwrap();
        let mut recovered = sqlx::SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(restored.path().join("metrics.sqlite3")),
        )
        .await
        .unwrap();
        let value: String = sqlx::query_scalar("SELECT value FROM sentinel")
            .fetch_one(&mut recovered)
            .await
            .unwrap();
        assert_eq!(value, "retained history");
        recovered.close().await.unwrap();
        preserve(profile.path(), "0.3.1").unwrap();
    }

    #[test]
    fn preserves_vault_wal_preferences_and_unknown_data_without_overwriting() {
        let profile = tempfile::tempdir().unwrap();
        let root = profile.path();
        let config: newport_domain::model::Config = serde_json::from_value(serde_json::json!({
            "servers": [{"id": uuid::Uuid::new_v4(), "name": "Backup fixture",
                "sshHost": "127.0.0.1", "sshPort": 22, "sshUser": "fixture"}],
            "tunnels": []
        }))
        .unwrap();
        super::super::vault::write(&root.join("profiles.stronghold"), &[7; 32], &config).unwrap();
        for name in [
            "metrics.sqlite3",
            "metrics.sqlite3-wal",
            "preferences.json",
            "future-state.json",
        ] {
            fs::write(root.join(name), name).unwrap();
        }
        fs::create_dir(root.join("upload-recovery.json.servers")).unwrap();
        fs::write(
            root.join("upload-recovery.json.servers/one.json"),
            "pending recovery",
        )
        .unwrap();
        fs::create_dir(root.join("native-client")).unwrap();
        fs::write(root.join("native-client/transient"), "socket fixture").unwrap();
        fs::write(root.join(super::super::instance::LOCK_FILES[0]), "locked").unwrap();
        preserve(root, "0.3.1").unwrap();
        let saved = root.join("upgrade-backups/native-0.3.1/data");
        assert!(!saved.join("native-client").exists());
        assert!(!saved.join("newport.lock").exists());
        assert_eq!(
            fs::read(saved.join("profiles.stronghold")).unwrap(),
            fs::read(root.join("profiles.stronghold")).unwrap()
        );
        assert_eq!(
            fs::read(saved.join("metrics.sqlite3-wal")).unwrap(),
            b"metrics.sqlite3-wal"
        );
        assert_eq!(
            fs::read(saved.join("upload-recovery.json.servers/one.json")).unwrap(),
            b"pending recovery"
        );
        fs::write(root.join("preferences.json"), "new preferences").unwrap();
        preserve(root, "0.3.1").unwrap();
        assert_eq!(
            fs::read(saved.join("preferences.json")).unwrap(),
            b"preferences.json"
        );
        fs::write(saved.join("preferences.json"), "damaged").unwrap();
        assert!(preserve(root, "0.3.1").is_err());
        assert_eq!(
            fs::read(root.join("preferences.json")).unwrap(),
            b"new preferences"
        );
    }

    #[test]
    fn snapshots_leave_out_leftovers_and_the_sign_in_and_only_two_are_kept() {
        let profile = tempfile::tempdir().unwrap();
        let root = profile.path();
        fs::write(root.join("preferences.json"), "{}").unwrap();
        fs::write(root.join("account.stronghold"), "credential").unwrap();
        fs::write(
            root.join("gpui-workspace-state.1234.tmp"),
            "kept: not a leftover name",
        )
        .unwrap();
        fs::create_dir(root.join(".tmpAbC123")).unwrap();
        fs::write(root.join(".tmpAbC123/account.stronghold"), "half-written").unwrap();
        preserve(root, "0.4.0").unwrap();
        let saved = root.join("upgrade-backups/native-0.4.0/data");
        assert!(saved.join("preferences.json").exists());
        assert!(!saved.join("account.stronghold").exists());
        assert!(!saved.join(".tmpAbC123").exists());
        // A staging folder from a crashed attempt goes with the next snapshot.
        fs::create_dir(root.join("upgrade-backups/.pending-stale")).unwrap();
        for version in ["0.4.1", "0.4.2"] {
            std::thread::sleep(std::time::Duration::from_millis(20));
            preserve(root, version).unwrap();
        }
        let mut kept: Vec<_> = fs::read_dir(root.join("upgrade-backups"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        kept.sort();
        assert_eq!(kept, ["native-0.4.1", "native-0.4.2"]);
    }

    #[cfg(unix)]
    #[test]
    fn failed_snapshot_never_publishes_success_or_follows_links() {
        let profile = tempfile::tempdir().unwrap();
        fs::write(profile.path().join("profiles.stronghold"), "original").unwrap();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), profile.path().join("linked")).unwrap();
        assert!(preserve(profile.path(), "0.3.1").is_err());
        assert!(!profile.path().join("upgrade-backups/native-0.3.1").exists());
        assert_eq!(
            fs::read(profile.path().join("profiles.stronghold")).unwrap(),
            b"original"
        );
        fs::remove_file(profile.path().join("linked")).unwrap();
        preserve(profile.path(), "0.3.1").unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(
                profile
                    .path()
                    .join("upgrade-backups/native-0.3.1/data/profiles.stronghold")
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }
}
