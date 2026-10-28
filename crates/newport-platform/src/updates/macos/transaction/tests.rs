use super::super::{IDENTIFIER, tests::write_bundle};
use super::*;

fn verify_fixture(path: &Path, expected: &semver::Version, _: &str) -> Result<(), String> {
    if &bundle_version(path)? != expected {
        return Err("Fixture version changed".into());
    }
    super::super::validate_executable(path)
}

fn fixture() -> (tempfile::TempDir, Transaction) {
    let parent = tempfile::tempdir().unwrap();
    let parent_path = parent.path().canonicalize().unwrap();
    let destination = write_bundle(&parent_path, IDENTIFIER, "1.0.0", "newport", 0x0100000c);
    let staging = tempfile::Builder::new()
        .prefix(".newport-update-")
        .tempdir_in(&parent_path)
        .unwrap();
    crate::filesystem::protect_directory(staging.path()).unwrap();
    write_bundle(staging.path(), IDENTIFIER, "2.0.0", "newport", 0x0100000c);
    let record = Record {
        schema: 1,
        destination: destination.clone(),
        previous: "1.0.0".into(),
        updated: "2.0.0".into(),
        team: "TEST".into(),
        intent: Intent::Install,
    };
    sync_tree(staging.path()).unwrap();
    write_record(staging.path(), &record).unwrap();
    let transaction = Transaction {
        root: staging.keep(),
        record,
        _lock: installation_lock(&destination).unwrap(),
    };
    (parent, transaction)
}

#[test]
fn startup_guard_holds_the_installation_lock_with_and_without_pending_recovery() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    drop(transaction);
    let guard = StartupGuard::acquire(&destination).unwrap();
    assert!(StartupGuard::acquire(&destination).is_err());
    let transaction = guard.into_transaction().unwrap();
    assert!(StartupGuard::acquire(&destination).is_err());
    drop(transaction);
    fs::remove_dir_all(root).unwrap();
    let guard = StartupGuard::acquire(&destination).unwrap();
    assert!(StartupGuard::acquire(&destination).is_err());
    assert!(guard.into_transaction().is_none());
    assert!(StartupGuard::acquire(&destination).is_ok());
}

#[test]
fn an_installation_this_user_cannot_update_still_starts_without_a_guard() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let parent_path = parent.path().canonicalize().unwrap();
    let destination = write_bundle(&parent_path, IDENTIFIER, "1.0.0", "newport", 0x0100000c);
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o555)).unwrap();
    let guard = StartupGuard::acquire(&destination);
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(guard.unwrap().into_transaction().is_none());
    assert!(!lock_path(&destination).exists());
}

#[test]
fn inspection_reports_intent_without_exchanging_and_allows_accepted_partial_cleanup() {
    let (_parent, mut transaction) = fixture();
    let status = transaction.inspect_with(verify_fixture).unwrap();
    assert_eq!(status.installed.to_string(), "1.0.0");
    assert_eq!(status.intended.to_string(), "2.0.0");
    assert!(!status.accepted);
    assert_eq!(
        bundle_version(transaction.destination())
            .unwrap()
            .to_string(),
        "1.0.0"
    );
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::Restore).unwrap();
    let status = transaction.inspect_with(verify_fixture).unwrap();
    assert_eq!(status.installed.to_string(), "2.0.0");
    assert_eq!(status.intended.to_string(), "1.0.0");
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::AcceptPrevious).unwrap();
    fs::remove_file(transaction.other().join("Contents/Info.plist")).unwrap();
    let status = transaction.inspect_with(verify_fixture).unwrap();
    assert!(status.accepted);
    assert_eq!(status.installed, status.intended);
    transaction
        .accept_with(&status.installed, verify_fixture)
        .unwrap();
}

#[test]
fn install_and_recovery_are_idempotent_before_and_after_the_exchange() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    drop(transaction);
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Updated
    );
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
    assert_eq!(
        bundle_version(&transaction.other()).unwrap().to_string(),
        "1.0.0"
    );
    drop(transaction);
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Updated
    );
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
}

#[test]
fn rollback_intent_survives_interruption_and_never_reinstalls_the_failed_version() {
    let (_parent, mut transaction) = fixture();
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::Restore).unwrap();
    let root = transaction.recovery_directory().to_owned();
    let destination = transaction.destination().to_owned();
    drop(transaction); // interrupted after recording intent but before exchange
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Previous
    );
    drop(transaction); // interrupted after exchange but before any acknowledgement
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Previous
    );
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
    assert_eq!(
        bundle_version(&transaction.other()).unwrap().to_string(),
        "2.0.0"
    );
}

#[test]
fn a_recorded_intent_wins_over_stale_memory_after_an_ambiguous_write() {
    let (_parent, mut transaction) = fixture();
    transaction.reconcile_with(verify_fixture).unwrap();
    let mut record: Record =
        serde_json::from_slice(&fs::read(transaction.root.join(JOURNAL)).unwrap()).unwrap();
    record.intent = Intent::Restore;
    write_record(&transaction.root, &record).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Previous
    );
}

