use super::{
    tests::{record, server},
    *,
};
use std::time::Duration;

/// Pause the real publication path after admission, before its filesystem work.
/// Dropping the release sender also unblocks the worker on a test failure.
fn pause_save(
    store: &Store,
    server: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    *store.server(server).unwrap().before_save.lock().unwrap() = Some(Box::new(move || {
        let _ = entered.send(());
        let _ = wait.recv_timeout(Duration::from_secs(5));
    }));
    (ready, release)
}

#[tokio::test(flavor = "current_thread")]
async fn slow_removal_save_keeps_readers_and_executor_live_with_a_scoped_barrier() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let a = server();
    let b = server();
    let mut installed = record(&a);
    installed.phase = Phase::Ready;
    store.begin(installed.clone()).await.unwrap();
    let helper = store
        .begin_helper_recovery(&b, Uuid::new_v4(), &"a".repeat(64), &"b".repeat(64))
        .await
        .unwrap();
    let snapshot = store.read(a.id).unwrap();
    let (ready, release) = pause_save(&store, a.id);
    let id = Uuid::new_v4();
    let save = {
        let (store, a) = (store.clone(), a.clone());
        tokio::spawn(async move { store.begin_removal(&a, id, &"c".repeat(64)).await })
    };
    ready.await.unwrap();
    // The only async executor remains responsive while the writer is stalled.
    tokio::spawn(async { tokio::task::yield_now().await })
        .await
        .unwrap();
    assert!(!save.is_finished());
    assert!(store.require_agent_available(&a).is_err());
    assert!(store.require_agent_available(&b).is_ok());
    let mut changed = a.clone();
    changed.ssh_port += 1;
    assert!(store.require_agent_available(&changed).is_ok());
    assert_eq!(
        store.get(installed.intent.operation_id).unwrap(),
        Some(installed)
    );
    assert_eq!(store.for_server(&a).unwrap().len(), 1);
    assert!(store.removal(&a, id).unwrap().is_none());
    assert!(store.removals_for_server(&a).unwrap().is_empty());
    assert_eq!(
        store.helper_recovery(&b, helper.operation_id).unwrap(),
        Some(helper)
    );
    assert_eq!(
        store
            .operations_snapshot(&b)
            .unwrap()
            .helper_recoveries
            .len(),
        1
    );
    assert!(
        Arc::ptr_eq(&snapshot, &store.read(a.id).unwrap()),
        "readers retain committed data"
    );
    release.send(()).unwrap();
    save.await.unwrap().unwrap();
    assert_eq!(
        store.removal(&a, id).unwrap().unwrap().phase,
        RemovalPhase::Preparing
    );
    assert!(Store::new(path).require_agent_available(&a).is_err());
    assert!(store.require_agent_available(&b).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_waiter_finishes_publication_and_other_writes_preserve_its_intent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let a = server();
    let b = server();
    let id = Uuid::new_v4();
    let (ready, release) = pause_save(&store, a.id);
    let save = {
        let (store, a) = (store.clone(), a.clone());
        tokio::spawn(async move { store.begin_removal(&a, id, &"c".repeat(64)).await })
    };
    ready.await.unwrap();
    save.abort();
    assert!(save.await.unwrap_err().is_cancelled());
    assert!(store.require_agent_available(&a).is_err());
    assert!(store.require_agent_available(&b).is_ok());
    let installation = record(&b);
    let next = {
        let (store, record) = (store.clone(), installation.clone());
        tokio::spawn(async move { store.begin(record).await })
    };
    // Server B must finish its own durable write before A's save is released.
    tokio::time::timeout(Duration::from_secs(2), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(store.operations_snapshot(&b).unwrap().operations.len(), 1);
    assert!(store.removal(&a, id).unwrap().is_none());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.removal(&a, id).unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reopened = Store::new(path);
    assert_eq!(
        reopened.get(installation.intent.operation_id).unwrap(),
        Some(installation)
    );
    assert!(reopened.removal(&a, id).unwrap().is_some());
    assert!(reopened.require_agent_available(&a).is_err());
    assert!(reopened.require_agent_available(&b).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn restore_keeps_admission_closed_until_the_confirmed_state_is_durable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let a = server();
    let b = server();
    let id = Uuid::new_v4();
    store.begin_removal(&a, id, &"c".repeat(64)).await.unwrap();
    store
        .update_removal(
            &a,
            id,
            RemovalPhase::Preparing,
            RemovalPhase::Restoring,
            None,
        )
        .await
        .unwrap();
    let (ready, release) = pause_save(&store, a.id);
    let save = {
        let (store, a) = (store.clone(), a.clone());
        tokio::spawn(async move {
            store
                .update_removal(
                    &a,
                    id,
                    RemovalPhase::Restoring,
                    RemovalPhase::Restored,
                    None,
                )
                .await
        })
    };
    ready.await.unwrap();
    assert!(store.require_agent_available(&a).is_err());
    assert!(store.require_agent_available(&b).is_ok());
    assert_eq!(
        store.removal(&a, id).unwrap().unwrap().phase,
        RemovalPhase::Restoring
    );
    release.send(()).unwrap();
    save.await.unwrap().unwrap();
    assert!(store.require_agent_available(&a).is_ok());
    assert!(Store::new(path).require_agent_available(&a).is_ok());
}

#[tokio::test]
async fn failed_restore_publication_preserves_evidence_and_never_reopens_admission() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let backup = directory.path().join("previous.json");
    let store = Store::new(path.clone());
    let a = server();
    let id = Uuid::new_v4();
    store.begin_removal(&a, id, &"c".repeat(64)).await.unwrap();
    store
        .update_removal(
            &a,
            id,
            RemovalPhase::Preparing,
            RemovalPhase::Restoring,
            None,
        )
        .await
        .unwrap();
    let shard = store.journal_path(a.id).unwrap();
    let original = fs::read(&shard).unwrap();
    let (ready, release) = pause_save(&store, a.id);
    let save = {
        let (store, a) = (store.clone(), a.clone());
        tokio::spawn(async move {
            store
                .update_removal(
                    &a,
                    id,
                    RemovalPhase::Restoring,
                    RemovalPhase::Restored,
                    None,
                )
                .await
        })
    };
    ready.await.unwrap();
    fs::rename(&shard, &backup).unwrap();
    fs::create_dir(&shard).unwrap();
    release.send(()).unwrap();
    assert!(save.await.unwrap().is_err());
    assert_eq!(fs::read(&backup).unwrap(), original);
    assert!(
        store.read(a.id).is_err(),
        "uncertain publication cannot claim success"
    );
    assert!(store.require_agent_available(&a).is_err());
    assert!(
        store
            .update_removal(
                &a,
                id,
                RemovalPhase::Restoring,
                RemovalPhase::Restored,
                None
            )
            .await
            .is_err()
    );
    // This failed replacement must not quarantine another server or its writer,
    // even after restarting while A's journal is still unreadable.
    let b = server();
    let other = record(&b);
    tokio::time::timeout(Duration::from_secs(2), store.begin(other.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(store.require_agent_available(&b).is_ok());
    let broken = Store::new(path.clone());
    assert!(broken.require_agent_available(&a).is_err());
    assert!(broken.require_agent_available(&b).is_ok());
    broken
        .update(other.intent.operation_id, Some(Phase::Ready), None)
        .await
        .unwrap();
    assert_eq!(
        broken
            .get(other.intent.operation_id)
            .unwrap()
            .unwrap()
            .phase,
        Phase::Ready
    );
    fs::remove_dir(&shard).unwrap();
    fs::rename(&backup, &shard).unwrap();
    let reopened = Store::new(path);
    assert_eq!(
        reopened.removal(&a, id).unwrap().unwrap().phase,
        RemovalPhase::Restoring
    );
    assert!(reopened.require_agent_available(&a).is_err());
}

#[tokio::test]
async fn duplicate_and_invalid_requests_leave_the_published_snapshot_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let a = server();
    let mut installed = record(&a);
    installed.phase = Phase::Ready;
    store.begin(installed.clone()).await.unwrap();
    let previous = store.read(a.id).unwrap();
    let original = fs::read(store.journal_path(a.id).unwrap()).unwrap();
    *store.server(a.id).unwrap().before_save.lock().unwrap() =
        Some(Box::new(|| panic!("a no-op must not write")));
    assert_eq!(store.begin(installed.clone()).await.unwrap(), installed);
    store
        .update(
            installed.intent.operation_id,
            Some(Phase::NeedsResolution),
            Some("late".into()),
        )
        .await
        .unwrap();
    assert!(
        store
            .begin_removal(&a, Uuid::nil(), &"a".repeat(64))
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(&previous, &store.read(a.id).unwrap()));
    assert_eq!(
        fs::read(store.journal_path(a.id).unwrap()).unwrap(),
        original
    );
    assert!(store.require_agent_available(&a).is_ok());
}

#[tokio::test]
async fn concurrent_server_updates_preserve_every_record_and_reject_shared_operation_ids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let records: Vec<_> = (0..12).map(|_| record(&server())).collect();
    let mut tasks = Vec::new();
    for record in records.clone() {
        let store = store.clone();
        tasks.push(tokio::spawn(
            async move { store.begin(record).await.unwrap() },
        ));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let reopened = Store::new(path);
    for record in &records {
        assert_eq!(
            reopened.get(record.intent.operation_id).unwrap().as_ref(),
            Some(record)
        );
    }
    let mut duplicate = record(&server());
    duplicate.intent.operation_id = records[0].intent.operation_id;
    assert!(store.begin(duplicate).await.is_err());
    for record in records {
        assert_eq!(store.read(record.server_id).unwrap().operations.len(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn operation_id_reservations_do_not_hold_the_registry_during_a_save() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path);
    let a = server();
    let b = server();
    let original = record(&a);
    let id = original.intent.operation_id;
    let (ready, release) = pause_save(&store, a.id);
    let save = {
        let store = store.clone();
        tokio::spawn(async move { store.begin(original).await })
    };
    ready.await.unwrap();
    let mut conflict = record(&b);
    conflict.intent.operation_id = id;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), store.begin(conflict))
            .await
            .unwrap()
            .is_err()
    );
    let valid = record(&b);
    tokio::time::timeout(Duration::from_secs(2), store.begin(valid.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        store.get(id).unwrap().is_none(),
        "reserved is not yet committed"
    );
    assert_eq!(store.get(valid.intent.operation_id).unwrap(), Some(valid));
    release.send(()).unwrap();
    save.await.unwrap().unwrap();
    assert_eq!(store.get(id).unwrap().unwrap().server_id, a.id);
}

#[tokio::test]
async fn versioned_migration_preserves_every_server_and_the_original_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let a = server();
    let b = server();
    let mut installed = record(&a);
    installed.phase = Phase::Ready;
    let removal = Removal {
        server_id: a.id,
        connection: scope(&a),
        operation_id: Uuid::new_v4(),
        build: "a".repeat(64),
        phase: RemovalPhase::Removing,
        error: Some("lost reply".into()),
    };
    let helpers = HelperRecovery {
        server_id: b.id,
        connection: scope(&b),
        operation_id: Uuid::new_v4(),
        build: "b".repeat(64),
        fingerprint: "c".repeat(64),
        phase: HelperRecoveryPhase::Restoring,
        error: Some("edited file".into()),
    };
    let legacy = Journal {
        operations: vec![installed.clone()],
        removals: vec![removal.clone()],
        helper_recoveries: vec![helpers.clone()],
        ..Default::default()
    };
    let source = serde_json::to_vec_pretty(&legacy).unwrap();
    fs::write(&path, &source).unwrap();
    let store = Store::new(path.clone());
    assert!(store.require_agent_available(&a).is_err());
    assert_eq!(
        store.helper_recovery(&b, helpers.operation_id).unwrap(),
        Some(helpers.clone())
    );
    assert_eq!(fs::read(&path).unwrap(), source);
    assert_eq!(
        fs::read_dir(directory.path()).unwrap().count(),
        1,
        "opening is read-only"
    );
    let c = server();
    store.begin(record(&c)).await.unwrap();
    let shard = store.journal_path(a.id).unwrap();
    let root = shard.parent().unwrap().parent().unwrap();
    assert_eq!(fs::read(root.join("legacy.json")).unwrap(), source);
    let descriptor: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        descriptor,
        serde_json::json!({"version":2,"storage":"server-journals"})
    );
    let reopened = Store::new(path);
    assert_eq!(
        reopened.get(installed.intent.operation_id).unwrap(),
        Some(installed)
    );
    assert_eq!(
        reopened.removal(&a, removal.operation_id).unwrap(),
        Some(removal)
    );
    assert_eq!(
        reopened.helper_recovery(&b, helpers.operation_id).unwrap(),
        Some(helpers)
    );
    assert!(reopened.require_agent_available(&a).is_err());
    assert!(reopened.require_agent_available(&b).is_ok());
    assert_eq!(reopened.for_server(&c).unwrap().len(), 1);
}

