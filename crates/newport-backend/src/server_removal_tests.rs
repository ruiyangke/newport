use super::*;
use crate::lifecycle_receipts::RemovalPhase;
use std::time::Duration;

struct Desktop;
impl crate::desktop_integration::DesktopIntegration for Desktop {
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        action();
        Ok(())
    }
    fn clipboard_revision(&self) -> Option<isize> {
        None
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        None
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn open_url(&self, _: &str) -> Result<(), String> {
        panic!("No browser requests expected")
    }
}

async fn connected(owner: &ServerSession) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if owner
                .integration
                .snapshot()
                .is_some_and(|s| s.connection.status == crate::model::Status::Connected)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("integration should resume inside the lifecycle owner");
}

#[tokio::test]
#[ignore = "Requires disposable Linux OpenSSH fixture"]
async fn owned_removal_restores_services_and_cancels_uncertain_dispatch() {
    let server: Server = serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4(), "name":"Removal owner fixture", "sshUser":"fixture",
        "sshHost":"127.0.0.1", "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),
        "agentInstallApproved":true, "browserEnabled":true, "clipboardEnabled":true
    })).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::new(directory.path());
    let owner = registry.session(&server, 0);
    let ssh = owner.acquire(&server).await.unwrap();
    // The fixture has a preseeded legacy binary. Start with this test's own
    // installation, avoiding compatibility evidence from another test release.
    ssh.execute("rm -rf \"$HOME/.local/share/newport/needle\"; rm -f \"$HOME/.local/bin/newport-agent\"; printf '# fixture personal settings\\n' > \"$HOME/.bashrc\"", None).await.unwrap();
    ssh.install_operation(true, false, Uuid::new_v4())
        .await
        .unwrap();
    let binary_build = ssh
        .check_installation()
        .await
        .unwrap()
        .installed_hash
        .unwrap();
    let binary_generation = owner.generation.load(Ordering::Acquire);
    crate::monitor::snapshot(&owner).await.unwrap();
    let binary_removal = Uuid::new_v4();
    ssh.remove_agent(binary_removal, &binary_build)
        .await
        .unwrap();
    assert_eq!(
        ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
        "# fixture personal settings\n"
    );
    assert_eq!(
        ssh.check_installation().await.unwrap().installed,
        Some(false)
    );
    assert_eq!(owner.generation.load(Ordering::Acquire), binary_generation);
    ssh.restore_removed_agent(binary_removal).await.unwrap();
    crate::monitor::snapshot(&owner).await.unwrap();
    assert_eq!(owner.generation.load(Ordering::Acquire), binary_generation);
    assert_eq!(
        ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
        "# fixture personal settings\n"
    );
    // Helper setup after a binary-only restore records ownership normally.
    ssh.install_operation(true, true, Uuid::new_v4())
        .await
        .unwrap();
    let build = ssh
        .check_installation()
        .await
        .unwrap()
        .installed_hash
        .unwrap();
    let shell = ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap();
    owner
        .integration
        .start(
            owner.clone(),
            server.clone(),
            Uuid::new_v4(),
            Arc::new(Desktop),
        )
        .await
        .unwrap();
    connected(&owner).await;
    crate::monitor::snapshot(&owner).await.unwrap();
    let generation = owner.generation.load(Ordering::Acquire);
    let id = Uuid::new_v4();
    let gate = owner.lifecycle.operation.lock().await;
    let waiter = tokio::spawn({
        let lease = owner.acquire(&server).await.unwrap();
        let build = build.clone();
        async move { lease.remove_agent(id, &build).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.lifecycle.tasks.active().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    drop(gate);
    tokio::time::timeout(Duration::from_secs(40), async {
        while owner.lifecycle.tasks.active().is_some() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let record = owner
        .lifecycle
        .receipts
        .removal(&server, id)
        .unwrap()
        .unwrap();
    assert_eq!(record.phase, RemovalPhase::Removed, "{:?}", record.error);
    assert!(
        crate::monitor::snapshot(&owner)
            .await
            .unwrap_err()
            .contains("removal")
    );
    assert_eq!(
        ssh.execute(
            "printf ssh-alive; test ! -e \"$HOME/.local/bin/newport-agent\"",
            None
        )
        .await
        .unwrap(),
        "ssh-alive"
    );
    assert_eq!(owner.generation.load(Ordering::Acquire), generation);
    assert!(owner.permissions.snapshot().desired.browser);
    assert_eq!(
        ssh.check_installation().await.unwrap().installed,
        Some(false)
    );
    assert_eq!(
        ssh.remove_agent(id, &build).await.unwrap(),
        "This Needle removal already completed."
    );
    ssh.restore_removed_agent(id).await.unwrap();
    connected(&owner).await;
    // A retry after interruption between durable restoration and local service
    // restart resumes saved demand without replaying the remote restoration.
    owner.permissions.disconnected(None);
    owner.integration.pause().await;
    ssh.restore_removed_agent(id).await.unwrap();
    connected(&owner).await;
    crate::monitor::snapshot(&owner).await.unwrap();
    assert_eq!(
        ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
        shell
    );
    assert_eq!(
        owner
            .lifecycle
            .receipts
            .removal(&server, id)
            .unwrap()
            .unwrap()
            .phase,
        RemovalPhase::Restored
    );

    let uncertain = Uuid::new_v4();
    owner
        .lifecycle
        .receipts
        .begin_removal(&server, uncertain, &build)
        .await
        .unwrap();
    owner
        .lifecycle
        .receipts
        .update_removal(
            &server,
            uncertain,
            RemovalPhase::Preparing,
            RemovalPhase::Removing,
            Some("reply lost before remote intent".into()),
        )
        .await
        .unwrap();
    ssh.restore_removed_agent(uncertain).await.unwrap();
    connected(&owner).await;
    // A delayed original command observes cancellation instead of deleting files.
    let output = ssh.execute(&format!("\"$HOME/.local/bin/newport-agent\" deployment remove {build} --operation {uncertain}"), None).await.unwrap();
    let receipt: crate::lifecycle_receipts::protocol::RemovalReceipt =
        serde_json::from_str(&output).unwrap();
    assert_eq!(
        receipt.state,
        crate::lifecycle_receipts::protocol::RemovalState::Restored
    );
    assert_eq!(
        ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
        shell
    );
    assert_eq!(owner.generation.load(Ordering::Acquire), generation);
    // Review removal-restoration conflicts through the same owned session API
    // used by GPUI; no saved profile or production installation participates.
    let conflicted = Uuid::new_v4();
    ssh.remove_agent(conflicted, &build).await.unwrap();
    assert!(ssh.review_helpers(conflicted).await.is_err());
    ssh.execute(
        "printf '# changed after removal\\n' > \"$HOME/.bashrc\"",
        None,
    )
    .await
    .unwrap();
    assert!(ssh.restore_removed_agent(conflicted).await.is_err());
    let report = ssh.review_helpers(conflicted).await.unwrap();
    let file = report
        .files
        .iter()
        .find(|f| f.path.ends_with("/.bashrc"))
        .unwrap();
    assert_eq!(file.status, "conflict");
    let index = file.index;
    let old = file.fingerprint.clone().unwrap();
    ssh.execute("printf '# newer user edit\\n' > \"$HOME/.bashrc\"", None)
        .await
        .unwrap();
    assert!(
        ssh.keep_helper(conflicted, index, old)
            .await
            .unwrap_err()
            .contains("changed since review")
    );
    let mut other_server = server.clone();
    other_server.id = Uuid::new_v4();
    let other_owner = registry.session(&other_server, 0);
    let other = other_owner.acquire(&other_server).await.unwrap();
    assert!(
        other.review_helpers(conflicted).await.is_err(),
        "another profile cannot reuse the reviewed operation"
    );
    let report = ssh.review_helpers(conflicted).await.unwrap();
    let fingerprint = report
        .files
        .iter()
        .find(|f| f.index == index)
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    ssh.keep_helper(conflicted, index, fingerprint.clone())
        .await
        .unwrap();
    ssh.keep_helper(conflicted, index, fingerprint)
        .await
        .unwrap();
    ssh.restore_removed_agent(conflicted).await.unwrap();
    assert!(
        ssh.review_helpers(conflicted).await.is_err(),
        "completed restore is no longer actionable"
    );
    connected(&owner).await;
    crate::monitor::snapshot(&owner).await.unwrap();
    assert_eq!(
        ssh.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
        "# newer user edit\n"
    );
    assert_eq!(owner.generation.load(Ordering::Acquire), generation);
    registry.shutdown().await;
    let reopened = Registry::new(directory.path());
    assert_eq!(
        reopened
            .receipts
            .removal(&server, uncertain)
            .unwrap()
            .unwrap()
            .phase,
        RemovalPhase::Restored
    );
}