#[test]
fn invalid_or_changed_bundles_are_never_exchanged() {
    let (_parent, mut transaction) = fixture();
    assert!(
        transaction.reconcile().is_err(),
        "Production verification must reject unsigned fixtures"
    );
    assert_eq!(
        bundle_version(transaction.destination())
            .unwrap()
            .to_string(),
        "1.0.0"
    );
    write_bundle(
        &transaction.root,
        IDENTIFIER,
        "3.0.0",
        "newport",
        0x0100000c,
    );
    assert!(transaction.reconcile_with(verify_fixture).is_err());
    assert_eq!(
        bundle_version(transaction.destination())
            .unwrap()
            .to_string(),
        "1.0.0"
    );
}

#[test]
fn installation_lock_excludes_other_transactions_and_releases_on_drop() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    assert!(installation_lock(&destination).is_err());
    assert!(Transaction::open(&destination, &root).is_err());
    drop(transaction);
    assert!(Transaction::open(&destination, &root).is_ok());
}

#[test]
fn accepted_updates_resume_partial_backup_cleanup_without_touching_the_live_app() {
    let (_parent, mut transaction) = fixture();
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::AcceptUpdated).unwrap();
    fs::remove_file(transaction.other().join("Contents/Info.plist")).unwrap();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    drop(transaction);
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Updated
    );
    transaction.cleanup().unwrap();
    assert!(!root.exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
}

#[test]
fn journals_cannot_redirect_recovery_to_another_installation() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    let mut record: Record =
        serde_json::from_slice(&fs::read(root.join(JOURNAL)).unwrap()).unwrap();
    record.destination = destination.with_file_name("Other.app");
    write_record(&root, &record).unwrap();
    drop(transaction);
    assert!(Transaction::open(&destination, &root).is_err());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
}

#[test]
fn acceptance_cannot_install_a_candidate_or_discard_backup_from_the_old_process() {
    let (_parent, transaction) = fixture();
    let root = transaction.root.clone();
    let destination = transaction.destination().to_owned();
    assert!(
        transaction
            .accept_with(&"2.0.0".parse().unwrap(), verify_fixture)
            .is_err()
    );
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
    let mut transaction = Transaction::open(&destination, &root).unwrap();
    transaction.reconcile_with(verify_fixture).unwrap();
    assert!(
        transaction
            .accept_with(&"1.0.0".parse().unwrap(), verify_fixture)
            .is_err()
    );
    assert!(root.join("Newport.app").exists());
    let transaction = Transaction::open(&destination, &root).unwrap();
    transaction
        .accept_with(&"2.0.0".parse().unwrap(), verify_fixture)
        .unwrap();
    assert!(!root.exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
}

#[test]
fn atomic_exchange_never_leaves_the_application_path_missing() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let reader = std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            assert!(fs::symlink_metadata(&destination).unwrap().is_dir());
        }
    });
    for _ in 0..100 {
        exchange(transaction.destination(), &transaction.other()).unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    reader.join().unwrap();
}

#[test]
fn discovery_finds_pending_install_and_restore_without_changing_either_bundle() {
    let (_parent, mut transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.root.clone();
    assert!(
        Transaction::discover(&destination).is_err(),
        "Live owner holds the lock"
    );
    drop(transaction);
    transaction = Transaction::discover(&destination).unwrap().unwrap();
    assert_eq!(transaction.root, root);
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::Restore).unwrap();
    drop(transaction);
    let mut transaction = Transaction::discover(&destination).unwrap().unwrap();
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
    assert_eq!(
        transaction.reconcile_with(verify_fixture).unwrap(),
        RecoveryState::Previous
    );
    transaction
        .accept_with(&"1.0.0".parse().unwrap(), verify_fixture)
        .unwrap();
    assert!(Transaction::discover(&destination).unwrap().is_none());
}

#[test]
fn a_new_install_cannot_bypass_an_unaccepted_or_interrupted_transaction() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let previous_root = transaction.root.clone();
    drop(transaction);
    let staging = tempfile::Builder::new()
        .prefix(".newport-update-")
        .tempdir_in(destination.parent().unwrap())
        .unwrap();
    crate::filesystem::protect_directory(staging.path()).unwrap();
    write_bundle(staging.path(), IDENTIFIER, "3.0.0", "newport", 0x0100000c);
    let staging_path = staging.path().to_owned();
    let prepared = PreparedBundle {
        staging,
        destination: destination.clone(),
        expected_version: "3.0.0".parse().unwrap(),
        installed_version: "1.0.0".parse().unwrap(),
        team: "TEST".into(),
    };
    let error = prepared
        .begin()
        .err()
        .expect("Must reject an existing recovery transaction");
    assert!(
        error.contains("earlier update still needs recovery"),
        "{error}"
    );
    assert!(
        !staging_path.exists(),
        "Only the new disposable preparation is removed"
    );
    assert!(previous_root.join(JOURNAL).exists());
    assert!(previous_root.join("Newport.app").exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
}

