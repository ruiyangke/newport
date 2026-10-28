//! Read-only inspection and explicit installation consent for native onboarding.
use crate::{agent_payload::TransferEncoding, manager::Shared, model::Server, server_session};
use serde_json::{Value, json};

fn current_owner(
    state: &Shared,
    expected: &Server,
) -> Result<(Server, std::sync::Arc<server_session::ServerSession>), String> {
    let (server, owner) = state.current(expected.id)?;
    if !server.same_connection(expected) {
        return Err("Server connection changed. Restart setup.".into());
    }
    Ok((server, owner))
}

fn current(manager: &crate::manager::Manager, expected: &Server) -> Result<Server, String> {
    let server = manager.server(expected.id)?;
    if !server.same_connection(expected) {
        return Err("Server connection changed. Restart setup.".into());
    }
    Ok(server)
}
const INSPECT: &str = r#"printf 'kernel='; uname -s
printf 'arch='; uname -m
printf 'os=%s\n' "$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release 2>/dev/null | tr -d '\"')"
printf 'cores='; getconf _NPROCESSORS_ONLN 2>/dev/null || true
awk '/^MemTotal:/ {printf "memory=%s\n", $2}' /proc/meminfo 2>/dev/null
printf 'free='; df -Pk "$HOME" | awk 'END {print $4}'
if test -x "$HOME/.local/bin/newport-agent"; then echo 'installed=yes'; else echo 'installed=no'; fi
"#;
fn parse(output: &str) -> Result<Value, String> {
    let fields: std::collections::HashMap<_, _> =
        output.lines().filter_map(|l| l.split_once('=')).collect();
    let get = |key| fields.get(key).copied().unwrap_or("");
    if get("kernel") != "Linux" {
        return Err("Needle requires a Linux server.".into());
    }
    let encoding = TransferEncoding::parse(get("transfer"))?;
    let total = crate::agent_lifecycle::transfer_size(get("arch"), encoding)
        .ok_or("Needle supports x86_64 and ARM64 servers.")?;
    let number = |key| get(key).parse::<u64>().ok();
    Ok(
        json!({"os":get("os"),"arch":get("arch"),"cores":number("cores"),"memoryKiB":number("memory"),"freeKiB":number("free"),"installed":get("installed")=="yes","total":total}),
    )
}
pub async fn inspect(state: &Shared, expected: Server) -> Result<Value, String> {
    let (server, owner) = current_owner(state, &expected)?;
    owner.retry_transport();
    let session = owner.acquire(&server).await?;
    let command = format!("{INSPECT}printf 'transfer='; {}", TransferEncoding::PROBE);
    let output = session.execute(&command, None).await;
    if owner.is_revoked() {
        return Err("Server connection changed. Restart setup.".into());
    }
    parse(&output?)
}
pub async fn install(state: &Shared, expected: Server) -> Result<(), String> {
    let (_, owner) = current_owner(state, &expected)?;
    let captured = owner.clone();
    let server = state
        .edit_configuration(None, move |manager| {
            current(manager, &expected)?;
            if captured.is_revoked()
                || !manager
                    .sessions
                    .get(expected.id)
                    .is_some_and(|owner| std::sync::Arc::ptr_eq(&owner, &captured))
            {
                return Err("Server connection changed. Restart setup.".into());
            }
            let mut config = manager.config.clone();
            let server = config
                .servers
                .iter_mut()
                .find(|s| s.id == expected.id)
                .unwrap();
            server.agent_install_approved = true;
            let server = server.clone();
            Ok((config, server))
        })
        .await?;
    crate::monitor::retry_installation(&owner).await;
    let session = owner.acquire(&server).await?;
    session.install(false).await?;
    if owner.is_revoked() {
        return Err("Server connection changed. Restart setup.".into());
    }
    Ok(())
}
/// Leaves setup without Needle on this connection. Pages that need Needle
/// offer setup again; uninstalling or a new connection starts it over.
pub async fn skip(state: &Shared, expected: Server) -> Result<(), String> {
    state
        .edit_configuration(None, move |manager| {
            current(manager, &expected)?;
            let mut config = manager.config.clone();
            let server = config
                .servers
                .iter_mut()
                .find(|s| s.id == expected.id)
                .unwrap();
            server.setup_complete = true;
            Ok((config, ()))
        })
        .await
}
pub async fn finish(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    expected: Server,
    browser: bool,
    clipboard: bool,
) -> Result<Option<String>, String> {
    let (server, owner, permission_revision, preferences) = {
        let (server, owner) = current_owner(state, &expected)?;
        let revision = owner.permissions.snapshot().desired.revision;
        let preferences = owner.permissions.preference_revisions();
        (server, owner, revision, preferences)
    };
    let session = owner.acquire(&server).await?;
    let notice = if browser || clipboard {
        // Installing with integrations writes the command links and shell
        // setup; what it couldn't write comes back as a notice.
        let outcome = session.install(true).await?;
        crate::agent_lifecycle::setup_notice(&outcome)
    } else {
        session.require_installed().await?;
        None
    };
    let id = expected.id;
    let captured = owner.clone();
    state
        .edit_configuration(None, move |manager| {
            let server = current(manager, &expected)?;
            if captured.is_revoked()
                || !manager
                    .sessions
                    .get(id)
                    .is_some_and(|owner| std::sync::Arc::ptr_eq(&owner, &captured))
            {
                return Err("Server connection changed. Restart setup.".into());
            }
            if captured.permissions.snapshot().desired.revision != permission_revision
                || captured.permissions.preference_revisions() != preferences
            {
                return Err(
                    "Integration preferences changed during setup. Review them and finish again."
                        .into(),
                );
            }
            if !server.agent_install_approved {
                return Err("Install Needle before enabling integrations.".into());
            }
            let mut config = manager.config.clone();
            let server = config
                .servers
                .iter_mut()
                .find(|s| s.id == expected.id)
                .unwrap();
            server.browser_enabled = browser;
            server.clipboard_enabled = clipboard;
            server.setup_complete = true;
            Ok((config, ()))
        })
        .await?;
    if owner.is_revoked()
        || !owner
            .permissions
            .commit_setup(permission_revision, preferences, clipboard, browser)
    {
        return Err(
            "Integration preferences changed while saving setup. Review them and finish again."
                .into(),
        );
    }
    let start = state.prepare_integration(id).await?;
    if !std::sync::Arc::ptr_eq(&start.owner, &owner) {
        return Err("Server connection changed. Restart setup.".into());
    }
    start.run(desktop).await?;
    Ok(notice)
}
#[cfg(test)]
#[path = "server_setup_lifecycle_tests.rs"]
mod lifecycle_tests;
#[cfg(test)]
mod tests {
    #[test]
    fn inspection_is_parsed_without_inventing_missing_specs() {
        let value = super::parse(
            "kernel=Linux\narch=x86_64\nos=Ubuntu\ncores=4\ninstalled=no\ntransfer=gzip\n",
        )
        .unwrap();
        assert_eq!(value["cores"], 4);
        assert!(value["memoryKiB"].is_null());
        assert_eq!(value["installed"], false);
        assert_eq!(
            value["total"],
            crate::agent_payload::payload("x86_64")
                .unwrap()
                .compressed_size
        );
        assert!(super::parse("kernel=Darwin\narch=arm64").is_err());
    }

    #[test]
    fn inspection_uses_the_selected_transfer_size_and_rejects_unknown_support() {
        for arch in ["x86_64", "aarch64", "arm64"] {
            let payload = crate::agent_payload::payload(arch).unwrap();
            for (encoding, expected) in [("gzip", payload.compressed_size), ("raw", payload.size)] {
                let value =
                    super::parse(&format!("kernel=Linux\narch={arch}\ntransfer={encoding}\n"))
                        .unwrap();
                assert_eq!(value["total"], expected);
            }
        }
        for encoding in ["", "unknown"] {
            assert!(
                super::parse(&format!("kernel=Linux\narch=x86_64\ntransfer={encoding}\n")).is_err()
            );
        }
    }
}
