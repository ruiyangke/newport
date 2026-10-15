use super::*;
use serde_json::json;

fn staging() -> String {
    format!("/work/.newport-upload-{}", Uuid::new_v4())
}
fn shard(path: &Path, server: Uuid) -> PathBuf {
    directory(path)
        .join(server.to_string())
        .join("journal.json")
}
fn value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn upload_migration_preserves_source_and_partitions_connection_scoped_records() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("upload-recovery.json");
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let old_a = json!([a, "old-host", "user"]);
    let new_a = json!([a, "new-host", "user"]);
    let scope_b = json!([b, "host", "user"]);
    let mut legacy = UploadCleanup::open(path.clone());
    let id = legacy.register(old_a.clone(), staging()).unwrap();
    legacy.register(new_a.clone(), staging()).unwrap();
    legacy.register(scope_b.clone(), staging()).unwrap();
    let bytes = fs::read(&path).unwrap();
    drop(legacy);
    let owners = Owners::<UploadCleanup>::open(path.clone());
    assert_eq!(
        value(&path),
        json!({"version":2,"storage":"server-journals"})
    );
    assert_eq!(
        fs::read(directory(&path).join("source.json")).unwrap(),
        bytes
    );
    let a_owner = owners.get(a).unwrap();
    let b_owner = owners.get(b).unwrap();
    assert!(!Arc::ptr_eq(&a_owner, &b_owner));
    assert_eq!(a_owner.lock().unwrap().take(&old_a)[0].0, id);
    assert!(a_owner.lock().unwrap().take(&scope_b).is_empty());
    assert_eq!(a_owner.lock().unwrap().take(&new_a).len(), 1);
    let b_before = fs::read(shard(&path, b)).unwrap();
    a_owner.lock().unwrap().finish(id, true);
    assert_eq!(fs::read(shard(&path, b)).unwrap(), b_before);
    assert_eq!(value(&path)["version"], 2);
    drop(owners);
    let reopened = Owners::<UploadCleanup>::open(path.clone());
    assert!(
        reopened
            .get(a)
            .unwrap()
            .lock()
            .unwrap()
            .take(&old_a)
            .is_empty()
    );
    assert_eq!(
        reopened
            .get(b)
            .unwrap()
            .lock()
            .unwrap()
            .take(&scope_b)
            .len(),
        1
    );
    // The obsolete monolithic reader refuses the descriptor.
    assert!(
        UploadCleanup::open(path)
            .register(old_a, staging())
            .is_err()
    );
}

