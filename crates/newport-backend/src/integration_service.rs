//! Session-owned integration demand, task lifetime, and observable state.
use crate::{
    desktop_integration::Desktop,
    model::{ConnectionState, Server, Status},
    server_session::ServerSession,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

/// Prepared from a published profile and owner, independently of profile writes.
/// The captured owner rejects connection revisions retired before admission.
pub(crate) struct Start {
    pub owner: Arc<ServerSession>,
    pub server: Server,
    pub client: uuid::Uuid,
}
impl Start {
    pub async fn run(self, app: Desktop) -> Result<(), String> {
        let result = self
            .owner
            .integration
            .start(self.owner.clone(), self.server, self.client, app)
            .await;
        if let Err(error) = &result
            && !self.owner.is_revoked()
        {
            self.owner
                .integration
                .set_state(ConnectionState::error(error.clone()));
        }
        result
    }
}

#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub connection: ConnectionState,
    pub message: Option<String>,
    pub path_needed: bool,
}
struct Running {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
        self.task.abort();
    }
}
#[derive(Default)]
pub(crate) struct Slot {
    running: tokio::sync::Mutex<Option<Running>>,
    state: Arc<Mutex<Option<Snapshot>>>,
}
impl Slot {
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.state.lock().unwrap().clone()
    }
    pub fn set_state(&self, connection: ConnectionState) {
        self.state
            .lock()
            .unwrap()
            .get_or_insert_with(Snapshot::default)
            .connection = connection;
    }
    pub async fn start(
        &self,
        owner: Arc<ServerSession>,
        mut server: Server,
        client: uuid::Uuid,
        app: Desktop,
    ) -> Result<(), String> {
        let mut running = self.running.lock().await;
        if owner.is_revoked() {
            return Err("Server session changed. Reconnect before retrying.".into());
        }
        if running.as_ref().is_some_and(|r| !r.task.is_finished()) {
            // An explicit start may find the enabled service suspended on an
            // authentication failure. Wake the shared transport, not a second
            // integration task or a second signing request.
            owner.retry_transport();
            if owner.permissions.snapshot().error.is_some() {
                owner.permissions.retry();
            }
            return Ok(());
        }
        // Another preference edit may have completed since admission was
        // prepared. Startup must use the latest desired permissions.
        let desired = owner.permissions.snapshot().desired;
        server.clipboard_enabled = desired.clipboard;
        server.browser_enabled = desired.browser;
        if !server.clipboard_enabled && !server.browser_enabled {
            return Ok(());
        }
        *self.state.lock().unwrap() = Some(Snapshot {
            connection: ConnectionState::new(Status::Connecting),
            ..Snapshot::default()
        });
        owner.retry_transport();
        owner.agent_runtime.retry_failed().await;
        let state = self.state.clone();
        let (cancel, rx) = watch::channel(false);
        let revoked = owner.cancellation();
        let stop = cancel.clone();
        let task = tokio::spawn(async move {
            let _demand = owner.transport_demand();
            let run = crate::clipboard::run(
                owner,
                server,
                client,
                app,
                rx,
                |message, path_needed| {
                    *state.lock().unwrap() = Some(Snapshot {
                        connection: ConnectionState::new(Status::Connected),
                        message: Some(message),
                        path_needed,
                    });
                },
                |_error| {
                    *state.lock().unwrap() = Some(Snapshot {
                        // SSH failures belong to the global connection owner.
                        // Sharing only reports its own service readiness.
                        connection: ConnectionState::new(Status::Reconnecting),
                        ..Snapshot::default()
                    });
                },
            );
            tokio::pin!(run);
            let mut revoked = revoked;
            let result = tokio::select! {
                biased;
                _ = async { if !*revoked.borrow() { let _ = revoked.changed().await; } } => {
                    stop.send_replace(true);
                    tokio::time::timeout(Duration::from_secs(3), &mut run).await.unwrap_or(Ok(()))
                },
                result = &mut run => result,
            };
            *state.lock().unwrap() = Some(Snapshot {
                connection: match result {
                    Ok(()) => ConnectionState::default(),
                    Err(error) => ConnectionState::error(error),
                },
                ..Snapshot::default()
            });
        });
        *running = Some(Running { cancel, task });
        Ok(())
    }
    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        if let Some(mut active) = running.take() {
            active.cancel.send_replace(true);
            if tokio::time::timeout(Duration::from_secs(3), &mut active.task)
                .await
                .is_err()
            {
                active.task.abort();
                let _ = (&mut active.task).await;
            }
        }
        *self.state.lock().unwrap() = Some(Snapshot::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoDesktop;
    impl crate::desktop_integration::DesktopIntegration for NoDesktop {
        fn on_main_thread(&self, _: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            panic!("no desktop work expected")
        }
        fn clipboard_revision(&self) -> Option<isize> {
            panic!("no clipboard work expected")
        }
        fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
            panic!("no clipboard work expected")
        }
        fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
            panic!("no clipboard work expected")
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            panic!("no browser work expected")
        }
    }

    #[tokio::test]
    async fn disabling_both_features_does_not_wait_for_service_or_lifecycle_admission() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name":"Revocation fixture", "sshHost":"127.0.0.1", "sshUser":"fixture", "sshPort":9,
            "browserEnabled":true, "clipboardEnabled":true
        })).unwrap();
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let desired = owner.permissions.snapshot().desired;
        owner
            .permissions
            .acknowledge(crate::agent::control::Applied {
                permissions: desired,
                error: None,
            });
        let access = manager.integration_access();
        let expected_connection = manager.connection_scope(server.id).unwrap();
        let state = crate::manager::share(manager);
        let _service = owner.integration.running.lock().await;
        let _lifecycle = owner.lifecycle.operation.lock().await;
        for feature in [
            crate::commands::IntegrationFeature::Clipboard,
            crate::commands::IntegrationFeature::Browser,
        ] {
            let browser = matches!(feature, crate::commands::IntegrationFeature::Browser);
            let mut command = Box::pin(crate::commands::set_integration_enabled(
                &state,
                Arc::new(NoDesktop),
                server.id,
                feature,
                false,
                expected_connection.clone(),
                &access,
            ));
            let early = tokio::select! {
                biased;
                result = &mut command => Some(result),
                _ = tokio::task::yield_now() => None,
            };
            // Revocation itself happens on the first poll, before storage.
            // Stronghold encryption can exceed three seconds on slower hosts;
            // it is not part of service/lifecycle admission. Keep both gates
            // held through completion to prove neither can block persistence.
            assert!(!if browser {
                owner.permissions.browser_enabled()
            } else {
                owner.permissions.clipboard_enabled()
            });
            match early {
                Some(result) => result,
                None => tokio::time::timeout(Duration::from_secs(20), command)
                    .await
                    .expect("saving revocation must not wait for service admission"),
            }
            .unwrap();
        }
        assert!(!owner.permissions.browser_enabled());
        assert!(!owner.permissions.clipboard_enabled());
        assert_eq!(owner.transport_snapshot().demand, 0);
        assert!(state.lock().await.config.integration_servers().is_empty());
    }

    #[tokio::test]
    async fn queued_integration_toggle_cannot_follow_a_replaced_connection() {
        for enabled in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut manager = crate::manager::Manager::new(crate::profile::Store::for_test(
                directory.path().into(),
            ));
            let server: Server = serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4(), "name":"Queued fixture", "sshHost":"127.0.0.1", "sshUser":"fixture", "sshPort":9,
                "browserEnabled":true, "clipboardEnabled":true
            })).unwrap();
            manager.config.servers.push(server.clone());
            let access = manager.integration_access();
            let expected = manager.connection_scope(server.id).unwrap();
            let state = crate::manager::share(manager);
            let mut manager = state.lock().await;
            let mut command = Box::pin(crate::commands::set_integration_enabled(
                &state,
                Arc::new(NoDesktop),
                server.id,
                crate::commands::IntegrationFeature::Browser,
                enabled,
                expected,
                &access,
            ));
            tokio::select! {
                biased;
                result = &mut command => panic!("command must wait for manager: {result:?}"),
                _ = tokio::task::yield_now() => {},
            }
            // Credentials may change without changing any public server fields.
            manager.advance_connection_revision(server.id);
            let replacement = manager
                .sessions
                .session(&server, manager.connection_revision(server.id));
            let before = serde_json::to_value(replacement.permissions.snapshot()).unwrap();
            drop(manager);
            assert!(command.await.unwrap_err().contains("connection changed"));
            assert_eq!(
                serde_json::to_value(replacement.permissions.snapshot()).unwrap(),
                before
            );
            assert_eq!(replacement.transport_snapshot().demand, 0);
            assert!(state.lock().await.config.servers[0].browser_enabled);
            crate::manager::shutdown(&state).await;
        }
    }

    #[tokio::test]
    async fn waiting_admission_releases_manager_and_obeys_later_disable_or_retirement() {
        for retire in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut manager = crate::manager::Manager::new(crate::profile::Store::for_test(
                directory.path().into(),
            ));
            let server: Server = serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4(), "name": "Admission fixture", "sshHost":"127.0.0.1", "sshUser":"fixture", "sshPort":9, "browserEnabled":true
            })).unwrap();
            manager.config.servers.push(server.clone());
            let owner = manager.sessions.session(&server, 0);
            let access = manager.integration_access();
            let expected_connection = manager.connection_scope(server.id).unwrap();
            let state = crate::manager::share(manager);
            // Hold the service admission gate, as a slow preceding stop would.
            let gate = owner.integration.running.lock().await;
            let mut command = Box::pin(crate::commands::set_integration_enabled(
                &state,
                Arc::new(NoDesktop),
                server.id,
                crate::commands::IntegrationFeature::Browser,
                true,
                expected_connection.clone(),
                &access,
            ));
            tokio::select! {
                biased;
                result = &mut command => panic!("admission should wait: {result:?}"),
                _ = tokio::task::yield_now() => {},
            }
            {
                let mut manager = state
                    .try_lock()
                    .expect("admission must not hold the app-wide lock");
                assert_eq!(manager.snapshot().config.servers.len(), 1);
                if retire {
                    manager.advance_connection_revision(server.id);
                } else {
                    manager.config.servers[0].browser_enabled = false;
                    owner.permissions.update(false, false);
                }
            }
            drop(gate);
            let result = command.await;
            if retire {
                assert!(result.unwrap_err().contains("Server session changed"));
            } else {
                result.unwrap();
            }
            assert!(owner.integration.running.lock().await.is_none());
            assert_eq!(owner.transport_snapshot().demand, 0);
            crate::manager::shutdown(&state).await;
        }
    }

    #[tokio::test]
    async fn shutdown_publishes_revocation_without_holding_manager_during_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            crate::manager::Manager::new(crate::profile::Store::for_test(directory.path().into()));
        let server: Server = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": "Shutdown fixture", "sshHost":"127.0.0.1", "sshUser":"fixture", "sshPort":9
        })).unwrap();
        manager.config.servers.push(server.clone());
        let owner = manager.sessions.session(&server, 0);
        let state = crate::manager::share(manager);
        let gate = owner.integration.running.lock().await;
        let mut shutdown = Box::pin(crate::manager::shutdown(&state));
        tokio::select! {
            biased;
            _ = &mut shutdown => panic!("cleanup should wait for the held service gate"),
            _ = tokio::task::yield_now() => {},
        }
        assert!(owner.is_revoked());
        {
            let manager = state.try_lock().expect("cleanup held the app-wide lock");
            assert_eq!(manager.snapshot().config.servers.len(), 1);
            assert!(manager.sessions.session(&server, 0).is_revoked());
        }
        drop(gate);
        shutdown.await;
    }

    #[tokio::test]
    async fn cancelled_stop_waiter_does_not_detach_a_stalled_service() {
        struct Finished(Arc<tokio::sync::Notify>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        let started = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(tokio::sync::Notify::new());
        let task_started = started.clone();
        let task_finished = finished.clone();
        let task = tokio::spawn(async move {
            let _finished = Finished(task_finished);
            task_started.notify_one();
            std::future::pending::<()>().await;
        });
        started.notified().await;
        let slot = Slot::default();
        let (cancel, _) = watch::channel(false);
        *slot.running.lock().await = Some(Running { cancel, task });
        let mut stopping = Box::pin(slot.stop());
        tokio::select! {
            biased;
            _ = &mut stopping => panic!("stalled task should still be waiting"),
            _ = tokio::task::yield_now() => {},
        }
        drop(stopping);
        tokio::time::timeout(Duration::from_secs(1), finished.notified())
            .await
            .expect("cancelling the waiter must abort its stalled task");
        assert!(slot.running.lock().await.is_none());
    }
}
