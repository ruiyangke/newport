use super::*;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

/// The command links and startup files Needle's shell setup writes.
const SHELL_SETUP: &str =
    "ls -l \"$HOME/.local/bin\"; cat \"$HOME/.bashrc\" \"$HOME/.profile\" 2>/dev/null";

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
    ssh.install(false).await.unwrap();
    finish(&state, Arc::new(Desktop), server.clone(), true, true)
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
    let shell_setup = ssh.execute(SHELL_SETUP, None).await.unwrap();
    let gate = owner.lifecycle.operation.lock().await;
    let task = tokio::spawn({
        let state = state.clone();
        let server = server.clone();
        async move { finish(&state, Arc::new(Desktop), server, true, true).await }
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
    // Setup wrote the shell setup; toggles never touch it.
    assert!(shell_setup.contains(">>> Newport >>>"), "{shell_setup}");
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
    assert_eq!(ssh.execute(SHELL_SETUP, None).await.unwrap(), shell_setup);
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
    let notice = finish(&state, Arc::new(Desktop), server.clone(), true, true)
        .await
        .unwrap()
        .unwrap();
    assert!(notice.contains(".bashrc"), "{notice}");
    assert!(notice.contains("symlink"));
    assert!(state.lock().await.server(server.id).unwrap().setup_complete);
    crate::monitor::snapshot(&owner).await.unwrap();
    let shell_files = "readlink \"$HOME/.bashrc\"; cat \"$HOME/managed-rc\"";
    let before = ssh.execute(shell_files, None).await.unwrap();
    crate::commands::reinstall_agent(&state, Arc::new(Desktop), server.id)
        .await
        .unwrap();
    let after = ssh.execute(shell_files, None).await.unwrap();
    assert_eq!(before, after);
    crate::monitor::snapshot(&owner).await.unwrap();
    // Finishing again reports the same notice.
    let repeated = finish(&state, Arc::new(Desktop), server, true, true)
        .await
        .unwrap();
    assert_eq!(repeated.as_deref(), Some(notice.as_str()));
}

#[tokio::test]
#[ignore = "Requires disposable Linux OpenSSH fixture"]
async fn uninstalling_leaves_nothing_of_needle_and_installing_again_works() {
    let directory = tempfile::tempdir().unwrap();
    let server: Server = serde_json::from_value(json!({
        "id":Uuid::new_v4(), "name":"Uninstall fixture", "sshHost":"127.0.0.1", "sshUser":"fixture",
        "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
        "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(), "agentInstallApproved":true
    }))
    .unwrap();
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
    // The disposable fixture's HOME: someone's own startup line, an xclip of
    // their own, and journals an older Needle left behind.
    ssh.execute(
        "rm -rf \"$HOME/.local/share/newport\" \"$HOME/.local/bin\"; mkdir -p \"$HOME/.local/bin\" \"$HOME/.local/share/newport/needle\"; chmod 700 \"$HOME/.local/share/newport\" \"$HOME/.local/share/newport/needle\"; printf 'export FIXTURE=1\\n' > \"$HOME/.bashrc\"; chmod 640 \"$HOME/.bashrc\"; printf 'their xclip' > \"$HOME/.local/bin/xclip\"; printf '[]' > \"$HOME/.local/share/newport/needle/removals.json\"",
        None,
    )
    .await
    .unwrap();
    install(&state, server.clone()).await.unwrap();
    finish(&state, Arc::new(Desktop), server.clone(), true, true)
        .await
        .unwrap();
    let installed = ssh
        .execute(
            "cat \"$HOME/.bashrc\"; ls \"$HOME/.local/bin\"; ls \"$HOME/.local/share/newport/needle\"",
            None,
        )
        .await
        .unwrap();
    assert!(installed.contains(">>> Newport >>>"), "{installed}");
    assert!(installed.contains("newport-browser"), "{installed}");
    assert!(
        !installed.contains("removals.json"),
        "installing deletes old journals: {installed}"
    );
    // A busy server, and something that keeps starting Needle through its
    // entry point while it is uninstalled: Newport's monitoring, or another
    // desktop connected to this server.
    ssh.execute(
        "for i in $(seq 800); do sleep 120 </dev/null >/dev/null 2>&1 & done; nohup sh -c 'while [ -x \"$HOME/.local/bin/newport-agent\" ]; do sleep 120 | \"$HOME/.local/bin/newport-agent\" runtime --stdio >/dev/null 2>&1 & sleep 0.1; done' </dev/null >/dev/null 2>&1 &",
        None,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_ne!(
        ssh.execute("pgrep -c -f 'newport-agent runtime' || true", None)
            .await
            .unwrap()
            .trim(),
        "0",
        "Needle runs while it is uninstalled"
    );
    let uninstalled = crate::commands::uninstall_agent(&state, server.clone())
        .await
        .unwrap();
    assert!(uninstalled.contains("uninstalled"), "{uninstalled}");
    let left = ssh
        .execute(
            "cat \"$HOME/.bashrc\"; stat -c %a \"$HOME/.bashrc\"; ls -A \"$HOME/.local/bin\"; for path in .local/share/newport .cache/newport .local/state/newport .config/fish/conf.d/newport.fish; do [ ! -e \"$HOME/$path\" ] || echo \"$path\"; done; [ ! -e \"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/newport-ipc\" ] || echo newport-ipc; for p in /proc/[0-9]*; do readlink \"$p/exe\" 2>/dev/null; done | grep -c /newport/ || true",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        left.lines().collect::<Vec<_>>(),
        ["export FIXTURE=1", "640", "xclip", "0"],
        "only what was there before stays"
    );
    assert_eq!(owner.lifecycle.snapshot().installed, Some(false));
    // Installing again starts from nothing.
    install(&state, server.clone()).await.unwrap();
    assert_eq!(owner.lifecycle.snapshot().installed, Some(true));
    crate::commands::uninstall_agent(&state, server)
        .await
        .unwrap();
    crate::manager::shutdown(&state).await;
}

#[tokio::test]
#[ignore = "Requires disposable Linux OpenSSH fixture"]
async fn uninstalling_also_removes_a_needle_installed_as_a_plain_file() {
    let directory = tempfile::tempdir().unwrap();
    let server: Server = serde_json::from_value(json!({
        "id":Uuid::new_v4(), "name":"Plain Needle fixture", "sshHost":"127.0.0.1", "sshUser":"fixture",
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
    install(&state, server.clone()).await.unwrap();
    // As installed by hand: the executable itself at the entry point, running.
    ssh.execute(
        "set -e; b=\"$HOME/.local/bin/newport-agent\"; cp \"$(readlink -f \"$b\")\" \"$b.plain\"; rm -f \"$b\"; mv \"$b.plain\" \"$b\"; rm -rf \"$HOME/.local/share/newport\"; nohup sh -c 'sleep 120 | \"$HOME/.local/bin/newport-agent\" runtime --stdio' </dev/null >/dev/null 2>&1 &",
        None,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let running = "for p in /proc/[0-9]*; do readlink \"$p/exe\" 2>/dev/null; done | grep -c '/.local/bin/newport-agent' || true";
    assert_ne!(ssh.execute(running, None).await.unwrap().trim(), "0");
    crate::commands::uninstall_agent(&state, server.clone())
        .await
        .unwrap();
    assert_eq!(ssh.execute(running, None).await.unwrap().trim(), "0");
    assert_eq!(
        ssh.execute(
            "[ ! -e \"$HOME/.local/bin/newport-agent\" ] && echo gone",
            None
        )
        .await
        .unwrap()
        .trim(),
        "gone"
    );
    install(&state, server.clone()).await.unwrap();
    assert_eq!(owner.lifecycle.snapshot().installed, Some(true));
    crate::commands::uninstall_agent(&state, server)
        .await
        .unwrap();
    crate::manager::shutdown(&state).await;
}
