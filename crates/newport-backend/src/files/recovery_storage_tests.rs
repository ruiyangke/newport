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
fn fresh_storage_writes_descriptor_and_keeps_server_journals_independent() {
    let profile = tempfile::tempdir().unwrap();
    let path = profile.path().join("upload-recovery.json");
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let scope_a = json!([a, "host", "user"]);
    let scope_b = json!([b, "host", "user"]);
    let owners = Owners::<UploadCleanup>::open(path.clone());
    assert_eq!(
        value(&path),
        json!({"version":2,"storage":"server-journals"})
    );
    let a_owner = owners.get(a).unwrap();
    let b_owner = owners.get(b).unwrap();
    assert!(!Arc::ptr_eq(&a_owner, &b_owner));
    let id = a_owner
        .lock()
        .unwrap()
        .register(scope_a.clone(), staging())
        .unwrap();
    b_owner
        .lock()
        .unwrap()
        .register(scope_b.clone(), staging())
        .unwrap();
    let b_before = fs::read(shard(&path, b)).unwrap();
    a_owner.lock().unwrap().finish(id, true);
    assert_eq!(fs::read(shard(&path, b)).unwrap(), b_before);
    drop((a_owner, b_owner, owners));
    let reopened = Owners::<UploadCleanup>::open(path.clone());
    assert!(
        reopened
            .get(a)
            .unwrap()
            .lock()
            .unwrap()
            .take(&scope_a)
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
fn invalid_descriptors_are_never_overwritten() {
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