#[test]
fn discovery_ignores_unbegun_staging_and_other_installations() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.root.clone();
    let other = destination.with_file_name("Other.app");
    fs::create_dir(&other).unwrap();
    let staging = tempfile::Builder::new()
        .prefix(".newport-update-")
        .tempdir_in(destination.parent().unwrap())
        .unwrap();
    crate::filesystem::protect_directory(staging.path()).unwrap();
    drop(transaction);
    assert!(Transaction::discover(&other).unwrap().is_none());
    let transaction = Transaction::discover(&destination).unwrap().unwrap();
    assert_eq!(transaction.root, root);
    drop(transaction);
    assert!(staging.path().exists());
    assert!(root.join(JOURNAL).exists());
}

#[test]
fn discovery_refuses_ambiguous_or_corrupt_journals_and_preserves_backups() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let first = transaction.root.clone();
    let second = tempfile::Builder::new()
        .prefix(".newport-update-")
        .tempdir_in(destination.parent().unwrap())
        .unwrap();
    crate::filesystem::protect_directory(second.path()).unwrap();
    write_record(second.path(), &transaction.record).unwrap();
    drop(transaction);
    let error = Transaction::discover(&destination).err().unwrap();
    assert!(error.contains("Multiple update journals"), "{error}");
    fs::write(second.path().join(JOURNAL), b"{").unwrap();
    assert!(Transaction::discover(&destination).is_err());
    fs::write(second.path().join(JOURNAL), vec![b' '; 64 * 1024 + 1]).unwrap();
    assert!(
        Transaction::discover(&destination)
            .err()
            .unwrap()
            .contains("size limit")
    );
    assert!(first.join("Newport.app").exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
}

#[test]
fn recovery_rejects_symlinks_and_nonprivate_journals() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.root.clone();
    drop(transaction);
    let alias = destination.with_file_name("Alias.app");
    symlink(&destination, &alias).unwrap();
    assert!(Transaction::discover(&alias).is_err());
    assert!(Transaction::open(&alias, &root).is_err());
    let journal = root.join(JOURNAL);
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Transaction::discover(&destination).is_err());
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&journal, root.join("saved.json")).unwrap();
    symlink("saved.json", &journal).unwrap();
    assert!(Transaction::discover(&destination).is_err());
    assert!(root.join("Newport.app").exists());
}

#[test]
fn discovery_resumes_accepted_partial_cleanup_without_a_valid_backup() {
    let (_parent, mut transaction) = fixture();
    transaction.reconcile_with(verify_fixture).unwrap();
    transaction.set_intent(Intent::AcceptUpdated).unwrap();
    fs::remove_file(transaction.other().join("Contents/Info.plist")).unwrap();
    let destination = transaction.destination().to_owned();
    let root = transaction.root.clone();
    drop(transaction);
    Transaction::discover(&destination)
        .unwrap()
        .unwrap()
        .accept_with(&"2.0.0".parse().unwrap(), verify_fixture)
        .unwrap();
    assert!(!root.exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
}

#[test]
fn abandoning_a_begun_install_removes_only_the_staged_candidate() {
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    transaction.abandon().unwrap();
    assert!(!root.exists());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "1.0.0");
    let guard = StartupGuard::acquire(&destination).unwrap();
    assert!(guard.into_transaction().is_none());
}

#[test]
fn an_exchanged_install_cannot_be_abandoned() {
    let (_parent, mut transaction) = fixture();
    transaction.reconcile_with(verify_fixture).unwrap();
    let destination = transaction.destination().to_owned();
    let root = transaction.recovery_directory().to_owned();
    assert!(transaction.abandon().is_err());
    assert_eq!(bundle_version(&destination).unwrap().to_string(), "2.0.0");
    assert_eq!(
        bundle_version(&root.join("Newport.app"))
            .unwrap()
            .to_string(),
        "1.0.0"
    );
}

#[test]
fn startup_removes_only_old_private_staging_without_a_journal() {
    use std::os::unix::fs::PermissionsExt;
    let (_parent, transaction) = fixture();
    let destination = transaction.destination().to_owned();
    let journaled = transaction.recovery_directory().to_owned();
    drop(transaction);
    let parent = destination.parent().unwrap();
    let staging = |name: &str, mode: u32, age: u64| {
        let root = parent.join(name);
        write_bundle(&root, IDENTIFIER, "2.0.0", "newport", 0x0100000c);
        fs::set_permissions(&root, fs::Permissions::from_mode(mode)).unwrap();
        let modified = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        File::open(&root).unwrap().set_modified(modified).unwrap();
        root
    };
    let abandoned = staging(".newport-update-abandoned", 0o700, 3600);
    let preparing = staging(".newport-update-preparing", 0o700, 0);
    let shared = staging(".newport-update-shared", 0o755, 3600);
    let unrelated = staging(".other-abandoned", 0o700, 3600);
    // Journaled recovery is old too, but must survive for recovery.
    File::open(&journaled)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    discard_unstarted(&destination, std::time::Duration::from_secs(600));
    assert!(!abandoned.exists());
    assert!(preparing.exists());
    assert!(shared.exists());
    assert!(unrelated.exists());
    assert!(journaled.join(JOURNAL).exists());
    assert!(destination.exists());
}