#[tokio::test]
async fn interrupted_migration_reuses_only_the_exact_staged_source() {
    for matching in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let a = server();
        let b = server();
        let first = record(&a);
        let legacy = Journal {
            operations: vec![first.clone()],
            ..Default::default()
        };
        let source = serde_json::to_vec(&legacy).unwrap();
        fs::write(&path, &source).unwrap();
        let store = Store::new(path.clone());
        let shard = store.journal_path(a.id).unwrap();
        let root = shard.parent().unwrap().parent().unwrap();
        fs::create_dir_all(shard.parent().unwrap()).unwrap();
        fs::write(&shard, &source).unwrap();
        let backup = if matching {
            source.as_slice()
        } else {
            b"preserve foreign evidence"
        };
        fs::write(root.join("legacy.json"), backup).unwrap();
        let result = store.begin(record(&b)).await;
        assert_eq!(result.is_ok(), matching);
        assert_eq!(fs::read(root.join("legacy.json")).unwrap(), backup);
        assert_eq!(fs::read(&shard).unwrap(), source);
        assert_eq!(store.get(first.intent.operation_id).unwrap(), Some(first));
        if !matching {
            assert_eq!(fs::read(&path).unwrap(), source);
        }
        let reopened = Store::new(path);
        if matching {
            assert_eq!(reopened.for_server(&a).unwrap().len(), 1);
            assert_eq!(reopened.for_server(&b).unwrap().len(), 1);
        } else {
            assert!(reopened.require_agent_available(&a).is_err());
        }
    }
}

