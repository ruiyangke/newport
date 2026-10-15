use super::*;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

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

#[tokio::test]
#[ignore = "Requires disposable Linux OpenSSH fixture"]
async fn setup_completion_cannot_overwrite_revocation_and_toggles_preserve_services() {
    let directory = tempfile::tempdir().unwrap();
    let server: Server = serde_json::from_value(json!({
        "id":Uuid::new_v4(), "name":"Setup permission fixture", "sshHost":"127.0.0.1", "sshUser":"fixture",
        "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved":true
    })).unwrap();
    let mut manager =
        crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
    manager
        .save(crate::model::Config {
            servers: vec![server.clone()],
            tunnels: vec![],
        })
        .await
        .unwrap();
    let owner = manager
        .sessions
        .session(&server, manager.connection_revision(server.id));
    let state = crate::manager::share(manager);
    let ssh = owner.acquire(&server).await.unwrap();
    ssh.execute(
        "rm -rf \"$HOME/.local/share/newport/needle\"; rm -f \"$HOME/.local/bin/newport-agent\"",
        None,
    )
    .await
    .unwrap();
    ssh.install_operation(true, false, Uuid::new_v4())
        .await
        .unwrap();
    finish(
        &state,
        Arc::new(Desktop),
        server.clone(),
        true,
        true,
        Some(Uuid::new_v4()),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !owner.permissions.browser_enabled() || !owner.permissions.clipboard_enabled() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    crate::monitor::snapshot(&owner).await.unwrap();
    let generation = owner.transport_snapshot().generation;
    let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
    let epoch = owner.permissions.snapshot().session_epoch;
    let helper_history = ssh
        .execute(
            "cat \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
            None,
        )
        .await
        .unwrap();
    let gate = owner.lifecycle.operation.lock().await;
    let task = tokio::spawn({
        let state = state.clone();
        let server = server.clone();
        async move {
            finish(
                &state,
                Arc::new(Desktop),
                server,
                true,
                true,
                Some(Uuid::new_v4()),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.lifecycle.tasks.active().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let access = state.lock().await.integration_access();
    let expected_connection = state.lock().await.connection_scope(server.id).unwrap();
    crate::commands::set_integration_enabled(
        &state,
        Arc::new(Desktop),
        server.id,
        crate::commands::IntegrationFeature::Clipboard,
        false,
        expected_connection.clone(),
        &access,
    )
    .await
    .unwrap();
    assert!(!owner.permissions.clipboard_enabled());
    assert!(owner.permissions.browser_enabled());
    drop(gate);
    assert!(
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .contains("preferences changed")
    );
    assert!(
        !state
            .lock()
            .await
            .server(server.id)
            .unwrap()
            .clipboard_enabled
    );
    assert!(!owner.permissions.snapshot().desired.clipboard);
    assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
    // The setup above may explicitly configure helpers; toggles must not.
    let before_toggles = ssh
        .execute(
            "cat \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
            None,
        )
        .await
        .unwrap();
    assert!(!helper_history.is_empty());
    crate::commands::set_integration_enabled(
        &state,
        Arc::new(Desktop),
        server.id,
        crate::commands::IntegrationFeature::Browser,
        false,
        expected_connection.clone(),
        &access,
    )
    .await
    .unwrap();
    assert!(!owner.permissions.browser_enabled());
    crate::monitor::snapshot(&owner).await.unwrap();
    assert_eq!(
        owner.agent_runtime.snapshot().hello.unwrap().instance_id,
        runtime
    );
    assert_eq!(owner.transport_snapshot().generation, generation);
    assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
    assert_eq!(
        ssh.execute(
            "cat \"$HOME/.local/share/newport/needle/helpers/journal.json\"",
            None
        )
        .await
        .unwrap(),
        before_toggles
    );
    crate::commands::set_integration_enabled(
        &state,
        Arc::new(Desktop),
        server.id,
        crate::commands::IntegrationFeature::Browser,
        true,
        expected_connection.clone(),
        &access,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !owner.permissions.browser_enabled() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(!owner.permissions.clipboard_enabled());
    assert_eq!(owner.permissions.snapshot().session_epoch, epoch);
    assert_eq!(owner.transport_snapshot().generation, generation);
    crate::manager::shutdown(&state).await;
}

#[tokio::test]
#[ignore = "Requires disposable Linux OpenSSH fixture"]
async fn managed_shell_setup_finishes_and_reinstall_never_rewrites_shell_files() {
    let directory = tempfile::tempdir().unwrap();
    let server: Server = serde_json::from_value(json!({
        "id":Uuid::new_v4(), "name":"Managed shell fixture", "sshHost":"127.0.0.1", "sshUser":"fixture",
        "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved":true
    })).unwrap();
    let mut manager =
        crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
    manager
        .save(crate::model::Config {
            servers: vec![server.clone()],
            tunnels: vec![],
        })
        .await
        .unwrap();
    let owner = manager
        .sessions
        .session(&server, manager.connection_revision(server.id));
    let state = crate::manager::share(manager);
    let ssh = owner.acquire(&server).await.unwrap();
    // This is the disposable fixture's HOME, never a configured user server.
    ssh.execute("rm -rf \"$HOME/.local/share/newport/needle\"; rm -f \"$HOME/.local/bin/newport-agent\" \"$HOME/.local/bin/newport-browser\"; printf '# managed by fixture\\n' > \"$HOME/managed-rc\"; rm -f \"$HOME/.bashrc\"; ln -s \"$HOME/managed-rc\" \"$HOME/.bashrc\"", None).await.unwrap();
    install(&state, server.clone()).await.unwrap();
    let helper = Uuid::new_v4();
    let notice = finish(
        &state,
        Arc::new(Desktop),
        server.clone(),
        true,
        true,
        Some(helper),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(notice.contains(".bashrc"), "{notice}");
    assert!(notice.contains("symlink"));
    assert_eq!(
        owner.lifecycle.receipts.get(helper).unwrap().unwrap().phase,
        crate::lifecycle_receipts::Phase::Ready
    );
    assert!(state.lock().await.server(server.id).unwrap().setup_complete);
    crate::monitor::snapshot(&owner).await.unwrap();
    let before = ssh.execute("sha256sum \"$HOME/.local/share/newport/needle/helpers/journal.json\"; readlink \"$HOME/.bashrc\"; cat \"$HOME/managed-rc\"", None).await.unwrap();
    let install_id = Uuid::new_v4();
    crate::commands::reinstall_agent_with_id(
        &state,
        Arc::new(Desktop),
        server.id,
        Some(install_id),
    )
    .await
    .unwrap();
    let record = owner.lifecycle.receipts.get(install_id).unwrap().unwrap();
    assert!(!record.intent.integrations);
    assert_eq!(record.phase, crate::lifecycle_receipts::Phase::Ready);
    let after = ssh.execute("sha256sum \"$HOME/.local/share/newport/needle/helpers/journal.json\"; readlink \"$HOME/.bashrc\"; cat \"$HOME/managed-rc\"", None).await.unwrap();
    assert_eq!(before, after);
    crate::monitor::snapshot(&owner).await.unwrap();
    // The result survives a duplicate finish request without reapplying helpers.
    let repeated = finish(&state, Arc::new(Desktop), server, true, true, Some(helper))
        .await
        .unwrap();
    assert_eq!(repeated.as_deref(), Some(notice.as_str()));
}
