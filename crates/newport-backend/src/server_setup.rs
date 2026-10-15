//! Read-only inspection and explicit installation consent for native onboarding.
use crate::{manager::Shared, model::Server, server_session};
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
    let total = crate::agent_lifecycle::payload_size(get("arch"))
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
    let output = session.execute(INSPECT, None).await;
    if owner.is_revoked() {
        return Err("Server connection changed. Restart setup.".into());
    }
    let mut specs = parse(&output?)?;
    specs["operationId"] = owner
        .lifecycle
        .receipts
        .for_server(&server)?
        .into_iter()
        .find(|r| !r.phase.terminal() && !r.intent.integrations)
        .map(|r| json!(r.intent.operation_id))
        .unwrap_or(Value::Null);
    specs["helperOperationId"] = owner
        .lifecycle
        .receipts
        .for_server(&server)?
        .into_iter()
        .find(|r| !r.phase.terminal() && !r.intent.kind.is_deployment())
        .map(|r| json!(r.intent.operation_id))
        .unwrap_or(Value::Null);
    Ok(specs)
}
pub async fn install(state: &Shared, expected: Server) -> Result<(), String> {
    install_with_id(state, expected, None).await
}
pub async fn install_with_id(
    state: &Shared,
    expected: Server,
    operation_id: Option<uuid::Uuid>,
) -> Result<(), String> {
    if operation_id.is_some_and(|id| id.is_nil()) {
        return Err("Invalid Needle operation ID".into());
    }
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
    let result = match operation_id {
        Some(id) => session.install_operation(false, false, id).await,
        None => session.install(false, false).await,
    };
    result?;
    if owner.is_revoked() {
        return Err("Server connection changed. Restart setup.".into());
    }
    Ok(())
}
pub async fn finish(
    state: &Shared,
    desktop: crate::desktop_integration::Desktop,
    expected: Server,
    browser: bool,
    clipboard: bool,
    operation_id: Option<uuid::Uuid>,
) -> Result<Option<String>, String> {
    let (server, owner, permission_revision, preferences) = {
        let (server, owner) = current_owner(state, &expected)?;
        let revision = owner.permissions.snapshot().desired.revision;
        let preferences = owner.permissions.preference_revisions();
        (server, owner, revision, preferences)
    };
    let session = owner.acquire(&server).await?;
    let ready = if browser || clipboard {
        match operation_id {
            Some(id) => session
                .configure_integrations_operation(id)
                .await
                .map(|_| ()),
            None => session.configure_integrations().await.map(|_| ()),
        }
    } else {
        session.require_installed().await
    };
    ready?;
    let notice = if browser || clipboard {
        owner
            .lifecycle
            .receipts
            .for_server(&server)?
            .into_iter()
            .rev()
            .find(|r| {
                !r.intent.kind.is_deployment()
                    && r.phase == crate::lifecycle_receipts::Phase::Ready
                    && operation_id.is_none_or(|id| r.intent.operation_id == id)
            })
            .and_then(|r| r.error)
    } else {
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
        let value =
            super::parse("kernel=Linux\narch=x86_64\nos=Ubuntu\ncores=4\ninstalled=no\n").unwrap();
        assert_eq!(value["cores"], 4);
        assert!(value["memoryKiB"].is_null());
        assert_eq!(value["installed"], false);
        assert!(super::parse("kernel=Darwin\narch=arm64").is_err());
    }
}