#[test]
fn legacy_downloads_recover_without_assigning_them_to_a_different_server() {
    let profile = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let path = profile.path().join("download-recovery.json");
    let mut legacy = DownloadCleanup::open(path.clone());
    let (_, file) = legacy.create(folder.path()).unwrap();
    let (handle, staged) = file.keep().unwrap();
    drop(handle);
    fs::write(folder.path().join("keep"), b"destination").unwrap();
    let bytes = fs::read(&path).unwrap();
    drop(legacy);
    let owners = Owners::<DownloadCleanup>::open(path.clone());
    assert!(!staged.exists());
    assert_eq!(
        fs::read(folder.path().join("keep")).unwrap(),
        b"destination"
    );
    assert_eq!(
        fs::read(directory(&path).join("source.json")).unwrap(),
        bytes
    );
    let unassigned = directory(&path).join("unassigned/journal.json");
    assert!(
        value(&unassigned)["entries"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    let before = fs::read(&unassigned).unwrap();
    let server = Uuid::new_v4();
    let owner = owners.get(server).unwrap();
    let (id, file) = owner.lock().unwrap().create(folder.path()).unwrap();
    file.close().unwrap();
    owner.lock().unwrap().finish(id);
    assert_eq!(fs::read(unassigned).unwrap(), before);
    assert!(
        value(&shard(&path, server))["entries"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn restart_recovers_new_download_shards_without_deleting_completed_destinations() {
    let profile = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let path = profile.path().join("download-recovery.json");
    let owners = Owners::<DownloadCleanup>::open(path.clone());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let (_, partial) = owners
        .get(a)
        .unwrap()
        .lock()
        .unwrap()
        .create(folder.path())
        .unwrap();
    let (handle, partial) = partial.keep().unwrap();
    drop(handle);
    let (_, complete) = owners
        .get(b)
        .unwrap()
        .lock()
        .unwrap()
        .create(folder.path())
        .unwrap();
    let destination = folder.path().join("saved");
    complete.persist(&destination).unwrap();
    drop(owners);
    let reopened = Owners::<DownloadCleanup>::open(path.clone());
    assert!(reopened.get(a).is_ok());
    assert!(reopened.get(b).is_ok());
    assert!(!partial.exists());
    assert!(destination.exists());
    for server in [a, b] {
        assert!(
            value(&shard(&path, server))["entries"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn damaged_or_wrong_owner_upload_shards_do_not_block_other_servers() {
    for damage in ["malformed", "wrong-owner", "directory"] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("upload-recovery.json");
        let owners = Owners::<UploadCleanup>::open(path.clone());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        owners
            .get(a)
            .unwrap()
            .lock()
            .unwrap()
            .register(json!([a]), staging())
            .unwrap();
        drop(owners);
        let a_path = shard(&path, a);
        match damage {
            "malformed" => fs::write(&a_path, b"invalid").unwrap(),
            "wrong-owner" => {
                let mut journal = value(&a_path);
                journal["entries"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .next()
                    .unwrap()["scope"] = json!([b]);
                fs::write(&a_path, serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            _ => {
                fs::remove_file(&a_path).unwrap();
                fs::create_dir(&a_path).unwrap();
            }
        }
        let before = fs::read(&a_path).ok();
        let reopened = Owners::<UploadCleanup>::open(path.clone());
        assert!(reopened.get(a).is_err(), "{damage}");
        reopened
            .get(b)
            .unwrap()
            .lock()
            .unwrap()
            .register(json!([b]), staging())
            .unwrap();
        assert_eq!(fs::read(&a_path).ok(), before);
    }
}

#[test]
fn failed_download_write_and_corrupt_shard_are_isolated_from_other_servers() {
    let profile = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let path = profile.path().join("download-recovery.json");
    let owners = Owners::<DownloadCleanup>::open(path.clone());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let owner_a = owners.get(a).unwrap();
    fs::create_dir(shard(&path, a)).unwrap();
    assert!(owner_a.lock().unwrap().create(folder.path()).is_err());
    assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 0);
    let (id, file) = owners
        .get(b)
        .unwrap()
        .lock()
        .unwrap()
        .create(folder.path())
        .unwrap();
    file.close().unwrap();
    owners.get(b).unwrap().lock().unwrap().finish(id);
    drop(owners);
    let reopened = Owners::<DownloadCleanup>::open(path);
    assert!(reopened.get(a).is_err());
    assert!(reopened.get(b).is_ok());
}

#[test]
fn interrupted_migration_resumes_only_an_exact_staged_source() {
    for changed in [false, true] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("upload-recovery.json");
        let server = Uuid::new_v4();
        let mut legacy = UploadCleanup::open(path.clone());
        legacy.register(json!([server]), staging()).unwrap();
        let source = fs::read(&path).unwrap();
        drop(legacy);
        migrate::<UploadCleanup>(&path).unwrap();
        if changed {
            UploadCleanup::open(shard(&path, server))
                .register(json!([server]), staging())
                .unwrap();
        }
        fs::write(&path, &source).unwrap();
        let before = fs::read(shard(&path, server)).unwrap();
        let owners = Owners::<UploadCleanup>::open(path.clone());
        assert_eq!(owners.get(server).is_err(), changed);
        assert_eq!(fs::read(shard(&path, server)).unwrap(), before);
        if changed {
            assert_eq!(fs::read(&path).unwrap(), source);
        } else {
            assert_eq!(value(&path)["version"], 2);
        }
    }
}

#[test]
fn missing_descriptor_or_storage_preserves_evidence_and_refuses_new_writes() {
    for remove_descriptor in [false, true] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("upload-recovery.json");
        let owners = Owners::<UploadCleanup>::open(path.clone());
        let server = Uuid::new_v4();
        owners
            .get(server)
            .unwrap()
            .lock()
            .unwrap()
            .register(json!([server]), staging())
            .unwrap();
        drop(owners);
        if remove_descriptor {
            fs::remove_file(&path).unwrap();
        } else {
            fs::rename(directory(&path), profile.path().join("saved")).unwrap();
        }
        let owners = Owners::<UploadCleanup>::open(path.clone());
        assert!(owners.get(server).is_err());
        assert!(owners.get(Uuid::new_v4()).is_err());
        if remove_descriptor {
            assert!(!path.exists());
            assert!(shard(&path, server).exists());
        } else {
            assert_eq!(value(&path)["version"], 2);
        }
    }
}

#[test]
fn invalid_legacy_records_are_never_overwritten_or_migrated() {
    for source in [
        b"broken".to_vec(),
        serde_json::to_vec(&json!({"version":3,"entries":{}})).unwrap(),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("upload-recovery.json");
        fs::write(&path, &source).unwrap();
        let owners = Owners::<UploadCleanup>::open(path.clone());
        assert!(owners.get(Uuid::new_v4()).is_err());
        assert_eq!(fs::read(&path).unwrap(), source);
        assert!(!directory(&path).exists());
    }
}

#[test]
fn interrupted_server_write_does_not_poison_other_servers_or_delete_evidence() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("upload-recovery.json");
    let owners = Owners::<UploadCleanup>::open(path.clone());
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    owners.get(a).unwrap();
    let partial = shard(&path, a).parent().unwrap().join(".tmp-unfinished");
    fs::write(&partial, b"incomplete journal").unwrap();
    drop(owners);
    let reopened = Owners::<UploadCleanup>::open(path.clone());
    assert!(reopened.get(a).is_err());
    reopened
        .get(b)
        .unwrap()
        .lock()
        .unwrap()
        .register(json!([b]), staging())
        .unwrap();
    assert_eq!(fs::read(partial).unwrap(), b"incomplete journal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&path, &shard(&path, b)] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
