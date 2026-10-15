//! Manager-level profile invariants shared by both desktop hosts.
use super::snapshot::Snapshot as Stronghold;
use crate::model::Config;
use std::fs;
use zeroize::Zeroizing;
const CLIENT: &[u8] = b"newport-profiles-v1";
const RECORD: &[u8] = b"config";

fn config() -> Config {
    serde_json::from_value(serde_json::json!({"servers":[{"id":uuid::Uuid::new_v4(),"name":"Private production","sshHost":"private.internal.example","sshPort":22,"sshUser":"private-user","identityFile":"/private/key"}],"tunnels":[]})).unwrap()
}
#[tokio::test]
async fn passwords_are_atomic_private_and_removed_with_the_server() {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::profile::Store::for_test(directory.path().into());
    let path = directory.path().join("profiles.stronghold");
    let mut manager = crate::manager::Manager::new(store);
    let mut config = config();
    let id = config.servers[0].id;
    let secret = "vault-only-test-password-482";
    manager
        .save_with_password(config.clone(), Some((id, Zeroizing::new(secret.into()))))
        .await
        .unwrap();
    assert_eq!(manager.store.password(id).unwrap().as_str(), secret);
    let snapshot = serde_json::to_string(&manager.snapshot()).unwrap();
    assert!(!snapshot.contains(secret));
    assert!(!snapshot.contains("passwords"));
    let bytes = fs::read(&path).unwrap();
    assert!(
        !bytes
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
    );
    config.servers[0].name = "Updated name".into();
    manager.save(config.clone()).await.unwrap();
    let reopened = crate::profile::Store::for_test(directory.path().into());
    assert_eq!(reopened.password(id).unwrap().as_str(), secret);
    let original = fs::read(&path).unwrap();
    let mut invalid = config.clone();
    invalid.servers[0].ssh_port = 0;
    assert!(
        manager
            .save_with_password(invalid, Some((id, Zeroizing::new("replacement".into()))))
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(manager.store.password(id).unwrap().as_str(), secret);
    manager.save(Config::default()).await.unwrap();
    assert!(manager.store.password(id).is_err());
    manager.save(config).await.unwrap();
    assert!(manager.store.password(id).is_err());
}

#[tokio::test]
async fn future_snapshot_blocks_manager_writes_without_changing_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profiles.stronghold");
    let vault = Stronghold::new(&path, vec![7; 32]).unwrap();
    let client = vault.create_client(CLIENT).unwrap();
    client
        .store()
        .insert(
            RECORD.to_vec(),
            serde_json::to_vec(&serde_json::json!({"schemaVersion": 999, "config": config()}))
                .unwrap(),
            None,
        )
        .unwrap();
    vault.save().unwrap();
    let original = fs::read(&path).unwrap();
    let mut manager =
        crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
    assert!(
        manager
            .load_error
            .as_ref()
            .unwrap()
            .contains("Unsupported profile version")
    );
    assert!(manager.save(Config::default()).await.is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}
