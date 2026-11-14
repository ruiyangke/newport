//! A server's held shells, for every Mac's tabs.
//! Needle's `watch` reports them while the app asks; the list lives only in
//! memory, and is dropped once the app stops asking.
use crate::{agent::Service, server_session::ServerSession};
use newport_domain::terminal::{AttachedMac, HeldShell, HeldShells, Mac};
use newport_protocol::{
    encoding::protobuf::tokio::FrameReader,
    terminal::{self as service, Reply, Request, wire},
};
use std::{
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// How long the watch runs after the app last asked.
const IDLE: Duration = Duration::from_secs(30);
/// How long a failed watch waits before it starts again.
const RETRY: Duration = Duration::from_secs(5);
/// How long a watch may take to send its first list before the app stops
/// waiting for it.
const FIRST_LIST: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct HeldList(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    shells: Option<Vec<HeldShell>>,
    /// The last watch failed before it sent a list: the server has none to
    /// wait for (an older Needle, or no link), until one arrives.
    failed: bool,
    asked: Option<Instant>,
    watching: bool,
}

/// The server's shells as last reported, starting the watch when it isn't
/// running.
pub(crate) fn read(owner: &Arc<ServerSession>) -> HeldShells {
    let mut state = owner
        .held_list
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.asked = Some(Instant::now());
    if !state.watching {
        state.watching = true;
        tokio::spawn(watch(Arc::downgrade(owner), owner.held_list.0.clone()));
    }
    let profile = owner.profile();
    HeldShells {
        desktop: owner.integration_session_id().to_string(),
        holds: profile.setup_complete && profile.agent_install_approved && !state.failed,
        shells: state.shells.clone(),
    }
}

/// Keeps the list current until the app stops asking or the server's
/// session ends.
async fn watch(owner: Weak<ServerSession>, state: Arc<Mutex<State>>) {
    loop {
        let owner = {
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let owner = owner.upgrade().filter(|owner| !owner.is_revoked());
            let asked = state.asked.is_some_and(|asked| asked.elapsed() < IDLE);
            match owner.filter(|_| asked) {
                Some(owner) => owner,
                None => {
                    state.watching = false;
                    state.shells = None;
                    state.failed = false;
                    return;
                }
            }
        };
        match watch_once(&owner, &state).await {
            Ok(()) => {}
            Err(error) => {
                log::debug!("Held terminals are not listed: {error}");
                {
                    let mut state = state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.failed = state.shells.is_none();
                }
                tokio::time::sleep(RETRY).await;
            }
        }
    }
}

/// One watch, until it fails, the app stops asking or the session ends.
/// Where Needle can't hold shells the list stays unknown.
async fn watch_once(owner: &Arc<ServerSession>, state: &Mutex<State>) -> Result<(), String> {
    // As configured now, as `holder_channel::run` decides.
    let profile = owner.profile();
    if !(profile.setup_complete && profile.agent_install_approved) {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shells = None;
        return Err("Needle is not set up on this server.".into());
    }
    let needle = owner.needle(&owner.server().clone())?;
    let started = tokio::time::Instant::now();
    let stream = tokio::time::timeout(FIRST_LIST, needle.launch(Service::Terminal))
        .await
        .map_err(|_| "Needle didn't start in time.".to_string())?
        .map_err(|error| error.to_string())?;
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(&wire::encode_request(&Request::Watch).map_err(|error| error.to_string())?)
        .await
        .map_err(|error| error.to_string())?;
    writer.flush().await.map_err(|error| error.to_string())?;
    let mut reader = FrameReader::new(reader, service::SERVER_LIMITS);
    let mut cancelled = owner.cancellation();
    let mut idle = tokio::time::interval(RETRY);
    loop {
        tokio::select! {
            read = reader.next() => {
                let body = read.map_err(|error| error.to_string())?.ok_or("The terminal watch ended.")?;
                let reply = wire::decode_reply(body).map_err(|error| error.to_string())?;
                let shells = project(reply)?;
                let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                state.shells = Some(shells);
                state.failed = false;
            }
            _ = idle.tick() => {
                let state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.shells.is_none() && started.elapsed() >= FIRST_LIST {
                    return Err("The terminal watch sent no list in time.".into());
                }
                if state.asked.is_none_or(|asked| asked.elapsed() >= IDLE) {
                    return Ok(());
                }
            }
            _ = cancelled.changed() => return Ok(()),
        }
    }
}

/// Project the service's holder records into the desktop's read model.
fn project(reply: Reply) -> Result<Vec<HeldShell>, String> {
    reply
        .validate_for(&Request::Watch)
        .map_err(|error| error.to_string())?;
    let holders = match reply {
        Reply::Listed(holders) => holders,
        Reply::Failed(error) => return Err(error.message),
        _ => return Err("The terminal watch sent no list.".into()),
    };
    let mut shells = holders
        .into_iter()
        .map(|holder| {
            Ok(HeldShell {
                terminal: Uuid::try_parse(&holder.id).map_err(|error| error.to_string())?,
                name: holder.name,
                title: holder.title,
                agent: holder.agent,
                creator: Mac {
                    id: holder.creator.id,
                    name: holder.creator.name,
                },
                attached: holder
                    .attached
                    .into_iter()
                    .map(|attached| AttachedMac {
                        id: attached.id,
                        name: attached.name,
                        active: attached.active,
                    })
                    .collect(),
                started: holder.started,
                exited: holder.exited.is_some(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    shells.sort_by_key(|shell| (shell.started, shell.terminal));
    Ok(shells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_server_whose_list_cannot_come_holds_no_shells_to_wait_for() {
        let server: crate::model::Server = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "name": "Unreachable", "sshHost": "unconnected.invalid",
            "sshUser": "fixture", "sshPort": 22,
            "setupComplete": true, "agentInstallApproved": true,
        }))
        .unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        assert!(read(&owner).holds, "waited for until the watch fails");
        let deadline = Instant::now() + FIRST_LIST + Duration::from_secs(5);
        while read(&owner).holds {
            assert!(Instant::now() < deadline, "the app would wait forever");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(read(&owner).shells, None);
    }

    #[test]
    fn watch_records_become_held_shells() {
        let id = Uuid::new_v4();
        let holder = service::Holder {
            id: id.simple().to_string(),
            socket: "/run/x.sock".into(),
            version: 1,
            columns: 80,
            rows: 24,
            started: 7,
            name: Some("build".into()),
            title: Some("dev@host: ~".into()),
            agent: Some("state=working:app=claude-code".into()),
            creator: service::Computer {
                id: Uuid::new_v4().to_string(),
                name: "Studio".into(),
            },
            attached: vec![service::Attached {
                id: Uuid::new_v4().to_string(),
                name: "Air".into(),
                active: true,
            }],
            exited: Some(service::Exit::Code(0)),
        };
        let earlier = service::Holder {
            id: Uuid::new_v4().simple().to_string(),
            started: 2,
            version: 99,
            name: None,
            exited: None,
            ..holder.clone()
        };
        let shells = project(Reply::Listed(vec![holder, earlier])).unwrap();
        assert_eq!(
            shells.len(),
            2,
            "incompatible holder rows remain visible; only the handshake authorizes attachment"
        );
        assert_eq!((shells[0].name.as_deref(), shells[0].exited), (None, false));
        let shell = &shells[1];
        assert_eq!(shell.terminal, id);
        assert_eq!(shell.name.as_deref(), Some("build"));
        assert_eq!(shell.title.as_deref(), Some("dev@host: ~"));
        assert_eq!(
            shell.agent.as_deref(),
            Some("state=working:app=claude-code")
        );
        assert_eq!(shell.creator.name, "Studio");
        assert!(shell.attached[0].active && shell.exited);
        assert!(project(Reply::Ended { ended: true }).is_err());
        assert_eq!(
            project(Reply::Failed(newport_protocol::common::Error {
                code: "unsupported".into(),
                message: "Persistent terminals require Linux.".into(),
                retry: newport_protocol::common::Retry::No,
            }))
            .unwrap_err(),
            "Persistent terminals require Linux."
        );
    }
}