#[tokio::test]
async fn unreadable_missing_and_wrong_owner_shards_are_quarantined_without_blocking_others() {
    for kind in ["malformed", "missing", "wrong owner"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let store = Store::new(path.clone());
        let a = server();
        let b = server();
        let first = record(&a);
        let second = record(&b);
        store.begin(first.clone()).await.unwrap();
        store.begin(second.clone()).await.unwrap();
        let shard = store.journal_path(a.id).unwrap();
        match kind {
            "malformed" => fs::write(&shard, b"broken").unwrap(),
            "missing" => fs::remove_file(&shard).unwrap(),
            _ => fs::write(
                &shard,
                serde_json::to_vec(&Journal {
                    operations: vec![record(&b)],
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap(),
        }
        let bad_bytes = fs::read(&shard).ok();
        let reopened = Store::new(path);
        assert!(reopened.require_agent_available(&a).is_err(), "{kind}");
        assert!(reopened.begin(record(&a)).await.is_err(), "{kind}");
        assert!(reopened.require_agent_available(&b).is_ok(), "{kind}");
        reopened
            .update(second.intent.operation_id, Some(Phase::Ready), None)
            .await
            .unwrap();
        assert_eq!(
            reopened
                .get(second.intent.operation_id)
                .unwrap()
                .unwrap()
                .phase,
            Phase::Ready
        );
        assert_eq!(fs::read(&shard).ok(), bad_bytes, "must preserve {kind}");
    }
}

#[tokio::test]
async fn cross_server_duplicate_ids_quarantine_only_the_conflicting_owners() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let store = Store::new(path.clone());
    let a = server();
    let b = server();
    let c = server();
    let first = record(&a);
    let second = record(&b);
    let third = record(&c);
    for row in [first.clone(), second.clone(), third.clone()] {
        store.begin(row).await.unwrap();
    }
    let mut conflicting = second;
    conflicting.intent.operation_id = first.intent.operation_id;
    fs::write(
        store.journal_path(b.id).unwrap(),
        serde_json::to_vec(&Journal {
            operations: vec![conflicting],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let reopened = Store::new(path);
    assert!(reopened.get(first.intent.operation_id).is_err());
    assert!(reopened.require_agent_available(&a).is_err());
    assert!(reopened.require_agent_available(&b).is_err());
    assert!(reopened.require_agent_available(&c).is_ok());
    reopened
        .update(third.intent.operation_id, Some(Phase::Ready), None)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get(third.intent.operation_id)
            .unwrap()
            .unwrap()
            .phase,
        Phase::Ready
    );
}

#[tokio::test]
async fn missing_or_rolled_back_descriptors_cannot_reopen_removed_server_admission() {
    for missing in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let a = server();
        let mut installed = record(&a);
        installed.phase = Phase::Ready;
        let old = serde_json::to_vec(&Journal {
            operations: vec![installed],
            ..Default::default()
        })
        .unwrap();
        fs::write(&path, &old).unwrap();
        let store = Store::new(path.clone());
        store
            .begin_removal(&a, Uuid::new_v4(), &"a".repeat(64))
            .await
            .unwrap();
        let evidence = fs::read(store.journal_path(a.id).unwrap()).unwrap();
        if missing {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, &old).unwrap();
        }
        let reopened = Store::new(path);
        assert!(reopened.require_agent_available(&a).is_err());
        assert!(reopened.begin(record(&server())).await.is_err());
        assert_eq!(
            fs::read(store.journal_path(a.id).unwrap()).unwrap(),
            evidence
        );
    }
}
